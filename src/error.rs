use std::fmt::Display;

use disk::CombineError;
use install::{
    chroot::ChrootError,
    download::DownloadError,
    genfstab::GenfstabError,
    grub::RunGrubError,
    locale::SetHwclockError,
    swap::SwapFileError,
    user::{AddUserError, SetFullNameError},
    utils::RunCmdError,
    zoneinfo::SetZoneinfoError,
    ConfigureSystemError, InstallErr, InstallSquashfsError, MountError, PostInstallationError,
    SetupGenfstabError, SetupPartitionError,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Serialize, Deserialize, Debug)]
pub struct DkError {
    pub message: String,
    pub t: String,
    pub data: Value,
}

impl Display for DkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl From<&CombineError> for DkError {
    fn from(value: &CombineError) -> Self {
        match value {
            CombineError::WrongCombine {
                table,
                bootmode,
                path,
            } => Self {
                message: value.to_string(),
                t: "WrongCombine".to_string(),
                data: {
                    json!({
                        "table": table.to_string(),
                        "bootmode": bootmode.to_string(),
                        "path": path.display().to_string()
                    })
                },
            },
            CombineError::PartitionType { source, path } => Self {
                message: value.to_string(),
                t: "PartitionType".to_string(),
                data: {
                    json!({
                        "message": source.to_string(),
                        "kind": source.kind().to_string(),
                        "path": path.display().to_string()
                    })
                },
            },
            CombineError::UnsupportedTable { t } => Self {
                message: value.to_string(),
                t: "UnsupportedTable".to_string(),
                data: {
                    json!({
                        "table": t.to_string()
                    })
                },
            },
        }
    }
}

#[cfg(not(target_arch = "powerpc64"))]
impl From<&RunGrubError> for DkError {
    fn from(value: &RunGrubError) -> Self {
        let RunGrubError::RunCommand { source } = value;
        DkError::from(source)
    }
}

#[cfg(target_arch = "powerpc64")]
impl From<&RunGrubError> for DkError {
    fn from(value: &RunGrubError) -> Self {
        match value {
            RunGrubError::OpenCpuInfo { source } => Self {
                message: value.to_string(),
                t: "OpenCpuInfo".to_string(),
                data: {
                    json!({
                        "message": source.to_string(),
                        "kind": source.kind().to_string(),
                    })
                },
            },
            RunGrubError::RunCommand { source } => DkError::from(source),
        }
    }
}

impl From<&InstallSquashfsError> for DkError {
    fn from(value: &InstallSquashfsError) -> Self {
        match value {
            InstallSquashfsError::Extract { source, from, to } => Self {
                message: value.to_string(),
                t: "ExtractSquashfs".to_string(),
                data: {
                    json!({
                        "stage": 3,
                        "message": source.to_string(),
                        "from": from.display().to_string(),
                        "to": to.display().to_string(),
                    })
                },
            },
            InstallSquashfsError::RemoveDownloadedFile { source } => Self {
                message: value.to_string(),
                t: "RemoveSquashfsFile".to_string(),
                data: {
                    json!({
                        "stage": 3,
                        "message": source.to_string(),
                    })
                },
            },
        }
    }
}

