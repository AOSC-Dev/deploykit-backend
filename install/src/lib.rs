use std::{
    fs,
    path::{Path, PathBuf},
};

use disk::{
    partition::{format_partition, DkPartition},
    PartitionError,
};
use download::download_file;
use extract::extract_squashfs;
use genfstab::genfstab_to_file;
use mount::mount_root_path;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use unsquashfs_wrapper::extract;

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
    #[error(transparent)]
    Partition(#[from] PartitionError),
    #[error("Partition value: {0:?} is none")]
    PartitionValueIsNone(PartitionNotSetValue),
    #[error("Local file {0:?} is not found")]
    LocalFileNotFound(String),
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
    pub fn start_install<F, F2, F3>(
        mut self,
        step: F,
        progress: F2,
        velocity: F3,
        tmp_mount_path: PathBuf,
    ) -> Result<(), InstallError>
    where
        F: Fn(u8),
        F2: Fn(usize),
        F3: Fn(usize),
    {
        step(1);
        progress(0);

        self.format_partitions()?;

        progress(100);

        step(2);
        progress(0);

        self.mount_partitions(&tmp_mount_path)?;
        let (squashfs_path, total_size) = download_file(&self.download, &progress, &velocity)?;

        let to_path = match self.download {
            DownloadType::Http { to_path, .. } => to_path,
            DownloadType::File(path) => path,
        };

        step(3);
        progress(0);

        extract_squashfs(
            total_size as f64,
            squashfs_path,
            to_path,
            &progress,
            &velocity,
        )?;

        step(4);
        progress(0);

        genfstab_to_file(
            &self
                .target_partition
                .path
                .ok_or(InstallError::PartitionValueIsNone(
                    PartitionNotSetValue::Path,
                ))?,
            &self
                .target_partition
                .fs_type
                .ok_or(InstallError::PartitionValueIsNone(
                    PartitionNotSetValue::FsType,
                ))?,
            &tmp_mount_path,
            Path::new("/"),
        )?;

        if let Some(ref efi_partition) = self.efi_partition {
            genfstab_to_file(
                &efi_partition
                    .path
                    .as_ref()
                    .ok_or(InstallError::PartitionValueIsNone(
                        PartitionNotSetValue::Path,
                    ))?,
                &efi_partition
                    .fs_type
                    .as_ref()
                    .ok_or(InstallError::PartitionValueIsNone(
                        PartitionNotSetValue::FsType,
                    ))?,
                &tmp_mount_path,
                Path::new("/efi"),
            )?;
        }

        progress(100);

        step(5);
        progress(0);

        Ok(())
    }

    fn mount_partitions(&self, tmp_mount_path: &Path) -> Result<(), InstallError> {
        mount_root_path(
            self.target_partition.path.as_deref(),
            &tmp_mount_path,
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

        Ok(())
    }

    fn format_partitions(&mut self) -> Result<(), InstallError> {
        format_partition(&self.target_partition)?;

        if let Some(efi) = &mut self.efi_partition {
            if efi.fs_type.is_none() {
                // format the un-formatted ESP partition
                efi.fs_type = Some("vfat".to_string());
            }
            format_partition(&efi)?;
        }

        Ok(())
    }
}
