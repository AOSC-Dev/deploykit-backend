use crate::partition::get_partition_table_type;
use std::{
    fmt::Display,
    io,
    path::{Path, PathBuf},
};

use gptman::linux::BlockError;
use serde::Serialize;
use snafu::Snafu;
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
    #[error("Failed to Get partition type {path}: {err}")]
    GetPartitionType { path: String, err: std::io::Error },
    #[error(transparent)]
    Utf8(#[from] std::str::Utf8Error),
    #[error("Failed to create partition: {path}: {err}")]
    CreatePartition { path: String, err: std::io::Error },
    #[error("Failed to format partition: {0}")]
    FormatPartition(std::io::Error),
    #[error("Failed to find esp partition: {path}")]
    FindEspPartition { path: String, err: std::io::Error },
    #[error("{path}, unsupport combo: {table} partition table and {bootmode} boot mode")]
    WrongCombo {
        table: Table,
        bootmode: BootMode,
        path: String,
    },
    #[error("Unsupport partition table: {0}")]
    UnsupportedTable(String),
    #[error(transparent)]
    GptMan(#[from] gptman::Error),
    #[error(transparent)]
    MbrMan(#[from] mbrman::Error),
    #[error("Failed to get optimal place")]
    GetOptimalPlace,
    #[error("Failed to reload table: {0:?}")]
    GetTable(BlockError),
    #[error("Failed to create partition table {path}: {err}")]
    NewDisk { path: String, err: std::io::Error },
    #[error("Failed to seek sector size: {0:?}")]
    SeekSector(std::io::Error),
    #[error("Failed to clear sector: {0:?}")]
    ClearSector(std::io::Error),
    #[error("Failed to flush disk: {0:?}")]
    Flush(std::io::Error),
    #[error("Failed to read mounts: {0:?}")]
    ReadMounts(std::io::Error),
    #[error("Failed to open dmsetup")]
    DmSetup { source: std::io::Error },
    #[error("Failed to open lvs")]
    OpenLvs(std::io::Error),
}

impl Serialize for PartitionError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

#[derive(Debug)]
pub enum Table {
    MBR,
    GPT,
}

impl Display for Table {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl Display for BootMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl TryFrom<&str> for Table {
    type Error = CombineError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "gpt" => Ok(Table::GPT),
            "msdos" | "mbr" => Ok(Table::MBR),
            _ => Err(CombineError::UnsupportedTable {
                t: value.to_string(),
            }),
        }
    }
}

#[derive(Debug)]
pub enum BootMode {
    BIOS,
    UEFI,
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

#[cfg(not(target_arch = "mips64"))]
pub fn is_efi_booted() -> bool {
    Path::new("/sys/firmware/efi").exists()
}

#[cfg(target_arch = "mips64")]
pub fn is_efi_booted() -> bool {
    Path::new("/sys/firmware/lefi").exists()
}

#[derive(Debug, Snafu)]
pub enum CombineError {
    #[snafu(display("{} has unsupport combo: {table} partition table and {bootmode} boot mode", path.display()))]
    WrongCombine {
        table: Table,
        bootmode: BootMode,
        path: PathBuf,
    },
    #[snafu(display("Failed to get partition table type: {}", path.display()))]
    PartitionType {
        source: std::io::Error,
        path: PathBuf,
    },
    #[snafu(display("Unsupported partition table type"))]
    UnsupportedTable { t: String },
}

#[cfg(not(target_arch = "powerpc64"))]
pub fn right_combine(device_path: &Path) -> Result<(), CombineError> {
    use snafu::ResultExt;

    let partition_table_t = get_partition_table_type(device_path).context(PartitionTypeSnafu {
        path: device_path.to_path_buf(),
    })?;

    let is_efi_booted = is_efi_booted();

    if (partition_table_t == "gpt" && is_efi_booted)
        || (partition_table_t == "msdos" && !is_efi_booted)
    {
        return Ok(());
    }

    let table = Table::try_from(partition_table_t.as_str())?;

    match table {
        Table::MBR if is_efi_booted => Err(CombineError::WrongCombine {
            table,
            bootmode: BootMode::UEFI,
            path: device_path.to_path_buf(),
        }),
        Table::GPT if !is_efi_booted => Err(CombineError::WrongCombine {
            table,
            bootmode: BootMode::BIOS,
            path: device_path.to_path_buf(),
        }),
        _ => Ok(()),
    }
}

#[cfg(target_arch = "powerpc64")]
pub fn right_combine(device_path: &Path) -> Result<(), CombineError> {
    Ok(())
}
