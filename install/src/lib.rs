use std::{
    fmt::{Display, Formatter},
    fs,
    os::fd::OwnedFd,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use disk::{
    is_efi_booted,
    partition::{format_partition, DkPartition},
    PartitionError,
};

use download::download_file;
use extract::extract_squashfs;
use genfstab::genfstab_to_file;
use mount::mount_root_path;
use num_enum::IntoPrimitive;
use serde::{Deserialize, Serialize};
use sysinfo::System;
use thiserror::Error;
use tracing::{debug, error, info};

use crate::{
    chroot::{dive_into_guest, escape_chroot, get_dir_fd},
    dracut::execute_dracut,
    genfstab::write_swap_entry_to_fstab,
    grub::execute_grub_install,
    hostname::set_hostname,
    locale::{set_hwclock_tc, set_locale},
    mount::{remove_files_mounts, umount_root_path},
    ssh::gen_ssh_key,
    swap::{create_swapfile, get_recommend_swap_size, swapoff},
    user::{add_new_user, passwd_set_fullname},
    zoneinfo::set_zoneinfo,
};

pub mod chroot;
mod download;
mod dracut;
mod extract;
mod genfstab;
mod grub;
mod hostname;
mod locale;
pub mod mount;
mod ssh;
pub mod swap;
mod user;
mod utils;
mod zoneinfo;

#[derive(Debug, Error)]
pub enum InstallError {
    #[error("Failed to unpack: {0}")]
    Unpack(std::io::Error),
    #[error("Failed to run command {command}, err: {err}")]
    RunCommand {
        command: String,
        err: std::io::Error,
    },
    #[error("Failed to mount filesystem device to {mount_point}, err: {err}")]
    MountFs {
        mount_point: String,
        err: std::io::Error,
    },
    #[error("Failed to umount filesystem device from {mount_point}, err: {err}")]
    UmountFs {
        mount_point: String,
        err: std::io::Error,
    },
    #[error("Failed to operate file or directory {path}, err: {err}")]
    OperateFile { path: String, err: std::io::Error },
    #[error("Full name is illegal: {0}")]
    FullNameIllegal(String),
    #[error("/etc/passwd is illegal")]
    PasswdIllegal,
    #[error("Failed to generate /etc/fstab: {0:?}")]
    GenFstab(GenFstabErrorKind),
    #[error(transparent)]
    Reqwest(#[from] reqwest::Error),
    #[error("Failed to download squashfs, checksum mismatch")]
    ChecksumMisMatch,
    #[error("Failed to create tokio runtime: {0}")]
    CreateTokioRuntime(std::io::Error),
    #[error("Value {0:?} is not set")]
    IsNotSet(NotSetValue),
    #[error(transparent)]
    Partition(#[from] PartitionError),
    #[error("Partition value: {0:?} is none")]
    PartitionValueIsNone(PartitionNotSetValue),
    #[error("Local file {0:?} is not found")]
    LocalFileNotFound(String),
    #[error("Download path is not set")]
    DownloadPathIsNotSet,
}

#[derive(Debug)]
pub enum PartitionNotSetValue {
    Path,
    FsType,
}

#[derive(Debug)]
pub enum NotSetValue {
    Locale,
    Timezone,
    Flaver,
    Download,
    User,
    Hostname,
    TargetPartition,
}

#[derive(Debug)]
pub enum PasswdIllegalKind {
    Username,
    Time,
    Uid,
    Gid,
    Fullname,
    Home,
    LoginShell,
}

#[derive(Debug)]
pub enum GenFstabErrorKind {
    UnsupportedFileSystem(String),
    UUID,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum DownloadType {
    Http {
        url: String,
        hash: String,
        to_path: Option<PathBuf>,
    },
    File(PathBuf),
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct InstallConfigPrepare {
    pub locale: Option<String>,
    pub timezone: Option<String>,
    pub download: Option<DownloadType>,
    pub user: Option<User>,
    pub rtc_as_localtime: bool,
    pub hostname: Option<String>,
    pub swapfile: SwapFile,
    pub target_partition: Arc<Mutex<Option<DkPartition>>>,
    pub efi_partition: Arc<Mutex<Option<DkPartition>>>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct User {
    pub username: String,
    pub password: String,
    pub root_password: Option<String>,
    pub full_name: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub enum SwapFile {
    Automatic,
    Custom(u64),
    Disable,
}

impl Default for InstallConfigPrepare {
    fn default() -> Self {
        Self {
            locale: None,
            timezone: None,
            download: None,
            user: None,
            rtc_as_localtime: false,
            hostname: None,
            swapfile: SwapFile::Automatic,
            target_partition: Arc::new(Mutex::new(None)),
            efi_partition: Arc::new(Mutex::new(None)),
        }
    }
}

pub struct InstallConfig {
    local: String,
    timezone: String,
    pub download: DownloadType,
    user: User,
    rtc_as_localtime: bool,
    hostname: String,
    swapfile: SwapFile,
    pub target_partition: DkPartition,
    efi_partition: Option<DkPartition>,
}

impl TryFrom<InstallConfigPrepare> for InstallConfig {
    type Error = InstallError;

    fn try_from(value: InstallConfigPrepare) -> Result<Self, Self::Error> {
        Ok(Self {
            local: value
                .locale
                .ok_or(InstallError::IsNotSet(NotSetValue::Locale))?,
            timezone: value
                .timezone
                .ok_or(InstallError::IsNotSet(NotSetValue::Timezone))?,
            download: value
                .download
                .ok_or(InstallError::IsNotSet(NotSetValue::Download))?,
            user: value
                .user
                .ok_or(InstallError::IsNotSet(NotSetValue::User))?,
            rtc_as_localtime: value.rtc_as_localtime,
            hostname: value
                .hostname
                .ok_or(InstallError::IsNotSet(NotSetValue::Hostname))?,
            swapfile: value.swapfile,
            target_partition: {
                let lock = value.target_partition.lock().unwrap();

                lock.clone()
                    .ok_or(InstallError::IsNotSet(NotSetValue::TargetPartition))?
            },
            efi_partition: {
                let lock = value.efi_partition.lock().unwrap();

                lock.clone()
            },
        })
    }
}

macro_rules! cancel_install_exit {
    ($cancel_install:ident) => {
        if $cancel_install.load(Ordering::SeqCst) {
            return Ok(false);
        }
    };
}

#[derive(Clone, IntoPrimitive)]
#[repr(u8)]
enum InstallationStage {
    SetupPartition = 1,
    DownloadSquashfs,
    ExtractSquashfs,
    GenerateFstab,
    Chroot,
    Dracut,
    InstallGrub,
    GenerateSshKey,
    ConfigureSystem,
    EscapeChroot,
    PostInstallation,
    Done,
}

impl Default for InstallationStage {
    fn default() -> Self {
        Self::SetupPartition
    }
}

impl Display for InstallationStage {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::SetupPartition => "setup partition",
            Self::DownloadSquashfs => "download squashfs",
            Self::ExtractSquashfs => "extract squashfs",
            Self::GenerateFstab => "generate fstab",
            Self::Chroot => "chroot",
            Self::Dracut => "run dracut",
            Self::InstallGrub => "install grub",
            Self::GenerateSshKey => "generate ssh key",
            Self::ConfigureSystem => "configure system",
            Self::EscapeChroot => "escape chroot",
            Self::PostInstallation => "post installation",
            Self::Done => "done",
        };

        write!(f, "{s}")
    }
}

impl InstallationStage {
    fn get_next_stage(&self) -> Self {
        match self {
            Self::SetupPartition => Self::DownloadSquashfs,
            Self::DownloadSquashfs => Self::ExtractSquashfs,
            Self::ExtractSquashfs => Self::GenerateFstab,
            Self::GenerateFstab => Self::Chroot,
            Self::Chroot => Self::Dracut,
            Self::Dracut => Self::InstallGrub,
            Self::InstallGrub => Self::GenerateSshKey,
            Self::GenerateSshKey => Self::ConfigureSystem,
            Self::ConfigureSystem => Self::EscapeChroot,
            Self::EscapeChroot => Self::PostInstallation,
            Self::PostInstallation => Self::Done,
            Self::Done => Self::Done,
        }
    }
}

impl InstallConfig {
    pub fn start_install<F, F2, F3>(
        &self,
        step: F,
        progress: F2,
        velocity: F3,
        tmp_mount_path: PathBuf,
        cancel_install: Arc<AtomicBool>,
    ) -> Result<bool, InstallError>
    where
        F: Fn(u8),
        F2: Fn(f64) + Send + Sync + 'static,
        F3: Fn(usize) + Send + Sync + 'static,
    {
        let progress = Arc::new(progress);
        let velocity = Arc::new(velocity);
        let root_fd = get_dir_fd(Path::new("/"))?;

        let mut stage = InstallationStage::default();

        let mut squashfs_path = None;
        let mut squashfs_total_size = None;

        let mut error_retry = 1;

        loop {
            debug!("Current stage: {stage}");

            // Done 只是为了编码方便，并不是真正的阶段
            if !matches!(stage, InstallationStage::Done) {
                // GUI 用户体验需求，一些步骤不应该执行 step 回掉
                let num = match stage {
                    InstallationStage::SetupPartition => 1,
                    InstallationStage::DownloadSquashfs => 2,
                    InstallationStage::ExtractSquashfs => 3,
                    InstallationStage::GenerateFstab => 4,
                    InstallationStage::Chroot => 4,
                    InstallationStage::Dracut => 5,
                    InstallationStage::InstallGrub => 6,
                    InstallationStage::GenerateSshKey => 7,
                    InstallationStage::ConfigureSystem => 8,
                    InstallationStage::EscapeChroot => 8,
                    InstallationStage::PostInstallation => 8,
                    InstallationStage::Done => 8,
                };

                step(num);
            }

            let res = match stage {
                InstallationStage::SetupPartition => {
                    self.setup_partition::<F2>(&progress, &tmp_mount_path, &cancel_install)
                }
                InstallationStage::DownloadSquashfs => self.download_squashfs::<F2, F3>(
                    Arc::clone(&progress),
                    Arc::clone(&velocity),
                    Arc::clone(&cancel_install),
                    (&mut squashfs_path, &mut squashfs_total_size),
                ),
                InstallationStage::ExtractSquashfs => self.extract_squashfs::<F2, F3>(
                    &progress,
                    &velocity,
                    &tmp_mount_path,
                    &cancel_install,
                    // 若能进行到这一步，则 squashfs_total_size 一定有值，故 unwrap 安全
                    squashfs_total_size.unwrap(),
                    squashfs_path.clone().unwrap(),
                ),
                InstallationStage::GenerateFstab => {
                    self.generate_fstab(progress.as_ref(), &tmp_mount_path, &cancel_install)
                }
                InstallationStage::Chroot => {
                    self.chroot(progress.as_ref(), &tmp_mount_path, &cancel_install)
                }
                InstallationStage::Dracut => run_dracut(&cancel_install, &progress),
                InstallationStage::InstallGrub => {
                    self.install_grub(progress.as_ref(), &cancel_install)
                }
                InstallationStage::GenerateSshKey => {
                    self.generate_ssh_key(progress.as_ref(), &cancel_install)
                }
                InstallationStage::ConfigureSystem => {
                    self.configure_system(progress.as_ref(), &cancel_install)
                }
                InstallationStage::EscapeChroot => {
                    self.escape_chroot(progress.as_ref(), &cancel_install, &root_fd)
                }
                InstallationStage::PostInstallation => {
                    self.post_installation(progress.as_ref(), &tmp_mount_path)
                }
                InstallationStage::Done => break,
            };

            stage = match res {
                Ok(v) if v => stage.get_next_stage(),
                Ok(_) => break,
                Err(e) => {
                    error!("Error occured in step {stage}: {e}");

                    if error_retry == 3 {
                        return Err(e);
                    }

                    error_retry += 1;

                    // TODO: 暂停安装，错误处理逻辑。目前临时的占位方案是等待并重试
                    std::thread::sleep(Duration::from_secs(10));
                    stage
                }
            };
        }

        Ok(true)
    }

    fn chroot<F>(
        &self,
        progress: &F,
        tmp_mount_path: &Path,
        cancel_install: &Arc<AtomicBool>,
    ) -> Result<bool, InstallError>
    where
        F: Fn(f64) + Send + Sync + 'static,
    {
        progress(0.0);

        cancel_install_exit!(cancel_install);

        info!("Chroot to installed system ...");
        dive_into_guest(tmp_mount_path)?;

        cancel_install_exit!(cancel_install);
        progress(100.0);

        Ok(true)
    }

    fn generate_fstab<F>(
        &self,
        progress: &F,
        tmp_mount_path: &Path,
        cancel_install: &Arc<AtomicBool>,
    ) -> Result<bool, InstallError>
    where
        F: Fn(f64) + Send + Sync + 'static,
    {
        progress(0.0);
        cancel_install_exit!(cancel_install);

        info!("Generate /etc/fstab");
        self.genfatab(tmp_mount_path)?;

        cancel_install_exit!(cancel_install);
        progress(100.0);

        Ok(true)
    }

    fn setup_partition<F>(
        &self,
        progress: &F,
        tmp_mount_path: &Path,
        cancel_install: &Arc<AtomicBool>,
    ) -> Result<bool, InstallError>
    where
        F: Fn(f64) + Send + Sync + 'static,
    {
        progress(0.0);

        self.format_partitions()?;
        cancel_install_exit!(cancel_install);

        self.mount_partitions(tmp_mount_path)?;
        cancel_install_exit!(cancel_install);

        progress(50.0);

        match self.swapfile {
            SwapFile::Automatic => {
                let mut sys = System::new_all();
                sys.refresh_memory();
                let total_memory = sys.total_memory();
                let size = get_recommend_swap_size(total_memory);
                cancel_install_exit!(cancel_install);
                create_swapfile(size, tmp_mount_path)?;
            }
            SwapFile::Custom(size) => {
                cancel_install_exit!(cancel_install);
                create_swapfile(size as f64, tmp_mount_path)?;
            }
            SwapFile::Disable => {}
        }

        progress(100.0);

        Ok(true)
    }

    fn download_squashfs<F1, F2>(
        &self,
        progress: Arc<F1>,
        velocity: Arc<F2>,
        cancel_install: Arc<AtomicBool>,
        res: (&mut Option<PathBuf>, &mut Option<usize>),
    ) -> Result<bool, InstallError>
    where
        F1: Fn(f64) + Send + Sync + 'static,
        F2: Fn(usize) + Send + Sync + 'static,
    {
        progress(0.0);

        cancel_install_exit!(cancel_install);

        let (squashfs_path, total_size) =
            download_file(&self.download, progress, velocity, cancel_install)?;

        *res.0 = Some(squashfs_path);
        *res.1 = Some(total_size);

        Ok(true)
    }

    fn extract_squashfs<F1, F2>(
        &self,
        progress: &F1,
        velocity: &F2,
        tmp_mount_path: &Path,
        cancel_install: &Arc<AtomicBool>,
        total_size: usize,
        squashfs_path: PathBuf,
    ) -> Result<bool, InstallError>
    where
        F1: Fn(f64) + Send + Sync + 'static,
        F2: Fn(usize) + Send + Sync + 'static,
    {
        progress(0.0);

        cancel_install_exit!(cancel_install);

        extract_squashfs(
            total_size as f64,
            squashfs_path,
            tmp_mount_path.to_path_buf(),
            progress,
            velocity,
            cancel_install.clone(),
        )?;

        cancel_install_exit!(cancel_install);

        velocity(0);

        Ok(true)
    }

    fn install_grub<F>(
        &self,
        progress: &F,
        cancel_install: &Arc<AtomicBool>,
    ) -> Result<bool, InstallError>
    where
        F: Fn(f64) + Send + Sync + 'static,
    {
        progress(0.0);
        cancel_install_exit!(cancel_install);

        info!("Installing grub ...");
        self.install_grub_impl()?;

        cancel_install_exit!(cancel_install);
        progress(100.0);

        Ok(true)
    }

    fn generate_ssh_key<F>(
        &self,
        progress: &F,
        cancel_install: &Arc<AtomicBool>,
    ) -> Result<bool, InstallError>
    where
        F: Fn(f64) + Send + Sync + 'static,
    {
        progress(0.0);
        cancel_install_exit!(cancel_install);

        info!("Generating SSH key ...");
        gen_ssh_key()?;

        cancel_install_exit!(cancel_install);
        progress(100.0);

        Ok(true)
    }

    fn escape_chroot<F>(
        &self,
        progress: &F,
        cancel_install: &Arc<AtomicBool>,
        root_fd: &OwnedFd,
    ) -> Result<bool, InstallError>
    where
        F: Fn(f64) + Send + Sync + 'static,
    {
        progress(0.0);
        cancel_install_exit!(cancel_install);

        info!("Escape chroot ...");
        // 如果能走到这里，则 owned_root_fd 一定为 Some，故此处 unwrap 安全
        escape_chroot(root_fd)?;

        progress(100.0);

        Ok(true)
    }

    fn configure_system<F>(
        &self,
        progress: &F,
        cancel_install: &Arc<AtomicBool>,
    ) -> Result<bool, InstallError>
    where
        F: Fn(f64) + Send + Sync + 'static,
    {
        progress(0.0);

        cancel_install_exit!(cancel_install);

        if self.swapfile != SwapFile::Disable {
            write_swap_entry_to_fstab()?;
        }

        cancel_install_exit!(cancel_install);

        progress(25.0);

        cancel_install_exit!(cancel_install);

        info!("Setting timezone as {} ...", self.timezone);
        set_zoneinfo(&self.timezone)?;

        cancel_install_exit!(cancel_install);

        info!("Setting rtc_as_localtime ...");
        set_hwclock_tc(!self.rtc_as_localtime)?;
        progress(50.0);

        cancel_install_exit!(cancel_install);

        info!("Setting hostname as {}", self.hostname);
        set_hostname(&self.hostname)?;
        progress(75.0);

        cancel_install_exit!(cancel_install);

        info!("Setting User ...");
        add_new_user(&self.user.username, &self.user.password)?;

        cancel_install_exit!(cancel_install);

        if let Some(full_name) = &self.user.full_name {
            passwd_set_fullname(full_name, &self.user.username)?;
        }

        cancel_install_exit!(cancel_install);

        progress(80.0);

        info!("Setting locale ...");
        set_locale(&self.local)?;
        progress(100.0);

        Ok(true)
    }

    fn post_installation<F>(
        &self,
        progress: &F,
        tmp_mount_path: &Path,
    ) -> Result<bool, InstallError>
    where
        F: Fn(f64) + Send + Sync + 'static,
    {
        progress(0.0);

        if self.swapfile != SwapFile::Disable || self.swapfile != SwapFile::Custom(0) {
            let mut retry = 1;
            while let Err(e) = swapoff(tmp_mount_path) {
                debug!("swapoff has error: {e:?}, retry {} times", retry);

                if retry == 5 {
                    break;
                }

                retry += 1;
                std::thread::sleep(Duration::from_millis(500));
            }
        }

        info!("Removing mounts ...");
        remove_files_mounts(tmp_mount_path)?;

        info!("Unmounting filesystems...");

        if is_efi_booted() {
            umount_root_path(&tmp_mount_path.join("efi"))?;
        }

        umount_root_path(tmp_mount_path)?;

        progress(100.0);

        Ok(true)
    }

    fn install_grub_impl(&self) -> Result<bool, InstallError> {
        if self.efi_partition.is_some() {
            info!("Installing grub to UEFI partition ...");
            execute_grub_install(None)?;
        } else {
            info!("Installing grub to MBR partition ...");
            execute_grub_install(Some(self.target_partition.parent_path.as_ref().unwrap()))?;
        }

        Ok(true)
    }

    fn genfatab(&self, tmp_mount_path: &Path) -> Result<bool, InstallError> {
        genfstab_to_file(
            self.target_partition
                .path
                .as_ref()
                .ok_or(InstallError::PartitionValueIsNone(
                    PartitionNotSetValue::Path,
                ))?,
            self.target_partition
                .fs_type
                .as_ref()
                .ok_or(InstallError::PartitionValueIsNone(
                    PartitionNotSetValue::FsType,
                ))?,
            tmp_mount_path,
            Path::new("/"),
        )?;

        if let Some(ref efi_partition) = self.efi_partition {
            genfstab_to_file(
                efi_partition
                    .path
                    .as_ref()
                    .ok_or(InstallError::PartitionValueIsNone(
                        PartitionNotSetValue::Path,
                    ))?,
                efi_partition
                    .fs_type
                    .as_ref()
                    .ok_or(InstallError::PartitionValueIsNone(
                        PartitionNotSetValue::FsType,
                    ))?,
                tmp_mount_path,
                Path::new("/efi"),
            )?;
        }

        Ok(true)
    }

    fn mount_partitions(&self, tmp_mount_path: &Path) -> Result<bool, InstallError> {
        mount_root_path(
            self.target_partition.path.as_deref(),
            tmp_mount_path,
            self.target_partition
                .fs_type
                .as_ref()
                .ok_or(InstallError::PartitionValueIsNone(
                    PartitionNotSetValue::FsType,
                ))?,
        )?;

        if let Some(ref efi) = self.efi_partition {
            let efi_mount_path = tmp_mount_path.join("efi");
            fs::create_dir_all(&efi_mount_path).map_err(|e| InstallError::OperateFile {
                path: efi_mount_path.display().to_string(),
                err: e,
            })?;

            mount_root_path(
                efi.path.as_deref(),
                &efi_mount_path,
                efi.fs_type
                    .as_ref()
                    .ok_or(InstallError::PartitionValueIsNone(
                        PartitionNotSetValue::FsType,
                    ))?,
            )?;
        }

        Ok(true)
    }

    fn format_partitions(&self) -> Result<bool, InstallError> {
        format_partition(&self.target_partition)?;

        if let Some(ref efi) = self.efi_partition {
            let mut efi = efi.clone();
            if efi.fs_type.is_none() {
                efi.fs_type = Some("vfat".to_string());
                format_partition(&efi)?;
            }
        }

        Ok(true)
    }
}

fn run_dracut<F>(cancel_install: &Arc<AtomicBool>, progress: &Arc<F>) -> Result<bool, InstallError>
where
    F: Fn(f64) + Send + Sync + 'static,
{
    info!("Running dracut ...");
    cancel_install_exit!(cancel_install);
    progress(0.0);
    execute_dracut()?;
    progress(100.0);
    cancel_install_exit!(cancel_install);
    Ok(true)
}
