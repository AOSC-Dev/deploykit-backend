use std::{
    ffi::OsString,
    fmt,
    io::Write,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
};

use disk::partition_identity::{PartitionID, PartitionSource};
use snafu::{OptionExt, ResultExt, Snafu};
use std::ffi::OsStr;

/// Describes a file system format, such as ext4 or fat32.
#[derive(Debug, PartialEq, Copy, Clone, Hash)]
pub enum FileSystem {
    Btrfs,
    Exfat,
    Ext2,
    Ext3,
    Ext4,
    F2fs,
    Fat16,
    Fat32,
    Ntfs,
    Swap,
    Xfs,
    Luks,
    Lvm,
}

impl From<FileSystem> for &'static str {
    fn from(val: FileSystem) -> Self {
        match val {
            FileSystem::Btrfs => "btrfs",
            FileSystem::Exfat => "exfat",
            FileSystem::Ext2 => "ext2",
            FileSystem::Ext3 => "ext3",
            FileSystem::Ext4 => "ext4",
            FileSystem::F2fs => "f2fs",
            FileSystem::Fat16 => "fat16",
            FileSystem::Fat32 => "fat32",
            FileSystem::Ntfs => "ntfs",
            FileSystem::Swap => "linux-swap(v1)",
            FileSystem::Xfs => "xfs",
            FileSystem::Lvm => "lvm",
            FileSystem::Luks => "luks",
        }
    }
}

impl fmt::Display for FileSystem {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        let str: &'static str = (*self).into();
        f.write_str(str)
    }
}

#[derive(Debug, Snafu)]
pub enum GenfstabError {
    #[snafu(display("Unsupported filesystem: {fs_type}"))]
    UnsupportedFileSystem { fs_type: String },
    #[snafu(display("Partition {} has no UUID", path.display()))]
    UUID { path: PathBuf },
    #[snafu(display("Failed to operate /etc/fstab"))]
    OperateFstabFile { source: std::io::Error },
}

/// Gen fstab to /etc/fstab
pub(crate) fn genfstab_to_file(
    partition_path: &Path,
    fs_type: &str,
    root_path: &Path,
    mount_path: &Path,
) -> Result<(), GenfstabError> {
    if cfg!(debug_assertions) {
        return Ok(());
    }

    let s = fstab_entries(partition_path, fs_type, Some(mount_path))?;
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(root_path.join("etc/fstab"))
        .context(OperateFstabFileSnafu)?;

    f.write_all(s.as_bytes()).context(OperateFstabFileSnafu)?;

    Ok(())
}

/// Must be used in a chroot context
pub fn write_swap_entry_to_fstab() -> Result<(), GenfstabError> {
    let s = "/swapfile none swap defaults,nofail 0 0\n";
    let mut fstab = std::fs::OpenOptions::new()
        .append(true)
        .open("/etc/fstab")
        .context(OperateFstabFileSnafu)?;

    fstab
        .write_all(s.as_bytes())
        .context(OperateFstabFileSnafu)?;

    Ok(())
}

fn fstab_entries(
    device_path: &Path,
    fs_type: &str,
    mount_path: Option<&Path>,
) -> Result<OsString, GenfstabError> {
    let (fs_type, option) = match fs_type {
        "vfat" | "fat16" | "fat32" => (FileSystem::Fat32, "defaults,nofail"),
        "ext4" => (FileSystem::Ext4, "defaults"),
        "btrfs" => (FileSystem::Btrfs, "defaults"),
        "xfs" => (FileSystem::Xfs, "defaults"),
        "f2fs" => (FileSystem::F2fs, "defaults"),
        "swap" => (FileSystem::Swap, "sw"),
        _ => {
            return Err(GenfstabError::UnsupportedFileSystem {
                fs_type: fs_type.to_string(),
            });
        }
    };

    let root_id = BlockInfo::get_partition_id(device_path, fs_type)
        .context(UUIDSnafu { path: device_path })?;

    let root = BlockInfo::new(root_id, fs_type, mount_path, option);
    let fstab = &mut OsString::new();
    root.write_entry(fstab);

    Ok(fstab.to_owned())
}

/// Information that will be used to generate a fstab entry for the given
/// partition.
/// Code copy from https://github.com/pop-os/distinst/blob/master/crates/fstab-generate
#[derive(Debug, PartialEq)]
struct BlockInfo<'a> {
    uid: PartitionID,
    mount: Option<PathBuf>,
    fs: &'static str,
    options: &'a str,
    dump: bool,
    pass: bool,
}

impl<'a> BlockInfo<'a> {
    fn new(uid: PartitionID, fs: FileSystem, target: Option<&Path>, options: &'a str) -> Self {
        let pass = target == Some(Path::new("/"));
        BlockInfo {
            uid,
            mount: if fs == FileSystem::Swap {
                None
            } else {
                Some(
                    target
                        .expect("unable to get block info due to lack of target")
                        .to_path_buf(),
                )
            },
            fs: match fs {
                FileSystem::Fat16 | FileSystem::Fat32 => "vfat",
                FileSystem::Swap => "swap",
                _ => fs.into(),
            },
            options,
            dump: false,
            pass,
        }
    }

