use std::path::PathBuf;

use disk::PartitionError;
use snafu::Snafu;
use snafu::prelude::*;

#[derive(Debug, Snafu)]
pub enum SetupPartitionError {
    #[snafu(display("Failed to format partition {path}"))]
    Format { path: String, source: PartitionError  },
    #[snafu(display("Failed to mount partition {path}"))]
    Mount { path: String, source: MountError },

}

#[derive(Debug, Snafu)]
pub enum MountError {
    #[snafu(display("value is not set: {t}"))]
    ValueNotSet { t: &'static str },
    #[snafu(display(""))]
    IOError { source: std::io::Error, path: String },
}
