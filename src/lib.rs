mod error;
mod ffi;
mod model;
mod platform;
mod protocol;

pub use error::{ErrorCode, GetSmartError};
pub use ffi::{
    getsmart_free_string, getsmart_get_smart_json, getsmart_list_devices_json, getsmart_version,
};
pub use model::{
    AtaIdentifyDevice, AtaSmartAttribute, AtaSmartReadData, AtaSmartThresholdEntry,
    AtaSmartThresholds, DeviceInfo, DeviceProtocol, NvmeIdentifyController, NvmeSmartHealthLog,
    SmartReport, SmartReportRaw, SmartSummary,
};

pub fn list_devices() -> Result<Vec<DeviceInfo>, GetSmartError> {
    platform::list_devices()
}

pub fn get_smart(device_id: &str) -> Result<SmartReport, GetSmartError> {
    let mut report = platform::get_smart(device_id)?;
    report.collected_at_utc = collected_at_utc();
    Ok(report)
}

pub(crate) fn collected_at_utc() -> String {
    use time::OffsetDateTime;
    use time::format_description::well_known::Rfc3339;

    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned())
}
