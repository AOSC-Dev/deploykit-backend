use thiserror::Error;

pub mod devices;

#[derive(Debug, Error)]
pub enum PartitionError {
    #[error("Failed to open device: {path}, {err}")]
    OpenDevice {
        path: String,
        err: std::io::Error,
    },
    #[error("Failed to open disk: {path}, {err}")]
    OpenDisk {
        path: String,
        err: std::io::Error,
    }
}