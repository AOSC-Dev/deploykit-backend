use crate::InstallError;

/// Runs dracut
/// Must be used in a chroot context
#[cfg(not(feature = "is_retro"))]
pub fn execute_dracut() -> Result<(), InstallError> {
    use crate::utils::run_command;

    let cmd = "/usr/bin/update-initramfs";
    run_command(cmd, &[] as &[&str])?;

    Ok(())
}

/// Runs dracut (dummy function for retro mode)
/// Must be used in a chroot context
#[cfg(feature = "is_retro")]
pub fn execute_dracut() -> Result<()> {
    no_need_to_run_info("dracut", true);

    Ok(())
}
