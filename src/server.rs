use std::{
    path::Path,
    sync::{
        mpsc::{self, Sender},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
};

use disk::{
    devices::list_devices,
    partition::{auto_create_partitions, DkPartition},
};
use install::{DownloadType, InstallConfig, InstallConfigPrepare, User};
use serde::{Deserialize, Serialize};
use tracing::{error, info};
use zbus::dbus_interface;

use crate::error::DeploykitError;

#[derive(Debug)]
pub struct DeploykitServer {
    config: InstallConfigPrepare,
    progress: Arc<Mutex<ProgressStatus>>,
    _progress_handle: JoinHandle<()>,
    step_tx: Sender<u8>,
    progress_tx: Sender<f64>,
    v_tx: Sender<usize>,
    install_thread: Option<JoinHandle<Result<(), DeploykitError>>>,
}

impl Default for DeploykitServer {
    fn default() -> Self {
        let ps = Arc::new(Mutex::new(ProgressStatus::Pending));
        let (step_tx, step_rx) = mpsc::channel();
        let (progress_tx, progress_rx) = mpsc::channel();
        let (v_tx, v_rx) = mpsc::channel();
        Self {
            config: InstallConfigPrepare::default(),
            progress: ps.clone(),
            _progress_handle: thread::spawn(move || loop {
                let mut ps = ps.lock().unwrap();
                if let Ok(v) = step_rx.try_recv() {
                    ps.change_step(v);
                }

                if let Ok(v) = progress_rx.try_recv() {
                    ps.change_progress(v);
                }

                if let Ok(v) = v_rx.try_recv() {
                    ps.change_velocity(v);
                }

                drop(ps);
            }),
            step_tx,
            progress_tx,
            v_tx,
            install_thread: None,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub enum ProgressStatus {
    Pending,
    Working(u8, f64, usize),
    Done,
}

impl ProgressStatus {
    fn change_step(&mut self, step: u8) {
        match self {
            ProgressStatus::Working(_, progress, v) => {
                *self = ProgressStatus::Working(step, *progress, *v);
            }
            _ => {}
        }
    }

    fn change_progress(&mut self, progress: f64) {
        match self {
            ProgressStatus::Working(step, _, v) => {
                *self = ProgressStatus::Working(*step, progress, *v);
            }
            _ => {}
        }
    }

    fn change_velocity(&mut self, velocity: usize) {
        match self {
            ProgressStatus::Working(step, progress, _) => {
                *self = ProgressStatus::Working(*step, *progress, velocity);
            }
            _ => {}
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct DkDevice {
    path: String,
    model: String,
    size: u64,
}

#[dbus_interface(name = "io.aosc.Deploykit1")]
impl DeploykitServer {
    fn get_config(&self, field: &str) -> String {
        if field.is_empty() {
            match serde_json::to_string(&self.config) {
                Ok(s) => s,
                Err(e) => {
                    error!("Failed to get config: {e}");
                    serde_json::to_string(&DeploykitError::get_config(e))
                        .expect("Failed to serialize error")
                }
            }
        } else {
            match field {
                "locale" => self
                    .config
                    .locale
                    .clone()
                    .unwrap_or_else(|| not_set_error(field)),
                "timezone" => self
                    .config
                    .timezone
                    .clone()
                    .unwrap_or_else(|| not_set_error(field)),
                "download" => serde_json::to_string(&self.config.download)
                    .unwrap_or_else(|_| not_set_error(field)),
                "user" => serde_json::to_string(&self.config.user.clone())
                    .unwrap_or_else(|_| not_set_error(field)),
                "hostname" => self
                    .config
                    .hostname
                    .clone()
                    .unwrap_or_else(|| not_set_error(field)),
                "rtc_as_localtime" => self.config.rtc_as_localtime.to_string(),
                "target_partition" => serde_json::to_string(&self.config.target_partition.clone())
                    .unwrap_or_else(|_| not_set_error(field)),
                "efi_partition" => serde_json::to_string(&self.config.target_partition.clone())
                    .unwrap_or_else(|_| not_set_error(field)),
                _ => {
                    error!("Unknown field: {field}");
                    serde_json::to_string(&DeploykitError::unknown_field(field))
                        .unwrap_or_else(|_| "Failed to serialize".to_string())
                }
            }
        }
    }

    fn set_config(&mut self, field: &str, value: &str) -> String {
        match set_config_inner(&mut self.config, field, value) {
            Ok(()) => "ok".to_string(),
            Err(e) => {
                error!("Failed to set config: {e}");
                serde_json::to_string(&e)
                    .unwrap_or_else(|_| "Failed to serialize error".to_string())
            }
        }
    }

    fn get_progress(&self) -> String {
        let ps = self.progress.lock().unwrap();
        serde_json::to_string(&*ps).unwrap_or_else(|_| "Failed to serialize".to_string())
    }

    fn reset_config(&mut self) -> String {
        self.config = InstallConfigPrepare::default();
        "ok".to_string()
    }

    fn get_list_devices(&self) -> String {
        let mut res = vec![];
        for i in list_devices() {
            res.push(DkDevice {
                path: i.path().display().to_string(),
                model: i.model().to_string(),
                size: i.sector_size() * i.length(),
            });
        }

        serde_json::to_string(&res).unwrap_or_else(|_| "Failed to serialize".to_string())
    }

    fn auto_partition(&mut self, dev: &str) -> String {
        let path = Path::new(dev);
        let p = auto_create_partitions(path);
        let s = match p {
            Ok((efi, p)) => {
                self.config.efi_partition = efi;
                self.config.target_partition = Some(p);
                "ok".to_string()
            }
            Err(e) => {
                error!("Failed to auto partition: {e}");
                serde_json::to_string(&DeploykitError::AutoPartition(e.to_string()))
                    .unwrap_or_else(|_| "Failed to serialize".to_string())
            }
        };

        s
    }

    fn start_install(&mut self) -> String {
        {
            let mut ps = self.progress.lock().unwrap();
            *ps = ProgressStatus::Working(0, 0.0, 0);
        }

        match start_install_inner(
            self.config.clone(),
            self.step_tx.clone(),
            self.progress_tx.clone(),
            self.v_tx.clone(),
            self.progress.clone(),
        ) {
            Ok(j) => self.install_thread = Some(j),
            Err(e) => {
                return serde_json::to_string(&e)
                    .unwrap_or_else(|_| "Failed to serialize".to_string());
            }
        }

        "ok".to_string()
    }
}

fn set_config_inner(
    config: &mut InstallConfigPrepare,
    field: &str,
    value: &str,
) -> Result<(), DeploykitError> {
    match field {
        "locale" => {
            config.locale = Some(value.to_string());
            Ok(())
        }
        "timezone" => {
            config.timezone = Some(value.to_string());
            Ok(())
        }
        "download" => {
            let download_type = serde_json::from_str::<DownloadType>(value)
                .map_err(|_| DeploykitError::SetValue("download".to_string(), value.to_string()))?;

            config.download = Some(download_type);

            Ok(())
        }
        "user" => {
            let user = serde_json::from_str::<User>(value)
                .map_err(|_| DeploykitError::SetValue("user".to_string(), value.to_string()))?;

            config.user = Some(user);
            Ok(())
        }
        "hostname" => {
            config.hostname = Some(value.to_string());
            Ok(())
        }
        "rtc_as_localtime" => match value {
            "0" | "false" => {
                config.rtc_as_localtime = false;
                Ok(())
            }
            "1" | "true" => {
                config.rtc_as_localtime = true;
                Ok(())
            }
            _ => Err(DeploykitError::SetValue(
                field.to_string(),
                value.to_string(),
            )),
        },
        "target_partition" => {
            let p = serde_json::from_str::<DkPartition>(value)
                .map_err(|_| DeploykitError::SetValue(field.to_string(), value.to_string()))?;
            config.target_partition = Some(p);
            Ok(())
        }
        "efi_partition" => {
            let p = serde_json::from_str::<DkPartition>(value)
                .map_err(|_| DeploykitError::SetValue(field.to_string(), value.to_string()))?;
            config.efi_partition = Some(p);
            Ok(())
        }
        _ => {
            error!("Unknown field: {field}");
            Err(DeploykitError::unknown_field(field))
        }
    }
}

fn not_set_error(field: &str) -> String {
    error!("field {field} is not set");
    serde_json::to_string(&DeploykitError::not_set(field))
        .unwrap_or_else(|_| "Failed to serialize".to_string())
}

fn start_install_inner(
    config: InstallConfigPrepare,
    step_tx: Sender<u8>,
    progress_tx: Sender<f64>,
    v_tx: Sender<usize>,
    ps: Arc<Mutex<ProgressStatus>>,
) -> Result<JoinHandle<Result<(), DeploykitError>>, DeploykitError> {
    let mut config =
        InstallConfig::try_from(config).map_err(|e| DeploykitError::Install(e.to_string()))?;

    info!("Starting install");

    let temp_dir = tempfile::tempdir()
        .map_err(|e| DeploykitError::Install(e.to_string()))?
        .into_path()
        .to_path_buf();

    let tmp_dir_clone = temp_dir.clone();

    if let DownloadType::Http { to_path, .. } = &mut config.download {
        *to_path = Some(tmp_dir_clone.join("squashfs"));
    }

    dbg!(&config.download);

    let t = thread::spawn(move || {
        let t = thread::spawn(move || {
            config
                .start_install(
                    |step| {
                        step_tx.send(step).unwrap();
                    },
                    move |progress| {
                        progress_tx.send(progress).unwrap();
                    },
                    move |v| v_tx.send(v).unwrap(),
                    temp_dir,
                )
                .map_err(|e| DeploykitError::Install(e.to_string()))
        });

        let res = t.join().unwrap();

        match res {
            Ok(()) => {
                info!("Install finished");
                let mut ps = ps.lock().unwrap();
                *ps = ProgressStatus::Done;
                drop(ps);
                res
            },
            Err(e) => {
                error!("Install failed: {e}");
                Err(e)
            }
        }
    });

    Ok(t)
}
