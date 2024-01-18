use thiserror::Error;

mod extract;
mod utils;
mod mount;
mod grub;
mod user;
mod genfstab;
mod swap;
mod chroot;
mod ssh;
mod dracut;
mod hostname;
mod download;

#[derive(Debug, Error)]
pub enum InstallError {
    #[error("Failed to unpack")]
    Unpack(std::io::Error),
    #[error("Failed to run command {command}, err: {err}")]
    RunCommand {
        command: String,
        err: std::io::Error,
    },
    #[error("Failed to mount filesystem device to {mount_point}, err: {err}")]
    MountFs {
        mount_point: String,
        err: std::io::Error,
    },
    #[error("Failed to umount filesystem device from {mount_point}, err: {err}")]
    UmountFs {
        mount_point: String,
        err: std::io::Error,
    },
    #[error("Failed to operate file or directory {path}, err: {err}")]
    OperateFile {
        path: String,
        err: std::io::Error,
    },
    #[error("Full name is illegal: {0}")]
    FullNameIllegal(String),
    #[error("/etc/passwd is illegal, kind: {0:?}")]
    PasswdIllegal(PasswdIllegalKind),
    #[error("Failed to generate /etc/fstab: {0:?}")]
    GenFstab(GenFstabErrorKind),
    #[error(transparent)]
    Reqwest(#[from] reqwest::Error),
    #[error("Failed to download squashfs, checksum mismatch")]
    ChecksumMisMatch,
    #[error("Failed to create tokio runtime: {0}")]
    CreateTokioRuntime(#[from] std::io::Error),
}

#[derive(Debug)]
pub enum PasswdIllegalKind {
    Username,
    Time,
    Uid,
    Gid,
    Fullname,
    Home,
    LoginShell,
}

#[derive(Debug)]
pub enum GenFstabErrorKind {
    UnsupportedFileSystem(String),
    UUID,
}
