use std::{
    ffi::OsString,
    io::Write,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
};

use disk::disk_types::FileSystem;
use fstab_generate::BlockInfo;
use snafu::{OptionExt, ResultExt, Snafu};

#[derive(Debug, Snafu)]
pub enum GenfstabError {
    #[snafu(display("Unsupport filesystem: {fs_type}"))]
    UnsupportedFileSystem { fs_type: String },
    #[snafu(display("Partition {} has no UUID", path.display()))]
    UUID { path: PathBuf },
    #[snafu(display("Failed to operate /etc/fstab"))]
    OperateFstabFile { source: std::io::Error },
}

/// Gen fstab to /etc/fstab
pub(crate) fn genfstab_to_file(
    partition_path: &Path,
    fs_type: &str,
    root_path: &Path,
    mount_path: &Path,
) -> Result<(), GenfstabError> {
    if cfg!(debug_assertions) {
        return Ok(());
    }

    let s = fstab_entries(partition_path, fs_type, Some(mount_path))?;
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(root_path.join("etc/fstab"))
        .context(OperateFstabFileSnafu)?;

    f.write_all(s.as_bytes()).context(OperateFstabFileSnafu)?;

    Ok(())
}

/// Must be used in a chroot context
pub(crate) fn write_swap_entry_to_fstab() -> Result<(), GenfstabError> {
    let s = "/swapfile none swap defaults,nofail 0 0\n";
    let mut fstab = std::fs::OpenOptions::new()
        .append(true)
        .open("/etc/fstab")
        .context(OperateFstabFileSnafu)?;

    fstab
        .write_all(s.as_bytes())
        .context(OperateFstabFileSnafu)?;

    Ok(())
}

fn fstab_entries(
    device_path: &Path,
    fs_type: &str,
    mount_path: Option<&Path>,
) -> Result<OsString, GenfstabError> {
    let (fs_type, option) = match fs_type {
        "vfat" | "fat16" | "fat32" => (FileSystem::Fat32, "defaults,nofail"),
        "ext4" => (FileSystem::Ext4, "defaults"),
        "btrfs" => (FileSystem::Btrfs, "defaults"),
        "xfs" => (FileSystem::Xfs, "defaults"),
        "f2fs" => (FileSystem::F2fs, "defaults"),
        "swap" => (FileSystem::Swap, "sw"),
        _ => {
            return Err(GenfstabError::UnsupportedFileSystem {
                fs_type: fs_type.to_string(),
            });
        }
    };

    let root_id = BlockInfo::get_partition_id(device_path, fs_type)
        .context(UUIDSnafu { path: device_path })?;

    let root = BlockInfo::new(root_id, fs_type, mount_path, option);
    let fstab = &mut OsString::new();
    root.write_entry(fstab);

    Ok(fstab.to_owned())
}
