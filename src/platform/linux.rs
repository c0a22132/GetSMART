use std::fs::{self, File, OpenOptions};
use std::mem::size_of;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

use libc::{c_int, c_ulong, c_void};

use crate::error::GetSmartError;
use crate::model::{DeviceInfo, DeviceProtocol, SmartReport, SmartReportRaw};
use crate::protocol::{ata, nvme};

const SG_IO: c_ulong = 0x2285;
const SG_DXFER_NONE: c_int = -1;
const SG_DXFER_FROM_DEV: c_int = -3;
const NVME_ADMIN_IDENTIFY: u8 = 0x06;
const NVME_ADMIN_GET_LOG_PAGE: u8 = 0x02;
const NVME_GLOBAL_NSID: u32 = 0xffff_ffff;
const ATA_PASS_THROUGH_16: u8 = 0x85;
const ATA_PROTOCOL_NON_DATA: u8 = 0x03;
const ATA_PROTOCOL_PIO_IN: u8 = 0x04;
const ATA_SMART_CMD: u8 = 0xb0;
const ATA_SMART_READ_DATA: u8 = 0xd0;
const ATA_SMART_READ_THRESHOLDS: u8 = 0xd1;
const ATA_SMART_RETURN_STATUS: u8 = 0xda;
const ATA_IDENTIFY_DEVICE: u8 = 0xec;
const ATA_SMART_LBA_LOW: u8 = 0x01;
const ATA_SMART_CYL_LOW: u8 = 0x4f;
const ATA_SMART_CYL_HIGH: u8 = 0xc2;
const NVME_IOCTL_ADMIN_CMD: c_ulong = iowr::<NvmeAdminCmd>(b'N', 0x41);

#[repr(C)]
#[derive(Debug)]
struct NvmeAdminCmd {
    opcode: u8,
    flags: u8,
    rsvd1: u16,
    nsid: u32,
    cdw2: u32,
    cdw3: u32,
    metadata: u64,
    addr: u64,
    metadata_len: u32,
    data_len: u32,
    cdw10: u32,
    cdw11: u32,
    cdw12: u32,
    cdw13: u32,
    cdw14: u32,
    cdw15: u32,
    timeout_ms: u32,
    result: u32,
}

#[repr(C)]
#[derive(Debug)]
struct SgIoHdr {
    interface_id: c_int,
    dxfer_direction: c_int,
    cmd_len: u8,
    mx_sb_len: u8,
    iovec_count: u16,
    dxfer_len: u32,
    dxferp: *mut c_void,
    cmdp: *mut u8,
    sbp: *mut u8,
    timeout: u32,
    flags: u32,
    pack_id: c_int,
    usr_ptr: *mut c_void,
    status: u8,
    masked_status: u8,
    msg_status: u8,
    sb_len_wr: u8,
    host_status: u16,
    driver_status: u16,
    resid: c_int,
    duration: u32,
    info: u32,
}

pub fn list_devices() -> Result<Vec<DeviceInfo>, GetSmartError> {
    let mut devices = Vec::new();
    devices.extend(list_nvme_devices()?);
    devices.extend(list_ata_devices()?);
    devices.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(devices)
}

pub fn get_smart(device_id: &str) -> Result<SmartReport, GetSmartError> {
    if let Some(controller) = device_id.strip_prefix("nvme:") {
        return get_nvme_smart(controller);
    }
    if let Some(block) = device_id.strip_prefix("ata:") {
        return get_ata_smart(block);
    }

    Err(GetSmartError::InvalidArgument(
        "Linux device_id must use nvme:<controller> or ata:<block>".to_owned(),
    ))
}

fn get_nvme_smart(controller: &str) -> Result<SmartReport, GetSmartError> {
    validate_component_name(controller)?;
    let path = PathBuf::from(format!("/dev/{controller}"));
    let file = open_rw(&path)?;

    let mut identify_bytes = vec![0u8; nvme::NVME_IDENTIFY_CONTROLLER_BYTES];
    nvme_admin_passthrough(&file, NVME_ADMIN_IDENTIFY, 0, 1, 0, &mut identify_bytes)?;

    let mut smart_log_bytes = vec![0u8; nvme::NVME_SMART_LOG_BYTES];
    let numd = ((smart_log_bytes.len() / 4) as u32).saturating_sub(1);
    let cdw10 = NVME_ADMIN_GET_LOG_PAGE_SMART | (numd << 16);
    nvme_admin_passthrough(
        &file,
        NVME_ADMIN_GET_LOG_PAGE,
        NVME_GLOBAL_NSID,
        cdw10,
        0,
        &mut smart_log_bytes,
    )?;

    let identify = nvme::parse_identify_controller(&identify_bytes)?;
    let smart_log = nvme::parse_smart_health_log(&smart_log_bytes)?;
    let mut device = nvme_device_info(controller)?;
    device.model = identify.model.clone().or(device.model);
    device.serial = identify.serial.clone().or(device.serial);
    device.firmware = identify.firmware.clone().or(device.firmware);

    Ok(SmartReport {
        device,
        collected_at_utc: String::new(),
        summary: nvme::derive_summary(&smart_log),
        raw: SmartReportRaw {
            identify_controller: Some(identify),
            smart_health_log: Some(smart_log),
            ..SmartReportRaw::default()
        },
    })
}

