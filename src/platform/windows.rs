use std::ffi::OsStr;
use std::mem::{offset_of, size_of};
use std::os::windows::ffi::OsStrExt;
use std::ptr::null_mut;
use std::slice;

use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
    DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, HDEVINFO, SP_DEVICE_INTERFACE_DATA,
    SP_DEVICE_INTERFACE_DETAIL_DATA_W, SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces,
    SetupDiGetClassDevsW, SetupDiGetDeviceInterfaceDetailW,
};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    BusTypeAta, BusTypeNvme, BusTypeSata, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ,
    FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::Storage::Nvme::{NVME_IDENTIFY_CNS_CONTROLLER, NVME_LOG_PAGE_HEALTH_INFO};
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::System::Ioctl::{
    CAP_SMART_CMD, GET_LENGTH_INFORMATION, GUID_DEVINTERFACE_DISK, ID_CMD,
    IOCTL_DISK_GET_LENGTH_INFO, IOCTL_STORAGE_GET_DEVICE_NUMBER, IOCTL_STORAGE_QUERY_PROPERTY,
    NVMeDataTypeIdentify, NVMeDataTypeLogPage, PropertyStandardQuery, ProtocolTypeNvme,
    READ_ATTRIBUTES, READ_THRESHOLDS, SENDCMDINPARAMS, SENDCMDOUTPARAMS, SMART_CMD,
    SMART_GET_VERSION, SMART_RCV_DRIVE_DATA, STORAGE_DEVICE_DESCRIPTOR, STORAGE_DEVICE_NUMBER,
    STORAGE_PROPERTY_QUERY, STORAGE_PROTOCOL_DATA_DESCRIPTOR, STORAGE_PROTOCOL_SPECIFIC_DATA,
    StorageAdapterProtocolSpecificProperty, StorageDeviceProperty,
    StorageDeviceProtocolSpecificProperty,
};

use crate::error::GetSmartError;
use crate::model::{DeviceInfo, DeviceProtocol, SmartReport, SmartReportRaw};
use crate::protocol::{ata, nvme};

const GENERIC_READ_ACCESS: u32 = 0x8000_0000;
const GENERIC_WRITE_ACCESS: u32 = 0x4000_0000;
const MAX_PHYSICAL_DRIVES: u32 = 64;
const ATA_SMART_CYL_LOW: u8 = 0x4f;
const ATA_SMART_CYL_HIGH: u8 = 0xc2;
const ATA_DRIVE_HEAD: u8 = 0xa0;
const INVALID_DEVICE_INFO_SET: HDEVINFO = -1isize;

#[derive(Debug)]
struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

#[derive(Debug)]
struct DeviceInfoSet(HDEVINFO);

impl Drop for DeviceInfoSet {
    fn drop(&mut self) {
        if self.0 != INVALID_DEVICE_INFO_SET {
            unsafe {
                SetupDiDestroyDeviceInfoList(self.0);
            }
        }
    }
}

