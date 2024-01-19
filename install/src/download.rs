use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use reqwest::header::HeaderValue;
use reqwest::{header::CONTENT_LENGTH, Client};
use sha2::Digest;
use sha2::Sha256;
use tokio::io::AsyncWriteExt;

use crate::{DownloadType, InstallError};

pub fn download_file<F: Fn(usize), F2: Fn(usize)>(
    download_type: &DownloadType,
    progress: F,
    velocity: F2,
) -> Result<(PathBuf, usize), InstallError> {
    match download_type {
        DownloadType::Http { url, hash, to_path } => Ok((
            to_path.clone(),
            http_download_file(url, to_path, hash, progress, velocity)?,
        )),
        DownloadType::File(path) => {
            if !path.exists() {
                return Err(InstallError::LocalFileNotFound(path.display().to_string()));
            }

            velocity(0);
            progress(100);

            let total = fs::metadata(path).map(|x| x.len()).unwrap_or(1) as usize;

            Ok((path.to_path_buf(), total))
        }
    }
}

fn http_download_file<F: Fn(usize), F2: Fn(usize)>(
    url: &str,
    path: &Path,
    hash: &str,
    progress: F,
    velocity: F2,
) -> Result<usize, InstallError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(InstallError::CreateTokioRuntime)?;

    runtime.block_on(async { http_download_file_inner(url, path, hash, progress, velocity).await })
}

async fn http_download_file_inner<F: Fn(usize), F2: Fn(usize)>(
    url: &str,
    path: &Path,
    hash: &str,
    progress: F,
    velocity: F2,
) -> Result<usize, InstallError> {
    let client = Client::builder().user_agent("oma").build()?;

    let head = client
        .head(url)
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

    let mut file = tokio::fs::File::create(path)
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
    let mut download_len = 0;

    while let Some(chunk) = resp.chunk().await? {
        if now.elapsed().as_secs() >= 1 {
            now = Instant::now();
            velocity((download_len / 1024) / 1);
        }
        file.write_all(&chunk).await.unwrap();
        progress(chunk.len() / total_size);
        v.update(&chunk);
        download_len += chunk.len();
    }

    let download_hash = v.finalize().to_vec();

    if download_hash != hash.as_bytes() {
        return Err(InstallError::ChecksumMisMatch);
    }

    file.shutdown()
        .await
        .map_err(|e| InstallError::OperateFile {
            path: path.display().to_string(),
            err: e,
        })?;

    Ok(total_size)
}
