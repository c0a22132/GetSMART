use crate::error::GetSmartError;
use crate::model::{
    AtaIdentifyDevice, AtaSmartAttribute, AtaSmartReadData, AtaSmartThresholdEntry,
    AtaSmartThresholds, SmartSummary,
};

pub const ATA_SECTOR_BYTES: usize = 512;

pub fn parse_identify_device(buffer: &[u8]) -> Result<AtaIdentifyDevice, GetSmartError> {
    let buffer = expect_sector(buffer, "ATA IDENTIFY DEVICE")?;
    let serial = decode_ata_string(&buffer[20..40]);
    let firmware = decode_ata_string(&buffer[46..54]);
    let model = decode_ata_string(&buffer[54..94]);
    let rotation_rate = u16::from_le_bytes([buffer[434], buffer[435]]);

    Ok(AtaIdentifyDevice {
        serial,
        model,
        firmware,
        rotation_rate_rpm: match rotation_rate {
            0 | 1 | 0xffff => None,
            value => Some(value),
        },
        raw_hex: hex::encode(buffer),
    })
}

pub fn parse_smart_read_data(buffer: &[u8]) -> Result<AtaSmartReadData, GetSmartError> {
    let buffer = expect_sector(buffer, "ATA SMART READ DATA")?;
    let revision = u16::from_le_bytes([buffer[0], buffer[1]]);
    let mut attributes = Vec::new();

    for entry in buffer[2..362].chunks_exact(12) {
        let id = entry[0];
        if id == 0 {
            continue;
        }

        let raw = &entry[5..11];
        let raw_value = raw.iter().enumerate().fold(0u64, |acc, (index, byte)| {
            acc | ((*byte as u64) << (index * 8))
        });

        attributes.push(AtaSmartAttribute {
            id,
            name: attribute_name(id).map(ToOwned::to_owned),
            flags: u16::from_le_bytes([entry[1], entry[2]]),
            current: entry[3],
            worst: entry[4],
            raw_value,
            raw_hex: hex::encode(raw),
        });
    }

    Ok(AtaSmartReadData {
        revision,
        offline_data_status: buffer[362],
        self_test_status: buffer[363],
        checksum_valid: checksum_is_valid(buffer),
        attributes,
        raw_hex: hex::encode(buffer),
    })
}

pub fn parse_smart_thresholds(buffer: &[u8]) -> Result<AtaSmartThresholds, GetSmartError> {
    let buffer = expect_sector(buffer, "ATA SMART READ THRESHOLDS")?;
    let revision = u16::from_le_bytes([buffer[0], buffer[1]]);
    let mut entries = Vec::new();

    for entry in buffer[2..362].chunks_exact(12) {
        let id = entry[0];
        if id == 0 {
            continue;
        }

        entries.push(AtaSmartThresholdEntry {
            id,
            name: attribute_name(id).map(ToOwned::to_owned),
            threshold: entry[1],
        });
    }

    Ok(AtaSmartThresholds {
        revision,
        checksum_valid: checksum_is_valid(buffer),
        entries,
        raw_hex: hex::encode(buffer),
    })
}

pub fn derive_summary(data: &AtaSmartReadData, passed: Option<bool>) -> SmartSummary {
    let power_on_hours = data
        .attributes
        .iter()
        .find(|attribute| attribute.id == 9)
        .map(|attribute| attribute.raw_value);
    let power_cycles = data
        .attributes
        .iter()
        .find(|attribute| attribute.id == 12)
        .map(|attribute| attribute.raw_value);
    let temperature_celsius = data
        .attributes
        .iter()
        .find(|attribute| attribute.id == 194)
        .or_else(|| data.attributes.iter().find(|attribute| attribute.id == 190))
        .and_then(|attribute| {
            let value = (attribute.raw_value & 0xff) as u16;
            (value > 0).then_some(value)
        });
    let percentage_used = data
        .attributes
        .iter()
        .find(|attribute| matches!(attribute.id, 177 | 202 | 231 | 233))
        .and_then(|attribute| (attribute.current <= 100).then_some(100 - attribute.current));

    SmartSummary {
        passed,
        temperature_celsius,
        power_on_hours,
        power_cycles,
        percentage_used,
    }
}

pub fn derive_passed_from_thresholds(
    data: &AtaSmartReadData,
    thresholds: &AtaSmartThresholds,
) -> Option<bool> {
    if thresholds.entries.is_empty() {
        return None;
    }

    for attribute in &data.attributes {
        if let Some(threshold) = thresholds
            .entries
            .iter()
            .find(|entry| entry.id == attribute.id)
            .map(|entry| entry.threshold)
        {
            if threshold != 0 && attribute.current <= threshold {
                return Some(false);
            }
        }
    }

    Some(true)
}

#[allow(dead_code)]
pub fn smart_return_status(cylinder_low: u8, cylinder_high: u8) -> Option<bool> {
    match (cylinder_low, cylinder_high) {
        (0x4f, 0xc2) => Some(true),
        (0xf4, 0x2c) => Some(false),
        _ => None,
    }
}

fn expect_sector<'a>(
    buffer: &'a [u8],
    label: &str,
) -> Result<&'a [u8; ATA_SECTOR_BYTES], GetSmartError> {
    buffer.try_into().map_err(|_| {
        GetSmartError::IoError(format!(
            "{label} buffer must be exactly {ATA_SECTOR_BYTES} bytes"
        ))
    })
}

fn checksum_is_valid(buffer: &[u8; ATA_SECTOR_BYTES]) -> bool {
    buffer.iter().fold(0u8, |acc, byte| acc.wrapping_add(*byte)) == 0
}

