use std::fmt::Debug;
use std::io;
use std::{ffi::OsStr, process::Command};

use tracing::info;

use crate::InstallError;

pub(crate) fn run_command<I, S>(command: &str, args: I) -> Result<(), InstallError>
where
    I: IntoIterator<Item = S> + Debug,
    S: AsRef<OsStr>,
{
    let cmd_str = format!("{command} {args:?}");
    info!("Running {}", cmd_str);

    let cmd = Command::new(command)
        .args(args)
        .output()
        .map_err(|e| InstallError::RunCommand {
            command: cmd_str.clone(),
            err: e,
        })?;

    if !cmd.status.success() {
        return Err(InstallError::RunCommand {
            command: cmd_str,
            err: io::Error::new(io::ErrorKind::Other, String::from_utf8_lossy(&cmd.stderr)),
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