fn get_ata_smart(block: &str) -> Result<SmartReport, GetSmartError> {
    validate_component_name(block)?;
    let sysfs_path = Path::new("/sys/block").join(block);
    ensure_internal_ata_device(&sysfs_path)?;

    let path = PathBuf::from(format!("/dev/{block}"));
    let file = open_rw(&path)?;

    let identify_bytes = ata_data_command(&file, build_ata_identify_cdb())?;
    let smart_data_bytes = ata_data_command(&file, build_ata_smart_read_cdb(ATA_SMART_READ_DATA))?;
    let threshold_bytes =
        ata_data_command(&file, build_ata_smart_read_cdb(ATA_SMART_READ_THRESHOLDS))?;
    let passed = ata_return_status(&file).ok().flatten();

    let identify = ata::parse_identify_device(&identify_bytes)?;
    let smart_data = ata::parse_smart_read_data(&smart_data_bytes)?;
    let thresholds = ata::parse_smart_thresholds(&threshold_bytes)?;
    let summary = ata::derive_summary(
        &smart_data,
        passed.or_else(|| ata::derive_passed_from_thresholds(&smart_data, &thresholds)),
    );

    let mut device = ata_device_info(block)?;
    device.model = identify.model.clone().or(device.model);
    device.serial = identify.serial.clone().or(device.serial);
    device.firmware = identify.firmware.clone().or(device.firmware);

    Ok(SmartReport {
        device,
        collected_at_utc: String::new(),
        summary,
        raw: SmartReportRaw {
            identify_device: Some(identify),
            smart_read_data: Some(smart_data),
            smart_thresholds: Some(thresholds),
            ..SmartReportRaw::default()
        },
    })
}

fn list_nvme_devices() -> Result<Vec<DeviceInfo>, GetSmartError> {
    let mut devices = Vec::new();
    let root = Path::new("/sys/class/nvme");
    if !root.exists() {
        return Ok(devices);
    }

    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("nvme") || name.contains('n') {
            continue;
        }

        devices.push(nvme_device_info(&name)?);
    }

    Ok(devices)
}

fn list_ata_devices() -> Result<Vec<DeviceInfo>, GetSmartError> {
    let mut devices = Vec::new();
    let root = Path::new("/sys/block");
    if !root.exists() {
        return Ok(devices);
    }

    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("sd") {
            continue;
        }
        if ensure_internal_ata_device(&entry.path()).is_err() {
            continue;
        }

        devices.push(ata_device_info(&name)?);
    }

    Ok(devices)
}

fn nvme_device_info(controller: &str) -> Result<DeviceInfo, GetSmartError> {
    let sysfs_path = Path::new("/sys/class/nvme").join(controller);
    Ok(DeviceInfo {
        id: format!("nvme:{controller}"),
        path: format!("/dev/{controller}"),
        protocol: DeviceProtocol::Nvme,
        model: read_optional_text(&sysfs_path.join("model")),
        serial: read_optional_text(&sysfs_path.join("serial")),
        firmware: read_optional_text(&sysfs_path.join("firmware_rev")),
        capacity_bytes: read_nvme_capacity(&sysfs_path),
    })
}

fn ata_device_info(block: &str) -> Result<DeviceInfo, GetSmartError> {
    let sysfs_path = Path::new("/sys/block").join(block);
    Ok(DeviceInfo {
        id: format!("ata:{block}"),
        path: format!("/dev/{block}"),
        protocol: DeviceProtocol::Ata,
        model: read_optional_text(&sysfs_path.join("device/model")),
        serial: read_optional_text(&sysfs_path.join("device/serial"))
            .or_else(|| read_optional_text(&sysfs_path.join("device/vpd_pg80"))),
        firmware: read_optional_text(&sysfs_path.join("device/rev")),
        capacity_bytes: read_sector_count(&sysfs_path.join("size")).map(|value| value * 512),
    })
}