fn decode_ata_string(bytes: &[u8]) -> Option<String> {
    let mut decoded = Vec::with_capacity(bytes.len());
    for chunk in bytes.chunks_exact(2) {
        decoded.push(chunk[1]);
        decoded.push(chunk[0]);
    }

    let text = String::from_utf8_lossy(&decoded);
    let trimmed = text.trim_matches(char::from(0)).trim();

    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn attribute_name(id: u8) -> Option<&'static str> {
    match id {
        5 => Some("reallocated_sector_count"),
        9 => Some("power_on_hours"),
        12 => Some("power_cycle_count"),
        177 => Some("wear_leveling_count"),
        190 => Some("airflow_temperature_celsius"),
        194 => Some("temperature_celsius"),
        202 => Some("percent_lifetime_remaining"),
        231 => Some("ssd_life_left"),
        233 => Some("media_wearout_indicator"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ata_identify_parser_decodes_swapped_strings() {
        let mut buffer = [0u8; ATA_SECTOR_BYTES];
        write_ata_string(&mut buffer[20..40], "SN1234");
        write_ata_string(&mut buffer[46..54], "FW1");
        write_ata_string(&mut buffer[54..94], "Example SSD");
        buffer[434..436].copy_from_slice(&7200u16.to_le_bytes());

        let identify = parse_identify_device(&buffer).expect("identify parsing should succeed");
        assert_eq!(identify.serial.as_deref(), Some("SN1234"));
        assert_eq!(identify.firmware.as_deref(), Some("FW1"));
        assert_eq!(identify.model.as_deref(), Some("Example SSD"));
        assert_eq!(identify.rotation_rate_rpm, Some(7200));
    }

    #[test]
    fn ata_smart_summary_prefers_attribute_194_for_temperature() {
        let mut buffer = [0u8; ATA_SECTOR_BYTES];
        buffer[0..2].copy_from_slice(&1u16.to_le_bytes());
        buffer[2..14].copy_from_slice(&[190, 0, 0, 80, 70, 30, 0, 0, 0, 0, 0, 0]);
        buffer[14..26].copy_from_slice(&[194, 0, 0, 81, 71, 40, 0, 0, 0, 0, 0, 0]);
        buffer[26..38].copy_from_slice(&[9, 0, 0, 99, 99, 0x34, 0x12, 0, 0, 0, 0, 0]);
        buffer[38..50].copy_from_slice(&[12, 0, 0, 99, 99, 7, 0, 0, 0, 0, 0, 0]);
        buffer[50..62].copy_from_slice(&[233, 0, 0, 92, 92, 0, 0, 0, 0, 0, 0, 0]);
        finalize_checksum(&mut buffer);

        let data = parse_smart_read_data(&buffer).expect("smart data should parse");
        let summary = derive_summary(&data, Some(true));

        assert_eq!(summary.passed, Some(true));
        assert_eq!(summary.temperature_celsius, Some(40));
        assert_eq!(summary.power_on_hours, Some(0x1234));
        assert_eq!(summary.power_cycles, Some(7));
        assert_eq!(summary.percentage_used, Some(8));
    }

    #[test]
    fn ata_threshold_parser_reads_entries() {
        let mut buffer = [0u8; ATA_SECTOR_BYTES];
        buffer[0..2].copy_from_slice(&1u16.to_le_bytes());
        buffer[2..14].copy_from_slice(&[194, 45, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        finalize_checksum(&mut buffer);

        let thresholds = parse_smart_thresholds(&buffer).expect("threshold data should parse");
        assert!(thresholds.checksum_valid);
        assert_eq!(thresholds.entries.len(), 1);
        assert_eq!(thresholds.entries[0].id, 194);
        assert_eq!(thresholds.entries[0].threshold, 45);
    }

    #[test]
    fn threshold_fallback_marks_drive_as_failed_when_current_drops_below_threshold() {
        let data = AtaSmartReadData {
            revision: 1,
            offline_data_status: 0,
            self_test_status: 0,
            checksum_valid: true,
            attributes: vec![AtaSmartAttribute {
                id: 5,
                name: Some("reallocated_sector_count".to_owned()),
                flags: 0,
                current: 9,
                worst: 9,
                raw_value: 1,
                raw_hex: "01".to_owned(),
            }],
            raw_hex: String::new(),
        };
        let thresholds = AtaSmartThresholds {
            revision: 1,
            checksum_valid: true,
            entries: vec![AtaSmartThresholdEntry {
                id: 5,
                name: Some("reallocated_sector_count".to_owned()),
                threshold: 10,
            }],
            raw_hex: String::new(),
        };

        assert_eq!(
            derive_passed_from_thresholds(&data, &thresholds),
            Some(false)
        );
    }

    fn write_ata_string(target: &mut [u8], value: &str) {
        let mut padded = vec![b' '; target.len()];
        for (index, byte) in value.bytes().enumerate() {
            padded[index] = byte;
        }

        for (pair, chunk) in target.chunks_exact_mut(2).zip(padded.chunks_exact(2)) {
            pair[0] = chunk[1];
            pair[1] = chunk[0];
        }
    }

    fn finalize_checksum(buffer: &mut [u8; ATA_SECTOR_BYTES]) {
        let checksum = buffer[..ATA_SECTOR_BYTES - 1]
            .iter()
            .fold(0u8, |acc, byte| acc.wrapping_add(*byte));
        buffer[ATA_SECTOR_BYTES - 1] = 0u8.wrapping_sub(checksum);
    }
}
