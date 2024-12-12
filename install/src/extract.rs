use std::{
    io::{self, BufRead, BufReader},
    path::Path,
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering},
        Arc,
    },
    time::Instant,
};

use snafu::{ensure, OptionExt, ResultExt, Snafu};
use sysinfo::System;
use tracing::{debug, error, warn};

use crate::utils::RunCmdError;

/// Extract the .squashfs and callback download progress
pub(crate) fn extract_squashfs<P>(
    file_size: f64,
    archive: P,
    path: P,
    progress: &AtomicU8,
    velocity: &AtomicUsize,
    cancel_install: Arc<AtomicBool>,
    eta: &AtomicUsize,
) -> Result<(), io::Error>
where
    P: AsRef<Path>,
{
    let mut sys = System::new_all();
    sys.refresh_memory();
    let total_memory = sys.total_memory() / 1024 / 1024 / 1024;

    let limit_thread = if total_memory <= 2 { Some(1) } else { None };

    let mut now = Instant::now();
    let now2 = Instant::now();
    let mut v_download_len = 0.0;

    unsquashfs_wrapper::extract(
        archive,
        path,
        limit_thread,
        move |count| {
            let elapsed = now.elapsed().as_secs();
            let v = ((v_download_len / 1024.0) / elapsed as f64) as usize;
            if elapsed >= 1 {
                now = Instant::now();
                velocity.store(v, Ordering::SeqCst);
                v_download_len = 0.0;
            }
            progress.store(count as u8, Ordering::SeqCst);
            eta.store(
                ((file_size as usize).checked_div(velocity.load(Ordering::SeqCst)))
                    .unwrap_or(0)
                    .saturating_sub(now2.elapsed().as_secs() as usize),
                Ordering::SeqCst,
            );
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
    #[snafu(display("Failed to get stdout"))]
    GetStdout,
    #[snafu(display("Failed to read stdout"))]
    ReadStdout { source: io::Error },
    #[snafu(display("Failed to parse rsync progress"))]
    ParseProgress { source: std::num::ParseIntError },
    #[snafu(display("Failed to parse rsync velocity"))]
    ParseVelocity { source: std::num::ParseIntError },
    #[snafu(display("rsync return non-zero status: {status}"))]
    RsyncFailed { status: i32 },
}

pub(crate) fn rsync_system(
    progress: &AtomicU8,
    velocity: &AtomicUsize,
    from: &Path,
    to: &Path,
    cancel_install: &AtomicBool,
    total: usize,
    eta: &AtomicUsize,
) -> Result<(), RsyncError> {
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

    let mut stdout = BufReader::new(child.stdout.take().context(GetStdoutSnafu)?);

    let now = Instant::now();
    let now2 = Instant::now();
    loop {
        if cancel_install.load(Ordering::SeqCst) {
            child.kill().ok();
            return Ok(());
        }

        let length = {
            let buffer = stdout.fill_buf().context(ReadStdoutSnafu)?;

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
                    debug!("rsync output: {line}");
                    let mut line_split = line.split_ascii_whitespace();
                    let prog = line_split.next_back();
                    if let Some((uncheck, total_files)) = prog
                        .and_then(|x| x.strip_suffix(')'))
                        .and_then(|x| x.strip_prefix("to-chk="))
                        .and_then(|x| x.split_once('/'))
                    {
                        let uncheck = uncheck.parse::<u64>().context(ParseProgressSnafu)?;
                        let total_files = total_files.parse::<u64>().context(ParseProgressSnafu)?;
                        progress.store(
                            (((total_files - uncheck) as f64 / total_files as f64) * 100.0) as u8,
                            Ordering::SeqCst,
                        );
                        let elapsed = now.elapsed().as_secs();
                        let v = total
                            * ((total_files - uncheck) as f64 / total_files as f64) as usize
                            / elapsed as usize;
                        if elapsed >= 1 {
                            velocity.store(v, Ordering::SeqCst);
                        }
                        eta.store(
                            ((total_files as usize).checked_div(velocity.load(Ordering::SeqCst)))
                                .unwrap_or(0)
                                .saturating_sub(now2.elapsed().as_secs() as usize),
                            Ordering::SeqCst,
                        );
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
