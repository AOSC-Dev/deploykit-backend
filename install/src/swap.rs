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
    let swap_size = match mem {
        x @ ..=1073741824 => (x * 2) as f64,
        x @ 1073741825.. => {
            let x = x as f64;
            x + x.sqrt().round()
        }
    };

    swap_size
}

/// Create swapfile
pub fn create_swapfile(size: f64, tempdir: &Path) -> Result<(), InstallError> {
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
        let res = Errno::from_raw_os_error(res).kind();
        return Err(InstallError::OperateFile {
            path: swap_path.display().to_string(),
            err: io::Error::new(res, "Failed to fallocate swapfile"),
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

pub fn swapoff(tempdir: &Path) {
    run_command("swapoff", [tempdir.join("swapfile")]).ok();
}
