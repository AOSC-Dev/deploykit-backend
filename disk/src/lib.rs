use std::{io, path::Path};

use thiserror::Error;

pub mod devices;
pub mod partition;

pub use disk_types;

#[derive(Debug, Error)]
pub enum PartitionError {
    #[error("Failed to open device {path}: {err}")]
    OpenDevice { path: String, err: std::io::Error },
    #[error("Failed to open disk {path}: {err}")]
    OpenDisk { path: String, err: std::io::Error },
    #[error("Failed to create partition table {path}: {err}")]
    NewPartitionTable { path: String, err: std::io::Error },
    #[error("Failed to commit partition table {path}: {err}")]
    CommitChanges { path: String, err: std::io::Error },
    #[error("Failed to Get partition type {path}: {err}")]
    GetPartitionType { path: String, err: std::io::Error },
    #[error(transparent)]
    Utf8(#[from] std::str::Utf8Error),
    #[error("Failed to create partition: {0}, partition size must less than 2TiB")]
    MBRMaxSizeLimit(String),
    #[error("Failed to remove partition: {path}, number: {number}: {err}")]
    RemovePartition {
        path: String,
        number: u32,
        err: std::io::Error,
    },
    #[error("Failed to create partition: {path}: {err}")]
    CreatePartition { path: String, err: std::io::Error },
    #[error("Failed to format partition: {0}")]
    FormatPartition(std::io::Error),
    #[error("Failed to sync device {path}: {err}")]
    SyncDevice { path: String, err: std::io::Error },
    #[error("Could not find partition by sector: {0}")]
    FindSector(u64),
    #[error("Failed to find esp partition: {path}")]
    FindEspPartition { path: String, err: std::io::Error }
}

pub fn is_efi_booted() -> bool {
    Path::new("/sys/firmware/efi").exists()
}

impl PartitionError {
    pub fn open_device(path: &Path, err: io::Error) -> Self {
        PartitionError::OpenDevice {
            path: path.display().to_string(),
            err,
        }
    }

    pub fn open_disk(path: &Path, err: io::Error) -> Self {
        PartitionError::OpenDisk {
            path: path.display().to_string(),
            err,
        }
    }

    pub fn create_partition(path: &Path, err: io::Error) -> Self {
        PartitionError::CreatePartition {
            path: path.display().to_string(),
            err,
        }
    }
}
