use std::{
    os::unix::prelude::OwnedFd,
    path::{Path, PathBuf},
    process::exit,
    sync::{
        atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use disk::{
    devices::{is_root_device, list_devices},
    is_efi_booted,
    partition::{
        self, all_esp_partitions, auto_create_partitions, find_root_mount_point, is_lvm_device,
        list_partitions, DkPartition,
    },
    PartitionError,
};
use install::{
    chroot::{escape_chroot, get_dir_fd},
    mount::{remove_files_mounts, sync_disk, umount_root_path},
    swap::{get_recommend_swap_size, swapoff},
    sync_and_reboot, umount_all, DownloadType, InstallConfig, InstallConfigPrepare, InstallErr,
    SwapFile, User,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sysinfo::System;
use tracing::{error, info, warn};
use zbus::interface;

use crate::error::DkError;

#[derive(Debug)]
pub struct DeploykitServer {
    config: InstallConfigPrepare,
    progress: Arc<Mutex<ProgressStatus>>,
    progress_num: Arc<AtomicU8>,
    step: Arc<AtomicU8>,
    v: Arc<AtomicUsize>,
    install_thread: Option<JoinHandle<()>>,
    partition_thread: Option<JoinHandle<()>>,
    cancel_run_install: Arc<AtomicBool>,
    auto_partition_progress: Arc<Mutex<AutoPartitionProgress>>,
}

impl Default for DeploykitServer {
    fn default() -> Self {
        let ps = Arc::new(Mutex::new(ProgressStatus::Pending));
        let progress_num = Arc::new(AtomicU8::new(0));
        let step = Arc::new(AtomicU8::new(0));
        let v = Arc::new(AtomicUsize::new(0));

        Self {
            config: InstallConfigPrepare::default(),
            progress: ps.clone(),
            progress_num: progress_num.clone(),
            step: step.clone(),
            v: v.clone(),
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
    Working {
        step: Arc<AtomicU8>,
        progress: Arc<AtomicU8>,
        v: Arc<AtomicUsize>,
    },
    Error(DkError),
    Finish,
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

#[derive(Debug, Serialize)]
#[serde(tag = "status")]
pub enum AutoPartitionProgress {
    Pending,
    Working,
    Finish {
        res: Result<(Option<DkPartition>, DkPartition), PartitionError>,
    },
}

#[interface(name = "io.aosc.Deploykit1")]
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
        let root = match find_root_mount_point() {
            Ok(v) => v,
            Err(e) => {
                error!("Failed to get root device: {e}");
                return Message::err(e);
            }
        };

        for mut i in list_devices() {
            if i.path().to_string_lossy() == root {
                // 如果 root 是一个块设备
                continue;
            }

            let is_root_device = match is_root_device(&root, &mut i) {
                Ok(v) => v,
                Err(e) => {
                    error!("Failed to get root device: {e}");
                    return Message::err(e);
                }
            };

            if !is_root_device {
                res.push(DkDevice {
                    path: i.path().display().to_string(),
                    model: i.model().to_string(),
                    size: i.sector_size() * i.length(),
                });
            }
        }

        Message::ok(&res)
    }

    fn get_list_partitions(&self, dev: &str) -> String {
        let path = PathBuf::from(dev);
        let res = list_partitions(path);

        Message::ok(&res)
    }

    fn get_all_esp_partitions(&self) -> String {
        match all_esp_partitions() {
            Ok(res) => Message::ok(&res),
            Err(e) => Message::err(e),
        }
    }

    fn auto_partition(&mut self, dev: &str) -> String {
        let path = if cfg!(debug_assertions) {
            PathBuf::from("/dev/loop30")
        } else {
            PathBuf::from(dev)
        };

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
                        lock.clone_from(&efi);
                    }

                    {
                        let mut lock = target_part.lock().unwrap();
                        *lock = Some(p.clone());
                    }

                    {
                        let mut lock = auto_partition_progress.lock().unwrap();
                        *lock = AutoPartitionProgress::Finish { res: Ok((efi, p)) };
                    }
                }
                Err(e) => {
                    error!("Failed to auto partition: {e}");
                    {
                        let mut lock = auto_partition_progress.lock().unwrap();
                        *lock = AutoPartitionProgress::Finish { res: Err(e) };
                    }
                }
            }
        }));

        Message::ok(&"")
    }

    fn get_auto_partition_progress(&self) -> String {
        let ps = self.auto_partition_progress.lock().unwrap();

        match &*ps {
            AutoPartitionProgress::Finish { res } => match res {
                Ok(_) => Message::ok(&*ps),
                Err(e) => Message::err(DkError {
                    message: e.to_string(),
                    t: "AutoPartition".to_string(),
                    // TODO
                    data: json!({}),
                }),
            },
            _ => Message::ok(&*ps),
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
            self.step.clone(),
            self.progress_num.clone(),
            self.v.clone(),
            self.progress.clone(),
            self.cancel_run_install.clone(),
        ) {
            Ok(j) => self.install_thread = Some(j),
            Err(e) => return Message::err(e),
        }

        {
            let mut ps = self.progress.lock().unwrap();
            *ps = ProgressStatus::Working {
                step: self.step.clone(),
                progress: self.progress_num.clone(),
                v: self.v.clone(),
            };
        }

        Message::ok(&"")
    }

    fn reset_progress_status(&mut self) -> String {
        let mut ps = self.progress.lock().unwrap();
        *ps = ProgressStatus::Pending;

        Message::ok(&"")
    }

    fn cancel_install(&mut self) -> String {
        if self.install_thread.is_some() {
            self.cancel_run_install.store(true, Ordering::SeqCst);
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
            Err(e) => Message::err(DkError {
                message: e.to_string(),
                t: "FindESPPartition".to_string(),
                // TODO
                data: json!({}),
            }),
        }
    }

    fn disk_is_right_combo(&self, dev: &str) -> String {
        let path = Path::new(dev);
        let res = disk::right_combine(path);

        match res {
            Ok(()) => Message::ok(&""),
            Err(e) => Message::err(DkError {
                message: e.to_string(),
                t: "CombineError".to_string(),
                data: serde_json::to_value(DkError::from(&e)).unwrap_or_else(|e| {
                    json!({
                        "message": format!("Failed to ser error message: {e}"),
                    })
                }),
            }),
        }
    }

    fn ping(&self) -> String {
        Message::ok(&"pong")
    }

    fn is_efi(&self) -> String {
        Message::ok(&is_efi_booted())
    }

    fn sync_disk(&self) -> String {
        sync_disk();

        Message::ok(&"")
    }

    fn sync_and_reboot(&self) -> String {
        let res = sync_and_reboot();

        match res {
            Ok(()) => Message::ok(&""),
            Err(e) => Message::err(e.to_string()),
        }
    }

    fn is_lvm_device(&self, p: &str) -> String {
        let res = is_lvm_device(Path::new(p));

        match res {
            Ok(v) => Message::ok(&v),
            Err(e) => Message::err(e.to_string()),
        }
    }
}