impl From<&InstallErr> for DkError {
    fn from(value: &InstallErr) -> Self {
        match value {
            InstallErr::ValueNotSet { v } => Self {
                message: value.to_string(),
                t: "ValueNotSet".to_string(),
                data: {
                    json!({
                        "stage": 0,
                        "value": v.to_string(),
                    })
                },
            },
            InstallErr::GetDirFd { source } => Self {
                message: value.to_string(),
                t: "GetDirFd".to_string(),
                data: {
                    json!({
                        "stage": 0,
                        "message": source.to_string(),
                        "kind": source.kind().to_string(),
                    })
                },
            },
            InstallErr::SetupPartition { source } => Self {
                message: value.to_string(),
                t: "SetupPartition".to_string(),
                data: {
                    json!({
                        "stage": 1,
                        "message": source.to_string(),
                        "data": DkError::from(source)
                    })
                },
            },
            InstallErr::DownloadSquashfs { source } => Self {
                message: value.to_string(),
                t: "DownloadSquashfs".to_string(),
                data: {
                    json!({
                        "stage": 2,
                        "message": source.to_string(),
                        "data": DkError::from(source)
                    })
                },
            },
            InstallErr::ExtractSquashfs { source } => Self {
                message: value.to_string(),
                t: "ExtractSquashfs".to_string(),
                data: json!({
                    "stage": 3,
                    "message": source.to_string(),
                    "data": DkError::from(source)
                }),
            },
            InstallErr::Genfstab { source } => Self {
                message: value.to_string(),
                t: "Genfstab".to_string(),
                data: {
                    json!({
                        "stage": 4,
                        "message": source.to_string(),
                        "data": DkError::from(source)
                    })
                },
            },
            InstallErr::Chroot { source } => Self {
                message: value.to_string(),
                t: "Chroot".to_string(),
                data: {
                    json!({
                        "stage": 5,
                        "message": source.to_string(),
                        "data": DkError::from(source)
                    })
                },
            },
            InstallErr::Dracut { source } => Self {
                message: value.to_string(),
                t: "Dracut".to_string(),
                data: {
                    json!({
                        "stage": 6,
                        "message": source.to_string(),
                        "data": DkError::from(source)
                    })
                },
            },
            InstallErr::Grub { source } => Self {
                message: value.to_string(),
                t: "Grub".to_string(),
                data: serde_json::to_value(DkError::from(source)).unwrap_or_else(|e| {
                    json!({
                        "message": format!("Failed to ser error message: {e}"),
                    })
                }),
            },
            InstallErr::GenerateSshKey { source } => Self {
                message: value.to_string(),
                t: "GenerateSshKey".to_string(),
                data: {
                    json!({
                        "stage": 8,
                        "message": source.to_string(),
                        "data": DkError::from(source)
                    })
                },
            },
            InstallErr::ConfigureSystem { source } => Self {
                message: value.to_string(),
                t: "ConfigureSystem".to_string(),
                data: {
                    json!({
                        "stage": 9,
                        "message": source.to_string(),
                        "data": DkError::from(source)
                    })
                },
            },
            InstallErr::EscapeChroot { source } => Self {
                message: value.to_string(),
                t: "EscapeChroot".to_string(),
                data: {
                    json!({
                        "stage": 10,
                        "message": source.to_string(),
                        "data": DkError::from(source)
                    })
                },
            },
            InstallErr::PostInstallation { source } => Self {
                message: value.to_string(),
                t: "PostInstallation".to_string(),
                data: {
                    json!({
                        "stage": 11,
                        "message": source.to_string(),
                        "data": DkError::from(source)
                    })
                },
            },
            InstallErr::CloneFd { source } => Self {
                message: value.to_string(),
                t: "CloneFd".to_string(),
                data: {
                    json!({
                        "stage": 0,
                        "message": source.to_string(),
                        "kind": source.kind().to_string(),
                    })
                },
            },
            InstallErr::CreateTempDir { source } => Self {
                message: value.to_string(),
                t: "CreateTempDir".to_string(),
                data: {
                    json!({
                        "stage": 0,
                        "message": source.to_string(),
                        "kind": source.kind().to_string(),
                    })
                },
            },
        }
    }
}

impl From<&PostInstallationError> for DkError {
    fn from(value: &PostInstallationError) -> Self {
        match value {
            PostInstallationError::Umount { source } => Self {
                message: value.to_string(),
                t: "Umount".to_string(),
                data: {
                    json!({
                        "message": source.to_string(),
                        "point": source.point,
                    })
                },
            },
        }
    }
}

impl From<&ConfigureSystemError> for DkError {
    fn from(value: &ConfigureSystemError) -> Self {
        match value {
            ConfigureSystemError::SwapToGenfstab { source } => Self {
                message: value.to_string(),
                t: "SwapToGenfstab".to_string(),
                data: {
                    json!({
                        "message": source.to_string(),
                        "data": DkError::from(source)
                    })
                },
            },
            ConfigureSystemError::SetZoneinfo { source, zone } => Self {
                message: value.to_string(),
                t: "SetZoneinfo".to_string(),
                data: {
                    json!({
                        "zone": zone.to_string(),
                        "message": source.to_string(),
                        "data": DkError::from(source)
                    })
                },
            },
            ConfigureSystemError::SetHwclock { source, is_rtc } => Self {
                message: value.to_string(),
                t: "SetHwclock".to_string(),
                data: {
                    json!({
                        "is_rtc": is_rtc,
                        "message": source.to_string(),
                        "data": DkError::from(source)
                    })
                },
            },
            ConfigureSystemError::SetHostname { source, hostname } => Self {
                message: value.to_string(),
                t: "SetHostname".to_string(),
                data: {
                    json!({
                        "hostname": hostname.to_string(),
                        "message": source.to_string(),
                        "kind": source.kind().to_string(),
                    })
                },
            },
            ConfigureSystemError::AddNewUser { source } => Self {
                message: value.to_string(),
                t: "AddNewUser".to_string(),
                data: {
                    json!({
                        "message": source.to_string(),
                        "data": DkError::from(source)
                    })
                },
            },
            ConfigureSystemError::SetFullName { source, fullname } => Self {
                message: value.to_string(),
                t: "SetFullName".to_string(),
                data: {
                    json!({
                        "fullname": fullname.to_string(),
                        "message": source.to_string(),
                        "data": DkError::from(source)
                    })
                },
            },
            ConfigureSystemError::SetLocale { source, locale } => Self {
                message: value.to_string(),
                t: "SetLocale".to_string(),
                data: {
                    json!({
                        "locale": locale.to_string(),
                        "message": source.to_string(),
                        "data": {
                            "message": source.to_string(),
                            "kind": source.kind().to_string(),
                        }
                    })
                },
            },
        }
    }
}

