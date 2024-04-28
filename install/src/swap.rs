use std::{
    fs::File,
    io::{self, Write},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use rustix::{fd::AsRawFd, fs::FallocateFlags};
use snafu::{ResultExt, Snafu};
use tracing::info;

use crate::utils::{run_command, RunCmdError};

#[derive(Debug, Snafu)]
pub enum SwapFileError {
    #[snafu(display("Failed to create swap file: {}", path.display()))]
    CreateFile {
        path: PathBuf,
        source: std::io::Error,
    },
    #[snafu(display("Failed to fallocate swap file: {}", path.display()))]
    Fallocate {
        path: PathBuf,
        source: std::io::Error,
    },
    #[snafu(display("Failed to flush swap file: {}", path.display()))]
    FlushSwapFile {
        path: PathBuf,
        source: std::io::Error,
    },
    #[snafu(display("Failed to set swap file permissions: {}", path.display()))]
    SetPermission {
        path: PathBuf,
        source: std::io::Error,
    },
    #[snafu(display("Failed to run swapon"))]
    Mkswap { source: RunCmdError },
}

pub fn get_recommend_swap_size(mem: u64) -> f64 {
    // 1073741824 is 1 * 1024 * 1024 * 1024 (1GiB => 1iB)
    let max: f64 = 32.0 * 1073741824.0;
    let res = match mem {
        ..=1073741824 => (mem * 2) as f64,
        1073741825.. => {
            let x = mem as f64;
            x + x.sqrt().round()
        }
    };

    if res > max {
        max
    } else {
        res
    }
}

/// Create swapfile
pub(crate) fn create_swapfile(size: f64, tempdir: &Path) -> Result<(), SwapFileError> {
    let swap_path = tempdir.join("swapfile");

    info!("Creating swapfile");
    let mut swapfile = File::create(&swap_path).context(CreateFileSnafu {
        path: swap_path.to_path_buf(),
    })?;

    let res = unsafe {
        libc::fallocate64(
            swapfile.as_raw_fd(),
            FallocateFlags::empty().bits() as i32,
            0,
            size as i64,
        )
    };

    if res != 0 {
        return Err(SwapFileError::Fallocate {
            path: swap_path.to_path_buf(),
            source: io::Error::from_raw_os_error(res),
        });
    }

    swapfile.flush().context(FlushSwapFileSnafu {
        path: swap_path.to_path_buf(),
    })?;

    info!("Set swapfile permission as 600");
    std::fs::set_permissions(&swap_path, std::fs::Permissions::from_mode(0o600)).context(
        SetPermissionSnafu {
            path: swap_path.to_path_buf(),
        },
    )?;

    run_command("mkswap", [&swap_path]).context(MkswapSnafu)?;
    run_command("swapon", [swap_path]).ok();

    Ok(())
}

pub fn swapoff(tempdir: &Path) -> Result<(), RunCmdError> {
    let swapfile_path = tempdir.join("swapfile");

    if !swapfile_path.is_file() {
        return Ok(());
    }

    run_command("swapoff", [swapfile_path])?;

    Ok(())
}
