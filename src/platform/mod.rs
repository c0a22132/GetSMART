#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "windows")]
mod windows;

use crate::error::GetSmartError;
use crate::model::{DeviceInfo, SmartReport};

pub fn list_devices() -> Result<Vec<DeviceInfo>, GetSmartError> {
    imp::list_devices()
}

pub fn get_smart(device_id: &str) -> Result<SmartReport, GetSmartError> {
    imp::get_smart(device_id)
}

#[cfg(target_os = "windows")]
use windows as imp;

#[cfg(target_os = "linux")]
use linux as imp;

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
mod imp {
    use crate::error::GetSmartError;
    use crate::model::{DeviceInfo, SmartReport};

    pub fn list_devices() -> Result<Vec<DeviceInfo>, GetSmartError> {
        Err(GetSmartError::UnsupportedPlatform(
            "SMART collection is only implemented for Windows and Linux".to_owned(),
        ))
    }

    pub fn get_smart(_device_id: &str) -> Result<SmartReport, GetSmartError> {
        Err(GetSmartError::UnsupportedPlatform(
            "SMART collection is only implemented for Windows and Linux".to_owned(),
        ))
    }
}
