use std::{
    ffi::CStr,
    io,
    path::{Path, PathBuf},
    process::Command,
};

use disk_types::{BlockDeviceExt, FileSystem, PartitionExt, PartitionType};
use libparted::{Device, Disk, DiskType, FileSystemType, Geometry, IsZero, Partition};
use libparted_sys::{PedPartitionFlag, PedPartitionType};
use tracing::{error, info};

use crate::{is_efi_booted, PartitionError};

#[derive(Debug, Clone)]
pub struct DkPartition {
    pub path: Option<PathBuf>,
    pub parent_path: Option<PathBuf>,
    pub fs_type: Option<String>,
    pub size: u64,
}

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

fn cvt<T: IsZero>(t: T) -> io::Result<T> {
    if t.is_zero() {
        Err(io::Error::last_os_error())
    } else {
        Ok(t)
    }
}

fn get_partition_table_type(device_path: &Path) -> Result<String, PartitionError> {
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
pub fn auto_create_partitions(dev: &Path) -> Result<DkPartition, PartitionError> {
    let mut device = libparted::Device::new(dev).map_err(|e| PartitionError::OpenDevice {
        path: dev.display().to_string(),
        err: e,
    })?;

    let device = &mut device as *mut Device;
    let device = unsafe { &mut (*device) };
    let efi_size = 512 * 1024 * 1024;
    let partition_table_end_size = 1024 * 1024;
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

    let mut device = libparted::Device::new(dev).map_err(|e| PartitionError::OpenDevice {
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

    let mut device = libparted::Device::new(dev).map_err(|e| PartitionError::OpenDevice {
        path: dev.display().to_string(),
        err: e,
    })?;

    let device = &mut device as *mut Device;
    let device = unsafe { &mut (*device) };

    let system_end_sector = if is_efi {
        length - (efi_size + partition_table_end_size) / sector_size
    } else {
        length - partition_table_end_size / sector_size
    };

    let mut flags = vec![];

    if !is_efi {
        flags.push(PedPartitionFlag::PED_PARTITION_BOOT);
    }

    let system = &PartitionCreate {
        path: dev.to_path_buf(),
        start_sector: 2048,
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
        path: Some(PathBuf::from("/dev/loop20p1")),
        parent_path: Some(dev.to_path_buf()),
        fs_type: Some("ext4".to_string()),
        size: system_end_sector * device.sector_size(),
    };

    format_partition(&p)?;

    if is_efi {
        let start_sector = length - (partition_table_end_size + efi_size) / sector_size + 1;
        let efi = &PartitionCreate {
            path: dev.to_path_buf(),
            start_sector,
            end_sector: length - partition_table_end_size / sector_size,
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
            path: Some(PathBuf::from("/dev/loop20p2")),
            parent_path: Some(dev.to_path_buf()),
            fs_type: Some("vfat".to_string()),
            size: 512 * 1024_u64.pow(2),
        };

        format_partition(&p)?;
    }

    device.sync().map_err(|e| PartitionError::SyncDevice {
        path: dev.display().to_string(),
        err: e,
    })?;

    Ok(p)
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
        _ => command.arg("-F"),
    };

    info!("{cmd:?}");
    let output = cmd
        .arg(partition.path.as_ref().ok_or_else(|| {
            PartitionError::FormatPartition(io::Error::new(
                io::ErrorKind::NotFound,
                "partition.path is empty",
            ))
        })?)
        .output()
        .map_err(PartitionError::FormatPartition)?;

    if !output.status.success() {
        return Err(PartitionError::FormatPartition(io::Error::new(
            io::ErrorKind::Other,
            String::from_utf8_lossy(&output.stderr),
        )));
    }

    Ok(())
}

#[cfg(not(debug_assertions))]
pub fn auto_create_partitions(dev: &Path) -> Result<DkPartition, PartitionError> {
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

    if is_efi {
        let efi = &PartitionCreate {
            path: dev.to_path_buf(),
            start_sector: 2048,
            end_sector: 2048 + (512 * 1024 * 1024 / device.sector_size()),
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

    let start_sector = if is_efi {
        2048 + (512 * 1024 * 1024 / sector_size) + 1
    } else {
        2048 + 1
    };

    let mut flags = vec![];

    if !is_efi {
        flags.push(PedPartitionFlag::PED_PARTITION_BOOT);
    }

    let length = device.length();

    let system = &PartitionCreate {
        path: dev.to_path_buf(),
        start_sector,
        end_sector: device.length() - 1 * 1024 * 1024 / sector_size,
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

    if is_efi {
        let part_efi = disk
            .get_partition_by_sector(2048)
            .ok_or_else(|| PartitionError::FindSector(2048))?;

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
    }

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

    Ok(p)
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