impl From<&SetFullNameError> for DkError {
    fn from(value: &SetFullNameError) -> Self {
        match value {
            SetFullNameError::OperatePasswdFile { source } => Self {
                message: value.to_string(),
                t: "OperatePasswdFile".to_string(),
                data: {
                    json!({
                        "message": source.to_string(),
                        "kind": source.kind().to_string(),
                    })
                },
            },
            SetFullNameError::Illegal { fullname } => Self {
                message: value.to_string(),
                t: "Illegal".to_string(),
                data: {
                    json!({
                        "fullname": fullname.to_string(),
                    })
                },
            },
            SetFullNameError::BrokenPassswd => Self {
                message: value.to_string(),
                t: "BrokenPassswd".to_string(),
                data: { json!({}) },
            },
            SetFullNameError::InvaildUsername { username } => Self {
                message: value.to_string(),
                t: "InvaildUsername".to_string(),
                data: {
                    json!({
                        "username": username.to_string(),
                    })
                },
            }
        }
    }
}

impl From<&AddUserError> for DkError {
    fn from(value: &AddUserError) -> Self {
        match value {
            AddUserError::RunCommand { source } => Self {
                message: value.to_string(),
                t: "RunCommand".to_string(),
                data: serde_json::to_value(DkError::from(source)).unwrap_or_else(|e| {
                    json!({
                        "message": format!("Failed to ser error message: {e}"),
                    })
                }),
            },
            AddUserError::ExecChpasswd { source } => Self {
                message: value.to_string(),
                t: "ExecChpasswd".to_string(),
                data: {
                    json!({
                        "message": source.to_string(),
                        "data": {
                            "message": source.to_string(),
                            "kind": source.kind().to_string(),
                        }
                    })
                },
            },
            AddUserError::ChpasswdStdin => Self {
                message: value.to_string(),
                t: "ChpasswdStdin".to_string(),
                data: { json!({}) },
            },
            AddUserError::WriteChpasswdStdin { source } => Self {
                message: value.to_string(),
                t: "WriteChpasswdStdin".to_string(),
                data: {
                    json!({
                        "message": source.to_string(),
                        "data": {
                            "message": source.to_string(),
                            "kind": source.kind().to_string(),
                        }
                    })
                },
            },
            AddUserError::FlushChpasswdStdin { source } => Self {
                message: value.to_string(),
                t: "FlushChpasswdStdin".to_string(),
                data: {
                    json!({
                        "message": source.to_string(),
                        "data": {
                            "message": source.to_string(),
                            "kind": source.kind().to_string(),
                        }
                    })
                },
            },
        }
    }
}

impl From<&SetHwclockError> for DkError {
    fn from(value: &SetHwclockError) -> Self {
        match value {
            SetHwclockError::OperateAdjtimeFile { source } => Self {
                message: value.to_string(),
                t: "OperateAdjtimeFile".to_string(),
                data: {
                    json!({
                        "message": source.to_string(),
                        "data": {
                            "message": source.to_string(),
                            "kind": source.kind().to_string(),
                        }
                    })
                },
            },
            SetHwclockError::RunCommand { source } => Self {
                message: value.to_string(),
                t: "RunCommand".to_string(),
                data: serde_json::to_value(DkError::from(source)).unwrap_or_else(|e| {
                    json!({
                        "message": format!("Failed to ser error message: {e}"),
                    })
                }),
            },
        }
    }
}

impl From<&SetZoneinfoError> for DkError {
    fn from(value: &SetZoneinfoError) -> Self {
        match value {
            SetZoneinfoError::RemoveLocaltimeFile { source } => Self {
                message: value.to_string(),
                t: "RemoveLocaltimeFile".to_string(),
                data: {
                    json!({
                        "message": source.to_string(),
                        "data": {
                            "message": source.to_string(),
                            "kind": source.kind().to_string(),
                        }
                    })
                },
            },
            SetZoneinfoError::Symlink { path, source } => Self {
                message: value.to_string(),
                t: "Symlink".to_string(),
                data: {
                    json!({
                        "path": path.display().to_string(),
                        "message": source.to_string(),
                        "data": {
                            "message": source.to_string(),
                            "kind": source.kind().to_string(),
                        }
                    })
                },
            },
        }
    }
}

