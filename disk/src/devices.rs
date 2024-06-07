use std::path::Path;

use fancy_regex::Regex;
use libparted::{Device, Disk};
use tracing::info;

use crate::{partition::find_root_mount_point, PartitionError};

pub fn list_devices() -> impl Iterator<Item = Device<'static>> {
    Device::devices(true).filter(|dev| {
        let is_sata = device_is_sata(dev.path());
        info!("{} is sata: {is_sata}", dev.path().display());

        let is_sdcard = device_is_sdcard(dev.path());
        info!("{} is sdcard: {is_sdcard}", dev.path().display());

        let is_nvme = device_is_nvme(dev.path());
        info!("{} is nvme: {is_nvme}", dev.path().display());

        is_sata || is_sdcard || is_nvme
    })
}

pub fn is_root_device(d: &mut Device) -> Result<bool, PartitionError> {
    let root = find_root_mount_point()?;

    for i in Disk::new(d)
        .map_err(|e| PartitionError::OpenDisk {
            path: "".to_string(),
            err: e,
        })?
        .parts()
    {
        if i.get_path()
            .map(|x| x.to_string_lossy() == root)
            .unwrap_or(false)
        {
            return Ok(true);
        }
    }

    Ok(false)
}

pub fn sync_disk() {
    rustix::fs::sync();
}

fn device_is_sata(path: &Path) -> bool {
    device_is_match(path, r"^([^0-9]+)$")
}

fn device_is_sdcard(path: &Path) -> bool {
    device_is_match(path, r"^(mmcblk[0-9]+)$")
}

fn device_is_nvme(path: &Path) -> bool {
    device_is_match(path, r"^(nvme[0-9]+n[0-9]+)$")
}

fn device_is_match(path: &Path, pattern: &str) -> bool {
    Regex::new(pattern)
        .ok()
        .and_then(|x| {
            path.display()
                .to_string()
                .split('/')
                .last()
                .and_then(|dev| x.is_match(dev).ok())
        })
        .unwrap_or(false)
}
