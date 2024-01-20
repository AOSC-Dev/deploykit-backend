use std::{
    fs::File,
    io::{Read, Write},
};

use tracing::info;

use crate::{utils::run_command, InstallError};

/// Sets locale in the guest environment
/// Must be used in a chroot context
pub fn set_locale(locale: &str) -> Result<(), InstallError> {
    let mut f = File::create("/etc/locale.conf").map_err(|e| InstallError::OperateFile {
        path: "/etc/locale.conf".to_string(),
        err: e,
    })?;

    f.write_all(b"LANG=")
        .map_err(|e| InstallError::OperateFile {
            path: "/etc/locale.conf".to_string(),
            err: e,
        })?;

    f.write_all(format!("{locale}\n").as_bytes())
        .map_err(|e| InstallError::OperateFile {
            path: "/etc/locale.conf".to_string(),
            err: e,
        })?;

    Ok(())
}

/// Sets utc/rtc time in the guest environment
/// Must be used in a chroot context
pub fn set_hwclock_tc(utc: bool) -> Result<(), InstallError> {
    let adjtime_file = std::fs::File::open("/etc/adjtime");
    let status_is_rtc = if let Ok(mut adjtime_file) = adjtime_file {
        let mut buf = String::new();

        adjtime_file
            .read_to_string(&mut buf)
            .map_err(|e| InstallError::OperateFile {
                path: "/etc/adjtime".to_string(),
                err: e,
            })?;

        let line: Vec<&str> = buf.split('\n').collect();
        if line.len() < 3 || line.get(2) == Some(&"UTC") {
            false
        } else {
            line[2] == "LOCAL"
        }
    } else {
        false
    };

    info!("Status is rtc: {}", status_is_rtc);
    if utc {
        if !status_is_rtc {
            return Ok(());
        } else {
            run_command("hwclock", ["-wu"])?;
        }
    } else if status_is_rtc {
        return Ok(());
    } else {
        run_command("hwclock", ["-wl"])?;
    }

    Ok(())
}
