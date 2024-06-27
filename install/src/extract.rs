use std::{
    io::{self},
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering},
        Arc,
    },
    thread,
    time::Instant,
};

use libxcp::{
    config::Config,
    drivers::{load_driver, Drivers},
    feedback::{ChannelUpdater, StatusUpdate, StatusUpdater},
};
use snafu::Snafu;
use sysinfo::System;
use tracing::debug;

/// Extract the .squashfs and callback download progress
pub(crate) fn extract_squashfs<P>(
    file_size: f64,
    archive: P,
    path: P,
    progress: Arc<AtomicU8>,
    velocity: Arc<AtomicUsize>,
    cancel_install: Arc<AtomicBool>,
) -> Result<(), io::Error>
where
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
                velocity.store(
                    ((v_download_len / 1024.0) / elapsed as f64) as usize,
                    Ordering::SeqCst,
                );
                v_download_len = 0.0;
            }
            progress.store(count as u8, Ordering::SeqCst);
            v_download_len += file_size * count as f64 / 100.0;
        },
        cancel_install,
    )?;

    Ok(())
}

#[derive(Debug, Snafu)]
pub enum CopyError {
    #[snafu(display("Failed to load driver"))]
    LoadDriver { e: String },
    #[snafu(display("Failed to during copy operation"))]
    Copy,
    #[snafu(display("Failed to copy file"))]
    XcpError { e: String },
}

pub(crate) fn copy_system(
    progress: Arc<AtomicU8>,
    velocity: Arc<AtomicUsize>,
    from: &Path,
    to: &Path,
    cancel_install: Arc<AtomicBool>,
    total: usize,
) -> Result<(), CopyError> {
    let sources = vec![from.to_path_buf()];
    let mut config = Config::default();
    config.no_target_directory = true;
    config.fsync = true;
    let config = Arc::new(config);

    let updater = ChannelUpdater::new(&config);
    // The ChannelUpdater is consumed by the driver (so it is properly closed
    // on completion). Retrieve our end of the connection before then.
    let stat_rx = updater.rx_channel();
    let stats: Arc<dyn StatusUpdater> = Arc::new(updater);

    let driver = load_driver(Drivers::ParFile, &config)
        .map_err(|e| CopyError::LoadDriver { e: e.to_string() })?;

    // As we want realtime updates via the ChannelUpdater the
    // copy operation should run in the background.
    let to = to.to_path_buf();
    let handle = thread::spawn(move || driver.copy(sources, &to, stats));

    // Gather the results as we go; our end of the channel has been
    // moved to the driver call and will end when drained.
    let mut timer = Instant::now();
    let mut size_v = 0;
    for stat in stat_rx {
        if cancel_install.load(Ordering::Relaxed) {
            return Ok(());
        }
        match stat {
            StatusUpdate::Copied(v) => {
                let progress_percent = (v as f64 / total as f64) * 100.0;
                progress.store(progress_percent as u8, Ordering::SeqCst);
                debug!("Copied {} bytes", v);
            }
            StatusUpdate::Size(v) => {
                size_v += v;
                if timer.elapsed().as_secs() > 1 {
                    velocity.store(((size_v as f64 / 1024.0) / 1.0) as usize, Ordering::SeqCst);
                    size_v = 0;
                    timer = Instant::now();
                }
                debug!("Size update: {}", v);
            }
            StatusUpdate::Error(e) => {
                panic!("Error during copy: {}", e);
            }
        }
    }

    handle
        .join()
        .map_err(|_| CopyError::Copy)?
        .map_err(|e| CopyError::XcpError { e: e.to_string() })?;

    Ok(())
}
