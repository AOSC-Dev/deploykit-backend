use std::{
    os::unix::fs::symlink,
    path::{Path, PathBuf},
};

use snafu::{ResultExt, Snafu};

#[derive(Debug, Snafu)]
pub enum SetZoneinfoError {
    #[snafu(display("Failed to remove /etc/localtime"))]
    RemoveLocaltimeFile { source: std::io::Error },
    #[snafu(display("Failed to symlink {} to /etc/localtime", path.display()))]
    Symlink {
        path: PathBuf,
        source: std::io::Error,
    },
}

/// Sets zoneinfo in the guest environment
/// Must be used in a chroot context
pub(crate) fn set_zoneinfo(zone: &str) -> Result<(), SetZoneinfoError> {
    if Path::new("/etc/localtime").exists() {
        std::fs::remove_file("/etc/localtime").context(RemoveLocaltimeFileSnafu)?;
    }

    let zone = if zone == "Asia/Beijing" {
        "Asia/Shanghai"
    } else {
        zone
    };

    let zone_path = PathBuf::from("/usr/share/zoneinfo").join(zone);
    symlink(&zone_path, "/etc/localtime").context(SymlinkSnafu { path: zone_path })?;

    Ok(())
}
