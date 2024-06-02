use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;
use std::{fs, thread};

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

pub(crate) fn download_file<F, F2>(
    download_type: &DownloadType,
    progress: Arc<F>,
    velocity: Arc<F2>,
    cancel_install: Arc<AtomicBool>,
) -> Result<(PathBuf, usize), DownloadError>
where
    F: Fn(f64) + Sync + Send + 'static,
    F2: Fn(usize) + Send + Sync + 'static,
{
    match download_type {
        DownloadType::Http { url, hash, to_path } => {
            let to_path = to_path.as_ref().context(DownloadPathIsNotSetSnafu)?;
            Ok((
                to_path.clone(),
                http_download_file(url, to_path, hash, progress, velocity, cancel_install)?,
            ))
        }
        DownloadType::File(path) => {
            ensure!(
                path.exists(),
                LocalFileNotFoundSnafu {
                    path: path.to_owned()
                }
            );

            velocity(0);
            progress(100.0);

            let total = fs::metadata(path).map(|x| x.len()).unwrap_or(1) as usize;

            Ok((path.to_path_buf(), total))
        }
    }
}

fn http_download_file<F, F2>(
    url: &str,
    path: &Path,
    hash: &str,
    progress: Arc<F>,
    velocity: Arc<F2>,
    cancel_install: Arc<AtomicBool>,
) -> Result<usize, DownloadError>
where
    F: Fn(f64) + Sync + Send + 'static,
    F2: Fn(usize) + Send + Sync + 'static,
{
    let url = url.to_string();
    let hash = hash.to_string();
    let path = path.to_path_buf();
    thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        runtime.block_on(async move {
            http_download_file_inner(url, path, hash, progress, velocity, cancel_install).await
        })
    })
    .join()
    .unwrap()
}

async fn http_download_file_inner<F, F2>(
    url: String,
    path: PathBuf,
    hash: String,
    progress: Arc<F>,
    velocity: Arc<F2>,
    cancel_install: Arc<AtomicBool>,
) -> Result<usize, DownloadError>
where
    F: Fn(f64) + Sync + Send,
    F2: Fn(usize) + Send + Sync,
{
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

    let mut v = Sha256::new();

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
            velocity(v_download_len / 1024);
            v_download_len = 0;
        }

        if cancel_install.load(Ordering::Relaxed) {
            return Ok(0);
        }

        file.write_all(&chunk)
            .await
            .context(WriteFileSnafu { path: path.clone() })?;

        progress((download_len as f64 / total_size as f64 * 100.0).round());
        v.update(&chunk);
        v_download_len += chunk.len();
        download_len += chunk.len();
    }

    tokio::task::spawn_blocking(move || {
        let download_hash = v.finalize().to_vec();
        let checksum = hex::encode(download_hash);
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