impl From<&ChrootError> for DkError {
    fn from(value: &ChrootError) -> Self {
        match value {
            ChrootError::Chdir { source } => Self {
                message: value.to_string(),
                t: "Chdir".to_string(),
                data: {
                    json!({
                        "message": source.to_string(),
                        "kind": source.kind().to_string(),
                    })
                },
            },
            ChrootError::Chroot { source, quit } => Self {
                message: value.to_string(),
                t: "Chroot".to_string(),
                data: {
                    json!({
                        "message": source.to_string(),
                        "kind": source.kind().to_string(),
                        "quit": quit
                    })
                },
            },
            ChrootError::SetCurrentDir { source } => Self {
                message: value.to_string(),
                t: "SetCurrentDir".to_string(),
                data: {
                    json!({
                        "message": source.to_string(),
                        "kind": source.kind().to_string(),
                    })
                },
            },
            ChrootError::SetupInnerMounts { source } => Self {
                message: value.to_string(),
                t: "SetupInnerMounts".to_string(),
                data: {
                    json!({
                        "message": source.to_string(),
                        "point": source.point,
                        "umount": source.umount,
                    })
                },
            },
        }
    }
}

impl From<&SetupGenfstabError> for DkError {
    fn from(value: &SetupGenfstabError) -> Self {
        match value {
            SetupGenfstabError::Genfstab { source } => Self {
                message: value.to_string(),
                t: "Genfstab".to_string(),
                data: {
                    json!({
                        "message": source.to_string(),
                        "data": DkError::from(source)
                    })
                },
            },
            SetupGenfstabError::ValueNotSetGenfstab { t } => Self {
                message: value.to_string(),
                t: "ValueNotSet".to_string(),
                data: {
                    json!({
                        "value": t.to_string(),
                    })
                },
            },
        }
    }
}

impl From<&GenfstabError> for DkError {
    fn from(value: &GenfstabError) -> Self {
        match value {
            GenfstabError::UnsupportedFileSystem { fs_type } => Self {
                message: value.to_string(),
                t: "UnsupportedFileSystem".to_string(),
                data: {
                    json!({
                        "fs_type": fs_type.to_string()
                    })
                },
            },
            GenfstabError::UUID { path } => Self {
                message: value.to_string(),
                t: "UUID".to_string(),
                data: {
                    json!({
                        "path": path.display().to_string()
                    })
                },
            },
            GenfstabError::OperateFstabFile { source } => Self {
                message: value.to_string(),
                t: "OperateFstabFile".to_string(),
                data: {
                    json!({
                        "message": source.to_string(),
                        "kind": source.kind().to_string(),
                    })
                },
            },
        }
    }
}

impl From<&DownloadError> for DkError {
    fn from(value: &DownloadError) -> Self {
        match value {
            DownloadError::DownloadPathIsNotSet => Self {
                message: value.to_string(),
                t: "DownloadPathIsNotSet".to_string(),
                data: json!({}),
            },
            DownloadError::LocalFileNotFound { path } => Self {
                message: value.to_string(),
                t: "LocalFileNotFound".to_string(),
                data: {
                    json!({
                        "path": path.display().to_string()
                    })
                },
            },
            DownloadError::BuildDownloadClient { source } => Self {
                message: value.to_string(),
                t: "BuildDownloadClient".to_string(),
                data: {
                    json!({
                        "message": source.to_string(),
                    })
                },
            },
            DownloadError::SendRequest { source } => Self {
                message: value.to_string(),
                t: "SendRequest".to_string(),
                data: {
                    json!({
                        "message": source.to_string(),
                    })
                },
            },
            DownloadError::CreateFile { source, path } => Self {
                message: value.to_string(),
                t: "CreateFile".to_string(),
                data: {
                    json!({
                        "message": source.to_string(),
                        "path": path.display().to_string()
                    })
                },
            },
            DownloadError::DownloadFile { source, path } => Self {
                message: value.to_string(),
                t: "DownloadFile".to_string(),
                data: {
                    json!({
                        "message": source.to_string(),
                        "path": path.display().to_string()
                    })
                },
            },
            DownloadError::WriteFile { source, path } => Self {
                message: value.to_string(),
                t: "WriteFile".to_string(),
                data: {
                    json!({
                        "message": source.to_string(),
                        "path": path.display().to_string()
                    })
                },
            },
            DownloadError::ChecksumMismatch => Self {
                message: value.to_string(),
                t: "ChecksumMismatch".to_string(),
                data: json!({}),
            },
            DownloadError::ShutdownFile { source, path } => Self {
                message: value.to_string(),
                t: "ShutdownFile".to_string(),
                data: {
                    json!({
                        "message": source.to_string(),
                        "path": path.display().to_string()
                    })
                },
            },
        }
    }
}

