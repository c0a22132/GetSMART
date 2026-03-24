use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceProtocol {
    Ata,
    Nvme,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub id: String,
    pub path: String,
    pub protocol: DeviceProtocol,
    pub model: Option<String>,
    pub serial: Option<String>,
    pub firmware: Option<String>,
    pub capacity_bytes: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SmartSummary {
    pub passed: Option<bool>,
    pub temperature_celsius: Option<u16>,
    pub power_on_hours: Option<u64>,
    pub power_cycles: Option<u64>,
    pub percentage_used: Option<u8>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SmartReportRaw {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identify_controller: Option<NvmeIdentifyController>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub smart_health_log: Option<NvmeSmartHealthLog>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identify_device: Option<AtaIdentifyDevice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub smart_read_data: Option<AtaSmartReadData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub smart_thresholds: Option<AtaSmartThresholds>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmartReport {
    pub device: DeviceInfo,
    pub collected_at_utc: String,
    pub summary: SmartSummary,
    pub raw: SmartReportRaw,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AtaIdentifyDevice {
    pub serial: Option<String>,
    pub model: Option<String>,
    pub firmware: Option<String>,
    pub rotation_rate_rpm: Option<u16>,
    pub raw_hex: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AtaSmartAttribute {
    pub id: u8,
    pub name: Option<String>,
    pub flags: u16,
    pub current: u8,
    pub worst: u8,
    pub raw_value: u64,
    pub raw_hex: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AtaSmartReadData {
    pub revision: u16,
    pub offline_data_status: u8,
    pub self_test_status: u8,
    pub checksum_valid: bool,
    pub attributes: Vec<AtaSmartAttribute>,
    pub raw_hex: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AtaSmartThresholdEntry {
    pub id: u8,
    pub name: Option<String>,
    pub threshold: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AtaSmartThresholds {
    pub revision: u16,
    pub checksum_valid: bool,
    pub entries: Vec<AtaSmartThresholdEntry>,
    pub raw_hex: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NvmeIdentifyController {
    pub vendor_id: u16,
    pub subsystem_vendor_id: u16,
    pub serial: Option<String>,
    pub model: Option<String>,
    pub firmware: Option<String>,
    pub ieee_oui: [u8; 3],
    pub raw_hex: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NvmeSmartHealthLog {
    pub critical_warning: u8,
    pub temperature_kelvin: u16,
    pub available_spare: u8,
    pub available_spare_threshold: u8,
    pub percentage_used: u8,
    pub data_units_read: u128,
    pub data_units_written: u128,
    pub host_reads: u128,
    pub host_writes: u128,
    pub controller_busy_time_minutes: u128,
    pub power_cycles: u128,
    pub power_on_hours: u128,
    pub unsafe_shutdowns: u128,
    pub media_errors: u128,
    pub num_err_log_entries: u128,
    pub warning_temp_time_minutes: u32,
    pub critical_temp_time_minutes: u32,
    pub temperature_sensors_kelvin: Vec<u16>,
    pub raw_hex: String,
}