fn set_config_inner(
    config: &mut InstallConfigPrepare,
    field: &str,
    value: &str,
) -> Result<(), DkError> {
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
            let download_type =
                serde_json::from_str::<DownloadType>(value).map_err(|e| DkError {
                    message: e.to_string(),
                    t: "SetValue".to_string(),
                    data: {
                        json!({
                            "field": "download".to_string(),
                            "value": value.to_string(),
                        })
                    },
                })?;

            config.download = Some(download_type);

            Ok(())
        }
        "user" => {
            let user = serde_json::from_str::<User>(value).map_err(|e| DkError {
                message: e.to_string(),
                t: "SetValue".to_string(),
                data: {
                    json!({
                        "field": "user".to_string(),
                        "value": value.to_string(),
                    })
                },
            })?;

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
            _ => Err(DkError {
                message: "rtc_as_localtime must be 0 or 1".to_string(),
                t: "SetValue".to_string(),
                data: {
                    json!({
                        "field": "rtc_as_localtime".to_string(),
                        "value": value.to_string(),
                    })
                },
            }),
        },
        "target_partition" => {
            #[cfg(not(debug_assertions))]
            {
                let p = serde_json::from_str::<DkPartition>(value).map_err(|e| DkError {
                    message: e.to_string(),
                    t: "SetValue".to_string(),
                    data: {
                        json!({
                            "field": "target_partition".to_string(),
                            "value": value.to_string(),
                        })
                    },
                })?;
                config.target_partition = Arc::new(Mutex::new(Some(p)));
                Ok(())
            }
            #[cfg(debug_assertions)]
            {
                let _p = serde_json::from_str::<DkPartition>(value).map_err(|e| DkError {
                    message: e.to_string(),
                    t: "SetValue".to_string(),
                    data: {
                        json!({
                            "field": "target_partition".to_string(),
                            "value": value.to_string(),
                        })
                    },
                })?;
                config.target_partition = Arc::new(Mutex::new(Some(DkPartition {
                    path: Some(PathBuf::from("/dev/loop30p1")),
                    parent_path: Some(PathBuf::from("/dev/loop30")),
                    fs_type: Some("ext4".to_string()),
                    size: 50 * 1024 * 1024 * 1024,
                    os: None,
                })));
                Ok(())
            }
        }
        "efi_partition" => {
            #[cfg(not(debug_assertions))]
            {
                let p = serde_json::from_str::<DkPartition>(value).map_err(|e| DkError {
                    message: e.to_string(),
                    t: "SetValue".to_string(),
                    data: {
                        json!({
                            "field": "efi_partition".to_string(),
                            "value": value.to_string(),
                        })
                    },
                })?;
                config.efi_partition = Arc::new(Mutex::new(Some(p)));
            }

            #[cfg(debug_assertions)]
            {
                let _p = serde_json::from_str::<DkPartition>(value).map_err(|e| DkError {
                    message: e.to_string(),
                    t: "SetValue".to_string(),
                    data: {
                        json!({
                            "field": "efi_partition".to_string(),
                            "value": value.to_string(),
                        })
                    },
                })?;
                config.efi_partition = Arc::new(Mutex::new(Some(DkPartition {
                    path: Some(PathBuf::from("/dev/loop30p2")),
                    parent_path: Some(PathBuf::from("/dev/loop30")),
                    fs_type: Some("vfat".to_string()),
                    size: 512 * 1024 * 1024,
                    os: None,
                })));
            }

            Ok(())
        }
        "swapfile" => {
            config.swapfile = serde_json::from_str::<SwapFile>(value).map_err(|e| DkError {
                message: e.to_string(),
                t: "SetValue".to_string(),
                data: {
                    json!({
                        "field": "swapfile".to_string(),
                        "value": value.to_string(),
                    })
                },
            })?;
            Ok(())
        }
        _ => {
            error!("Unknown field: {field}");
            Err(DkError {
                message: "Unknown field".to_string(),
                t: "SetValue".to_string(),
                data: {
                    json!({
                        "field": field.to_string(),
                        "value": value.to_string(),
                    })
                },
            })
        }
    }
}

