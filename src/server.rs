use std::{
    os::unix::prelude::OwnedFd,
    path::{Path, PathBuf},
    process::exit,
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
use install::{
    chroot::{escape_chroot, get_dir_fd},
    mount::{remove_bind_mounts, umount_root_path},
    DownloadType, InstallConfig, InstallConfigPrepare, User,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
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

#[derive(Serialize, Deserialize)]
#[serde(tag = "result")]
pub enum Message {
    Ok { data: Value },
    Error { data: Value },
}

impl Message {
    pub fn ok<T: Serialize>(value: &T) -> String {
        match serde_json::to_value(value).and_then(|x| serde_json::to_string(&Self::Ok { data: x }))
        {
            Ok(v) => v,
            Err(e) => serde_json::to_string(&Self::Error {
                data: Value::String(format!("Failed to serialize data: {e:?}")),
            })
            .unwrap(),
        }
    }

    pub fn err<T: Serialize>(value: T) -> String {
        match serde_json::to_value(value)
            .and_then(|x| serde_json::to_string(&Self::Error { data: x }))
        {
            Ok(v) => v,
            Err(e) => serde_json::to_string(&Self::Error {
                data: Value::String(format!("Failed to serialize data: {e:?}")),
            })
            .unwrap(),
        }
    }

    pub fn check_is_set<T: Serialize>(v_name: &str, v: &Option<T>) -> String {
        match v {
            Some(v) => Self::ok(&v),
            None => Self::err(format!("{v_name} is not set")),
        }
    }
}

#[dbus_interface(name = "io.aosc.Deploykit1")]
impl DeploykitServer {
    fn get_config(&self, field: &str) -> String {
        if field.is_empty() {
            Message::ok(&self.config)
        } else {
            match field {
                "locale" => Message::check_is_set(field, &self.config.locale),
                "timezone" => Message::check_is_set(field, &self.config.timezone),
                "download" => Message::check_is_set(field, &self.config.download),
                "user" => Message::check_is_set(field, &self.config.user),
                "hostname" => Message::check_is_set(field, &self.config.hostname),
                "rtc_as_localtime" => self.config.rtc_as_localtime.to_string(),
                "target_partition" => Message::check_is_set(field, &self.config.target_partition),
                "efi_partition" => Message::check_is_set(field, &self.config.efi_partition),
                _ => {
                    error!("Unknown field: {field}");
                    Message::err(format!("Unknown field: {field}"))
                }
            }
        }
    }

    fn set_config(&mut self, field: &str, value: &str) -> String {
        match set_config_inner(&mut self.config, field, value) {
            Ok(()) => Message::ok(&""),
            Err(e) => {
                error!("Failed to set config: {e}");
                Message::err(e)
            }
        }
    }

    fn get_progress(&self) -> String {
        let ps = self.progress.lock().unwrap();
        Message::ok(&*ps)
    }

    fn reset_config(&mut self) -> String {
        self.config = InstallConfigPrepare::default();
        Message::ok(&"")
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

        Message::ok(&res)
    }

    fn auto_partition(&mut self, dev: &str) -> String {
        let path = Path::new(dev);
        let p = auto_create_partitions(path);
        let s = match p {
            Ok((efi, p)) => {
                self.config.efi_partition = efi;
                self.config.target_partition = Some(p);
                Message::ok(&"")
            }
            Err(e) => {
                error!("Failed to auto partition: {e}");
                Message::err(DeploykitError::AutoPartition(e.to_string()))
            }
        };

        s
    }

    fn start_install(&mut self) -> String {
        match start_install_inner(
            self.config.clone(),
            self.step_tx.clone(),
            self.progress_tx.clone(),
            self.v_tx.clone(),
            self.progress.clone(),
        ) {
            Ok(j) => self.install_thread = Some(j),
            Err(e) => return Message::err(e),
        }

        {
            let mut ps = self.progress.lock().unwrap();
            *ps = ProgressStatus::Working(0, 0.0, 0);
        }

        Message::ok(&"")
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
    let tmp_dir_clone2 = tmp_dir_clone.clone();
    let tmp_dir_clone3 = tmp_dir_clone.clone();

    if let DownloadType::Http { to_path, .. } = &mut config.download {
        *to_path = Some(tmp_dir_clone.join("squashfs"));
    }

    let root_fd = get_dir_fd(Path::new("/")).map_err(|e| DeploykitError::Install(e.to_string()))?;
    let root_fd_clone = root_fd
        .try_clone()
        .map_err(|e| DeploykitError::Install(e.to_string()))?;

    ctrlc::set_handler(move || {
        safe_exit_env(root_fd_clone.try_clone().unwrap(), tmp_dir_clone3.clone());
        exit(1);
    })
    .unwrap();

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
                Ok(())
            }
            Err(e) => {
                error!("Install failed: {e}");
                safe_exit_env(root_fd, tmp_dir_clone2);

                Err(e)
            }
        }
    });

    Ok(t)
}

fn safe_exit_env(root_fd: OwnedFd, tmp_dir: PathBuf) {
    escape_chroot(root_fd).ok();
    remove_bind_mounts(&tmp_dir).ok();
    umount_root_path(&tmp_dir).ok();
}
