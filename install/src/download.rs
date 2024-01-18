use std::path::{Path, PathBuf};

use reqwest::header::HeaderValue;
use reqwest::{header::CONTENT_LENGTH, Client};
use sha2::Digest;
use sha2::Sha256;
use tokio::io::AsyncWriteExt;

use crate::InstallError;

pub enum DownloadType {
    Http {
        url: String,
        hash: String,
        to_path: PathBuf,
    },
    File(PathBuf),
}

pub fn download_file<F: Fn(usize)>(
    download_type: &DownloadType,
    f: F,
) -> Result<PathBuf, InstallError> {
    match download_type {
        DownloadType::Http { url, hash, to_path } => {
            http_download_file(url, to_path, hash, f)?;
            Ok(to_path.clone())
        }
        DownloadType::File(path) => Ok(path.to_path_buf()),
    }
}

fn http_download_file<F: Fn(usize)>(
    url: &str,
    path: &Path,
    hash: &str,
    f: F,
) -> Result<(), InstallError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    runtime.block_on(async { http_download_file_inner(url, path, hash, f).await })?;

    Ok(())
}

async fn http_download_file_inner<F: Fn(usize)>(
    url: &str,
    path: &Path,
    hash: &str,
    f: F,
) -> Result<(), InstallError> {
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
        .and_then(|x| x.parse::<u64>().ok())
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

    while let Some(chunk) = resp.chunk().await? {
        file.write_all(&chunk).await.unwrap();
        f(chunk.len() / total_size as usize);
        v.update(&chunk);
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

    Ok(())
}
