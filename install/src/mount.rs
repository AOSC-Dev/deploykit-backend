use disk::is_efi_booted;
use rustix::mount::{self, MountFlags};
use std::{io, path::Path};

use crate::InstallError;

const BIND_MOUNTS: &[&str] = &["/dev", "/proc", "/sys", "/run/udev"];
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
pub fn setup_bind_mounts(root: &Path) -> Result<(), InstallError> {
    for mount in BIND_MOUNTS {
        let mut root = root.to_owned();
        root.push(&mount[1..]);
        std::fs::create_dir_all(root.clone()).map_err(|e| InstallError::OperateFile {
            path: root.display().to_string(),
            err: io::Error::new(e.kind(), e.to_string()),
        })?;

        mount_inner(Some(mount), &root, None, MountFlags::BIND)?;
    }

    if is_efi_booted() {
        let root = root.join(&EFIVARS_PATH[1..]);
        std::fs::create_dir_all(&root).map_err(|e| InstallError::OperateFile {
            path: root.display().to_string(),
            err: io::Error::new(e.kind(), e.to_string()),
        })?;

        mount_inner(Some(EFIVARS_PATH), &root, None, MountFlags::BIND)?;
    }

    Ok(())
}

/// Remove bind mounts
/// Note: This function should be called outside of the chroot context
pub fn remove_bind_mounts(root: &Path) -> Result<(), InstallError> {
    for mount in BIND_MOUNTS {
        let mut root = root.to_owned();
        root.push(&mount[1..]);
        mount::unmount(&root, mount::UnmountFlags::empty()).map_err(|e| InstallError::UmountFs {
            mount_point: mount.to_string(),
            err: io::Error::new(e.kind(), "Failed to umount fs"),
        })?;
    }

    Ok(())  
}
