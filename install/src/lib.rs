use std::path::PathBuf;

use disk::partition::DkPartition;
use serde::{Deserialize, Serialize};
use thiserror::Error;

mod chroot;
mod download;
mod dracut;
mod extract;
mod genfstab;
mod grub;
mod hostname;
mod mount;
mod ssh;
mod swap;
mod user;
mod utils;

#[derive(Debug, Error)]
pub enum InstallError {
    #[error("Failed to unpack")]
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
    #[error("/etc/passwd is illegal, kind: {0:?}")]
    PasswdIllegal(PasswdIllegalKind),
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

#[derive(Debug, Serialize, Deserialize)]
pub enum DownloadType {
    Http {
        url: String,
        hash: String,
        to_path: PathBuf,
    },
    File(PathBuf),
}

#[derive(Debug, Serialize, Deserialize)]
pub struct InstallConfigPrepare {
    pub locale: Option<String>,
    pub timezone: Option<String>,
    pub flaver: Option<String>,
    pub download: Option<DownloadType>,
    pub user: Option<User>,
    pub rtc_as_localtime: bool,
    pub hostname: Option<String>,
    pub swapfile: SwapFile,
    pub target_partition: Option<DkPartition>,
    pub efi_partition: Option<DkPartition>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct User {
    pub username: String,
    pub password: String,
    pub root_password: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
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
            flaver: None,
            download: None,
            user: None,
            rtc_as_localtime: false,
            hostname: None,
            swapfile: SwapFile::Automatic,
            target_partition: None,
            efi_partition: None,
        }
    }
}

pub struct InstallConfig {
    local: String,
    timezone: String,
    flaver: String,
    download: DownloadType,
    user: User,
    rtc_as_localtime: bool,
    hostname: String,
    swapfile: SwapFile,
    target_partition: DkPartition,
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
            flaver: value
                .flaver
                .ok_or(InstallError::IsNotSet(NotSetValue::Flaver))?,
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
            target_partition: value
                .target_partition
                .ok_or(InstallError::IsNotSet(NotSetValue::TargetPartition))?,
            efi_partition: value.efi_partition,
        })
    }
}

impl InstallConfig {
    pub fn start_install<F: Fn(&str), F2: Fn(usize)>(
        self,
        progress_str: F,
        progress_num: F2,
    ) -> Result<(), InstallError> {
        progress_str("Step 1/8 Formatting disk...");
        progress_num(0);

        Ok(())
    }
}
