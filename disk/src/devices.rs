use std::path::Path;

use dbus_udisks2::{DiskDevice, Disks, UDisks2};
use fancy_regex::Regex;
use tracing::info;

pub fn list_devices_udisk2() -> impl Iterator<Item = DiskDevice> {
    let udisks2 = UDisks2::new().unwrap();
    let disks = Disks::new(&udisks2);

    disks.devices.into_iter().filter(|x| {
        let is_sata = device_is_sata(&x.parent.device);
        info!("{} is sata: {is_sata}", &x.parent.device.display());

        let is_sdcard = device_is_sdcard(&x.parent.device);
        info!("{} is sdcard: {is_sdcard}", &x.parent.device.display());

        let is_nvme = device_is_nvme(&x.parent.device);
        info!("{} is nvme: {is_nvme}", &x.parent.device.display());

        is_sata || is_sdcard || is_nvme
    })
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
