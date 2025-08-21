use std::{
    fs::File,
    io::{self},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use rustix::{fd::AsRawFd, fs::FallocateFlags};
use snafu::{ResultExt, Snafu};
use tracing::info;

use crate::utils::{run_command, RunCmdError};

const MAX_MEMORY: f64 = 32.0;

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
    #[snafu(display("Failed to run mkswap {}", path.display()))]
    Mkswap { path: PathBuf, source: RunCmdError },
}

pub fn get_recommend_swap_size(mem: u64) -> f64 {
    let mem: f64 = mem as f64 / 1024.0 / 1024.0 / 1024.0;

    let res = if mem <= 1.0 {
        mem * 2.0
    } else {
        mem + mem.sqrt().round()
    };

    if res >= MAX_MEMORY {
        MAX_MEMORY * 1024.0_f32.powi(3) as f64
    } else {
        res * 1024.0_f32.powi(3) as f64
    }
}

/// Create swapfile
pub fn create_swapfile(size: f64, tempdir: &Path) -> Result<(), SwapFileError> {
    let swap_path = tempdir.join("swapfile");

    info!("Creating swapfile");
    let swapfile = File::create(&swap_path).context(CreateFileSnafu {
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

    swapfile.sync_all().context(FlushSwapFileSnafu {
        path: swap_path.to_path_buf(),
    })?;

    info!("Set swapfile permission as 600");
    std::fs::set_permissions(&swap_path, std::fs::Permissions::from_mode(0o600)).context(
        SetPermissionSnafu {
            path: swap_path.to_path_buf(),
        },
    )?;

    run_command("mkswap", [&swap_path], vec![] as Vec<(String, String)>).context(MkswapSnafu {
        path: swap_path.clone(),
    })?;
    run_command("swapon", [swap_path], vec![] as Vec<(String, String)>).ok();

    Ok(())
}

pub fn swapoff(tempdir: &Path) -> Result<(), RunCmdError> {
    let swapfile_path = tempdir.join("swapfile");

    if !swapfile_path.is_file() {
        return Ok(());
    }

    run_command("swapoff", [swapfile_path], vec![] as Vec<(String, String)>)?;

    Ok(())
}