fn ensure_internal_ata_device(sysfs_path: &Path) -> Result<(), GetSmartError> {
    let canonical = fs::canonicalize(sysfs_path)?;
    let path_text = canonical.to_string_lossy();
    if path_text.contains("/ata") && !path_text.contains("/usb") && !path_text.contains("/virtual")
    {
        return Ok(());
    }

    Err(GetSmartError::UnsupportedDevice(format!(
        "{} is not an internal ATA device",
        sysfs_path.display()
    )))
}

fn read_nvme_capacity(controller_path: &Path) -> Option<u64> {
    let controller_name = controller_path.file_name()?.to_string_lossy();
    let mut total_bytes = 0u64;

    for entry in fs::read_dir(controller_path).ok()? {
        let entry = entry.ok()?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with(controller_name.as_ref()) || !name.contains('n') {
            continue;
        }

        if let Some(sectors) = read_sector_count(&entry.path().join("size")) {
            total_bytes = total_bytes.saturating_add(sectors.saturating_mul(512));
        }
    }

    (total_bytes > 0).then_some(total_bytes)
}

fn read_sector_count(path: &Path) -> Option<u64> {
    let raw = fs::read_to_string(path).ok()?;
    raw.trim().parse::<u64>().ok()
}

fn read_optional_text(path: &Path) -> Option<String> {
    let raw = fs::read(path).ok()?;
    let text = String::from_utf8_lossy(&raw);
    let trimmed = text.trim_matches(char::from(0)).trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn validate_component_name(name: &str) -> Result<(), GetSmartError> {
    if name.is_empty() || name.contains('/') || name.contains('\\') {
        return Err(GetSmartError::InvalidArgument(
            "device_id contains an invalid Linux device component".to_owned(),
        ));
    }

    Ok(())
}

fn open_rw(path: &Path) -> Result<File, GetSmartError> {
    Ok(OpenOptions::new().read(true).write(true).open(path)?)
}

fn nvme_admin_passthrough(
    file: &File,
    opcode: u8,
    nsid: u32,
    cdw10: u32,
    cdw11: u32,
    data: &mut [u8],
) -> Result<(), GetSmartError> {
    let mut command = NvmeAdminCmd {
        opcode,
        flags: 0,
        rsvd1: 0,
        nsid,
        cdw2: 0,
        cdw3: 0,
        metadata: 0,
        addr: data.as_mut_ptr() as u64,
        metadata_len: 0,
        data_len: data.len() as u32,
        cdw10,
        cdw11,
        cdw12: 0,
        cdw13: 0,
        cdw14: 0,
        cdw15: 0,
        timeout_ms: 15_000,
        result: 0,
    };

    let status = unsafe { libc::ioctl(file.as_raw_fd(), NVME_IOCTL_ADMIN_CMD, &mut command) };
    if status < 0 {
        return Err(std::io::Error::last_os_error().into());
    }

    Ok(())
}

fn ata_data_command(file: &File, mut cdb: [u8; 16]) -> Result<Vec<u8>, GetSmartError> {
    let mut data = vec![0u8; ata::ATA_SECTOR_BYTES];
    sg_io(file, &mut cdb, Some(&mut data), SG_DXFER_FROM_DEV)?;
    Ok(data)
}

fn ata_return_status(file: &File) -> Result<Option<bool>, GetSmartError> {
    let mut cdb = build_ata_return_status_cdb();
    let sense = sg_io(file, &mut cdb, None, SG_DXFER_NONE)?;
    Ok(parse_ata_status_from_sense(&sense))
}

fn sg_io(
    file: &File,
    cdb: &mut [u8; 16],
    mut data: Option<&mut [u8]>,
    direction: c_int,
) -> Result<Vec<u8>, GetSmartError> {
    let mut sense = vec![0u8; 32];
    let mut header = SgIoHdr {
        interface_id: i32::from(b'S'),
        dxfer_direction: direction,
        cmd_len: cdb.len() as u8,
        mx_sb_len: sense.len() as u8,
        iovec_count: 0,
        dxfer_len: data.as_ref().map_or(0, |buffer| buffer.len() as u32),
        dxferp: data
            .as_mut()
            .map_or(std::ptr::null_mut(), |buffer| buffer.as_mut_ptr().cast()),
        cmdp: cdb.as_mut_ptr(),
        sbp: sense.as_mut_ptr(),
        timeout: 15_000,
        flags: 0,
        pack_id: 0,
        usr_ptr: std::ptr::null_mut(),
        status: 0,
        masked_status: 0,
        msg_status: 0,
        sb_len_wr: 0,
        host_status: 0,
        driver_status: 0,
        resid: 0,
        duration: 0,
        info: 0,
    };

    let result = unsafe { libc::ioctl(file.as_raw_fd(), SG_IO, &mut header) };
    if result < 0 {
        return Err(std::io::Error::last_os_error().into());
    }

    let sense_len = usize::from(header.sb_len_wr).min(sense.len());
    sense.truncate(sense_len);

    if header.status != 0 && parse_ata_status_from_sense(&sense).is_none() {
        return Err(GetSmartError::IoError(format!(
            "SG_IO failed with status=0x{:02x}, host_status=0x{:04x}, driver_status=0x{:04x}",
            header.status, header.host_status, header.driver_status
        )));
    }

    Ok(sense)
}

fn build_ata_identify_cdb() -> [u8; 16] {
    build_ata_data_in_cdb(0, 1, 0, 0, 0, ATA_IDENTIFY_DEVICE)
}

fn build_ata_smart_read_cdb(feature: u8) -> [u8; 16] {
    build_ata_data_in_cdb(
        feature,
        1,
        ATA_SMART_LBA_LOW,
        ATA_SMART_CYL_LOW,
        ATA_SMART_CYL_HIGH,
        ATA_SMART_CMD,
    )
}

fn build_ata_return_status_cdb() -> [u8; 16] {
    [
        ATA_PASS_THROUGH_16,
        ATA_PROTOCOL_NON_DATA << 1,
        0x2c,
        0,
        ATA_SMART_RETURN_STATUS,
        0,
        0,
        0,
        0,
        0,
        ATA_SMART_CYL_LOW,
        0,
        ATA_SMART_CYL_HIGH,
        0,
        ATA_SMART_CMD,
        0,
    ]
}

fn build_ata_data_in_cdb(
    feature: u8,
    sector_count: u8,
    lba_low: u8,
    lba_mid: u8,
    lba_high: u8,
    command: u8,
) -> [u8; 16] {
    [
        ATA_PASS_THROUGH_16,
        ATA_PROTOCOL_PIO_IN << 1,
        0x0e,
        0,
        feature,
        0,
        sector_count,
        0,
        lba_low,
        0,
        lba_mid,
        0,
        lba_high,
        0,
        command,
        0,
    ]
}

fn parse_ata_status_from_sense(sense: &[u8]) -> Option<bool> {
    if sense.len() < 2 {
        return None;
    }

    match sense[0] {
        0x72 | 0x73 => {
            let mut offset = 8usize;
            while offset + 1 < sense.len() {
                let length = usize::from(sense[offset + 1]) + 2;
                if offset + length > sense.len() {
                    break;
                }
                if sense[offset] == 0x09 && length >= 14 {
                    return ata::smart_return_status(sense[offset + 9], sense[offset + 11]);
                }
                offset += length;
            }
            None
        }
        0x70 | 0x71 if sense.len() > 11 => ata::smart_return_status(sense[10], sense[11]),
        _ => None,
    }
}

const NVME_ADMIN_GET_LOG_PAGE_SMART: u32 = 0x02;

const IOC_NRBITS: u64 = 8;
const IOC_TYPEBITS: u64 = 8;
const IOC_SIZEBITS: u64 = 14;
const IOC_NRSHIFT: u64 = 0;
const IOC_TYPESHIFT: u64 = IOC_NRSHIFT + IOC_NRBITS;
const IOC_SIZESHIFT: u64 = IOC_TYPESHIFT + IOC_TYPEBITS;
const IOC_DIRSHIFT: u64 = IOC_SIZESHIFT + IOC_SIZEBITS;
const IOC_READ: u64 = 2;
const IOC_WRITE: u64 = 1;

const fn ioc(dir: u64, ty: u8, nr: u8, size: usize) -> c_ulong {
    ((dir << IOC_DIRSHIFT)
        | ((ty as u64) << IOC_TYPESHIFT)
        | ((nr as u64) << IOC_NRSHIFT)
        | ((size as u64) << IOC_SIZESHIFT)) as c_ulong
}

const fn iowr<T>(ty: u8, nr: u8) -> c_ulong {
    ioc(IOC_READ | IOC_WRITE, ty, nr, size_of::<T>())
}
