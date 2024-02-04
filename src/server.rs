use std::{
    os::unix::prelude::OwnedFd,
    path::{Path, PathBuf},
    process::exit,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Sender},
        Arc, Mutex,
    },
    thread::{self, sleep, JoinHandle},
    time::Duration,
};

use disk::{
    devices::list_devices,
    partition::{self, auto_create_partitions, list_partitions, DkPartition},
    PartitionError,
};
use install::{
    chroot::{escape_chroot, get_dir_fd},
    mount::{remove_bind_mounts, umount_root_path},
    swap::{get_recommend_swap_size, swapoff},
    DownloadType, InstallConfig, InstallConfigPrepare, SwapFile, User,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sysinfo::System;
use tracing::{error, info, warn};
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
    install_thread: Option<JoinHandle<()>>,
    partition_thread: Option<JoinHandle<()>>,
    cancel_run_install: Arc<AtomicBool>,
    auto_partition_progress: Arc<Mutex<AutoPartitionProgress>>,
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
            partition_thread: None,
            cancel_run_install: Arc::new(AtomicBool::new(false)),
            auto_partition_progress: Arc::new(Mutex::new(AutoPartitionProgress::Pending)),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "status")]
pub enum ProgressStatus {
    Pending,
    Working { step: u8, progress: f64, v: usize },
    Error(DeploykitError),
    Finish,
}

impl ProgressStatus {
    fn change_step(&mut self, step: u8) {
        if let ProgressStatus::Working { progress, v, .. } = self {
            *self = ProgressStatus::Working {
                step,
                progress: *progress,
                v: *v,
            }
        }
    }

    fn change_progress(&mut self, progress: f64) {
        if let ProgressStatus::Working { step, v, .. } = self {
            *self = ProgressStatus::Working {
                step: *step,
                progress,
                v: *v,
            }
        }
    }

