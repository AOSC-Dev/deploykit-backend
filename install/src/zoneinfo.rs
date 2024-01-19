use std::{os::unix::fs::symlink, path::Path};

use crate::InstallError;

/// Sets zoneinfo in the guest environment
/// Must be used in a chroot context
pub fn set_zoneinfo(zone: &str) -> Result<(), InstallError> {
    if Path::new("/etc/localtime").exists() {
        std::fs::remove_file("/etc/localtime").map_err(|e| InstallError::OperateFile {
            path: "/etc/localtime".to_string(),
            err: e,
        })?;
    }

    symlink(format!("/usr/share/zoneinfo/{zone}"), "/etc/localtime").map_err(|e| InstallError::OperateFile {
        path: "/etc/localtime".to_string(),
        err: e,
    })?;

    Ok(())
}
