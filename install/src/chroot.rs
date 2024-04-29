use std::path::Path;

use rustix::fd::{AsFd, OwnedFd};
use rustix::fs::{Mode, OFlags};
use rustix::io::Errno;
use rustix::{fs, process};
use snafu::{ResultExt, Snafu};
use tracing::info;

use crate::mount::{setup_files_mounts, MountInnerError};

#[derive(Debug, Snafu)]
pub enum ChrootError {
    #[snafu(display("Failed to chdir"))]
    Chdir { source: Errno },
    #[snafu(display("Failed to change root"))]
    Chroot { source: Errno, quit: bool },
    #[snafu(display("Failed to set current dir as /"))]
    SetCurrentDir { source: std::io::Error },
    #[snafu(transparent)]
    SetupInnerMounts { source: MountInnerError },
}

/// Escape the chroot context using the previously obtained `root_fd` as a trampoline
pub fn escape_chroot<F: AsFd>(root_fd: F) -> Result<(), ChrootError> {
    process::fchdir(root_fd).context(ChdirSnafu)?;
    process::chroot(".").context(ChrootSnafu { quit: true })?;

    // reset cwd (on host)
    std::env::set_current_dir("/").context(SetCurrentDirSnafu)?;
    info!("Escaped chroot environment");

    Ok(())
}

/// Setup bind mounts and chroot into the guest system
/// Warning: This will make the program trapped in the new root directory
pub fn dive_into_guest(root: &Path) -> Result<(), ChrootError> {
    setup_files_mounts(root)?;
    process::chroot(root).context(ChrootSnafu { quit: false })?;

    // jump to the root directory after chroot
    std::env::set_current_dir("/").context(SetCurrentDirSnafu)?;

    Ok(())
}

/// Get the open file descriptor to the specified path
pub fn get_dir_fd(path: &Path) -> Result<OwnedFd, Errno> {
    fs::open(
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NONBLOCK,
        Mode::empty(),
    )
}
