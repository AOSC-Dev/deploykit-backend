use crate::InstallError;

/// Runs ssh-keygen -A (dummy function for non-retro mode)
/// Must be used in a chroot context
#[cfg(not(feature = "is_retro"))]
pub fn gen_ssh_key() -> Result<(), InstallError> {
    use crate::utils::no_need_to_run_info;
    no_need_to_run_info("ssh-keygen", false);

    Ok(())
}

/// Runs ssh-keygen -A
/// Must be used in a chroot context
#[cfg(feature = "is_retro")]
pub fn gen_ssh_key() -> Result<(), InstallError> {
    run_command("ssh-keygen", &["-A"])?;

    Ok(())
}
