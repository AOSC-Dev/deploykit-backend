use std::{fs::File, io::Write};

use crate::InstallError;

/// Sets hostname in the guest environment
/// Must be used in a chroot context
pub fn set_hostname(name: &str) -> Result<(), InstallError> {
    let mut f = File::create("/etc/hostname").map_err(|e| InstallError::OperateFile {
        path: "/etc/hostname".to_string(),
        err: e,
    })?;

    f.write_all(name.as_bytes())
        .map_err(|e| InstallError::OperateFile {
            path: "/etc/hostname".to_string(),
            err: e,
        })?;

    Ok(())
}
