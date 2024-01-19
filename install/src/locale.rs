use std::{fs::File, io::Write};

use crate::InstallError;

/// Sets locale in the guest environment
/// Must be used in a chroot context
pub fn set_locale(locale: &str) -> Result<(), InstallError> {
    let mut f = File::create("/etc/locale.conf").map_err(|e| InstallError::OperateFile {
        path: "/etc/locale.conf".to_string(),
        err: e,
    })?;

    f.write_all(b"LANG=")
        .map_err(|e| InstallError::OperateFile {
            path: "/etc/locale.conf".to_string(),
            err: e,
        })?;

    f.write_all(format!("{locale}\n").as_bytes())
        .map_err(|e| InstallError::OperateFile {
            path: "/etc/locale.conf".to_string(),
            err: e,
        })?;

    Ok(())
}