fn start_install_inner(
    config: InstallConfigPrepare,
    step: Arc<AtomicU8>,
    progress: Arc<AtomicU8>,
    v: Arc<AtomicUsize>,
    ps: Arc<Mutex<ProgressStatus>>,
    cancel_install: Arc<AtomicBool>,
) -> Result<JoinHandle<()>, DkError> {
    let mut config = InstallConfig::try_from(config).map_err(|e| DkError::from(&e))?;

    info!("Starting install");

    let temp_dir = tempfile::tempdir()
        .map_err(|e| InstallErr::CreateTempDir { source: e })
        .map_err(|e| DkError::from(&e))?
        .into_path()
        .to_path_buf();

    let tmp_dir = Arc::new(temp_dir);
    let tmp_dir_clone2 = tmp_dir.clone();
    let tmp_dir_clone3 = tmp_dir.clone();

    if let DownloadType::Http { to_path, .. } = &mut config.download {
        *to_path = Some(tmp_dir.join("squashfs"));
    }

    let root_fd = get_dir_fd(Path::new("/"))
        .map_err(|e| InstallErr::GetDirFd { source: e })
        .map_err(|e| DkError::from(&e))?;

    let root_fd_clone = root_fd
        .try_clone()
        .map_err(|e| InstallErr::CloneFd { source: e })
        .map_err(|e| DkError::from(&e))?;

    ctrlc::set_handler(move || {
        if let Ok(root_fd) = root_fd_clone.try_clone() {
            exit_env(root_fd, tmp_dir_clone3.clone());
        } else {
            warn!("Failed to clone root_fd");
        }

        exit(1);
    })
    .ok();

    let ps_clone = ps.clone();

    let cancel_install_clone = cancel_install.clone();

    let t = thread::spawn(move || {
        let t = tmp_dir_clone2.clone();
        let t2 = tmp_dir_clone2.clone();
        let install_thread = thread::spawn(move || {
            let res = config
                .start_install(
                    step.clone(),
                    progress.clone(),
                    v.clone(),
                    t.clone(),
                    cancel_install_clone,
                )
                .map_err(|e| DkError::from(&e));

            if let Err(e) = res {
                {
                    let mut ps = ps_clone.lock().unwrap();
                    *ps = ProgressStatus::Error(e);
                }
            }
        });

        let mut is_cancel = false;

        loop {
            if !is_cancel {
                is_cancel = cancel_install.load(Ordering::SeqCst);
            };

            if install_thread.is_finished() {
                // 需要先确保安装线程已经结束再退出环境
                if is_cancel {
                    exit_env(root_fd, tmp_dir_clone2.clone());
                    cancel_install.store(false, Ordering::SeqCst);
                    {
                        let mut ps = ps.lock().unwrap();
                        *ps = ProgressStatus::Pending;
                    }
                    return;
                }

                let mut ps = ps.lock().unwrap();

                if let ProgressStatus::Error(e) = &*ps {
                    error!("Failed to install system: {e:?}");
                    exit_env(root_fd, t2);
                    return;
                }

                *ps = ProgressStatus::Finish;
                return;
            }

            thread::sleep(Duration::from_millis(10));
        }
    });

    Ok(t)
}

fn exit_env(root_fd: OwnedFd, tmp_dir: Arc<PathBuf>) {
    sync_disk();
    escape_chroot(root_fd).ok();

    sync_disk();
    swapoff(&tmp_dir).ok();

    sync_disk();
    remove_files_mounts(&tmp_dir).ok();

    let efi_path = tmp_dir.join("efi");
    if is_efi_booted() {
        sync_disk();
        for _ in 0..3 {
            if umount_root_path(&efi_path).is_ok() {
                break;
            }
            thread::sleep(Duration::from_secs(5));
        }
    }

    let mut res = Ok(());
    for _ in 0..3 {
        sync_disk();
        res = umount_root_path(&tmp_dir);

        if res.is_ok() {
            break;
        }

        thread::sleep(Duration::from_secs(5));
    }

    if res.is_err() {
        umount_all(&tmp_dir);
    }
}
