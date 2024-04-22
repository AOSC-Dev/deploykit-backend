use std::path::Path;

use disk::partition::get_partition_uuid;
use fstab::{FsEntry, FsTab};
use tracing::debug;

use crate::{GenFstabErrorKind, InstallError};

pub fn fstab_add_entry(
    fstab: &mut FsTab,
    partition_path: &Path,
    fs_type: &str,
    mount_path: &Path,
) -> Result<(), InstallError> {
    let uuid_str = if fs_type != "swap" {
        let uuid = get_partition_uuid(partition_path)
            .ok_or_else(|| InstallError::GenFstab(GenFstabErrorKind::UUID))?;
        let uuid_str = format!("UUID={}", uuid);

        uuid_str
    } else {
        "/swapfile".to_string()
    };

    let mut mount_opts = vec!["defaults".to_string()];

    if ["vfat", "fat16", "fat32", "swap"].contains(&fs_type) {
        mount_opts.push("nofail".to_string())
    }

    let pass = match fs_type {
        "vfat" | "fat16" | "fat32" => 2,
        "swap" => 0,
        _ => 1,
    };

    fstab
        .add_entry(FsEntry {
            fs_spec: uuid_str,
            mountpoint: mount_path.to_path_buf(),
            vfs_type: fs_type.to_string(),
            mount_options: mount_opts,
            dump: false,
            fsck_order: pass,
        })
        .map_err(|e| {
            debug!("{e}");
            InstallError::GenFstab(GenFstabErrorKind::FsEntryInvaild)
        })?;

    Ok(())
}