#[derive(Debug, Clone)]
struct StorageDescriptor {
    bus_type: i32,
    model: Option<String>,
    serial: Option<String>,
    firmware: Option<String>,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct StorageProtocolQuery {
    property_id: i32,
    query_type: i32,
    protocol_data: STORAGE_PROTOCOL_SPECIFIC_DATA,
}

pub fn list_devices() -> Result<Vec<DeviceInfo>, GetSmartError> {
    let mut devices = Vec::new();
    let mut saw_permission_error = None;

    for index in 0..MAX_PHYSICAL_DRIVES {
        match open_physical_drive(index, 0) {
            Ok(handle) => {
                if let Ok(descriptor) = query_storage_descriptor(&handle) {
                    if let Ok(info) = build_device_info(index, &handle, &descriptor) {
                        devices.push(info);
                    }
                }
            }
            Err(GetSmartError::NotFound(_)) => continue,
            Err(GetSmartError::PermissionDenied(message)) => {
                if saw_permission_error.is_none() {
                    saw_permission_error = Some(message);
                }
            }
            Err(_) => continue,
        }
    }

    if devices.is_empty() {
        if let Some(message) = saw_permission_error {
            return Err(GetSmartError::PermissionDenied(message));
        }
    }

    Ok(devices)
}

pub fn get_smart(device_id: &str) -> Result<SmartReport, GetSmartError> {
    let index = parse_device_id(device_id)?;
    let probe_handle = open_physical_drive(index, 0)?;
    let descriptor = query_storage_descriptor(&probe_handle)?;
    let device = build_device_info(index, &probe_handle, &descriptor)?;

    match device.protocol {
        DeviceProtocol::Nvme => {
            drop(probe_handle);
            let interface_path = find_disk_interface_path(index)?;
            let handle = open_device_path(&interface_path, 0)?;
            get_nvme_report(handle, device)
        }
        DeviceProtocol::Ata => {
            drop(probe_handle);
            let handle = open_ata_physical_drive(index)?;
            get_ata_report(handle, device, index)
        }
    }
}

fn open_ata_physical_drive(index: u32) -> Result<Handle, GetSmartError> {
    // Some ATA/SATA devices reject GENERIC_WRITE with ERROR_BUSY (170).
    // Retry with progressively lower access so SMART IOCTLs can still proceed.
    let access_candidates = [GENERIC_READ_ACCESS | GENERIC_WRITE_ACCESS, GENERIC_READ_ACCESS, 0];
    let mut last_error = None;

    for desired_access in access_candidates {
        match open_physical_drive(index, desired_access) {
            Ok(handle) => return Ok(handle),
            Err(error) => last_error = Some(error),
        }
    }

    Err(last_error.unwrap_or_else(|| {
        GetSmartError::IoError(format!(
            "failed to open ATA drive PhysicalDrive{index} with all access modes"
        ))
    }))
}

fn get_nvme_report(handle: Handle, device: DeviceInfo) -> Result<SmartReport, GetSmartError> {
    let identify = query_nvme_protocol_data(
        &handle,
        StorageAdapterProtocolSpecificProperty,
        NVMeDataTypeIdentify as u32,
        NVME_IDENTIFY_CNS_CONTROLLER as u32,
        0,
        nvme::NVME_IDENTIFY_CONTROLLER_BYTES,
    )
    .ok()
    .and_then(|bytes| nvme::parse_identify_controller(&bytes).ok());
    let smart_bytes = query_nvme_protocol_data(
        &handle,
        StorageDeviceProtocolSpecificProperty,
        NVMeDataTypeLogPage as u32,
        NVME_LOG_PAGE_HEALTH_INFO as u32,
        0,
        nvme::NVME_SMART_LOG_BYTES,
    )?;

    let smart_log = nvme::parse_smart_health_log(&smart_bytes)?;
    let summary = nvme::derive_summary(&smart_log);

    Ok(SmartReport {
        device,
        collected_at_utc: String::new(),
        summary,
        raw: SmartReportRaw {
            identify_controller: identify,
            smart_health_log: Some(smart_log),
            ..SmartReportRaw::default()
        },
    })
}

fn get_ata_report(
    handle: Handle,
    device: DeviceInfo,
    index: u32,
) -> Result<SmartReport, GetSmartError> {
    ensure_smart_supported(&handle)?;
    let drive_number = u8::try_from(index).map_err(|_| {
        GetSmartError::UnsupportedDevice("Windows ATA drive index exceeds u8 range".to_owned())
    })?;

    let identify_bytes = issue_ata_receive_data(&handle, drive_number, ID_CMD as u8, 0)?;
    let smart_data_bytes = issue_ata_receive_data(
        &handle,
        drive_number,
        SMART_CMD as u8,
        READ_ATTRIBUTES as u8,
    )?;
    let threshold_bytes = issue_ata_receive_data(
        &handle,
        drive_number,
        SMART_CMD as u8,
        READ_THRESHOLDS as u8,
    )?;

    let identify = ata::parse_identify_device(&identify_bytes)?;
    let smart_data = ata::parse_smart_read_data(&smart_data_bytes)?;
    let thresholds = ata::parse_smart_thresholds(&threshold_bytes)?;
    let passed = ata::derive_passed_from_thresholds(&smart_data, &thresholds);
    let summary = ata::derive_summary(&smart_data, passed);

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

fn parse_device_id(device_id: &str) -> Result<u32, GetSmartError> {
    let index = device_id
        .strip_prefix("physicaldrive:")
        .ok_or_else(|| {
            GetSmartError::InvalidArgument(
                "Windows device_id must use the physicaldrive:<n> format".to_owned(),
            )
        })?
        .parse::<u32>()
        .map_err(|_| {
            GetSmartError::InvalidArgument(
                "Windows device_id must end with a valid drive number".to_owned(),
            )
        })?;

    Ok(index)
}

fn build_device_info(
    index: u32,
    handle: &Handle,
    descriptor: &StorageDescriptor,
) -> Result<DeviceInfo, GetSmartError> {
    let protocol = protocol_from_bus(descriptor.bus_type).ok_or_else(|| {
        GetSmartError::UnsupportedDevice(format!(
            "PhysicalDrive{index} uses unsupported bus type {}",
            descriptor.bus_type
        ))
    })?;

    Ok(DeviceInfo {
        id: format!("physicaldrive:{index}"),
        path: physical_drive_path(index),
        protocol,
        model: descriptor.model.clone(),
        serial: descriptor.serial.clone(),
        firmware: descriptor.firmware.clone(),
        capacity_bytes: query_capacity(handle).ok(),
    })
}

fn protocol_from_bus(bus_type: i32) -> Option<DeviceProtocol> {
    match bus_type {
        value if value == BusTypeNvme => Some(DeviceProtocol::Nvme),
        value if value == BusTypeAta || value == BusTypeSata => Some(DeviceProtocol::Ata),
        _ => None,
    }
}

fn open_physical_drive(index: u32, desired_access: u32) -> Result<Handle, GetSmartError> {
    let path = physical_drive_path(index);
    open_device_path(&path, desired_access)
}

fn open_device_path(path: &str, desired_access: u32) -> Result<Handle, GetSmartError> {
    let mut wide_path: Vec<u16> = OsStr::new(&path).encode_wide().collect();
    wide_path.push(0);

    let handle = unsafe {
        CreateFileW(
            wide_path.as_ptr(),
            desired_access,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            null_mut(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            null_mut(),
        )
    };

    if handle == INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error().into());
    }

    Ok(Handle(handle))
}

fn find_disk_interface_path(device_number: u32) -> Result<String, GetSmartError> {
    let info_set = unsafe {
        SetupDiGetClassDevsW(
            &GUID_DEVINTERFACE_DISK,
            null_mut(),
            null_mut(),
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        )
    };

    if info_set == INVALID_DEVICE_INFO_SET {
        return Err(std::io::Error::last_os_error().into());
    }

    let info_set = DeviceInfoSet(info_set);
    let mut member_index = 0;

    loop {
        let mut interface_data = SP_DEVICE_INTERFACE_DATA::default();
        interface_data.cbSize = size_of::<SP_DEVICE_INTERFACE_DATA>() as u32;

        let ok = unsafe {
            SetupDiEnumDeviceInterfaces(
                info_set.0,
                std::ptr::null(),
                &GUID_DEVINTERFACE_DISK,
                member_index,
                &mut interface_data,
            )
        };

        if ok == 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(259) {
                break;
            }
            return Err(error.into());
        }

        let mut required_size = 0u32;
        unsafe {
            SetupDiGetDeviceInterfaceDetailW(
                info_set.0,
                &interface_data,
                null_mut(),
                0,
                &mut required_size,
                null_mut(),
            );
        }

        if required_size == 0 {
            return Err(std::io::Error::last_os_error().into());
        }

        let mut detail_buffer = vec![0u8; required_size as usize];
        let detail_ptr = detail_buffer
            .as_mut_ptr()
            .cast::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>();
        unsafe {
            (*detail_ptr).cbSize = size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
        }

        let ok = unsafe {
            SetupDiGetDeviceInterfaceDetailW(
                info_set.0,
                &interface_data,
                detail_ptr,
                required_size,
                &mut required_size,
                null_mut(),
            )
        };

        if ok == 0 {
            return Err(std::io::Error::last_os_error().into());
        }

        let path = utf16_from_detail_buffer(&detail_buffer);
        let handle = match open_device_path(&path, 0) {
            Ok(handle) => handle,
            Err(_) => {
                member_index += 1;
                continue;
            }
        };

        if query_device_number(&handle).ok() == Some(device_number) {
            return Ok(path);
        }

        member_index += 1;
    }

