use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;
use std::{fs, thread};

use reqwest::header::HeaderValue;
use reqwest::{header::CONTENT_LENGTH, Client};
use sha2::Digest;
use sha2::Sha256;
use tokio::io::AsyncWriteExt;

use crate::{DownloadType, InstallError};

pub fn download_file<F, F2>(
    download_type: &DownloadType,
    progress: Arc<F>,
    velocity: Arc<F2>,
    cancel_install: Arc<AtomicBool>,
) -> Result<(PathBuf, usize), InstallError>
where
    F: Fn(f64) + Sync + Send + 'static,
    F2: Fn(usize) + Send + Sync + 'static,
{
    match download_type {
        DownloadType::Http { url, hash, to_path } => {
            let to_path = to_path.as_ref().ok_or(InstallError::DownloadPathIsNotSet)?;
            Ok((
                to_path.clone(),
                http_download_file(url, to_path, hash, progress, velocity, cancel_install)?,
            ))
        }
        DownloadType::File(path) => {
            if !path.exists() {
                return Err(InstallError::LocalFileNotFound(path.display().to_string()));
            }

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
) -> Result<usize, InstallError>
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
) -> Result<usize, InstallError>
where
    F: Fn(f64) + Sync + Send,
    F2: Fn(usize) + Send + Sync,
{
    let client = Client::builder().user_agent("deploykit").build()?;

    let head = client
        .head(&url)
        .send()
        .await
        .and_then(|x| x.error_for_status())?;

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
        .map_err(|e| InstallError::OperateFile {
            path: path.display().to_string(),
            err: e,
        })?;

    let mut resp = client
        .get(url)
        .send()
        .await
        .and_then(|x| x.error_for_status())?;

    let mut v = Sha256::new();

    let mut now = Instant::now();
    let mut v_download_len = 0;
    let mut download_len = 0;

    while let Some(chunk) = resp.chunk().await? {
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
            .map_err(|e| InstallError::OperateFile {
                path: path.display().to_string(),
                err: e,
            })?;

        progress((download_len as f64 / total_size as f64 * 100.0).round());
        v.update(&chunk);
        v_download_len += chunk.len();
        download_len += chunk.len();
    }

    tokio::task::spawn_blocking(move || {
        let download_hash = v.finalize().to_vec();

        if hex::encode(download_hash) != hash {
            return Err(InstallError::ChecksumMisMatch);
        }

        Ok(())
    })
    .await
    .unwrap()?;

    file.shutdown()
        .await
        .map_err(|e| InstallError::OperateFile {
            path: path.display().to_string(),
            err: e,
        })?;

    Ok(total_size)
}
