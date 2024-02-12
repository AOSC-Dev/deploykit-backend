use std::{
    ffi::CStr,
    fs,
    io::{self, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::Command,
};

use bincode::serialize_into;
use disk_types::{BlockDeviceExt, FileSystem, PartitionExt, PartitionType};
use gptman::GPT;
use libparted::{Device, Disk, DiskType, FileSystemType, Geometry, IsZero, Partition};
use libparted_sys::{PedPartitionFlag, PedPartitionType};
use mbrman::MBR;
use rand::Rng;
use serde::{Deserialize, Serialize};
use tracing::{error, info};
use uuid::{uuid, Uuid};

use crate::{is_efi_booted, PartitionError};

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

pub fn create_parition_table(dev: &Path) -> Result<(), PartitionError> {
    let mut device = Device::new(dev).map_err(|e| PartitionError::OpenDevice {
        path: dev.display().to_string(),
        err: e,
    })?;

    let part_table = if is_efi_booted() { "gpt" } else { "msdos" };

    info!(
        "Creating new {} partition table on {}",
        part_table,
        dev.display()
    );

    let mut disk =
        Disk::new_fresh(&mut device, DiskType::get(part_table).unwrap()).map_err(|e| {
            PartitionError::NewPartitionTable {
                path: dev.display().to_string(),
                err: e,
            }
        })?;

    info!("Commit changes on {}", dev.display());
    disk.commit().map_err(|e| PartitionError::CommitChanges {
        path: dev.display().to_string(),
        err: e,
    })?;

    Ok(())
}

/// Defines a new partition to be created on the file system.
#[derive(Debug, Clone, PartialEq)]
pub struct PartitionCreate {
    /// The location of the disk in the system.
    pub path: PathBuf,
    /// The start sector that the partition will have.
    pub start_sector: u64,
    /// The end sector that the partition will have.
    pub end_sector: u64,
    /// Whether the filesystem should be formatted.
    pub format: bool,
    /// The format that the file system should be formatted to.
    pub file_system: Option<FileSystem>,
    /// Whether the partition should be primary or logical.
    pub kind: PartitionType,
    /// Flags which should be set on the partition.
    pub flags: Vec<PedPartitionFlag>,
    /// Defines the label to apply
    pub label: Option<String>,
}

impl BlockDeviceExt for PartitionCreate {
    fn get_device_path(&self) -> &Path {
        &self.path
    }

    fn get_mount_point(&self) -> Option<&Path> {
        None
    }
}

impl PartitionExt for PartitionCreate {
    fn get_file_system(&self) -> Option<FileSystem> {
        self.file_system
    }

    fn get_sector_end(&self) -> u64 {
        self.end_sector
    }

    fn get_sector_start(&self) -> u64 {
        self.start_sector
    }

    fn get_partition_flags(&self) -> &[PedPartitionFlag] {
        &self.flags
    }

    fn get_partition_label(&self) -> Option<&str> {
        self.label.as_deref()
    }

    fn get_partition_type(&self) -> PartitionType {
        self.kind
    }
}

/// Creates a new partition on the device using the info in the `partition` parameter.
/// The partition table should reflect the changes before this function exits.
pub fn create_partition<P>(device: &mut Device, partition: &P) -> io::Result<()>
where
    P: PartitionExt,
{
    // Create a new geometry from the start sector and length of the new partition.
    let length = partition.get_sector_end() - partition.get_sector_start();
    let geometry = Geometry::new(device, partition.get_sector_start() as i64, length as i64)
        .map_err(|why| io::Error::new(why.kind(), format!("failed to create geometry: {}", why)))?;

    // Convert our internal partition type enum into libparted's variant.
    let part_type = match partition.get_partition_type() {
        PartitionType::Primary => PedPartitionType::PED_PARTITION_NORMAL,
        PartitionType::Logical => PedPartitionType::PED_PARTITION_LOGICAL,
        PartitionType::Extended => PedPartitionType::PED_PARTITION_EXTENDED,
    };

    // Open the disk, create the new partition, and add it to the disk.
    let (start, end) = (geometry.start(), geometry.start() + geometry.length());

    info!(
        "creating new partition with {} sectors: {} - {}",
        length, start, end
    );

    let fs_type = partition
        .get_file_system()
        .and_then(|fs| FileSystemType::get(fs.into()));

    {
        let mut disk = Disk::new(device)?;
        let mut part =
            Partition::new(&disk, part_type, fs_type.as_ref(), start, end).map_err(|why| {
                io::Error::new(
                    why.kind(),
                    format!(
                        "failed to create new partition: {}: {}",
                        partition.get_device_path().display(),
                        why
                    ),
                )
            })?;

        for &flag in partition.get_partition_flags() {
            if part.is_flag_available(flag) && part.set_flag(flag, true).is_err() {
                error!("unable to set {:?}", flag);
            }
        }

        if let Some(label) = partition.get_partition_label() {
            if part.set_name(label).is_err() {
                error!("unable to set partition name: {}", label);
            }
        }

        // Add the partition, and commit the changes to the disk.
        let constraint = geometry.exact().expect("exact constraint not found");
        disk.add_partition(&mut part, &constraint).map_err(|why| {
            io::Error::new(
                why.kind(),
                format!(
                    "failed to create new partition: {}: {}",
                    partition.get_device_path().display(),
                    why
                ),
            )
        })?;

        // Attempt to write the new partition to the disk.
        info!(
            "committing new partition ({}:{}) on {}",
            start,
            end,
            partition.get_device_path().display()
        );

        disk.commit()?;
    }

    device.sync()?;

    Ok(())
}

pub fn cvt<T: IsZero>(t: T) -> io::Result<T> {
    if t.is_zero() {
        Err(io::Error::last_os_error())
    } else {
        Ok(t)
    }
}

pub fn get_partition_table_type(device_path: &Path) -> Result<String, PartitionError> {
    let device = Device::new(device_path).map_err(|e| PartitionError::OpenDevice {
        path: device_path.display().to_string(),
        err: e,
    })?;

    let partition_t =
        cvt(unsafe { libparted_sys::ped_disk_probe(device.ped_device()) }).map_err(|e| {
            PartitionError::GetPartitionType {
                path: device_path.display().to_string(),
                err: e,
            }
        })?;

    let partition_t_name =
        unsafe { cvt((*partition_t).name) }.map_err(|e| PartitionError::GetPartitionType {
            path: device_path.display().to_string(),
            err: e,
        })?;

    let partition_t = unsafe { CStr::from_ptr(partition_t_name) };
    let partition_t = partition_t.to_str()?.to_string();

    Ok(partition_t)
}

#[cfg(debug_assertions)]
pub fn auto_create_partitions(
    dev: &Path,
) -> Result<(Option<DkPartition>, DkPartition), PartitionError> {
    let mut device = Device::new(dev).map_err(|e| PartitionError::OpenDevice {
        path: dev.display().to_string(),
        err: e,
    })?;

    let device = &mut device as *mut Device;
    let device = unsafe { &mut (*device) };
    let efi_size = 512 * 1024 * 1024;
    let is_efi = is_efi_booted();

    let length = device.length();
    let sector_size = device.sector_size();
    let size = length * sector_size;

    if get_partition_table_type(dev)
        .map(|x| x == "msdos")
        .unwrap_or(false)
        && size > 512 * (2_u64.pow(31) - 1)
    {
        return Err(PartitionError::MBRMaxSizeLimit(dev.display().to_string()));
    }

    let disk = libparted::Disk::new(&mut *device).map_err(|e| PartitionError::OpenDisk {
        path: dev.display().to_string(),
        err: e,
    })?;

    let mut nums = vec![];

    for i in disk.parts() {
        let num = i.num();
        if num > 0 {
            nums.push(num as u32);
        }
    }

    let mut device = Device::new(dev).map_err(|e| PartitionError::OpenDevice {
        path: dev.display().to_string(),
        err: e,
    })?;

    let device = &mut device as *mut Device;
    let device = unsafe { &mut (*device) };
    let mut disk = libparted::Disk::new(&mut *device).map_err(|e| PartitionError::OpenDisk {
        path: dev.display().to_string(),
        err: e,
    })?;

    for i in nums {
        disk.remove_partition_by_number(i)
            .map_err(|e| PartitionError::RemovePartition {
                path: dev.display().to_string(),
                number: i,
                err: e,
            })?;
    }

    disk.commit().map_err(|e| PartitionError::CommitChanges {
        path: dev.display().to_string(),
        err: e,
    })?;

    create_parition_table(dev)?;

    let mut device = Device::new(dev).map_err(|e| PartitionError::OpenDevice {
        path: dev.display().to_string(),
        err: e,
    })?;

    let device = &mut device as *mut Device;
    let device = unsafe { &mut (*device) };

    let start_sector = 1024 * 1024 / sector_size;

    let system_end_sector = if is_efi {
        length - efi_size / sector_size + start_sector
    } else {
        length + start_sector
    };

    let mut flags = vec![];

    if !is_efi {
        flags.push(PedPartitionFlag::PED_PARTITION_BOOT);
    }

    let system = &PartitionCreate {
        path: dev.to_path_buf(),
        start_sector,
        end_sector: system_end_sector,
        format: true,
        file_system: Some(FileSystem::Ext4),
        kind: PartitionType::Primary,
        flags,
        label: None,
    };

    create_partition(device, system).map_err(|e| PartitionError::CreatePartition {
        path: dev.display().to_string(),
        err: e,
    })?;

    let p = DkPartition {
        path: Some(PathBuf::from("/dev/loop30p1")),
        parent_path: Some(dev.to_path_buf()),
        fs_type: Some("ext4".to_string()),
        size: system_end_sector * device.sector_size(),
    };

    format_partition(&p)?;

    let efi = if is_efi {
        let start_sector = system_end_sector;

        // Ref: https://en.wikipedia.org/wiki/GUID_Partition_Table#Partition_entries_(LBA_2%E2%80%9333)
        let last_usable_sector = length - 34;

        let mmod = (last_usable_sector - start_sector) % (1024 * 1024 / 512);

        let efi = &PartitionCreate {
            path: dev.to_path_buf(),
            start_sector,
            end_sector: last_usable_sector - mmod,
            format: true,
            file_system: Some(FileSystem::Fat32),
            kind: PartitionType::Primary,
            flags: vec![
                PedPartitionFlag::PED_PARTITION_BOOT,
                PedPartitionFlag::PED_PARTITION_ESP,
            ],
            label: None,
        };

        create_partition(device, efi).map_err(|e| PartitionError::CreatePartition {
            path: dev.display().to_string(),
            err: e,
        })?;

        let p = DkPartition {
            path: Some(PathBuf::from("/dev/loop30p2")),
            parent_path: Some(dev.to_path_buf()),
            fs_type: Some("vfat".to_string()),
            size: 512 * 1024_u64.pow(2),
        };

        format_partition(&p)?;
        Some(p)
    } else {
        None
    };

    device.sync().map_err(|e| PartitionError::SyncDevice {
        path: dev.display().to_string(),
        err: e,
    })?;

    Ok((efi, p))
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

#[cfg(not(debug_assertions))]
pub fn auto_create_partitions(
    dev: &Path,
) -> Result<(Option<DkPartition>, DkPartition), PartitionError> {
    let mut device = Device::new(dev).map_err(|e| PartitionError::open_device(dev, e))?;

    let device = &mut device as *mut Device;
    let device = unsafe { &mut (*device) };

    let is_efi = is_efi_booted();
    let sector_size = device.sector_size();

    let size = device.length() * sector_size;

    if get_partition_table_type(dev)
        .map(|x| x == "msdos")
        .unwrap_or(false)
        && size > 512 * (2_u64.pow(31) - 1)
    {
        return Err(PartitionError::MBRMaxSizeLimit(dev.display().to_string()));
    }

    if let Ok(disk) = Disk::new(&mut *device) {
        info!("Disk already exists, open disk and remove existing partitions");
        let mut nums = vec![];

        // 先删除主分区
        let primarys = disk.parts().filter(|x| x.type_get_name() == "primary");

        for i in primarys {
            let num = i.num();
            if num > 0 {
                nums.push(num as u32);
            }
        }

        remove_part_by_nums(dev, nums)?;

        let mut device = Device::new(dev).map_err(|e| PartitionError::open_device(dev, e))?;

        let device = &mut device as *mut Device;
        let device = unsafe { &mut (*device) };
        let disk = Disk::new(&mut *device).map_err(|e| PartitionError::open_disk(dev, e))?;

        // 再删除逻辑分区
        let logical = disk.parts().filter(|x| x.type_get_name() == "logical");

        let mut nums = vec![];
        for i in logical {
            let num = i.num();
            if num > 0 {
                nums.push(num as u32);
            }
        }

        remove_part_by_nums(dev, nums)?;

        let mut device = Device::new(dev).map_err(|e| PartitionError::open_device(dev, e))?;
        let device = &mut device as *mut Device;
        let device = unsafe { &mut (*device) };
        let disk = Disk::new(&mut *device).map_err(|e| PartitionError::open_disk(dev, e))?;

        let mut nums = vec![];
        for i in disk.parts() {
            let num = i.num();
            if num > 0 {
                nums.push(num as u32);
            }
        }

        // 再删除其他分区
        remove_part_by_nums(dev, nums)?;
    } else {
        info!("Disk does not exists, creating new ...");
    }

    create_parition_table(dev)?;

    let mut device = Device::new(dev).map_err(|e| PartitionError::open_device(dev, e))?;
    let device = &mut device as *mut Device;
    let mut device = unsafe { &mut (*device) };

    let start_sector = 1024 * 1024 / sector_size;
    let end_sector = start_sector + (512 * 1024 * 1024 / device.sector_size());

    if is_efi {
        let efi = &PartitionCreate {
            path: dev.to_path_buf(),
            start_sector,
            end_sector,
            format: true,
            file_system: Some(FileSystem::Fat32),
            kind: PartitionType::Primary,
            flags: vec![
                PedPartitionFlag::PED_PARTITION_BOOT,
                PedPartitionFlag::PED_PARTITION_ESP,
            ],
            label: None,
        };

        create_partition(&mut device, efi).map_err(|e| PartitionError::create_partition(dev, e))?;
    }

    let mut flags = vec![];

    if !is_efi {
        flags.push(PedPartitionFlag::PED_PARTITION_BOOT);
    }

    let length = device.length();
    let system_start_sector = if is_efi { end_sector } else { start_sector };

    // Ref: https://en.wikipedia.org/wiki/GUID_Partition_Table#Partition_entries_(LBA_2%E2%80%9333)
    let last_usable_sector = device.length() - 34;
    let mmod = (last_usable_sector - system_start_sector) % (1024 * 1024 / sector_size);

    let system = &PartitionCreate {
        path: dev.to_path_buf(),
        start_sector: system_start_sector,
        end_sector: last_usable_sector - mmod,
        format: true,
        file_system: Some(FileSystem::Ext4),
        kind: PartitionType::Primary,
        flags,
        label: None,
    };

    create_partition(&mut device, system).map_err(|e| PartitionError::create_partition(dev, e))?;

    let disk = Disk::new(&mut device).map_err(|e| PartitionError::open_disk(dev, e))?;
    let mut last = None;
    for p in disk.parts() {
        if let Some(path) = p.get_path() {
            last = Some(path.to_path_buf());
        }
    }

    let efi = if is_efi {
        let part_efi = disk
            .get_partition_by_sector(start_sector as i64)
            .ok_or_else(|| PartitionError::FindSector(start_sector as i64))?;

        let geom_length = part_efi.geom_length();
        let part_length = if geom_length < 0 {
            0
        } else {
            geom_length as u64
        };

        let p = DkPartition {
            path: part_efi.get_path().map(|x| x.to_path_buf()),
            parent_path: Some(dev.to_path_buf()),
            fs_type: Some("vfat".to_string()),
            size: part_length * sector_size,
        };

        format_partition(&p)?;

        Some(p)
    } else {
        None
    };

    let p = last.ok_or_else(|| PartitionError::CreatePartition {
        path: dev.display().to_string(),
        err: io::Error::new(io::ErrorKind::Other, "Unexcept error"),
    })?;

    let p = DkPartition {
        path: Some(p),
        parent_path: Some(dev.to_path_buf()),
        fs_type: Some("ext4".to_owned()),
        size: (length - start_sector) * sector_size,
    };

    format_partition(&p)?;

    Ok((efi, p))
}

#[cfg(not(debug_assertions))]
fn remove_part_by_nums(dev: &Path, nums: Vec<u32>) -> Result<(), PartitionError> {
    let mut device = Device::new(dev).map_err(|e| PartitionError::OpenDevice {
        path: dev.display().to_string(),
        err: e,
    })?;

    let device = &mut device as *mut Device;
    let device = unsafe { &mut (*device) };
    let mut disk = Disk::new(&mut *device).map_err(|e| PartitionError::OpenDisk {
        path: dev.display().to_string(),
        err: e,
    })?;

    for i in nums {
        disk.remove_partition_by_number(i)
            .map_err(|e| PartitionError::RemovePartition {
                path: dev.display().to_string(),
                number: i,
                err: e,
            })?;
    }

    disk.commit().map_err(|e| PartitionError::CommitChanges {
        path: dev.display().to_string(),
        err: e,
    })?;

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

#[cfg(debug_assertions)]
pub fn auto_create_partitions_gpt(
    device_path: &Path,
) -> Result<(DkPartition, DkPartition), PartitionError> {
    let sector_size = get_sector_size(device_path)?;

    let mut f = fs::OpenOptions::new()
        .write(true)
        .open(device_path)
        .map_err(|e| PartitionError::OpenDevice {
            path: device_path.display().to_string(),
            err: e,
        })?;

    // 创建新的分区表
    let mut gpt = GPT::new_from(&mut f, sector_size, generate_gpt_random_uuid())?;

    // 写一个假的 MBR 保护分区
    write_protective_mbr_into(&mut f, sector_size).unwrap();

    // 起始扇区为 1MiB 除以扇区大小
    let starting_lba = 1024 * 1024 / sector_size;

    // EFI 的大小
    let efi_size = 512 * 1024 * 1024;

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

    // 应用分区表的修改
    gpt.write_into(&mut f)?;

    gptman::linux::reread_partition_table(&mut f).map_err(PartitionError::ReloadTable)?;

    let system = DkPartition {
        path: Some(PathBuf::from("/dev/loop30p1")),
        parent_path: Some(device_path.to_path_buf()),
        fs_type: Some("ext4".to_string()),
        size: gpt.header.primary_lba * sector_size,
    };

    let efi = DkPartition {
        path: Some(PathBuf::from("/dev/loop30p2")),
        parent_path: Some(device_path.to_path_buf()),
        fs_type: Some("vfat".to_string()),
        size: 512 * 1024_u64.pow(2),
    };

    format_partition(&system)?;
    format_partition(&efi)?;

    Ok((efi, system))
}

#[cfg(debug_assertions)]
pub fn auto_create_partitions_mbr(device_path: &Path) -> Result<DkPartition, PartitionError> {
    let sector_size = get_sector_size(device_path)? as u32;

    let mut f = fs::OpenOptions::new()
        .write(true)
        .open(device_path)
        .map_err(|e| PartitionError::OpenDevice {
            path: device_path.display().to_string(),
            err: e,
        })?;

    let mut mbr = MBR::new_from(&mut f, sector_size, disk_signature())?;

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

    let system = DkPartition {
        path: Some(PathBuf::from("/dev/loop30p1")),
        parent_path: Some(device_path.to_path_buf()),
        fs_type: Some("ext4".to_string()),
        size: mbr.disk_size as u64,
    };

    format_partition(&system)?;

    Ok(system)
}

fn generate_gpt_random_uuid() -> [u8; 16] {
    rand::thread_rng().gen()
}

fn disk_signature() -> [u8; 4] {
    rand::thread_rng().gen()
}

fn get_sector_size(dev: &Path) -> Result<u64, PartitionError> {
    let device_name = dev
        .file_name()
        .map(|x| x.to_string_lossy())
        .ok_or_else(|| PartitionError::OpenDevice {
            path: dev.display().to_string(),
            err: io::Error::new(io::ErrorKind::NotFound, "Failed to get device name"),
        })?;

    let path = Path::new("/sys/class/block/")
        .join(device_name.to_string())
        .join("queue/logical_block_size");

    let size = fs::read_to_string(path)
        .map_err(|e| PartitionError::OpenDevice {
            path: dev.display().to_string(),
            err: e,
        })?
        .trim()
        .parse::<u64>()
        .map_err(|e| PartitionError::OpenDevice {
            path: dev.display().to_string(),
            err: io::Error::new(io::ErrorKind::InvalidData, e),
        })?;

    Ok(size)
}

pub fn wipe(device: &Path) -> io::Result<()> {
    std::process::Command::new("wipefs")
        .arg("-a")
        .arg(device)
        .output()
        .map(|_| ())
}

pub fn write_protective_mbr_into<W: ?Sized>(
    mut writer: &mut W,
    sector_size: u64,
) -> bincode::Result<()>
where
    W: Write + Seek,
{
    let size = writer.seek(SeekFrom::End(0))? / sector_size - 1;
    writer.seek(SeekFrom::Start(446))?;
    // partition 1
    writer.write_all(&[
        0x00, // status
        0x00, 0x02, 0x00, // CHS address of first absolute sector
        0xee, // partition type
        0xff, 0xff, 0xff, // CHS address of last absolute sector
        0x01, 0x00, 0x00, 0x00, // LBA of first absolute sector
    ])?;

    // number of sectors in partition 1
    serialize_into(
        &mut writer,
        &(if size > u64::from(u32::max_value()) {
            u32::max_value()
        } else {
            size as u32
        }),
    )?;

    writer.write_all(&[0; 16])?; // partition 2
    writer.write_all(&[0; 16])?; // partition 3
    writer.write_all(&[0; 16])?; // partition 4
    writer.write_all(&[0x55, 0xaa])?; // signature

    Ok(())
}
