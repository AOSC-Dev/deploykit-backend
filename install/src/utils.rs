use std::fmt::Debug;
use std::{ffi::OsStr, process::Command};

use snafu::{ResultExt, Snafu};
use tracing::info;

#[derive(Debug, Snafu)]
pub enum RunCmdError {
    #[snafu(display("Failed to execute command: {cmd}"))]
    Exec { cmd: String, source: std::io::Error },
    #[snafu(display("return non-zero value run command: {cmd}"))]
    RunFailed {
        cmd: String,
        stdout: String,
        stderr: String,
    },
}

pub(crate) fn run_command<I, S>(command: &str, args: I) -> Result<(), RunCmdError>
where
    I: IntoIterator<Item = S> + Debug,
    S: AsRef<OsStr>,
{
    let cmd_str = format!("{command} {args:?}");
    info!("Running {}", cmd_str);

    let cmd = Command::new(command)
        .args(args)
        .output()
        .context(ExecSnafu { cmd: cmd_str.to_string() })?;

    if !cmd.status.success() {
        return Err(RunCmdError::RunFailed {
            cmd: cmd_str,
            stdout: String::from_utf8_lossy(&cmd.stdout).to_string(),
            stderr: String::from_utf8_lossy(&cmd.stderr).to_string(),
        });
    }

    info!("Run {} Successfully!", cmd_str);

    Ok(())
}

/// AOSC OS specific architecture mapping for ppc64
#[cfg(target_arch = "powerpc64")]
#[inline]
pub(crate) fn get_arch_name() -> Option<&'static str> {
    let mut endian: libc::c_int = -1;
    let result;
    unsafe {
        result = libc::prctl(libc::PR_GET_ENDIAN, &mut endian as *mut libc::c_int);
    }
    if result < 0 {
        return None;
    }
    match endian {
        libc::PR_ENDIAN_LITTLE | libc::PR_ENDIAN_PPC_LITTLE => Some("ppc64el"),
        libc::PR_ENDIAN_BIG => Some("ppc64"),
        _ => None,
    }
}

/// AOSC OS specific architecture mapping table
#[cfg(not(target_arch = "powerpc64"))]
#[inline]
pub(crate) fn get_arch_name() -> Option<&'static str> {
    use std::env::consts::ARCH;
    match ARCH {
        "x86_64" => Some("amd64"),
        "x86" => Some("i486"),
        "powerpc" => Some("powerpc"),
        "aarch64" => Some("arm64"),
        "mips64" => Some("loongson3"),
        "riscv64" => Some("riscv64"),
        "loongarch64" => Some("loongarch64"),
        _ => None,
    }
}

pub fn no_need_to_run_info(s: &str, str_is_retro: bool) {
    if str_is_retro {
        info!("Retro system no need to run {}", s);
    } else {
        info!("Non retro system no need to run {}", s);
    }
}
