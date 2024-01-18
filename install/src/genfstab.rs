use std::{ffi::OsString, io::Write, os::unix::ffi::OsStrExt, path::Path};

use disk::disk_types::FileSystem;
use fstab_generate::BlockInfo;

use crate::{GenFstabErrorKind, InstallError};

/// Gen fstab to /etc/fstab
pub fn genfstab_to_file(
    partition_path: &Path,
    fs_type: &str,
    root_path: &Path,
    mount_path: &Path,
) -> Result<(), InstallError> {
    if cfg!(debug_assertions) {
        return Ok(());
    }

    let s = fstab_entries(partition_path, fs_type, Some(mount_path))?;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .append(true)
        .open(root_path.join("etc/fstab"))
        .map_err(|e| InstallError::OperateFile {
            path: "/etc/fstab".to_string(),
            err: e,
        })?;

    f.write_all(s.as_bytes())
        .map_err(|e| InstallError::OperateFile {
            path: "/etc/fstab".to_string(),
            err: e,
        })?;

    Ok(())
}
fn fstab_entries(
    device_path: &Path,
    fs_type: &str,
    mount_path: Option<&Path>,
) -> Result<OsString, InstallError> {
    let (fs_type, option) = match fs_type {
        "vfat" | "fat16" | "fat32" => (FileSystem::Fat32, "defaults,nofail"),
        "ext4" => (FileSystem::Ext4, "defaults"),
        "btrfs" => (FileSystem::Btrfs, "defaults"),
        "xfs" => (FileSystem::Xfs, "defaults"),
        "f2fs" => (FileSystem::F2fs, "defaults"),
        "swap" => (FileSystem::Swap, "sw"),
        _ => {
            return Err(InstallError::GenFstab(
                GenFstabErrorKind::UnsupportedFileSystem(fs_type.to_string()),
            ));
        }
    };

    let root_id = BlockInfo::get_partition_id(device_path, fs_type)
        .ok_or(InstallError::GenFstab(GenFstabErrorKind::UUID))?;
    let root = BlockInfo::new(root_id, fs_type, mount_path, option);
    let fstab = &mut OsString::new();
    root.write_entry(fstab);

    Ok(fstab.to_owned())
}
