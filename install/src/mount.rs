use disk::is_efi_booted;
use rustix::{
    io::Errno,
    mount::{self, MountFlags},
};
use snafu::{ResultExt, Snafu};
use std::{
    fs::{self, read_dir},
    path::Path,
};
use tracing::debug;

use crate::utils::{run_command, RunCmdError};

const EFIVARS_PATH: &str = "sys/firmware/efi/efivars";

#[derive(Debug, Snafu)]
#[snafu(display("Failed to umount point: {point}"))]
pub struct UmountError {
    source: RunCmdError,
    pub point: String,
}

/// Mount the filesystem
pub(crate) fn mount_root_path(
    partition: Option<&Path>,
    target: &Path,
    fs_type: &str,
) -> Result<(), Errno> {
    let mut fs_type = fs_type;
    if fs_type.starts_with("fat") {
        fs_type = "vfat";
    }

    mount_inner(partition, target, Some(fs_type), MountFlags::empty())?;

    Ok(())
}

fn mount_inner<P: AsRef<Path>>(
    partition: Option<P>,
    target: &Path,
    fs_type: Option<&str>,
    flag: MountFlags,
) -> Result<(), Errno> {
    let partition = partition.as_ref().map(|p| p.as_ref());

    mount::mount(
        partition.unwrap_or(Path::new("")),
        target,
        fs_type.unwrap_or(""),
        flag,
        "",
    )
}

/// Unmount the filesystem given at `root` and then do a sync
pub fn umount_root_path(root: &Path) -> Result<(), UmountError> {
    run_command("umount", [root], vec![] as Vec<(String, String)>).context(UmountSnafu {
        point: root.display().to_string(),
    })?;

    Ok(())
}

pub fn sync_disk() {
    rustix::fs::sync();
}

#[derive(Debug, Snafu)]
#[snafu(display("failed to mount {point}"))]
pub struct MountInnerError {
    source: Errno,
    pub point: &'static str,
    pub umount: bool,
}

/// Setup all the necessary bind mounts
pub fn setup_files_mounts(root: &Path) -> Result<(), MountInnerError> {
    mount_inner(
        Some("proc"),
        &root.join("proc"),
        Some("proc"),
        MountFlags::NOSUID | MountFlags::NOEXEC | MountFlags::NODEV,
    )
    .context(MountInnerSnafu {
        point: "proc",
        umount: false,
    })?;

    mount_inner(
        Some("sys"),
        &root.join("sys"),
        Some("sysfs"),
        MountFlags::NOSUID | MountFlags::NOEXEC | MountFlags::NODEV | MountFlags::RDONLY,
    )
    .context(MountInnerSnafu {
        point: "sys",
        umount: false,
    })?;

    if is_efi_booted() {
        mount_inner(
            Some("efivarfs"),
            &root.join(EFIVARS_PATH),
            Some("efivarfs"),
            MountFlags::NOSUID | MountFlags::NOEXEC | MountFlags::NODEV,
        )
        .context(MountInnerSnafu {
            point: "efivarfs",
            umount: false,
        })?;
    }

    mount_inner(
        Some("udev"),
        &root.join("dev"),
        Some("devtmpfs"),
        MountFlags::NOSUID,
    )
    .context(MountInnerSnafu {
        point: "udev",
        umount: false,
    })?;

    mount_inner(
        Some("devpts"),
        &root.join("dev").join("pts"),
        Some("devpts"),
        MountFlags::NOSUID | MountFlags::NOEXEC,
    )
    .context(MountInnerSnafu {
        point: "devpts",
        umount: false,
    })?;

    mount_inner(
        Some("shm"),
        &root.join("dev").join("shm"),
        Some("devpts"),
        MountFlags::NOSUID | MountFlags::NODEV,
    )
    .context(MountInnerSnafu {
        point: "shm",
        umount: false,
    })?;

    Ok(())
}

/// Remove bind mounts
/// Note: This function should be called outside of the chroot context
pub fn remove_files_mounts(system_path: &Path) -> Result<(), UmountError> {
    // 先把日志写回已安装的系统
    if let Ok(tmp) = read_dir("/tmp") {
        for entry in tmp {
            if let Ok(entry) = entry {
                if entry.file_name().to_string_lossy().starts_with("dk.log") {
                    fs::copy(entry.path(), system_path.join("var/log/dk.log")).ok();
                }
            }
        }
    }

    let mut mounts = ["proc", "sys", EFIVARS_PATH, "dev", "dev/pts", "dev/shm"];

    // 需要按顺序卸载挂载点
    mounts.reverse();

    for i in mounts {
        if i == "efivarfs" && !is_efi_booted() {
            continue;
        }

        let mount_point = system_path.join(i);

        debug!("umounting point {}", mount_point.display());
        run_command(
            "umount",
            [&mount_point.to_string_lossy().to_string()],
            vec![] as Vec<(String, String)>,
        )
        .context(UmountSnafu {
            point: mount_point.display().to_string(),
        })?;
    }

    Ok(())
}
