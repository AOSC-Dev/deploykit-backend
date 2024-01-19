use std::{path::Path, time::Instant};

use sysinfo::System;

use crate::InstallError;

/// Extract the .squashfs and callback download progress
pub(crate) fn extract_squashfs<F, F2, P>(
    file_size: f64,
    archive: P,
    path: P,
    progress: F,
    velocity: F2,
) -> Result<(), InstallError>
where
    F: Fn(usize),
    F2: Fn(usize),
    P: AsRef<Path>,
{
    let mut sys = System::new_all();
    sys.refresh_memory();
    let total_memory = sys.total_memory() / 1024 / 1024 / 1024;

    let limit_thread = if total_memory <= 2 { Some(1) } else { None };

    let mut now = Instant::now();
    unsquashfs_wrapper::extract(archive, path, limit_thread, move |count| {
        if now.elapsed().as_secs() >= 1 {
            now = Instant::now();
            velocity((((file_size / 1024.0) * count as f64 / 100.0) / 1.0) as usize)
        }
        progress(count as usize);
    })
    .map_err(InstallError::Unpack)?;

    Ok(())
}
