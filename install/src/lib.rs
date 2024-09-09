use std::{
    fmt::{Display, Formatter},
    fs,
    io::{self, Write},
    os::fd::OwnedFd,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use chroot::ChrootError;
use disk::{
    is_efi_booted,
    partition::{format_partition, DkPartition},
    PartitionError,
};

use download::{download_file, DownloadError, FilesType};
use extract::{extract_squashfs, rsync_system, RsyncError};
use genfstab::{genfstab_to_file, GenfstabError};
use grub::RunGrubError;
use locale::SetHwclockError;
use mount::{mount_root_path, UmountError};
use num_enum::IntoPrimitive;
use rustix::{
    fs::sync,
    io::Errno,
    system::{reboot, RebootCommand},
};
use serde::{Deserialize, Serialize};
use snafu::{OptionExt, ResultExt, Snafu};
use swap::SwapFileError;
use sysinfo::System;
use tracing::{debug, error, info};
use user::{AddUserError, SetFullNameError};
use utils::RunCmdError;
use zoneinfo::SetZoneinfoError;

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
pub mod download;
mod dracut;
mod extract;
pub mod genfstab;
pub mod grub;
mod hostname;
pub mod locale;
pub mod mount;
mod ssh;
pub mod swap;
pub mod user;
pub mod utils;
pub mod zoneinfo;

#[derive(Debug, Snafu)]
pub enum MountError {
    #[snafu(display("Failed to create dir {}", path.display()))]
    CreateDir {
        source: std::io::Error,
        path: PathBuf,
    },
    #[snafu(display("Failed to mount {}", path.display()))]
    MountRoot { source: Errno, path: PathBuf },
    #[snafu(display("value is not set: {t}"))]
    ValueNotSetMount { t: &'static str },
}

#[derive(Debug, Snafu)]
pub enum SetupPartitionError {
    #[snafu(display("Failed to format partition"))]
    Format { source: PartitionError },
    #[snafu(display("Failed to mount partition"))]
    Mount { source: MountError },
    #[snafu(display("Failed to create swap file"))]
    SwapFile { source: SwapFileError },
}

#[derive(Debug, Snafu)]
pub enum InstallErr {
    #[snafu(display("Failed to clone fd"))]
    CloneFd { source: std::io::Error },
    #[snafu(display("Failed to create tempdir"))]
    CreateTempDir { source: std::io::Error },
    #[snafu(display("Value is not set: {v:?}"))]
    ValueNotSet { v: NotSetValue },
    #[snafu(display("Failed to get root dir fd"))]
    GetDirFd { source: Errno },
    #[snafu(display("Failed to setup partition"))]
    SetupPartition { source: SetupPartitionError },
    #[snafu(display("Failed to download squashfs"))]
    DownloadSquashfs { source: download::DownloadError },
    #[snafu(display("Failed to extract squashfs"))]
    ExtractSquashfs { source: InstallSquashfsError },
    #[snafu(display("Failed to generate fstab"))]
    Genfstab { source: SetupGenfstabError },
    #[snafu(display("Failed to chroot"))]
    Chroot { source: ChrootError },
    #[snafu(display("Failed to run dracut"))]
    Dracut { source: RunCmdError },
    #[snafu(display("Failed to install grub"))]
    Grub { source: RunGrubError },
    #[snafu(display("Failed to generate ssh key"))]
    GenerateSshKey { source: RunCmdError },
    #[snafu(display("Failed to configure system"))]
    ConfigureSystem { source: ConfigureSystemError },
    #[snafu(display("Failed to escape chroot"))]
    EscapeChroot { source: ChrootError },
    #[snafu(display("Failed to post installation"))]
    PostInstallation { source: PostInstallationError },
}

#[derive(Debug, Snafu)]
pub enum PostInstallationError {
    #[snafu(display("Failed to umount point"))]
    Umount { source: UmountError },
}

#[derive(Debug, Snafu)]
pub enum ConfigureSystemError {
    #[snafu(display("Failed to append swap config to fstab"))]
    SwapToGenfstab { source: GenfstabError },
    #[snafu(display("Failed to set zoneinfo: {zone}"))]
    SetZoneinfo {
        source: SetZoneinfoError,
        zone: String,
    },
    #[snafu(display("Failed to set hwclock: is_rtc: {is_rtc}"))]
    SetHwclock {
        source: SetHwclockError,
        is_rtc: bool,
    },
    #[snafu(display("Failed to set hostname: {hostname}"))]
    SetHostname {
        source: std::io::Error,
        hostname: String,
    },
    #[snafu(display("Failed to add new user"))]
    AddNewUser { source: AddUserError },
    #[snafu(display("Failed to set fullname: {fullname}"))]
    SetFullName {
        source: SetFullNameError,
        fullname: String,
    },
    #[snafu(display("Failed to set locale: {locale}"))]
    SetLocale {
        source: std::io::Error,
        locale: String,
    },
}

#[derive(Debug, Snafu)]
pub enum SetupGenfstabError {
    #[snafu(transparent)]
    Genfstab { source: GenfstabError },
    #[snafu(display("value is not set: {t}"))]
    ValueNotSetGenfstab { t: &'static str },
}

#[derive(Debug, Snafu)]
pub enum InstallSquashfsError {
    #[snafu(display("Failed to extract squashfs {} to {}", from.display(), to.display()))]
    Extract {
        source: std::io::Error,
        from: PathBuf,
        to: PathBuf,
    },
    #[snafu(display("Failed to remove downloaded squashfs file"))]
    RemoveDownloadedFile { source: std::io::Error },
    #[snafu(transparent)]
    RsyncError { source: RsyncError },
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

impl Display for NotSetValue {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            NotSetValue::Locale => write!(f, "locale"),
            NotSetValue::Timezone => write!(f, "timezone"),
            NotSetValue::Flaver => write!(f, "flaver"),
            NotSetValue::Download => write!(f, "download"),
            NotSetValue::User => write!(f, "user"),
            NotSetValue::Hostname => write!(f, "hostname"),
            NotSetValue::TargetPartition => write!(f, "target partition"),
        }
    }
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
    Dir(PathBuf),
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
    type Error = InstallErr;

    fn try_from(value: InstallConfigPrepare) -> Result<Self, Self::Error> {
        Ok(Self {
            local: value.locale.context(ValueNotSetSnafu {
                v: NotSetValue::Locale,
            })?,
            timezone: value.timezone.context(ValueNotSetSnafu {
                v: NotSetValue::Timezone,
            })?,
            download: value.download.context(ValueNotSetSnafu {
                v: NotSetValue::Download,
            })?,
            user: value.user.context(ValueNotSetSnafu {
                v: NotSetValue::User,
            })?,
            rtc_as_localtime: value.rtc_as_localtime,
            hostname: value.hostname.context(ValueNotSetSnafu {
                v: NotSetValue::Hostname,
            })?,
            swapfile: value.swapfile,
            target_partition: {
                let lock = value.target_partition.lock().unwrap();

                lock.clone().context(ValueNotSetSnafu {
                    v: NotSetValue::TargetPartition,
                })?
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
    SwapOff,
    UmountInnerPath,
    UmountEFIPath,
    UmountRootPath,
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
            Self::SwapOff => "swap off",
            Self::UmountInnerPath => "umount inner path",
            Self::UmountEFIPath => "umount EFI path",
            Self::UmountRootPath => "umount root path",
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
            Self::EscapeChroot => Self::SwapOff,
            Self::SwapOff => Self::UmountInnerPath,
            Self::UmountInnerPath => Self::UmountEFIPath,
            Self::UmountEFIPath => Self::UmountRootPath,
            Self::UmountRootPath => Self::Done,
            Self::Done => Self::Done,
        }
    }
}

impl InstallConfig {
    pub fn start_install(
        &self,
        step: Arc<AtomicU8>,
        progress: Arc<AtomicU8>,
        velocity: Arc<AtomicUsize>,
        tmp_mount_path: Arc<PathBuf>,
        cancel_install: Arc<AtomicBool>,
    ) -> Result<bool, InstallErr> {
        let root_fd = get_dir_fd(Path::new("/")).context(GetDirFdSnafu)?;

        let mut stage = InstallationStage::default();

        let mut files_type = None;

        let mut error_retry = 1;

        loop {
            debug!("Current stage: {stage}");

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
                InstallationStage::SwapOff => 8,
                InstallationStage::UmountInnerPath => 8,
                InstallationStage::UmountEFIPath => 8,
                InstallationStage::UmountRootPath => 8,
                InstallationStage::Done => 8,
            };

            step.store(num, Ordering::SeqCst);

            let res = match stage {
                InstallationStage::SetupPartition => self
                    .setup_partition(progress.clone(), &tmp_mount_path, &cancel_install)
                    .context(SetupPartitionSnafu),
                InstallationStage::DownloadSquashfs => self
                    .download_squashfs(
                        progress.clone(),
                        velocity.clone(),
                        Arc::clone(&cancel_install),
                        &mut files_type,
                    )
                    .context(DownloadSquashfsSnafu),
                InstallationStage::ExtractSquashfs => self
                    .extract_squashfs(
                        progress.clone(),
                        velocity.clone(),
                        &tmp_mount_path,
                        &cancel_install,
                        // 若能进行到这一步，则 squashfs_total_size 一定有值，故 unwrap 安全
                        files_type.clone().unwrap(),
                    )
                    .context(ExtractSquashfsSnafu),
                InstallationStage::GenerateFstab => self
                    .generate_fstab(progress.clone(), &tmp_mount_path, &cancel_install)
                    .context(GenfstabSnafu),
                InstallationStage::Chroot => self
                    .chroot(progress.clone(), &tmp_mount_path, &cancel_install)
                    .context(ChrootSnafu),
                InstallationStage::Dracut => {
                    run_dracut(cancel_install.clone(), progress.clone()).context(DracutSnafu)
                }
                InstallationStage::InstallGrub => self
                    .install_grub(progress.clone(), &cancel_install)
                    .context(GrubSnafu),
                InstallationStage::GenerateSshKey => self
                    .generate_ssh_key(progress.clone(), &cancel_install)
                    .context(GenerateSshKeySnafu),
                InstallationStage::ConfigureSystem => self
                    .configure_system(progress.clone(), &cancel_install)
                    .context(ConfigureSystemSnafu),
                InstallationStage::EscapeChroot => self
                    .escape_chroot(progress.clone(), &cancel_install, &root_fd)
                    .context(EscapeChrootSnafu),
                InstallationStage::SwapOff => self
                    .swapoff_impl(&tmp_mount_path)
                    .context(PostInstallationSnafu),
                InstallationStage::UmountInnerPath => remove_files_mounts(&tmp_mount_path)
                    .context(UmountSnafu)
                    .context(PostInstallationSnafu)
                    .map(|_| true),
                InstallationStage::UmountEFIPath => {
                    if is_efi_booted() {
                        let path = tmp_mount_path.join("efi");
                        umount_root_path(&path)
                            .context(UmountSnafu)
                            .context(PostInstallationSnafu)
                            .map(|_| true)
                    } else {
                        Ok(true)
                    }
                }
                InstallationStage::UmountRootPath => umount_root_path(&tmp_mount_path)
                    .context(UmountSnafu)
                    .context(PostInstallationSnafu)
                    .map(|_| true),
                InstallationStage::Done => break,
            };

            stage = match res {
                Ok(v) if v => stage.get_next_stage(),
                Ok(_) => break,
                Err(e) => {
                    error!("Error occured in step {stage}: {e:?}");

                    sync();

                    if error_retry == 3 {
                        if matches!(stage, InstallationStage::UmountRootPath)
                            || matches!(stage, InstallationStage::UmountEFIPath)
                            || matches!(stage, InstallationStage::UmountInnerPath)
                        {
                            umount_all(&tmp_mount_path);

                            return Ok(true);
                        }
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

    fn chroot(
        &self,
        progress: Arc<AtomicU8>,
        tmp_mount_path: &Path,
        cancel_install: &Arc<AtomicBool>,
    ) -> Result<bool, ChrootError> {
        progress.store(0, Ordering::SeqCst);

        cancel_install_exit!(cancel_install);

        info!("Chroot to installed system ...");
        dive_into_guest(tmp_mount_path)?;

        cancel_install_exit!(cancel_install);
        progress.store(100, Ordering::SeqCst);

        Ok(true)
    }

    fn generate_fstab(
        &self,
        progress: Arc<AtomicU8>,
        tmp_mount_path: &Path,
        cancel_install: &Arc<AtomicBool>,
    ) -> Result<bool, SetupGenfstabError> {
        progress.store(0, Ordering::SeqCst);
        cancel_install_exit!(cancel_install);

        info!("Generate /etc/fstab");
        self.genfatab(tmp_mount_path)?;

        cancel_install_exit!(cancel_install);
        progress.store(100, Ordering::SeqCst);

        Ok(true)
    }

    fn setup_partition(
        &self,
        progress: Arc<AtomicU8>,
        tmp_mount_path: &Path,
        cancel_install: &Arc<AtomicBool>,
    ) -> Result<bool, SetupPartitionError> {
        progress.store(0, Ordering::SeqCst);

        self.format_partitions().context(FormatSnafu)?;
        cancel_install_exit!(cancel_install);

        self.mount_partitions(tmp_mount_path).context(MountSnafu)?;
        cancel_install_exit!(cancel_install);

        progress.store(50, Ordering::SeqCst);

        match self.swapfile {
            SwapFile::Automatic => {
                let mut sys = System::new_all();
                sys.refresh_memory();
                let total_memory = sys.total_memory();
                let size = get_recommend_swap_size(total_memory);
                cancel_install_exit!(cancel_install);
                create_swapfile(size, tmp_mount_path).context(SwapFileSnafu)?;
            }
            SwapFile::Custom(size) => {
                cancel_install_exit!(cancel_install);
                create_swapfile(size as f64, tmp_mount_path).context(SwapFileSnafu)?;
            }
            SwapFile::Disable => {}
        }

        progress.store(100, Ordering::SeqCst);

        Ok(true)
    }

    fn download_squashfs(
        &self,
        progress: Arc<AtomicU8>,
        velocity: Arc<AtomicUsize>,
        cancel_install: Arc<AtomicBool>,
        res: &mut Option<FilesType>,
    ) -> Result<bool, DownloadError> {
        progress.store(0, Ordering::SeqCst);

        cancel_install_exit!(cancel_install);

        let f = download_file(&self.download, progress, velocity, cancel_install)?;

        *res = Some(f);

        Ok(true)
    }

    fn extract_squashfs(
        &self,
        progress: Arc<AtomicU8>,
        velocity: Arc<AtomicUsize>,
        tmp_mount_path: &Path,
        cancel_install: &Arc<AtomicBool>,
        files_type: FilesType,
    ) -> Result<bool, InstallSquashfsError> {
        progress.store(0, Ordering::SeqCst);

        cancel_install_exit!(cancel_install);

        match files_type {
            FilesType::File {
                path: squashfs_path,
                total: total_size,
            } => {
                extract_squashfs(
                    total_size as f64,
                    squashfs_path.clone(),
                    tmp_mount_path.to_path_buf(),
                    progress,
                    velocity.clone(),
                    cancel_install.clone(),
                )
                .context(ExtractSnafu {
                    from: squashfs_path.clone(),
                    to: tmp_mount_path.to_path_buf(),
                })?;

                cancel_install_exit!(cancel_install);

                if let DownloadType::Http { .. } = self.download {
                    debug!(
                        "Removing downloaded squashfs file {}",
                        squashfs_path.display()
                    );
                    fs::remove_file(&squashfs_path).context(RemoveDownloadedFileSnafu)?;
                }
            }
            FilesType::Dir { path, total } => {
                cancel_install_exit!(cancel_install);

                rsync_system(
                    progress,
                    velocity.clone(),
                    &path,
                    tmp_mount_path,
                    cancel_install.clone(),
                    total,
                )?;

                cancel_install_exit!(cancel_install);
            }
        }

        velocity.store(0, Ordering::SeqCst);

        Ok(true)
    }

    fn install_grub(
        &self,
        progress: Arc<AtomicU8>,
        cancel_install: &Arc<AtomicBool>,
    ) -> Result<bool, RunGrubError> {
        progress.store(0, Ordering::SeqCst);
        cancel_install_exit!(cancel_install);

        info!("Installing grub ...");
        self.install_grub_impl()?;

        cancel_install_exit!(cancel_install);
        progress.store(100, Ordering::SeqCst);

        Ok(true)
    }

    fn generate_ssh_key(
        &self,
        progress: Arc<AtomicU8>,
        cancel_install: &Arc<AtomicBool>,
    ) -> Result<bool, RunCmdError> {
        progress.store(0, Ordering::SeqCst);
        cancel_install_exit!(cancel_install);

        info!("Generating SSH key ...");
        gen_ssh_key()?;

        cancel_install_exit!(cancel_install);
        progress.store(100, Ordering::SeqCst);

        Ok(true)
    }

    fn escape_chroot(
        &self,
        progress: Arc<AtomicU8>,
        cancel_install: &Arc<AtomicBool>,
        root_fd: &OwnedFd,
    ) -> Result<bool, ChrootError> {
        progress.store(0, Ordering::SeqCst);
        cancel_install_exit!(cancel_install);

        info!("Escape chroot ...");
        // 如果能走到这里，则 owned_root_fd 一定为 Some，故此处 unwrap 安全
        escape_chroot(root_fd)?;

        progress.store(100, Ordering::SeqCst);

        Ok(true)
    }

    fn configure_system(
        &self,
        progress: Arc<AtomicU8>,
        cancel_install: &Arc<AtomicBool>,
    ) -> Result<bool, ConfigureSystemError> {
        progress.store(0, Ordering::SeqCst);

        cancel_install_exit!(cancel_install);

        if self.swapfile != SwapFile::Disable {
            write_swap_entry_to_fstab().context(SwapToGenfstabSnafu)?;
        }

        cancel_install_exit!(cancel_install);

        progress.store(25, Ordering::SeqCst);

        cancel_install_exit!(cancel_install);

        info!("Setting timezone as {} ...", self.timezone);
        set_zoneinfo(&self.timezone).context(SetZoneinfoSnafu {
            zone: self.timezone.to_string(),
        })?;

        cancel_install_exit!(cancel_install);

        info!("Setting rtc_as_localtime ...");
        set_hwclock_tc(!self.rtc_as_localtime).context(SetHwclockSnafu {
            is_rtc: self.rtc_as_localtime,
        })?;
        progress.store(50, Ordering::SeqCst);

        cancel_install_exit!(cancel_install);

        info!("Setting hostname as {}", self.hostname);
        set_hostname(&self.hostname).context(SetHostnameSnafu {
            hostname: self.hostname.to_string(),
        })?;
        progress.store(75, Ordering::SeqCst);

        cancel_install_exit!(cancel_install);

        info!("Setting User ...");
        add_new_user(&self.user.username, &self.user.password).context(AddNewUserSnafu)?;

        cancel_install_exit!(cancel_install);

        if let Some(full_name) = &self.user.full_name {
            passwd_set_fullname(full_name, &self.user.username).context(SetFullNameSnafu {
                fullname: full_name.to_string(),
            })?;
        }

        cancel_install_exit!(cancel_install);

        progress.store(80, Ordering::SeqCst);

        info!("Setting locale ...");
        set_locale(&self.local).context(SetLocaleSnafu {
            locale: self.local.to_string(),
        })?;

        progress.store(100, Ordering::SeqCst);

        Ok(true)
    }

    fn swapoff_impl(&self, tmp_mount_path: &Path) -> Result<bool, PostInstallationError> {
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

        Ok(true)
    }

    fn install_grub_impl(&self) -> Result<bool, RunGrubError> {
        if self.efi_partition.is_some() {
            info!("Installing grub to UEFI partition ...");
            execute_grub_install(None)?;
        } else {
            info!("Installing grub to MBR partition ...");
            execute_grub_install(Some(self.target_partition.parent_path.as_ref().unwrap()))?;
        }

        Ok(true)
    }

    fn genfatab(&self, tmp_mount_path: &Path) -> Result<bool, SetupGenfstabError> {
        genfstab_to_file(
            self.target_partition
                .path
                .as_ref()
                .context(ValueNotSetGenfstabSnafu {
                    t: "system partition path",
                })?,
            self.target_partition
                .fs_type
                .as_ref()
                .context(ValueNotSetGenfstabSnafu {
                    t: "system partition fstype",
                })?,
            tmp_mount_path,
            Path::new("/"),
        )?;

        if let Some(ref efi_partition) = self.efi_partition {
            genfstab_to_file(
                efi_partition
                    .path
                    .as_ref()
                    .context(ValueNotSetGenfstabSnafu {
                        t: "efi partition path",
                    })?,
                efi_partition
                    .fs_type
                    .as_ref()
                    .context(ValueNotSetGenfstabSnafu {
                        t: "efi partition fstype",
                    })?,
                tmp_mount_path,
                Path::new("/efi"),
            )?;
        }

        Ok(true)
    }

    fn mount_partitions(&self, tmp_mount_path: &Path) -> Result<bool, MountError> {
        let fs_type = self
            .target_partition
            .fs_type
            .as_ref()
            .context(ValueNotSetMountSnafu {
                t: "system partition fstype",
            })?;

        mount_root_path(
            self.target_partition.path.as_deref(),
            tmp_mount_path,
            fs_type,
        )
        .context(MountRootSnafu {
            path: self
                .target_partition
                .path
                .as_ref()
                .context(ValueNotSetMountSnafu {
                    t: "system mount path",
                })?,
        })?;

        if let Some(ref efi) = self.efi_partition {
            let efi_mount_path = tmp_mount_path.join("efi");
            fs::create_dir_all(&efi_mount_path).context(CreateDirSnafu {
                path: efi_mount_path.to_path_buf(),
            })?;

            mount_root_path(
                efi.path.as_deref(),
                &efi_mount_path,
                efi.fs_type.as_ref().context(ValueNotSetMountSnafu {
                    t: "efi partition fstype",
                })?,
            )
            .context(MountRootSnafu {
                path: efi
                    .path
                    .as_ref()
                    .context(ValueNotSetMountSnafu { t: "efi path" })?,
            })?;
        }

        Ok(true)
    }

    fn format_partitions(&self) -> Result<bool, PartitionError> {
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

fn run_dracut(
    cancel_install: Arc<AtomicBool>,
    progress: Arc<AtomicU8>,
) -> Result<bool, RunCmdError> {
    info!("Running dracut ...");
    cancel_install_exit!(cancel_install);

    progress.store(0, Ordering::SeqCst);
    execute_dracut()?;
    progress.store(100, Ordering::SeqCst);

    cancel_install_exit!(cancel_install);
    Ok(true)
}

pub fn sync_and_reboot() -> io::Result<()> {
    sync();

    if sysrq_reboot().is_err() {
        reboot(RebootCommand::Restart)?;
    }

    Ok(())
}

pub fn umount_all(tmp_mount_path: &Path) {
    debug!(
        "Try to use umount -R {} to umount",
        tmp_mount_path.display()
    );
    if let Ok(out) = Command::new("umount")
        .arg("-R")
        .arg(tmp_mount_path)
        .output()
    {
        debug!(
            "umount -R {} stdout: {}",
            tmp_mount_path.display(),
            String::from_utf8_lossy(&out.stdout)
        );
        debug!(
            "umount -R {} stderr: {}",
            tmp_mount_path.display(),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

fn sysrq_reboot() -> io::Result<()> {
    let mut f = fs::OpenOptions::new()
        .write(true)
        .open("/proc/sysrq-trigger")?;

    f.write_all(b"b")?;
    f.sync_all()?;

    Ok(())
}