    Err(GetSmartError::NotFound(format!(
        "could not resolve a disk interface path for PhysicalDrive{device_number}"
    )))
}

fn query_storage_descriptor(handle: &Handle) -> Result<StorageDescriptor, GetSmartError> {
    let mut query = STORAGE_PROPERTY_QUERY::default();
    query.PropertyId = StorageDeviceProperty;
    query.QueryType = PropertyStandardQuery;

    let mut output = vec![0u8; 1024];
    device_io_control(
        handle,
        IOCTL_STORAGE_QUERY_PROPERTY,
        as_bytes(&query),
        &mut output,
    )?;

    let descriptor: STORAGE_DEVICE_DESCRIPTOR = read_unaligned(&output, 0)?;
    let vendor = read_string_at_offset(&output, descriptor.VendorIdOffset);
    let product = read_string_at_offset(&output, descriptor.ProductIdOffset);
    let firmware = read_string_at_offset(&output, descriptor.ProductRevisionOffset);
    let serial = read_string_at_offset(&output, descriptor.SerialNumberOffset);
    let model = join_model(vendor, product);

    Ok(StorageDescriptor {
        bus_type: descriptor.BusType,
        model,
        serial,
        firmware,
    })
}

fn query_capacity(handle: &Handle) -> Result<u64, GetSmartError> {
    let mut output = vec![0u8; size_of::<GET_LENGTH_INFORMATION>()];
    device_io_control(handle, IOCTL_DISK_GET_LENGTH_INFO, &[], &mut output)?;
    let info: GET_LENGTH_INFORMATION = read_unaligned(&output, 0)?;
    u64::try_from(info.Length)
        .map_err(|_| GetSmartError::IoError("disk capacity reported a negative length".to_owned()))
}

