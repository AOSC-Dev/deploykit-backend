use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
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
use sysinfo::System;
use thiserror::Error;
use tracing::info;

use crate::{
    chroot::{dive_into_guest, escape_chroot, get_dir_fd},
    dracut::execute_dracut,
    genfstab::write_swap_entry_to_fstab,
    grub::execute_grub_install,
    hostname::set_hostname,
    locale::{set_hwclock_tc, set_locale},
    mount::{remove_bind_mounts, umount_root_path},
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

impl InstallConfig {
    pub fn start_install<F, F2, F3>(
        &self,
        step: F,
        progress: F2,
        velocity: F3,
        tmp_mount_path: PathBuf,
    ) -> Result<(), InstallError>
    where
        F: Fn(u8),
        F2: Fn(f64) + Send + Sync + 'static,
        F3: Fn(usize) + Send + Sync + 'static,
    {
        step(1);
        progress(0.0);

        self.format_partitions()?;
        self.mount_partitions(&tmp_mount_path)?;

        progress(50.0);

        match self.swapfile {
            SwapFile::Automatic => {
                let mut sys = System::new_all();
                sys.refresh_memory();
                let total_memory = sys.total_memory();
                let size = get_recommend_swap_size(total_memory);
                create_swapfile(size, &tmp_mount_path)?;
            }
            SwapFile::Custom(size) => {
                create_swapfile(size as f64, &tmp_mount_path)?;
            }
            SwapFile::Disable => {}
        }

        progress(100.0);

        step(2);
        progress(0.0);

        let progress_arc = Arc::new(progress);
        let velocity_arc = Arc::new(velocity);
        let progress = progress_arc.clone();
        let velocity = velocity_arc.clone();
        
        let (squashfs_path, total_size) =
            download_file(&self.download, progress_arc, velocity_arc)?;

        step(3);
        progress(0.0);

        extract_squashfs(
            total_size as f64,
            squashfs_path,
            tmp_mount_path.to_path_buf(),
            &*progress,
            &*velocity,
        )?;

        velocity(0);
        step(4);
        progress(0.0);

        info!("Generate /etc/fstab");
        self.genfatab(&tmp_mount_path)?;

        progress(100.0);

        step(5);
        progress(0.0);

        info!("Chroot to installed system ...");
        let owned_root_fd = get_dir_fd(Path::new("/"))?;
        dive_into_guest(&tmp_mount_path)?;

        info!("Running dracut ...");
        execute_dracut()?;

        progress(100.0);

        step(6);
        progress(0.0);

        info!("Installing grub ...");
        self.install_grub()?;

        progress(100.0);

        step(7);
        progress(0.0);

        info!("Generating SSH key ...");
        gen_ssh_key()?;

        progress(100.0);

        step(8);
        progress(0.0);

        if self.swapfile != SwapFile::Disable {
            write_swap_entry_to_fstab()?;
        }

        progress(25.0);

        info!("Setting timezone as {} ...", self.timezone);
        set_zoneinfo(&self.timezone)?;

        info!("Setting rtc_as_localtime ...");
        set_hwclock_tc(!self.rtc_as_localtime)?;
        progress(50.0);

        info!("Setting hostname as {}", self.hostname);
        set_hostname(&self.hostname)?;
        progress(75.0);

        info!("Setting User ...");
        add_new_user(&self.user.username, &self.user.password)?;

        if let Some(full_name) = &self.user.full_name {
            passwd_set_fullname(full_name, &self.user.username)?;
        }

        progress(80.0);

        info!("Setting locale ...");
        set_locale(&self.local)?;
        progress(90.0);

        info!("Escape chroot ...");
        escape_chroot(owned_root_fd)?;

        if self.swapfile != SwapFile::Disable {
            swapoff(&tmp_mount_path).ok();
        }

        info!("Removing bind mounts ...");
        remove_bind_mounts(&tmp_mount_path)?;

        info!("Unmounting filesystems...");
        umount_root_path(&tmp_mount_path)?;

        progress(100.0);

        Ok(())
    }

    fn install_grub(&self) -> Result<(), InstallError> {
        if self.efi_partition.is_some() {
            info!("Installing grub to UEFI partition ...");
            execute_grub_install(None)?;
        } else {
            info!("Installing grub to MBR partition ...");
            execute_grub_install(Some(self.target_partition.parent_path.as_ref().unwrap()))?;
        }

        Ok(())
    }

    fn genfatab(&self, tmp_mount_path: &Path) -> Result<(), InstallError> {
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

        Ok(())
    }

    fn mount_partitions(&self, tmp_mount_path: &Path) -> Result<(), InstallError> {
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

        Ok(())
    }

    fn format_partitions(&self) -> Result<(), InstallError> {
        format_partition(&self.target_partition)?;

        if let Some(ref efi) = self.efi_partition {
            let mut efi = efi.clone();
            if efi.fs_type.is_none() {
                efi.fs_type = Some("vfat".to_string());
                format_partition(&efi)?;
            }
        }

        Ok(())
    }
}

