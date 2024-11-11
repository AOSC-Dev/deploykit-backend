use std::{
    fs::File,
    io::{self, BufRead, BufReader, Write},
};

use snafu::{ResultExt, Snafu};
use tracing::info;

use crate::utils::{run_command, RunCmdError};

#[derive(Debug, Snafu)]
pub enum SetHwclockError {
    #[snafu(display("Failed to operate /etc/adjtime"))]
    OperateAdjtimeFile { source: std::io::Error },
    #[snafu(transparent)]
    RunCommand { source: RunCmdError },
}

/// Sets locale in the guest environment
/// Must be used in a chroot context
pub(crate) fn set_locale(locale: &str) -> Result<(), io::Error> {
    let mut f = File::create("/etc/locale.conf")?;
    f.write_all(b"LANG=")?;
    f.write_all(format!("{locale}\n").as_bytes())?;

    Ok(())
}

/// Sets utc/rtc time in the guest environment
/// Must be used in a chroot context
pub(crate) fn set_hwclock_tc(utc: bool) -> Result<(), SetHwclockError> {
    let adjtime_file = std::fs::File::open("/etc/adjtime");
    let status_is_rtc = if let Ok(adjtime_file) = adjtime_file {
        let lines = BufReader::new(adjtime_file)
            .lines()
            .collect::<io::Result<Vec<_>>>()
            .context(OperateAdjtimeFileSnafu)?;

        if lines.len() < 3 || lines.get(2).map(|x| x == "UTC").unwrap_or(false) {
            false
        } else {
            lines[2] == "LOCAL"
        }
    } else {
        false
    };

    info!("Status is rtc: {}", status_is_rtc);
    if utc {
        if !status_is_rtc {
            return Ok(());
        } else {
            run_command("hwclock", ["-wu"], vec![] as Vec<(String, String)>)?;
        }
    } else if status_is_rtc {
        return Ok(());
    } else {
        run_command("hwclock", ["-wl"], vec![] as Vec<(String, String)>)?;
    }

    Ok(())
}
