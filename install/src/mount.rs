use disk::is_efi_booted;
use rustix::{
    io::Errno,
    mount::{self, MountFlags},
};
use snafu::{ResultExt, Snafu};
use std::{
    fs::create_dir_all,
    io,
    path::{Path, PathBuf},
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
        None,
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
pub enum MountInnerError {
    #[snafu(display("failed to mount {point}"))]
    MountInner {
        source: Errno,
        point: &'static str,
        umount: bool,
    },
    #[snafu(display("failed to crate dir: {}", dir.display()))]
    CreateDir { dir: PathBuf, source: io::Error },
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

    if is_efi_booted() && !cfg!(target_arch = "mips64") {
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

    let run_dev = root.join("run").join("udev");
    create_dir_all(&run_dev).context(CreateDirSnafu {
        dir: run_dev.to_path_buf(),
    })?;

    mount_inner(Some("/run/udev"), &run_dev, Some("tmpfs"), MountFlags::BIND).context(
        MountInnerSnafu {
            point: "tmpfs",
            umount: false,
        },
    )?;

    Ok(())
}

/// Remove bind mounts
/// Note: This function should be called outside of the chroot context
pub fn remove_files_mounts(system_path: &Path) -> Result<(), UmountError> {
    let mut mounts = [
        "proc",
        "sys",
        EFIVARS_PATH,
        "dev",
        "dev/pts",
        "dev/shm",
        "run/udev",
    ];

    // 需要按顺序卸载挂载点
    mounts.reverse();

    for i in mounts {
        if (cfg!(target_arch = "mips64") || !is_efi_booted()) && i == EFIVARS_PATH {
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
