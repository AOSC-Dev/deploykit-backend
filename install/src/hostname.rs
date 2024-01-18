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

pub fn is_valid_hostname(hostname: &str) -> bool {
    if hostname.is_empty() || hostname.starts_with('-') {
        return false;
    }
    for c in hostname.as_bytes() {
        if c.is_ascii_alphanumeric() || *c == b'-' {
            continue;
        } else {
            return false;
        }
    }

    true
}

#[test]
fn test_hostname_validation() {
    assert!(is_valid_hostname("foo"));
    assert!(is_valid_hostname("foo-2e10"));
    assert!(is_valid_hostname("jeffbai-device"));
    assert!(!is_valid_hostname("invalid_host"));
    assert!(!is_valid_hostname("-invalid"));
    assert!(!is_valid_hostname("+invalid"));
    assert!(is_valid_hostname("JellyDimension"));
    assert!(!is_valid_hostname("Jelly_Dimension"));
}
