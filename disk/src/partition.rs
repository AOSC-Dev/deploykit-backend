use std::{
    fs,
    io::{self, ErrorKind, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::Command,
    str::FromStr,
};

use dbus_udisks2::{Disks, UDisks2};
use gptman::GPT;
use mbrman::MBR;
use rand::Rng;
use serde::{Deserialize, Serialize};
use tracing::info;
use uuid::{uuid, Uuid};

use crate::{is_efi_booted, PartitionError};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DkPartition {
    pub path: Option<PathBuf>,
    pub parent_path: Option<PathBuf>,
    pub fs_type: Option<String>,
    pub size: u64,
}

const EFI: Uuid = uuid!("C12A7328-F81F-11D2-BA4B-00A0C93EC93B");
const LINUX_FS: Uuid = uuid!("0FC63DAF-8483-4772-8E79-3D69D8477DE4");

pub fn get_partition_table_type_udisk2(device_path: &Path) -> Result<String, PartitionError> {
    let udisks2 = UDisks2::new().unwrap();
    let disk = Disks::new(&udisks2);
    for d in disk.devices {
        if d.parent.device == device_path {
            if let Some(t) = d.parent.table {
                return Ok(t.type_);
            }
        }
    }

    Err(PartitionError::GetPartitionType {
        path: device_path.display().to_string(),
        err: io::Error::new(
            ErrorKind::Other,
            format!(
                "Failed to get partition table type: {}",
                device_path.display()
            ),
        ),
    })
}

pub fn auto_create_partitions(
    dev_path: &Path,
) -> Result<(Option<DkPartition>, DkPartition), PartitionError> {
    if is_efi_booted() {
        let (efi, system) = auto_create_partitions_gpt(dev_path)?;
        return Ok((Some(efi), system));
    }

    Ok((None, auto_create_partitions_mbr(dev_path)?))
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

    let udisks2 = UDisks2::new().unwrap();
    let disks = Disks::new(&udisks2);

    for d in disks.devices {
        for part in d.partitions {
            if let Some(p) = part.partition {
                // 若不是 container，则一定是主分区或逻辑分区
                if !p.is_container {
                    partitions.push(DkPartition {
                        path: Some(part.device),
                        parent_path: Some(device_path.clone()),
                        size: part.size,
                        fs_type: part.id_type,
                    });
                }
            }
        }
    }

    partitions
}

pub fn find_esp_partition(device_path: &Path) -> Result<DkPartition, PartitionError> {
    let udisks2 = UDisks2::new().unwrap();
    let disk = Disks::new(&udisks2);

    for d in disk.devices {
        if d.parent.device == device_path {
            for part in d.partitions {
                if let Some(p) = part.partition {
                    if p.type_ == EFI.to_string() {
                        return Ok(DkPartition {
                            path: Some(part.device),
                            parent_path: Some(d.parent.device),
                            fs_type: part.id_type,
                            size: part.size,
                        });
                    }
                }
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

    let sector_size = gptman::linux::get_sector_size(&mut f).map_err(PartitionError::GetTable)?;

    // 创建新的分区表
    let mut gpt = GPT::new_from(&mut f, sector_size, generate_gpt_random_uuid())?;

    clear_start_sector(&mut f, sector_size)?;

    // 写一个假的 MBR 保护分区头
    GPT::write_protective_mbr_into(&mut f, sector_size).unwrap();

    // 起始扇区为 1MiB 除以扇区大小
    let starting_lba = 1024 * 1024 / sector_size;

    // EFI 的大小
    let efi_size = 512 * 1024 * 1024;

    // 分区方案
    gpt_partition(&mut gpt, efi_size, sector_size, starting_lba);

    // 应用分区表的修改
    gpt.write_into(&mut f)?;
    f.flush().map_err(PartitionError::Flush)?;

    // 重新读取分区表以读取刚刚的修改
    gptman::linux::reread_partition_table(&mut f).map_err(PartitionError::GetTable)?;

    // 关闭文件，确保 libparted 能正确地读到分区
    drop(f);

    // 遍历分区表，找到分区路径并格式化
    let udisks2 = UDisks2::new().unwrap();
    let disk = Disks::new(&udisks2);

    let mut efi = None;
    let mut system = None;

    for d in disk.devices {
        if d.parent.device == device_path {
            for part in d.partitions {
                if let Some(p) = part.partition {
                    if p.type_ == EFI.to_string() {
                        let e = DkPartition {
                            path: Some(part.device),
                            parent_path: Some(device_path.to_path_buf()),
                            fs_type: Some("vfat".to_string()),
                            size: part.size,
                        };

                        format_partition(&e)?;
                        efi = Some(e);

                        continue;
                    }

                    let s = DkPartition {
                        path: Some(part.device),
                        parent_path: Some(device_path.to_path_buf()),
                        fs_type: Some("ext4".to_string()),
                        size: part.size,
                    };

                    format_partition(&s)?;
                    system = Some(s);
                }
            }
        }
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

    let sector_size =
        gptman::linux::get_sector_size(&mut f).map_err(PartitionError::GetTable)? as u32;

    let mut mbr = MBR::new_from(&mut f, sector_size, mbr_disk_signature())?;

    clear_start_sector(&mut f, sector_size as u64)?;

    let sectors = mbr.get_maximum_partition_size()?;
    let starting_lba = mbr
        .find_optimal_place(sectors)
        .ok_or_else(|| PartitionError::GetOptimalPlace)?;

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

    let udisks2 = UDisks2::new().unwrap();
    let disk = Disks::new(&udisks2);

    for d in disk.devices {
        if d.parent.device == device_path {
            for part in d.partitions {
                if let Some(p) = part.partition {
                    if !p.is_container {
                        let system = DkPartition {
                            path: Some(part.device),
                            parent_path: Some(device_path.to_path_buf()),
                            fs_type: Some("ext4".to_string()),
                            size: part.size,
                        };

                        format_partition(&system)?;

                        return Ok(system);
                    }
                }
            }
        }
    }

    Err(PartitionError::NewDisk {
        path: device_path.display().to_string(),
        err: io::Error::new(ErrorKind::Other, "Failed to create new partition"),
    })
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

pub fn all_esp_partitions() -> Vec<DkPartition> {
    let mut res = vec![];

    let udisks2 = UDisks2::new().unwrap();
    let disks = Disks::new(&udisks2);

    for device in disks.devices {
        for parts in device.partitions {
            if let Some(p) = parts.partition {
                if p.type_ == EFI.to_string() {
                    res.push(DkPartition {
                        path: Some(parts.device),
                        parent_path: Some(device.parent.device.clone()),
                        fs_type: parts.id_type,
                        size: p.size,
                    });
                }
            }
        }
    }

    res
}

pub fn get_partition_uuid(partition_path: &Path) -> Option<Uuid> {
    let udisks2 = UDisks2::new().unwrap();
    let block = udisks2.get_block(&partition_path.display().to_string())?;

    block.id_uuid.and_then(|x| Uuid::from_str(&x).ok())
}
