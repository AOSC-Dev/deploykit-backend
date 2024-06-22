use std::{
    io::{self, BufRead, BufReader},
    path::Path,
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Instant,
};

use snafu::{ensure, OptionExt, ResultExt, Snafu};
use sysinfo::System;
use tracing::{error, warn};

use crate::utils::RunCmdError;

/// Extract the .squashfs and callback download progress
pub(crate) fn extract_squashfs<F, F2, P>(
    file_size: f64,
    archive: P,
    path: P,
    progress: F,
    velocity: F2,
    cancel_install: Arc<AtomicBool>,
) -> Result<(), io::Error>
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
    )?;

    Ok(())
}

#[derive(Debug, Snafu)]
pub enum RsyncError {
    #[snafu(transparent)]
    RunCmdError { source: RunCmdError },
    #[snafu(display("Failed to get stderr"))]
    GetStderr,
    #[snafu(display("Failed to read stderr"))]
    ReadStderr { source: io::Error },
    #[snafu(display("Failed to parse rsync progress"))]
    ParseProgress { source: std::num::ParseFloatError },
    #[snafu(display("Failed to parse rsync velocity"))]
    ParseVelocity { source: std::num::ParseIntError },
    #[snafu(display("rsync return non-zero status: {status}"))]
    RsyncFailed { status: i32 },
}

pub(crate) fn rsync_system<F, F2>(
    progress: F,
    velocity: F2,
    from: &Path,
    to: &Path,
    cancel_install: Arc<AtomicBool>,
    total: usize,
) -> Result<(), RsyncError>
where
    F: Fn(f64),
    F2: Fn(usize),
{
    let mut from = from.to_string_lossy().to_string();
    let mut to = to.to_string_lossy().to_string();

    for i in [&mut from, &mut to] {
        if !i.ends_with('/') {
            *i += "/";
        }
    }

    let mut child = Command::new("rsync")
        .arg("-a")
        .arg("-x")
        .arg("-H")
        .arg("-A")
        .arg("-X")
        .arg("-S")
        .arg("-W")
        .arg("--numeric-ids")
        .arg("--info=progress2")
        .arg("--no-i-r")
        .arg(&from)
        .arg(&to)
        .stdout(Stdio::piped())
        .env("LANG", "C.UTF-8")
        .spawn()
        .map_err(|e| RunCmdError::Exec {
            cmd: format!(
                "rsync -a -H -A -X -S -W --numeric-ids --info=progress2 --no-i-r {} {}",
                from, to
            ),
            source: e,
        })?;

    let mut stdout = BufReader::new(child.stdout.take().context(GetStderrSnafu)?);

    let mut now = Instant::now();
    loop {
        if cancel_install.load(Ordering::SeqCst) {
            return Ok(());
        }

        let length = {
            let buffer = stdout.fill_buf().context(ReadStderrSnafu)?;

            let line_size = buffer
                .iter()
                .take_while(|c| **c != b'\n' || **c != b'\r')
                .count();

            if line_size == 0 {
                break;
            }

            let line = std::str::from_utf8(&buffer[..line_size]);

            match line {
                Ok(line) => {
                    let mut line_split = line.split_ascii_whitespace();
                    let prog = line_split.nth(1);
                    if let Some(prog) = prog
                        .and_then(|x| x.strip_suffix("%"))
                        .and_then(|x| x.parse::<f64>().ok())
                    {
                        progress(prog);
                        let elapsed = now.elapsed().as_secs();
                        if elapsed >= 1 {
                            velocity(((total as f64 * (prog / 100.0)) / elapsed as f64) as usize);
                            now = Instant::now();
                        }
                    } else {
                        warn!("rsync progress has except output: {}", line);
                    }
                }
                Err(e) => {
                    error!("Failed to parse rsync progress: {}", e);
                }
            }

            line_size
                + if line_size < buffer.len() {
                    // we found a delimiter
                    if line_size + 1 < buffer.len() // we look if we found two delimiter
                && buffer[line_size] == b'\r'
                && buffer[line_size + 1] == b'\n'
                    {
                        2
                    } else {
                        1
                    }
                } else {
                    0
                }
        };

        stdout.consume(length);
    }

    let rsync_finish = child.wait().map_err(|e| RunCmdError::Exec {
        cmd: format!(
            "rsync -a -H -A -X -S -W --info=progress2 --numeric-ids --no-i-r {} {}",
            from, to
        ),
        source: e,
    })?;

    ensure!(
        rsync_finish.success(),
        RsyncFailedSnafu {
            status: rsync_finish.code().unwrap_or(1)
        }
    );

    Ok(())
}