    fn change_velocity(&mut self, velocity: usize) {
        if let ProgressStatus::Working { step, progress, .. } = self {
            *self = ProgressStatus::Working {
                step: *step,
                progress: *progress,
                v: velocity,
            }
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

#[derive(Debug)]
pub enum AutoPartitionProgress {
    Pending,
    Working,
    Finish(Option<PartitionError>),
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
                "rtc_as_localtime" => Message::ok(&self.config.rtc_as_localtime.to_string()),
                "target_partition" => Message::check_is_set(field, {
                    let lock = self.config.target_partition.lock().unwrap();

                    &lock.clone()
                }),
                "efi_partition" => {
                    let lock = self.config.efi_partition.lock().unwrap();

                    Message::check_is_set(field, &lock.clone())
                }
                "swapfile" => Message::ok(&self.config.swapfile),
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

    fn get_list_partitions(&self, dev: &str) -> String {
        let path = PathBuf::from(dev);
        let res = list_partitions(path);

        Message::ok(&res)
    }

    fn auto_partition(&mut self, dev: &str) -> String {
        let path = PathBuf::from(dev);
        let efi_arc = self.config.efi_partition.clone();
        let target_part = self.config.target_partition.clone();

        {
            let mut lock = self.auto_partition_progress.lock().unwrap();
            *lock = AutoPartitionProgress::Working;
        }

        let auto_partition_progress = self.auto_partition_progress.clone();

        self.partition_thread = Some(thread::spawn(move || {
            let p = auto_create_partitions(&path);

            match p {
                Ok((efi, p)) => {
                    {
                        let mut lock = efi_arc.lock().unwrap();
                        *lock = efi;
                    }

                    {
                        let mut lock = target_part.lock().unwrap();
                        *lock = Some(p);
                    }

                    {
                        let mut lock = auto_partition_progress.lock().unwrap();
                        *lock = AutoPartitionProgress::Finish(None);
                    }
                }
                Err(e) => {
                    error!("Failed to auto partition: {e}");

                    {
                        let mut lock = auto_partition_progress.lock().unwrap();
                        *lock = AutoPartitionProgress::Finish(Some(e));
                    }
                }
            }
        }));

        Message::ok(&"")
    }

    fn get_auto_partition_progress(&self) -> String {
        let ps = self.auto_partition_progress.lock().unwrap();

        match &*ps {
            AutoPartitionProgress::Pending => Message::ok(&"Pending"),
            AutoPartitionProgress::Working => Message::ok(&"Working"),
            AutoPartitionProgress::Finish(e) => match e {
                None => Message::ok(&"Finish"),
                Some(e) => Message::err(DeploykitError::AutoPartition(e.to_string())),
            },
        }
    }

    fn start_install(&mut self) -> String {
        {
            let ps = self.progress.lock().unwrap();
            if let ProgressStatus::Working { .. } = *ps {
                return Message::err("Another installation is working.");
            }
        }

        match start_install_inner(
            self.config.clone(),
            self.step_tx.clone(),
            self.progress_tx.clone(),
            self.v_tx.clone(),
            self.progress.clone(),
            self.cancel_run_install.clone(),
        ) {
            Ok(j) => self.install_thread = Some(j),
            Err(e) => return Message::err(e),
        }

        {
            let mut ps = self.progress.lock().unwrap();
            *ps = ProgressStatus::Working {
                step: 0,
                progress: 0.0,
                v: 0,
            };
        }

        Message::ok(&"")
    }

    fn cancel_install(&mut self) -> String {
        if self.install_thread.is_some() {
            self.cancel_run_install.store(true, Ordering::SeqCst);
            sleep(Duration::from_millis(100));
            self.cancel_run_install.store(false, Ordering::SeqCst);
        }

        Message::ok(&"")
    }

    fn get_recommend_swap_size(&self) -> String {
        let mut sys = System::new_all();
        sys.refresh_memory();
        let total_memory = sys.total_memory();
        let size = get_recommend_swap_size(total_memory);

        Message::ok(&size)
    }

    fn get_memory(&self) -> String {
        let mut sys = System::new_all();
        sys.refresh_memory();
        let total_memory = sys.total_memory();

        Message::ok(&total_memory)
    }

    fn find_esp_partition(&self, dev: &str) -> String {
        let path = Path::new(dev);
        let res = partition::find_esp_partition(path);

        match res {
            Ok(p) => Message::ok(&p),
            Err(e) => Message::err(DeploykitError::FindEspPartition(e.to_string())),
        }
    }

    fn disk_is_right_combo(&self, dev: &str) -> String {
        let path = Path::new(dev);
        let res = disk::right_combine(path);

        match res {
            Ok(()) => Message::ok(&""),
            Err(e) => Message::err(DeploykitError::UnsupportedDiskCombo(e.to_string())),
        }
    }

    fn ping(&self) -> String {
        Message::ok(&"pong")
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
            #[cfg(not(debug_assertions))]
            {
                let p = serde_json::from_str::<DkPartition>(value)
                    .map_err(|_| DeploykitError::SetValue(field.to_string(), value.to_string()))?;
                config.target_partition = Arc::new(Mutex::new(Some(p)));
                Ok(())
            }
            #[cfg(debug_assertions)]
            {
                let _p = serde_json::from_str::<DkPartition>(value)
                    .map_err(|_| DeploykitError::SetValue(field.to_string(), value.to_string()))?;
                config.target_partition = Arc::new(Mutex::new(Some(DkPartition {
                    path: Some(PathBuf::from("/dev/loop30p1")),
                    parent_path: Some(PathBuf::from("/dev/loop30")),
                    fs_type: Some("ext4".to_string()),
                    size: 50 * 1024 * 1024 * 1024,
                })));
                Ok(())
            }
        }
        "efi_partition" => {
            #[cfg(not(debug_assertions))]
            {
                let p = serde_json::from_str::<DkPartition>(value)
                    .map_err(|_| DeploykitError::SetValue(field.to_string(), value.to_string()))?;
                config.efi_partition = Arc::new(Mutex::new(Some(p)));
            }

            #[cfg(debug_assertions)]
            {
                let _p = serde_json::from_str::<DkPartition>(value)
                    .map_err(|_| DeploykitError::SetValue(field.to_string(), value.to_string()))?;
                config.efi_partition = Arc::new(Mutex::new(Some(DkPartition {
                    path: Some(PathBuf::from("/dev/loop30p2")),
                    parent_path: Some(PathBuf::from("/dev/loop30")),
                    fs_type: Some("vfat".to_string()),
                    size: 512 * 1024 * 1024,
                })));
            }

            Ok(())
        }
        "swapfile" => {
            config.swapfile = serde_json::from_str::<SwapFile>(value)
                .map_err(|_| DeploykitError::SetValue(field.to_string(), value.to_string()))?;
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
    cancel_install: Arc<AtomicBool>,
) -> Result<JoinHandle<()>, DeploykitError> {
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
        if let Ok(root_fd) = root_fd_clone.try_clone() {
            safe_exit_env(root_fd, tmp_dir_clone3.clone());
        } else {
            warn!("Failed to clone root_fd");
        }

        exit(1);
    })
    .ok();

    let ps_clone = ps.clone();

    let t = thread::spawn(move || {
        let (tx, rx) = mpsc::channel();
        let t = thread::spawn(move || {
            let res = config
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
                .map_err(|e| DeploykitError::Install(e.to_string()));

            if let Err(e) = res {
                {
                    let ps = ps_clone.lock().unwrap();
                    if let ProgressStatus::Working { .. } = *ps {
                        tx.send(e).expect("Install thread is exit.");
                    }
                }
            }
        });

        loop {
            if cancel_install.load(Ordering::SeqCst) {
                safe_exit_env(root_fd, tmp_dir_clone2);

                {
                    let mut ps = ps.lock().unwrap();
                    *ps = ProgressStatus::Pending;
                }

                return;
            }

            if t.is_finished() {
                let mut ps = ps.lock().unwrap();

                if let Ok(e) = rx.recv_timeout(Duration::from_millis(10)) {
                    error!("Failed to install system: {e:?}");
                    *ps = ProgressStatus::Error(e);
                    return;
                }

                *ps = ProgressStatus::Finish;
                return;
            }
        }
    });

    Ok(t)
}

fn safe_exit_env(root_fd: OwnedFd, tmp_dir: PathBuf) {
    escape_chroot(root_fd).ok();
    swapoff(&tmp_dir).ok();
    remove_bind_mounts(&tmp_dir).ok();
    umount_root_path(&tmp_dir).ok();
}
