use std::{
    fs::File,
    io::{self, Write},
    os::unix::fs::PermissionsExt,
    path::Path,
};

use rustix::{fd::AsRawFd, fs::FallocateFlags, io::Errno};
use tracing::info;

use crate::{utils::run_command, InstallError};

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
pub(crate) fn create_swapfile(size: f64, tempdir: &Path) -> Result<(), InstallError> {
    let swap_path = tempdir.join("swapfile");

    info!("Creating swapfile");
    let mut swapfile = File::create(&swap_path).map_err(|e| InstallError::OperateFile {
        path: swap_path.display().to_string(),
        err: e,
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
        return Err(InstallError::OperateFile {
            path: swap_path.display().to_string(),
            err: io::Error::new(
                Errno::from_raw_os_error(res).kind(),
                "Failed to create swapfile",
            ),
        });
    }

    swapfile.flush().map_err(|e| InstallError::OperateFile {
        path: swap_path.display().to_string(),
        err: e,
    })?;

    info!("Set swapfile permission as 600");
    std::fs::set_permissions(&swap_path, std::fs::Permissions::from_mode(0o600)).map_err(|e| {
        InstallError::OperateFile {
            path: swap_path.display().to_string(),
            err: e,
        }
    })?;

    run_command("mkswap", [&swap_path])?;
    run_command("swapon", [swap_path]).ok();

    Ok(())
}

pub fn swapoff(tempdir: &Path) -> Result<(), InstallError> {
    let swapfile_path = tempdir.join("swapfile");

    if !swapfile_path.is_file() {
        return Ok(());
    }

    run_command("swapoff", [swapfile_path])
}
