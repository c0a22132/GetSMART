use crate::error::GetSmartError;
use crate::model::{NvmeIdentifyController, NvmeSmartHealthLog, SmartSummary};

pub const NVME_IDENTIFY_CONTROLLER_BYTES: usize = 4096;
pub const NVME_SMART_LOG_BYTES: usize = 512;

pub fn parse_identify_controller(buffer: &[u8]) -> Result<NvmeIdentifyController, GetSmartError> {
    if buffer.len() < NVME_IDENTIFY_CONTROLLER_BYTES {
        return Err(GetSmartError::IoError(format!(
            "NVMe identify controller buffer must be at least {NVME_IDENTIFY_CONTROLLER_BYTES} bytes"
        )));
    }

    Ok(NvmeIdentifyController {
        vendor_id: u16::from_le_bytes([buffer[0], buffer[1]]),
        subsystem_vendor_id: u16::from_le_bytes([buffer[2], buffer[3]]),
        serial: parse_ascii(&buffer[4..24]),
        model: parse_ascii(&buffer[24..64]),
        firmware: parse_ascii(&buffer[64..72]),
        ieee_oui: [buffer[73], buffer[74], buffer[75]],
        raw_hex: hex::encode(&buffer[..NVME_IDENTIFY_CONTROLLER_BYTES]),
    })
}

pub fn parse_smart_health_log(buffer: &[u8]) -> Result<NvmeSmartHealthLog, GetSmartError> {
    if buffer.len() < NVME_SMART_LOG_BYTES {
        return Err(GetSmartError::IoError(format!(
            "NVMe SMART log buffer must be at least {NVME_SMART_LOG_BYTES} bytes"
        )));
    }

    let sensors = (0..8)
        .map(|index| {
            let offset = 200 + (index * 2);
            u16::from_le_bytes([buffer[offset], buffer[offset + 1]])
        })
        .filter(|value| *value != 0)
        .collect();

    Ok(NvmeSmartHealthLog {
        critical_warning: buffer[0],
        temperature_kelvin: u16::from_le_bytes([buffer[1], buffer[2]]),
        available_spare: buffer[3],
        available_spare_threshold: buffer[4],
        percentage_used: buffer[5],
        data_units_read: parse_u128(&buffer[32..48]),
        data_units_written: parse_u128(&buffer[48..64]),
        host_reads: parse_u128(&buffer[64..80]),
        host_writes: parse_u128(&buffer[80..96]),
        controller_busy_time_minutes: parse_u128(&buffer[96..112]),
        power_cycles: parse_u128(&buffer[112..128]),
        power_on_hours: parse_u128(&buffer[128..144]),
        unsafe_shutdowns: parse_u128(&buffer[144..160]),
        media_errors: parse_u128(&buffer[160..176]),
        num_err_log_entries: parse_u128(&buffer[176..192]),
        warning_temp_time_minutes: u32::from_le_bytes([
            buffer[192],
            buffer[193],
            buffer[194],
            buffer[195],
        ]),
        critical_temp_time_minutes: u32::from_le_bytes([
            buffer[196],
            buffer[197],
            buffer[198],
            buffer[199],
        ]),
        temperature_sensors_kelvin: sensors,
        raw_hex: hex::encode(&buffer[..NVME_SMART_LOG_BYTES]),
    })
}

pub fn derive_summary(log: &NvmeSmartHealthLog) -> SmartSummary {
    SmartSummary {
        passed: Some(log.critical_warning == 0),
        temperature_celsius: kelvin_to_celsius(log.temperature_kelvin),
        power_on_hours: u64::try_from(log.power_on_hours).ok(),
        power_cycles: u64::try_from(log.power_cycles).ok(),
        percentage_used: Some(log.percentage_used),
    }
}

fn parse_ascii(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes);
    let trimmed = text.trim_matches(char::from(0)).trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn parse_u128(bytes: &[u8]) -> u128 {
    bytes.iter().enumerate().fold(0u128, |acc, (index, byte)| {
        acc | ((*byte as u128) << (index * 8))
    })
}

fn kelvin_to_celsius(value: u16) -> Option<u16> {
    (value != 0).then_some(value.saturating_sub(273))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nvme_identify_parser_reads_fixed_ascii_fields() {
        let mut buffer = vec![0u8; NVME_IDENTIFY_CONTROLLER_BYTES];
        buffer[0..2].copy_from_slice(&0x1234u16.to_le_bytes());
        buffer[2..4].copy_from_slice(&0x5678u16.to_le_bytes());
        write_ascii(&mut buffer[4..24], "SN-01");
        write_ascii(&mut buffer[24..64], "NVMe Drive");
        write_ascii(&mut buffer[64..72], "FW1");
        buffer[73..76].copy_from_slice(&[1, 2, 3]);

        let identify = parse_identify_controller(&buffer).expect("identify parsing should work");
        assert_eq!(identify.vendor_id, 0x1234);
        assert_eq!(identify.subsystem_vendor_id, 0x5678);
        assert_eq!(identify.serial.as_deref(), Some("SN-01"));
        assert_eq!(identify.model.as_deref(), Some("NVMe Drive"));
        assert_eq!(identify.firmware.as_deref(), Some("FW1"));
        assert_eq!(identify.ieee_oui, [1, 2, 3]);
    }

    #[test]
    fn nvme_summary_uses_critical_warning_and_temperature() {
        let mut buffer = vec![0u8; NVME_SMART_LOG_BYTES];
        buffer[1..3].copy_from_slice(&300u16.to_le_bytes());
        buffer[5] = 9;
        buffer[112] = 4;
        buffer[128] = 8;

        let log = parse_smart_health_log(&buffer).expect("smart log should parse");
        let summary = derive_summary(&log);

        assert_eq!(summary.passed, Some(true));
        assert_eq!(summary.temperature_celsius, Some(27));
        assert_eq!(summary.percentage_used, Some(9));
        assert_eq!(summary.power_cycles, Some(4));
        assert_eq!(summary.power_on_hours, Some(8));
    }

    fn write_ascii(target: &mut [u8], value: &str) {
        for (slot, byte) in target.iter_mut().zip(value.bytes()) {
            *slot = byte;
        }
    }
}
