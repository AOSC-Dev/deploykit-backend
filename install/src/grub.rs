use snafu::Snafu;
use tracing::info;

use crate::utils::RunCmdError;
use crate::utils::{get_arch_name, run_command};
use std::path::Path;

#[cfg(not(target_arch = "powerpc64"))]
#[derive(Debug, Snafu)]
pub enum RunGrubError {
    #[snafu(transparent)]
    RunCommand { source: RunCmdError },
}

#[cfg(target_arch = "powerpc64")]
#[derive(Debug, Snafu)]
pub enum RunGrubError {
    #[snafu(transparent)]
    RunCommand { source: RunCmdError },
    #[snafu(display("Failed to open /proc/cpuinfo"))]
    OpenCpuInfo { source: std::io::Error },
}

/// Runs grub-install and grub-mkconfig
/// Must be used in a chroot context
#[cfg(not(target_arch = "powerpc64"))]
pub(crate) fn execute_grub_install(mbr_dev: Option<&Path>, lang: &str) -> Result<(), RunCmdError> {
    use tracing::warn;

    let mut grub_install_args = vec![];

    if let Some(mbr_dev) = mbr_dev {
        grub_install_args.push("--target=i386-pc".to_string());
        let path = mbr_dev.display().to_string();
        grub_install_args.push(path);
    } else {
        let (target, is_efi) = match get_arch_name() {
            Some("amd64") => (&[][..], true),
            Some("arm64") => (&["--force-extra-removable"][..], true),
            Some("riscv64") => (&["--force-extra-removable"][..], true),
            Some("loongarch64") => (&["--force-extra-removable"][..], true),
            Some("loongson3") => (&["--force-extra-removable"][..], true),
            Some(arch) => {
                info!("This architecture {arch} does not support grub");
                return Ok(());
            }
            None => {
                warn!("Install GRUB: What is this architecture???");
                return Ok(());
            }
        };
        grub_install_args.push("--bootloader-id=AOSC OS".to_string());
        grub_install_args.extend(target.iter().map(|x| x.to_string()));
        if is_efi {
            grub_install_args.push("--efi-directory=/efi".to_string());
        }
    };

    run_command(
        "grub-install",
        grub_install_args,
        vec![("LANG", lang.to_string())],
    )?;
    run_command(
        "grub-mkconfig",
        ["-o", "/boot/grub/grub.cfg"],
        vec![("LANG", lang.to_string())],
    )?;

    Ok(())
}

#[cfg(target_arch = "powerpc64")]
pub(crate) fn execute_grub_install(
    _mbr_dev: Option<&Path>,
    lang: &str,
) -> Result<(), RunGrubError> {
    use snafu::ResultExt;
    use std::io::BufRead;
    use std::io::BufReader;

    let target = get_arch_name();
    let cpuinfo = std::fs::File::open("/proc/cpuinfo").context(OpenCpuInfoSnafu)?;
    let r = BufReader::new(cpuinfo);

    let find = r.lines().flatten().find(|x| x.starts_with("firmware"));

    let needs_install = if let Some(find) = find {
        let s = find.split(':').nth(1).map(|x| x.trim());

        s != Some("OPAL")
    } else {
        true
    };

    let install_args = match target {
        Some("ppc64el") | Some("ppc64") | Some("powerpc") => "--target=powerpc-ieee1275",
        _ => {
            info!("This architecture does not support grub");
            return Ok(());
        }
    };

    if needs_install {
        run_command(
            "grub-install",
            &[install_args],
            vec![("LANG", lang.to_string())],
        )?;
    }

    run_command(
        "grub-mkconfig",
        ["-o", "/boot/grub/grub.cfg"],
        vec![("LANG", lang.to_string())],
    )?;

    Ok(())
}