    /// Writes a single line to the fstab buffer for this file system.
    fn write_entry(&self, fstab: &mut OsString) {
        let mount_variant = match self.uid.variant {
            PartitionSource::ID => "ID=",
            PartitionSource::Label => "LABEL=",
            PartitionSource::PartLabel => "PARTLABEL=",
            PartitionSource::PartUUID => "PARTUUID=",
            PartitionSource::Path => "",
            PartitionSource::UUID => "UUID=",
        };

        fstab.push(mount_variant);
        fstab.push(&self.uid.id);
        fstab.push("  ");
        fstab.push(self.mount());
        fstab.push("  ");
        fstab.push(self.fs);
        fstab.push("  ");
        fstab.push(self.options);
        fstab.push("  ");
        fstab.push(if self.dump { "1" } else { "0" });
        fstab.push("  ");
        fstab.push(if self.pass { "1" } else { "0" });
        fstab.push("\n");
    }

    /// Retrieve the mount point, which is `none` if non-existent.
    fn mount(&self) -> &OsStr {
        self.mount
            .as_ref()
            .map_or(OsStr::new("none"), |path| path.as_os_str())
    }

    /// Helper for fetching the Partition ID of a partition.
    ///
    /// # Notes
    /// FAT partitions are prone to UUID collisions, so PartUUID will be used instead.
    fn get_partition_id(path: &Path, fs: FileSystem) -> Option<PartitionID> {
        if fs == FileSystem::Fat16 || fs == FileSystem::Fat32 {
            PartitionID::get_partuuid(path)
        } else {
            PartitionID::get_uuid(path)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn fstab_entries() {
        let swap_id = PartitionID {
            id: "SWAP".into(),
            variant: PartitionSource::UUID,
        };
        let swap = BlockInfo::new(swap_id, FileSystem::Swap, None, "sw");
        let efi_id = PartitionID {
            id: "EFI".into(),
            variant: PartitionSource::PartUUID,
        };
        let efi = BlockInfo::new(
            efi_id,
            FileSystem::Fat32,
            Some(Path::new("/boot/efi")),
            "defaults",
        );
        let root_id = PartitionID {
            id: "ROOT".into(),
            variant: PartitionSource::UUID,
        };
        let root = BlockInfo::new(root_id, FileSystem::Ext4, Some(Path::new("/")), "defaults");

        let fstab = &mut OsString::new();
        swap.write_entry(fstab);
        efi.write_entry(fstab);
        root.write_entry(fstab);

        assert_eq!(
            *fstab,
            OsString::from(
                r#"UUID=SWAP  none  swap  sw  0  0
PARTUUID=EFI  /boot/efi  vfat  defaults  0  0
UUID=ROOT  /  ext4  defaults  0  1
"#
            )
        );
    }

    #[test]
    fn block_info_swap() {
        let id = PartitionID {
            variant: PartitionSource::UUID,
            id: "TEST".to_owned(),
        };
        let swap = BlockInfo::new(id, FileSystem::Swap, None, "sw");
        assert_eq!(
            swap,
            BlockInfo {
                uid: PartitionID {
                    variant: PartitionSource::UUID,
                    id: "TEST".to_owned()
                },
                mount: None,
                fs: "swap",
                options: "sw",
                dump: false,
                pass: false,
            }
        );
        assert_eq!(swap.mount(), OsStr::new("none"));
    }

    #[test]
    fn block_info_efi() {
        let id = PartitionID {
            variant: PartitionSource::PartUUID,
            id: "TEST".to_owned(),
        };
        let efi = BlockInfo::new(
            id,
            FileSystem::Fat32,
            Some(Path::new("/boot/efi")),
            "defaults",
        );
        assert_eq!(
            efi,
            BlockInfo {
                uid: PartitionID {
                    variant: PartitionSource::PartUUID,
                    id: "TEST".to_owned()
                },
                mount: Some(PathBuf::from("/boot/efi")),
                fs: "vfat",
                options: "defaults",
                dump: false,
                pass: false,
            }
        );
        assert_eq!(efi.mount(), OsStr::new("/boot/efi"));
    }

    #[test]
    fn block_info_root() {
        let id = PartitionID {
            variant: PartitionSource::UUID,
            id: "TEST".to_owned(),
        };
        let root = BlockInfo::new(id, FileSystem::Ext4, Some(Path::new("/")), "defaults");
        assert_eq!(
            root,
            BlockInfo {
                uid: PartitionID {
                    variant: PartitionSource::UUID,
                    id: "TEST".to_owned()
                },
                mount: Some(PathBuf::from("/")),
                fs: FileSystem::Ext4.into(),
                options: "defaults",
                dump: false,
                pass: true,
            }
        );
        assert_eq!(root.mount(), OsStr::new("/"));
    }
}
