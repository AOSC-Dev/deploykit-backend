use std::{
    fs::File,
    io::{self, Write},
};

/// Sets hostname in the guest environment
/// Must be used in a chroot context
pub fn set_hostname(name: &str) -> Result<(), io::Error> {
    let mut f = File::create("/etc/hostname")?;
    f.write_all(name.as_bytes())?;

    Ok(())
}