fn query_device_number(handle: &Handle) -> Result<u32, GetSmartError> {
    let mut output = vec![0u8; size_of::<STORAGE_DEVICE_NUMBER>()];
    device_io_control(handle, IOCTL_STORAGE_GET_DEVICE_NUMBER, &[], &mut output)?;
    let device_number: STORAGE_DEVICE_NUMBER = read_unaligned(&output, 0)?;
    Ok(device_number.DeviceNumber)
}

fn ensure_smart_supported(handle: &Handle) -> Result<(), GetSmartError> {
    let mut output = vec![0u8; size_of::<windows_sys::Win32::System::Ioctl::GETVERSIONINPARAMS>()];
    device_io_control(handle, SMART_GET_VERSION, &[], &mut output)?;
    let capabilities = u32::from_le_bytes(output[4..8].try_into().unwrap());

    if capabilities & CAP_SMART_CMD == 0 {
        return Err(GetSmartError::UnsupportedDevice(
            "drive does not expose Windows SMART capabilities".to_owned(),
        ));
    }

    Ok(())
}

fn issue_ata_receive_data(
    handle: &Handle,
    drive_number: u8,
    command: u8,
    feature: u8,
) -> Result<Vec<u8>, GetSmartError> {
    let mut input = SENDCMDINPARAMS::default();
    input.cBufferSize = ata::ATA_SECTOR_BYTES as u32;
    input.bDriveNumber = drive_number;
    input.irDriveRegs.bSectorCountReg = 1;
    input.irDriveRegs.bSectorNumberReg = 1;
    input.irDriveRegs.bDriveHeadReg = ATA_DRIVE_HEAD;
    input.irDriveRegs.bCommandReg = command;

    if command == SMART_CMD as u8 {
        input.irDriveRegs.bFeaturesReg = feature;
        input.irDriveRegs.bCylLowReg = ATA_SMART_CYL_LOW;
        input.irDriveRegs.bCylHighReg = ATA_SMART_CYL_HIGH;
    }

    let input_bytes = &as_bytes(&input)[..size_of::<SENDCMDINPARAMS>() - 1];
    let mut output = vec![0u8; size_of::<SENDCMDOUTPARAMS>() - 1 + ata::ATA_SECTOR_BYTES];
    device_io_control(handle, SMART_RCV_DRIVE_DATA, input_bytes, &mut output)?;

    let driver_error = output[4];
    if driver_error != 0 {
        return Err(GetSmartError::IoError(format!(
            "SMART command failed with driver error 0x{driver_error:02x}"
        )));
    }

    let payload_offset = size_of::<SENDCMDOUTPARAMS>() - 1;
    Ok(output[payload_offset..payload_offset + ata::ATA_SECTOR_BYTES].to_vec())
}

fn query_nvme_protocol_data(
    handle: &Handle,
    property_id: i32,
    data_type: u32,
    request_value: u32,
    request_sub_value: u32,
    data_length: usize,
) -> Result<Vec<u8>, GetSmartError> {
    let mut query = StorageProtocolQuery::default();
    query.property_id = property_id;
    query.query_type = PropertyStandardQuery;
    query.protocol_data.ProtocolType = ProtocolTypeNvme;
    query.protocol_data.DataType = data_type;
    query.protocol_data.ProtocolDataRequestValue = request_value;
    query.protocol_data.ProtocolDataRequestSubValue = request_sub_value;
    query.protocol_data.ProtocolDataOffset = size_of::<STORAGE_PROTOCOL_SPECIFIC_DATA>() as u32;
    query.protocol_data.ProtocolDataLength = data_length as u32;

    let mut buffer = vec![0u8; size_of::<StorageProtocolQuery>() + data_length.max(512)];
    buffer[..size_of::<StorageProtocolQuery>()].copy_from_slice(as_bytes(&query));
    let buffer_length = buffer.len() as u32;
    device_io_control_in_place(
        handle,
        IOCTL_STORAGE_QUERY_PROPERTY,
        &mut buffer,
        buffer_length,
    )
    .map_err(|error| {
        GetSmartError::IoError(format!(
            "NVMe protocol query failed (property_id={property_id}, data_type={data_type}, request_value={request_value}, request_sub_value={request_sub_value}): {error}"
        ))
    })?;

    let descriptor: STORAGE_PROTOCOL_DATA_DESCRIPTOR = read_unaligned(&buffer, 0)?;
    let protocol = descriptor.ProtocolSpecificData;
    let base_offset = offset_of!(STORAGE_PROTOCOL_DATA_DESCRIPTOR, ProtocolSpecificData);
    let data_offset = base_offset + usize::try_from(protocol.ProtocolDataOffset).unwrap_or(0);
    let protocol_length = usize::try_from(protocol.ProtocolDataLength).unwrap_or(0);

    if protocol_length < data_length || data_offset + data_length > buffer.len() {
        return Err(GetSmartError::IoError(
            "NVMe protocol response did not include the expected payload".to_owned(),
        ));
    }

    Ok(buffer[data_offset..data_offset + data_length].to_vec())
}

