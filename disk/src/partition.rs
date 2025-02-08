use std::{
    ffi::CStr,
    fs,
    io::{self, BufRead, BufReader, ErrorKind, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::Command,
};

use gptman::GPT;
use libparted::{Device, Disk, IsZero};
use mbrman::MBR;
use rand::Rng;
use serde::{Deserialize, Serialize};
use snafu::Snafu;
use tracing::{debug, info};
use uuid::{uuid, Uuid};

use crate::{devices::list_devices, is_efi_booted, PartitionError};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DkPartition {
    pub path: Option<PathBuf>,
    pub parent_path: Option<PathBuf>,
    pub fs_type: Option<String>,
    pub size: u64,
}

const SUPPORT_PARTITION_TYPE: &[&str] = &["primary", "logical"];
const EFI: Uuid = uuid!("C12A7328-F81F-11D2-BA4B-00A0C93EC93B");
const LINUX_FS: Uuid = uuid!("0FC63DAF-8483-4772-8E79-3D69D8477DE4");

#[derive(Debug, Snafu)]
pub enum PartitionErr {
    #[snafu(display("Failed to open device: {}", path.display()))]
    OpenDevice {
        source: std::io::Error,
        path: PathBuf,
    },
    #[snafu(display("Failed to probe disk: {}", path.display()))]
    ProbeDisk {
        source: std::io::Error,
        path: PathBuf,
    },
}

pub fn cvt<T: IsZero>(t: T) -> io::Result<T> {
    if t.is_zero() {
        Err(io::Error::last_os_error())
    } else {
        Ok(t)
    }
}

pub fn get_partition_table_type(device_path: &Path) -> Result<String, io::Error> {
    let device = Device::new(device_path)?;
    let partition_t = cvt(unsafe { libparted_sys::ped_disk_probe(device.ped_device()) })?;
    let partition_t_name = unsafe { cvt((*partition_t).name) }?;
    let partition_t = unsafe { CStr::from_ptr(partition_t_name) };
    let partition_t = partition_t.to_string_lossy().to_string();

    Ok(partition_t)
}

pub fn auto_create_partitions(
    dev_path: &Path,
) -> Result<(Option<DkPartition>, DkPartition), PartitionError> {
    // 处理 lvm 的情况
    if is_lvm_device(dev_path)? {
        remove_all_lvm_devive()?;
    }

    if is_efi_booted() {
        let (efi, system) = auto_create_partitions_gpt(dev_path)?;
        return Ok((Some(efi), system));
    }

    Ok((None, auto_create_partitions_mbr(dev_path)?))
}

fn remove_all_lvm_devive() -> Result<(), PartitionError> {
    let output = Command::new("dmsetup")
        .arg("ls")
        .output()
        .map_err(|e| PartitionError::DmSetup { source: e })?;

    let output = String::from_utf8_lossy(&output.stdout);
    let lines = output.lines();

    for line in lines {
        let mut line = line.split_whitespace();
        let lvm_name = line.next().ok_or_else(|| PartitionError::DmSetup {
            source: io::Error::new(ErrorKind::BrokenPipe, "Failed to read dmsetup stdout"),
        })?;

        if lvm_name != "live-base" && lvm_name != "live-rw" {
            info!("Running dmsetup remove {}", lvm_name);
            let remove = Command::new("dmsetup")
                .arg("remove")
                .arg(lvm_name)
                .output()
                .map_err(|e| PartitionError::DmSetup { source: e })?;

            debug!("Stdout: {}", String::from_utf8_lossy(&remove.stdout));
            debug!("Stderr: {}", String::from_utf8_lossy(&remove.stderr));

            if !remove.status.success() {
                return Err(PartitionError::DmSetup {
                    source: io::Error::new(
                        io::ErrorKind::Other,
                        format!("Failed to remove lvm device: {}", lvm_name),
                    ),
                });
            }
        }
    }

    Ok(())
}

pub fn is_lvm_device(p: &Path) -> Result<bool, PartitionError> {
    let cmd = Command::new("lvs")
        .arg("--segments")
        .arg("-o")
        .arg("+devices")
        .output()
        .map_err(PartitionError::OpenLvs)?;

    let output = String::from_utf8_lossy(&cmd.stdout);
    for i in output.lines().skip(1) {
        let mut split = i.split_whitespace();
        if split
            .next_back()
            .map(|x| x.trim().starts_with(&p.to_string_lossy().to_string()))
            .unwrap_or(false)
        {
            return Ok(true);
        }
    }

    Ok(false)
}

pub fn format_partition(partition: &DkPartition) -> Result<(), PartitionError> {
    let fs_type = partition.fs_type.as_ref().ok_or_else(|| {
        PartitionError::FormatPartition(io::Error::new(
            io::ErrorKind::InvalidInput,
            "fs_type is not set",
        ))
    })?;

    let mut command = Command::new(format!("mkfs.{fs_type}"));

    let cmd = match fs_type.as_str() {
        "ext4" => command.arg("-Fq"),
        "vfat" => command.arg("-F32"),
        _ => command.arg("-f"),
    };

    let cmd = cmd.arg(partition.path.as_ref().ok_or_else(|| {
        PartitionError::FormatPartition(io::Error::new(
            io::ErrorKind::NotFound,
            "partition.path is empty",
        ))
    })?);

    info!("{cmd:?}");

    let output = cmd.output().map_err(PartitionError::FormatPartition)?;

    if !output.status.success() {
        return Err(PartitionError::FormatPartition(io::Error::new(
            io::ErrorKind::Other,
            String::from_utf8_lossy(&output.stderr),
        )));
    }

    Ok(())
}

pub fn list_partitions(device_path: PathBuf) -> Vec<DkPartition> {
    let mut partitions = Vec::new();
    if let Ok(mut dev) = Device::new(&device_path) {
        let sector_size = dev.sector_size();
        if let Ok(disk) = libparted::Disk::new(&mut dev) {
            for mut part in disk.parts() {
                if part.num() < 0 {
                    continue;
                }

                let geom_length: i64 = part.geom_length();
                let part_length = if geom_length < 0 {
                    0
                } else {
                    geom_length as u64
                };

                let fs_type = if let Ok(type_) = part.get_geom().probe_fs() {
                    Some(type_.name().to_owned())
                } else {
                    None
                };

                if SUPPORT_PARTITION_TYPE.contains(&part.type_get_name()) {
                    partitions.push(DkPartition {
                        path: part.get_path().map(|path| path.to_owned()),
                        parent_path: Some(device_path.clone()),
                        size: sector_size * part_length,
                        fs_type,
                    });
                }
            }
        }
    }

    partitions
}

pub fn find_esp_partition(device_path: &Path) -> Result<DkPartition, PartitionError> {
    let mut device =
        Device::get(device_path).map_err(|e| PartitionError::open_device(device_path, e))?;
    if let Ok(disk) = libparted::Disk::new(&mut device) {
        for mut part in disk.parts() {
            if part.num() < 0 {
                continue;
            }
            if part.get_flag(libparted::PartitionFlag::PED_PARTITION_ESP) {
                let fs_type = if let Ok(type_) = part.get_geom().probe_fs() {
                    Some(type_.name().to_owned())
                } else {
                    None
                };
                let path = part
                    .get_path()
                    .ok_or_else(|| PartitionError::FindEspPartition {
                        path: device_path.display().to_string(),
                        err: io::Error::new(io::ErrorKind::Other, "Unexcept error"),
                    })?;
                return Ok(DkPartition {
                    path: Some(path.to_owned()),
                    parent_path: None,
                    size: 0,
                    fs_type,
                });
            }
        }
    }

    Err(PartitionError::FindEspPartition {
        path: device_path.display().to_string(),
        err: io::Error::new(io::ErrorKind::Other, "Unexcept error"),
    })
}

pub fn auto_create_partitions_gpt(
    device_path: &Path,
) -> Result<(DkPartition, DkPartition), PartitionError> {
    let mut f = fs::OpenOptions::new()
        .write(true)
        .open(device_path)
        .map_err(|e| PartitionError::OpenDevice {
            path: device_path.display().to_string(),
            err: e,
        })?;

    let sector_size: u64 = gptman::linux::get_sector_size(&mut f)
        .map_err(PartitionError::GetTable)?
        .try_into()
        .map_err(PartitionError::Convert)?;

    clear_start_sector(&mut f, sector_size)?;

    // 创建新的分区表
    let mut gpt = GPT::new_from(&mut f, sector_size, generate_gpt_random_uuid())?;

    // 写一个假的 MBR 保护分区头
    GPT::write_protective_mbr_into(&mut f, sector_size).map_err(PartitionError::GptMan)?;

    // 起始扇区为 1MiB 除以扇区大小
    let starting_lba = 1024 * 1024 / sector_size;

    // EFI 的大小
    let efi_size = 512 * 1024 * 1024;

    // 分区方案
    gpt_partition(&mut gpt, efi_size, sector_size, starting_lba);

    // 应用分区表的修改
    gpt.write_into(&mut f)?;
    f.sync_all().map_err(PartitionError::Flush)?;

    // 重新读取分区表以读取刚刚的修改
    gptman::linux::reread_partition_table(&mut f).map_err(PartitionError::GetTable)?;

    // 关闭文件，确保 libparted 能正确地读到分区
    drop(f);

    // 使用 libparted 便利分区表，找到分区路径并格式化
    // TODO: 自己实现设备路径寻找逻辑，彻底扔掉 libparted
    let mut device =
        libparted::Device::new(device_path).map_err(|e| PartitionError::OpenDevice {
            path: device_path.display().to_string(),
            err: e,
        })?;

    let disk = Disk::new(&mut device).map_err(|e| PartitionError::OpenDisk {
        path: device_path.display().to_string(),
        err: e,
    })?;

    let mut efi = None;
    let mut system = None;

    for i in disk.parts() {
        if i.num() < 0 {
            continue;
        }

        if i.get_flag(libparted::PartitionFlag::PED_PARTITION_ESP) {
            let e = DkPartition {
                path: i.get_path().map(|x| x.to_path_buf()),
                parent_path: Some(device_path.to_path_buf()),
                fs_type: Some("vfat".to_string()),
                size: match i.geom_length() {
                    ..=0 => 0,
                    x @ 1.. => x as u64 * sector_size,
                },
            };

            format_partition(&e)?;
            efi = Some(e);

            continue;
        }

        let s = DkPartition {
            path: i.get_path().map(|x| x.to_path_buf()),
            parent_path: Some(device_path.to_path_buf()),
            fs_type: Some("ext4".to_string()),
            size: match i.geom_length() {
                ..=0 => 0,
                x @ 1.. => x as u64 * sector_size,
            },
        };

        format_partition(&s)?;
        system = Some(s);
    }

    let efi = efi.ok_or_else(|| PartitionError::CreatePartition {
        path: device_path.display().to_string(),
        err: io::Error::new(
            io::ErrorKind::NotFound,
            "Failed to find created esp partition",
        ),
    })?;

    let system: DkPartition = system.ok_or_else(|| PartitionError::CreatePartition {
        path: device_path.display().to_string(),
        err: io::Error::new(
            io::ErrorKind::NotFound,
            "Failed to find created system partition",
        ),
    })?;

    Ok((efi, system))
}

fn clear_start_sector(f: &mut fs::File, sector_size: u64) -> Result<(), PartitionError> {
    f.seek(SeekFrom::Start(0))
        .map_err(PartitionError::SeekSector)?;
    let buf: Vec<u8> = vec![0; sector_size as usize];
    f.write_all(&buf).map_err(PartitionError::ClearSector)?;
    f.sync_all().map_err(PartitionError::Flush)?;

    Ok(())
}

pub fn auto_create_partitions_mbr(device_path: &Path) -> Result<DkPartition, PartitionError> {
    let mut f = fs::OpenOptions::new()
        .write(true)
        .open(device_path)
        .map_err(|e| PartitionError::OpenDevice {
            path: device_path.display().to_string(),
            err: e,
        })?;

    let sector_size: u64 = gptman::linux::get_sector_size(&mut f)
        .map_err(PartitionError::GetTable)?
        .try_into()
        .map_err(PartitionError::Convert)?;

    clear_start_sector(&mut f, sector_size)?;

    let mut mbr = MBR::new_from(&mut f, sector_size as u32, mbr_disk_signature())?;
    let sectors = mbr.get_maximum_partition_size()?;
    let starting_lba = mbr
        .find_optimal_place(sectors)
        .ok_or(PartitionError::GetOptimalPlace)?;

    mbr[1] = mbrman::MBRPartitionEntry {
        boot: mbrman::BOOT_INACTIVE,     // boot flag
        first_chs: mbrman::CHS::empty(), // first CHS address (only useful for old computers)
        sys: 0x83,                       // Linux filesystem
        last_chs: mbrman::CHS::empty(),  // last CHS address (only useful for old computers)
        starting_lba,                    // the sector where the partition starts
        sectors,                         // the number of sectors in that partition
    };

    mbr.write_into(&mut f)?;
    drop(f);

    // TODO: 自己实现设备路径寻找逻辑，彻底扔掉 libparted
    let mut device =
        libparted::Device::new(device_path).map_err(|e| PartitionError::OpenDevice {
            path: device_path.display().to_string(),
            err: e,
        })?;

    let disk = Disk::new(&mut device).map_err(|e| PartitionError::OpenDisk {
        path: device_path.display().to_string(),
        err: e,
    })?;

    let part =
        disk.parts()
            .find(|x| x.num() > 0)
            .ok_or_else(|| PartitionError::CreatePartition {
                path: device_path.display().to_string(),
                err: io::Error::new(
                    io::ErrorKind::NotFound,
                    "Failed to find created system partition",
                ),
            })?;

    let system = DkPartition {
        path: part.get_path().map(|x| x.to_path_buf()),
        parent_path: Some(device_path.to_path_buf()),
        fs_type: Some("ext4".to_string()),
        size: match part.geom_length() {
            ..=0 => 0,
            x @ 1.. => x as u64 * sector_size as u64,
        },
    };

    format_partition(&system)?;

    Ok(system)
}

fn generate_gpt_random_uuid() -> [u8; 16] {
    rand::thread_rng().gen()
}

fn mbr_disk_signature() -> [u8; 4] {
    rand::thread_rng().gen()
}

#[cfg(debug_assertions)]
fn gpt_partition(gpt: &mut GPT, efi_size: u64, sector_size: u64, starting_lba: u64) {
    // 系统分区
    // 所经历的扇区数为最后一个有用的扇区减去 efi 扇区
    let sector = gpt.header.last_usable_lba - efi_size / sector_size;

    // 需要取整以保证对齐，最终得到系统分区的末尾扇区
    let mmod = sector % (1024 * 1024 / sector_size);
    let system_ending_lba = sector - mmod + starting_lba - 1;

    gpt[1] = gptman::GPTPartitionEntry {
        partition_type_guid: LINUX_FS.to_bytes_le(),
        unique_partition_guid: generate_gpt_random_uuid(),
        starting_lba,
        ending_lba: system_ending_lba,
        attribute_bits: 0,
        partition_name: "".into(),
    };

    let efi_starting_lba = system_ending_lba + 1;

    let mmod = (gpt.header.last_usable_lba - efi_starting_lba) % (1024 * 1024 / sector_size);
    let ending_lba = gpt.header.last_usable_lba - mmod - 1;

    // EFI 分区
    gpt[2] = gptman::GPTPartitionEntry {
        partition_type_guid: EFI.to_bytes_le(),
        unique_partition_guid: generate_gpt_random_uuid(),
        starting_lba: efi_starting_lba,
        ending_lba,
        attribute_bits: 0,
        partition_name: "".into(),
    };
}

#[cfg(not(debug_assertions))]
fn gpt_partition(gpt: &mut GPT, efi_size: u64, sector_size: u64, starting_lba: u64) {
    let efi_ending_lba = efi_size / sector_size + starting_lba - 1;
    gpt[1] = gptman::GPTPartitionEntry {
        partition_type_guid: EFI.to_bytes_le(),
        unique_partition_guid: generate_gpt_random_uuid(),
        starting_lba,
        ending_lba: efi_ending_lba,
        attribute_bits: 0,
        partition_name: "".into(),
    };

    let system_starting_lba = efi_ending_lba + 1;

    let mmod = (gpt.header.last_usable_lba - system_starting_lba) % (1024 * 1024 / sector_size);
    let ending_lba = gpt.header.last_usable_lba - mmod - 1;

    gpt[2] = gptman::GPTPartitionEntry {
        partition_type_guid: LINUX_FS.to_bytes_le(),
        unique_partition_guid: generate_gpt_random_uuid(),
        starting_lba: system_starting_lba,
        ending_lba,
        attribute_bits: 0,
        partition_name: "".into(),
    };
}

pub fn all_esp_partitions() -> Result<Vec<DkPartition>, PartitionError> {
    let root = find_root_mount_point()?;
    let devices = list_devices();
    let mut dev_path_and_sector = vec![];

    for dev in devices {
        let path = dev.path();
        if let Some(gpt) = fs::File::open(path)
            .ok()
            .and_then(|mut x| GPT::find_from(&mut x).ok())
        {
            for (_, c) in gpt.iter() {
                if c.partition_type_guid == EFI.to_bytes_le() {
                    dev_path_and_sector.push((path.to_path_buf(), c.starting_lba));
                }
            }
        }
    }

    let mut res = vec![];

    'a: for (path, lba) in dev_path_and_sector {
        // 一些固件会把一整个块设备挂载到 livemnt
        if path.to_string_lossy() == root {
            continue 'a;
        }

        if let Ok(mut d) = Device::new(&path) {
            let sector_size = d.sector_size();
            if let Ok(disk) = Disk::new(&mut d) {
                // 不把 Livekit 的 EFI 分区加入到列表里
                for i in disk.parts() {
                    if i.get_path()
                        .map(|p| p.to_string_lossy() == root)
                        .unwrap_or(false)
                    {
                        continue 'a;
                    }
                }

                let part = disk.get_partition_by_sector(lba as i64);

                if let Some(mut part) = part {
                    res.push(DkPartition {
                        path: part.get_path().map(|x| x.to_path_buf()),
                        parent_path: Some(path),
                        fs_type: part
                            .get_geom()
                            .probe_fs()
                            .ok()
                            .map(|x| x.name().to_string()),
                        size: match part.geom_length() {
                            ..=0 => 0,
                            x @ 1.. => x as u64 * sector_size,
                        },
                    });
                }
            }
        }
    }

    Ok(res)
}

pub fn find_root_mount_point() -> Result<String, PartitionError> {
    let f = fs::File::open("/proc/mounts").map_err(PartitionError::ReadMounts)?;
    let lines = BufReader::new(f).lines();

    let mut match_livemnt = None;
    let mut match_rootfs = None;

    for i in lines.map_while(Result::ok) {
        if match_livemnt.is_some() && match_rootfs.is_some() {
            break;
        }

        let i = i.split_ascii_whitespace().collect::<Vec<_>>();
        if i[1] == "/" {
            // Livekit
            match_rootfs = Some(i[0].to_string());
        } else if i[1] == "/run/livekit/livemnt" {
            // Installer
            match_livemnt = Some(i[0].to_string());
        }
    }

    if let Some(match_livemnt) = match_livemnt {
        return Ok(match_livemnt);
    } else if let Some(match_rootfs) = match_rootfs {
        return Ok(match_rootfs);
    }

    Err(PartitionError::ReadMounts(io::Error::new(
        ErrorKind::InvalidInput,
        "Failed to read /proc/mounts",
    )))
}
