use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;
use std::{fs, thread};

use faster_hex::hex_string;
use reqwest::header::HeaderValue;
use reqwest::{header::CONTENT_LENGTH, Client};
use sha2::Digest;
use sha2::Sha256;
use snafu::{ensure, OptionExt, ResultExt, Snafu};
use tokio::io::AsyncWriteExt;
use tracing::debug;

use crate::DownloadType;

#[derive(Debug, Snafu)]
pub enum DownloadError {
    #[snafu(display("Download path is not set"))]
    DownloadPathIsNotSet,
    #[snafu(display("Local file not found: {}", path.display()))]
    LocalFileNotFound { path: PathBuf },
    #[snafu(display("Failed to build download client"))]
    BuildDownloadClient { source: reqwest::Error },
    #[snafu(display("Failed to send request"))]
    SendRequest { source: reqwest::Error },
    #[snafu(display("Failed to create file: {}", path.display()))]
    CreateFile {
        source: std::io::Error,
        path: PathBuf,
    },
    #[snafu(display("Failed to download file: {}", path.display()))]
    DownloadFile {
        source: reqwest::Error,
        path: PathBuf,
    },
    #[snafu(display("Failed to write file: {}", path.display()))]
    WriteFile {
        source: std::io::Error,
        path: PathBuf,
    },
    #[snafu(display("Checksum mismatch"))]
    ChecksumMismatch,
    #[snafu(display("Failed to shutdown file"))]
    ShutdownFile {
        source: std::io::Error,
        path: PathBuf,
    },
}

#[derive(Clone)]
pub enum FilesType {
    File { path: PathBuf, total: usize },
    Dir { path: PathBuf, total: usize },
}

#[derive(Debug, Clone)]
pub struct OverlayEntry {
    pub path: PathBuf,
    pub total: usize,
    pub percent: f32,
}

pub(crate) fn download_file(
    download_type: &DownloadType,
    progress: Arc<AtomicU8>,
    velocity: Arc<AtomicUsize>,
    cancel_install: Arc<AtomicBool>,
) -> Result<FilesType, DownloadError> {
    match download_type {
        DownloadType::Http { url, hash, to_path } => {
            let to_path = to_path.as_ref().context(DownloadPathIsNotSetSnafu)?;
            let size = http_download_file(
                url,
                to_path,
                hash,
                progress.clone(),
                velocity.clone(),
                cancel_install,
            )?;
            Ok(FilesType::File {
                path: to_path.clone(),
                total: size,
            })
        }
        DownloadType::File(path) => {
            ensure!(
                path.exists(),
                LocalFileNotFoundSnafu {
                    path: path.to_owned()
                }
            );

            velocity.store(0, Ordering::SeqCst);
            progress.store(100, Ordering::SeqCst);

            let total = fs::metadata(path).map(|x| x.len()).unwrap_or(1) as usize;

            Ok(FilesType::File {
                path: path.clone(),
                total,
            })
        }
        DownloadType::Dir(path) => {
            ensure!(
                path.exists(),
                LocalFileNotFoundSnafu {
                    path: path.to_owned()
                }
            );

            velocity.store(0, Ordering::SeqCst);
            progress.store(100, Ordering::SeqCst);

            Ok(FilesType::Dir {
                path: path.clone(),
                total: fs::metadata(path).map(|x| x.len()).unwrap_or(1) as usize,
            })
        }
    }
}

fn http_download_file(
    url: &str,
    path: &Path,
    hash: &str,
    progress: Arc<AtomicU8>,
    velocity: Arc<AtomicUsize>,
    cancel_install: Arc<AtomicBool>,
) -> Result<usize, DownloadError> {
    let url = url.to_string();
    let hash = hash.to_string();
    let path = path.to_path_buf();
    thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async move {
                http_download_file_inner(url, path, hash, &progress, &velocity, &cancel_install)
                    .await
            })
    })
    .join()
    .unwrap()
}

async fn http_download_file_inner(
    url: String,
    path: PathBuf,
    hash: String,
    progress: &AtomicU8,
    velocity: &AtomicUsize,
    cancel_install: &AtomicBool,
) -> Result<usize, DownloadError> {
    let client = Client::builder()
        .user_agent("deploykit")
        .build()
        .context(BuildDownloadClientSnafu)?;

    let head = client
        .head(&url)
        .send()
        .await
        .and_then(|x| x.error_for_status())
        .context(SendRequestSnafu)?;

    let total_size = head
        .headers()
        .get(CONTENT_LENGTH)
        .map(|x| x.to_owned())
        .unwrap_or_else(|| HeaderValue::from(1));

    let total_size = total_size
        .to_str()
        .ok()
        .and_then(|x| x.parse::<usize>().ok())
        .unwrap_or(1);

    let mut file = tokio::fs::File::create(&path)
        .await
        .context(CreateFileSnafu { path: path.clone() })?;

    let mut resp = client
        .get(url)
        .send()
        .await
        .and_then(|x| x.error_for_status())
        .context(SendRequestSnafu)?;

    let mut now = Instant::now();
    let mut v_download_len = 0;
    let mut download_len = 0;

    while let Some(chunk) = resp
        .chunk()
        .await
        .context(DownloadFileSnafu { path: path.clone() })?
    {
        if now.elapsed().as_secs() >= 1 {
            now = Instant::now();
            velocity.store(v_download_len / 1024, Ordering::SeqCst);
            v_download_len = 0;
        }

        if cancel_install.load(Ordering::Relaxed) {
            return Ok(0);
        }

        file.write_all(&chunk)
            .await
            .context(WriteFileSnafu { path: path.clone() })?;

        progress.store(
            (download_len as f64 / total_size as f64 * 100.0).round() as u8,
            Ordering::SeqCst,
        );

        v_download_len += chunk.len();
        download_len += chunk.len();
    }

    let pc = path.clone();

    tokio::task::spawn_blocking(move || {
        let mut file = std::fs::File::open(&pc).context(CreateFileSnafu { path: pc.clone() })?;

        let mut sha256 = Sha256::new();
        std::io::copy(&mut file, &mut sha256).context(WriteFileSnafu { path: pc.clone() })?;

        let download_hash = sha256.finalize().to_vec();
        let checksum = hex_string(&download_hash);

        debug!("Right hash: {hash}");
        debug!("Now checksum: {checksum}");
        ensure!(checksum == hash, ChecksumMismatchSnafu);
        debug!("Checksum is ok");

        Ok(())
    })
    .await
    .unwrap()?;

    file.shutdown()
        .await
        .context(ShutdownFileSnafu { path: path.clone() })?;

    Ok(total_size)
}
