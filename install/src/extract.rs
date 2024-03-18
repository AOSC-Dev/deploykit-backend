use std::{
    path::Path,
    sync::{atomic::AtomicBool, Arc},
    time::Instant,
};

use sysinfo::System;

use crate::InstallError;

/// Extract the .squashfs and callback download progress
pub(crate) fn extract_squashfs<F, F2, P>(
    file_size: f64,
    archive: P,
    path: P,
    progress: F,
    velocity: F2,
    cancel_install: Arc<AtomicBool>,
) -> Result<(), InstallError>
where
    F: Fn(f64),
    F2: Fn(usize),
    P: AsRef<Path>,
{
    let mut sys = System::new_all();
    sys.refresh_memory();
    let total_memory = sys.total_memory() / 1024 / 1024 / 1024;

    let limit_thread = if total_memory <= 2 { Some(1) } else { None };

    let mut now = Instant::now();
    let mut v_download_len = 0.0;

    unsquashfs_wrapper::extract(
        archive,
        path,
        limit_thread,
        move |count| {
            let elapsed = now.elapsed().as_secs();
            if elapsed >= 1 {
                now = Instant::now();
                velocity(((v_download_len / 1024.0) / elapsed as f64) as usize);
                v_download_len = 0.0;
            }
            progress(count as f64);
            v_download_len += file_size * count as f64 / 100.0;
        },
        cancel_install,
    )
    .map_err(InstallError::Unpack)?;

    Ok(())
}
