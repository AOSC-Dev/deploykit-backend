use std::{io::Read, path::Path};

use sysinfo::System;

use crate::InstallError;

/// Extract the .squashfs and callback download progress
pub(crate) fn extract_squashfs<F, P>(
    file_size: f64,
    archive: P,
    path: P,
    f: F,
) -> Result<(), InstallError>
where
    F: Fn(f64),
    P: AsRef<Path>,
{
    let mut sys = System::new_all();
    sys.refresh_memory();
    let total_memory = sys.total_memory() / 1024 / 1024 / 1024;

    let limit_thread = if total_memory <= 2 { Some(1) } else { None };

    unsquashfs_wrapper::extract(archive, path, limit_thread, move |count| {
        f(file_size * count as f64 / 100.0);
    })
    .map_err(InstallError::Unpack)?;

    Ok(())
}

