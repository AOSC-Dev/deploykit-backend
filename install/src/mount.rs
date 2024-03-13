use disk::is_efi_booted;
use rustix::mount::{self, MountFlags};
use std::{io, path::Path};

use crate::InstallError;

const EFIVARS_PATH: &str = "/sys/firmware/efi/efivars";

/// Mount the filesystem
pub(crate) fn mount_root_path(
    partition: Option<&Path>,
    target: &Path,
    fs_type: &str,
) -> Result<(), InstallError> {
    let mut fs_type = fs_type;
    if fs_type.starts_with("fat") {
        fs_type = "vfat";
    }

    mount_inner(partition, target, Some(fs_type), MountFlags::empty())?;

    Ok(())
}

fn mount_inner<P: AsRef<Path>>(
    partition: Option<P>,
    target: &Path,
    fs_type: Option<&str>,
    flag: MountFlags,
) -> Result<(), InstallError> {
    let partition = partition.as_ref().map(|p| p.as_ref());

    mount::mount(
        partition.unwrap_or(Path::new("")),
        target,
        fs_type.unwrap_or(""),
        flag,
        "",
    )
    .map_err(|e| InstallError::MountFs {
        mount_point: target.display().to_string(),
        err: io::Error::new(e.kind(), e.to_string()),
    })
}

/// Unmount the filesystem given at `root` and then do a sync
pub fn umount_root_path(root: &Path) -> Result<(), InstallError> {
    mount::unmount(root, mount::UnmountFlags::empty()).map_err(|e| InstallError::UmountFs {
        mount_point: root.display().to_string(),
        err: io::Error::new(e.kind(), e.to_string()),
    })?;

    sync_disk();

    Ok(())
}

pub fn sync_disk() {
    rustix::fs::sync();
}

/// Setup all the necessary bind mounts
pub fn setup_files_mounts(root: &Path) -> Result<(), InstallError> {
    mount_inner(
        Some("proc"),
        &root.join("proc"),
        Some("proc"),
        MountFlags::NOSUID | MountFlags::NOEXEC | MountFlags::NODEV,
    )?;

    mount_inner(
        Some("sys"),
        &root.join("sys"),
        Some("sysfs"),
        MountFlags::NOSUID | MountFlags::NOEXEC | MountFlags::NODEV | MountFlags::RDONLY,
    )?;

    if is_efi_booted() {
        mount_inner(
            Some("efivarfs"),
            &root.join(EFIVARS_PATH),
            Some("efivarfs"),
            MountFlags::NOSUID | MountFlags::NOEXEC | MountFlags::NODEV,
        )?;
    }

    mount_inner(
        Some("udev"),
        &root.join("dev"),
        Some("devtmpfs"),
        MountFlags::NOSUID,
    )?;

    mount_inner(
        Some("devpts"),
        &root.join("dev").join("pts"),
        Some("devpts"),
        MountFlags::NOSUID | MountFlags::NOEXEC,
    )?;

    mount_inner(
        Some("shm"),
        &root.join("dev").join("shm"),
        Some("devpts"),
        MountFlags::NOSUID | MountFlags::NODEV,
    )?;

    mount_inner(
        Some("run"),
        &root.join("run"),
        Some("devpts"),
        MountFlags::NOSUID | MountFlags::NODEV,
    )?;

    mount_inner(
        Some("tmp"),
        &root.join("tmp"),
        Some("tmpfs"),
        MountFlags::STRICTATIME | MountFlags::NODEV | MountFlags::NOSUID,
    )?;

    Ok(())
}

/// Remove bind mounts
/// Note: This function should be called outside of the chroot context
pub fn remove_bind_mounts() -> Result<(), InstallError> {
    for i in ["proc", "sys", "udev", "devpts", "shm", "run", "tmp"] {
        mount::unmount(i, mount::UnmountFlags::empty()).map_err(|e| InstallError::UmountFs {
            mount_point: i.to_string(),
            err: io::Error::new(e.kind(), "Failed to umount fs"),
        })?;
    }

    Ok(())
}