fn device_io_control(
    handle: &Handle,
    code: u32,
    input: &[u8],
    output: &mut [u8],
) -> Result<u32, GetSmartError> {
    let mut returned = 0u32;
    let ok = unsafe {
        DeviceIoControl(
            handle.0,
            code,
            if input.is_empty() {
                null_mut()
            } else {
                input.as_ptr().cast_mut().cast()
            },
            input.len() as u32,
            if output.is_empty() {
                null_mut()
            } else {
                output.as_mut_ptr().cast()
            },
            output.len() as u32,
            &mut returned,
            null_mut(),
        )
    };

    if ok == 0 {
        return Err(std::io::Error::last_os_error().into());
    }

    Ok(returned)
}

fn device_io_control_in_place(
    handle: &Handle,
    code: u32,
    buffer: &mut [u8],
    input_length: u32,
) -> Result<u32, GetSmartError> {
    let mut returned = 0u32;
    let ok = unsafe {
        DeviceIoControl(
            handle.0,
            code,
            if input_length == 0 {
                null_mut()
            } else {
                buffer.as_mut_ptr().cast()
            },
            input_length,
            if buffer.is_empty() {
                null_mut()
            } else {
                buffer.as_mut_ptr().cast()
            },
            buffer.len() as u32,
            &mut returned,
            null_mut(),
        )
    };

    if ok == 0 {
        return Err(std::io::Error::last_os_error().into());
    }

    Ok(returned)
}

fn read_string_at_offset(buffer: &[u8], offset: u32) -> Option<String> {
    if offset == 0 {
        return None;
    }

    let start = usize::try_from(offset).ok()?;
    let bytes = buffer.get(start..)?;
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    let text = String::from_utf8_lossy(&bytes[..end]);
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn utf16_from_detail_buffer(buffer: &[u8]) -> String {
    let start = offset_of!(SP_DEVICE_INTERFACE_DETAIL_DATA_W, DevicePath);
    let bytes = &buffer[start..];
    let mut utf16 = Vec::with_capacity(bytes.len() / 2);

    for chunk in bytes.chunks_exact(2) {
        let code_unit = u16::from_le_bytes([chunk[0], chunk[1]]);
        if code_unit == 0 {
            break;
        }
        utf16.push(code_unit);
    }

    String::from_utf16_lossy(&utf16)
}

fn join_model(vendor: Option<String>, product: Option<String>) -> Option<String> {
    match (vendor, product) {
        (Some(vendor), Some(product)) => {
            let combined = format!("{vendor} {product}");
            let trimmed = combined.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_owned())
        }
        (Some(vendor), None) => Some(vendor),
        (None, Some(product)) => Some(product),
        (None, None) => None,
    }
}

fn read_unaligned<T>(buffer: &[u8], offset: usize) -> Result<T, GetSmartError>
where
    T: Copy,
{
    if buffer.len() < offset + size_of::<T>() {
        return Err(GetSmartError::IoError(format!(
            "buffer too small to read {} bytes",
            size_of::<T>()
        )));
    }

    Ok(unsafe { std::ptr::read_unaligned(buffer.as_ptr().add(offset).cast::<T>()) })
}

fn as_bytes<T>(value: &T) -> &[u8] {
    unsafe { slice::from_raw_parts((value as *const T).cast::<u8>(), size_of::<T>()) }
}

fn physical_drive_path(index: u32) -> String {
    format!(r"\\.\PhysicalDrive{index}")
}
