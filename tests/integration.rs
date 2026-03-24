use getsmart::{get_smart, list_devices};

#[test]
#[ignore = "requires a real SMART-capable device and elevated privileges"]
fn reads_real_device_when_env_var_is_present() {
    let device_id = match std::env::var("GETSMART_TEST_DEVICE_ID") {
        Ok(value) => value,
        Err(_) => return,
    };

    let devices = list_devices().expect("device enumeration should succeed");
    assert!(
        devices.iter().any(|device| device.id == device_id),
        "expected {device_id} to be returned by list_devices"
    );

    let report = get_smart(&device_id).expect("SMART read should succeed");
    assert_eq!(report.device.id, device_id);
}