impl From<&SetupPartitionError> for DkError {
    fn from(value: &SetupPartitionError) -> Self {
        match value {
            SetupPartitionError::Format { .. } => Self {
                message: value.to_string(),
                t: "Format".to_string(),
                // TODO
                data: json!({}),
            },
            SetupPartitionError::Mount { source } => Self {
                message: value.to_string(),
                t: "Mount".to_string(),
                data: serde_json::to_value(DkError::from(source)).unwrap_or_else(|e| {
                    json!({
                        "message": format!("Failed to ser error message: {e}"),
                    })
                }),
            },
            SetupPartitionError::SwapFile { source } => Self {
                message: value.to_string(),
                t: "SwapFile".to_string(),
                data: serde_json::to_value(DkError::from(source)).unwrap_or_else(|e| {
                    json!({
                        "message": format!("Failed to ser error message: {e}"),
                    })
                }),
            },
        }
    }
}

impl From<&MountError> for DkError {
    fn from(value: &MountError) -> Self {
        match value {
            MountError::CreateDir { source, path } => Self {
                message: value.to_string(),
                t: "CreateDir".to_string(),
                data: {
                    json!({
                        "message": source.to_string(),
                        "kind": source.kind().to_string(),
                        "path": path.display().to_string()
                    })
                },
            },
            MountError::MountRoot { source, path } => Self {
                message: value.to_string(),
                t: "MountRoot".to_string(),
                data: {
                    json!({
                        "message": source.to_string(),
                        "kind": source.kind().to_string(),
                        "path": path.display().to_string()
                    })
                },
            },
            MountError::ValueNotSetMount { t } => Self {
                message: value.to_string(),
                t: "ValueNotSet".to_string(),
                data: {
                    json!({
                        "value": t.to_string(),
                    })
                },
            },
        }
    }
}

impl From<&SwapFileError> for DkError {
    fn from(value: &SwapFileError) -> Self {
        match value {
            SwapFileError::CreateFile { path, source } => Self {
                message: value.to_string(),
                t: "CreateFile".to_string(),
                data: {
                    json!({
                        "path": path.display().to_string(),
                        "message": source.to_string(),
                        "kind": source.kind().to_string(),
                    })
                },
            },
            SwapFileError::Fallocate { path, source } => Self {
                message: value.to_string(),
                t: "Fallocate".to_string(),
                data: {
                    json!({
                        "path": path.display().to_string(),
                        "message": source.to_string(),
                        "kind": source.kind().to_string(),
                    })
                },
            },
            SwapFileError::FlushSwapFile { path, source } => Self {
                message: value.to_string(),
                t: "FlushSwapFile".to_string(),
                data: {
                    json!({
                        "path": path.display().to_string(),
                        "message": source.to_string(),
                        "kind": source.kind().to_string(),
                    })
                },
            },
            SwapFileError::SetPermission { path, source } => Self {
                message: value.to_string(),
                t: "SetPermission".to_string(),
                data: {
                    json!({
                        "path": path.display().to_string(),
                        "message": source.to_string(),
                        "kind": source.kind().to_string(),
                    })
                },
            },
            SwapFileError::Mkswap { path, source } => Self {
                message: value.to_string(),
                t: "Mkswap".to_string(),
                data: {
                    json!({
                        "path": path.display().to_string(),
                        "message": source.to_string(),
                        "data": DkError::from(source)
                    })
                },
            },
        }
    }
}

impl From<&RunCmdError> for DkError {
    fn from(value: &RunCmdError) -> Self {
        match value {
            RunCmdError::Exec { cmd, source } => Self {
                message: value.to_string(),
                t: "Exec".to_string(),
                data: {
                    json!({
                        "cmd": cmd.to_string(),
                        "message": source.to_string(),
                        "kind": source.kind().to_string(),
                    })
                },
            },
            RunCmdError::RunFailed {
                cmd,
                stdout,
                stderr,
            } => Self {
                message: value.to_string(),
                t: "RunFailed".to_string(),
                data: {
                    json!({
                        "cmd": cmd.to_string(),
                        "stdout": stdout.to_string(),
                        "stderr": stderr.to_string(),
                    })
                },
            },
        }
    }
}
