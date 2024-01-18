use std::io;
use std::path::Path;

use rustix::fd::{AsFd, OwnedFd};
use rustix::fs::{Mode, OFlags};
use rustix::{fs, process};
use tracing::info;

use crate::mount::setup_bind_mounts;
use crate::InstallError;

/// Escape the chroot context using the previously obtained `root_fd` as a trampoline
pub fn escape_chroot<F: AsFd>(root_fd: F) -> Result<(), InstallError> {
    process::fchdir(root_fd).map_err(|e| InstallError::OperateFile {
        path: "/".to_string(),
        err: io::Error::new(e.kind(), "Failed to change directory"),
    })?;

    process::chroot(".").map_err(|e| InstallError::OperateFile {
        path: "/".to_string(),
        err: io::Error::new(e.kind(), "Failed to chroot"),
    })?;

    // reset cwd (on host)
    std::env::set_current_dir("/").map_err(|e| InstallError::OperateFile {
        path: "/".to_string(),
        err: io::Error::new(e.kind(), "Failed to reset current directory"),
    })?;

    info!("Escaped chroot environment");

    Ok(())
}

/// Setup bind mounts and chroot into the guest system
/// Warning: This will make the program trapped in the new root directory
pub fn dive_into_guest(root: &Path) -> Result<(), InstallError> {
    setup_bind_mounts(root)?;
    process::chroot(root).map_err(|e| InstallError::OperateFile {
        path: "/".to_string(),
        err: io::Error::new(e.kind(), "Failed to chroot"),
    })?;

    // jump to the root directory after chroot
    std::env::set_current_dir("/").map_err(|e| InstallError::OperateFile {
        path: "/".to_string(),
        err: io::Error::new(e.kind(), "Failed to reset current directory"),
    })?;

    Ok(())
}

/// Get the open file descriptor to the specified path
pub fn get_dir_fd(path: &Path) -> Result<OwnedFd, InstallError> {
    fs::open(
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NONBLOCK,
        Mode::empty(),
    )
    .map_err(|e| InstallError::OperateFile {
        path: path.display().to_string(),
        err: io::Error::new(e.kind(), "Failed to open file"),
    })
}
