//! Opt-in, read-only hardware checks. Never run by ordinary `cargo test` or CI.
use unbound_protocol::{
    Result,
    one::{ConnectionOptions, ControlClient, ReportsClient},
};

#[test]
#[ignore = "requires a connected One-family device with USB Ethernet enabled"]
fn live_one_tcp_read_only() -> Result<()> {
    let mut options = ConnectionOptions::default();
    if let Ok(source) = std::env::var("UNBOUND_SOURCE") {
        options.source = Some(
            source
                .parse()
                .expect("UNBOUND_SOURCE must be an IP address"),
        );
    }
    let mut client = ControlClient::connect(&options)?;
    assert!(!client.software_version()?.is_empty());
    assert!(!client.dsp_version()?.is_empty());
    assert!(!client.serial()?.is_empty());
    let config = client.configuration()?;
    assert!(config.trim_start().starts_with('{'));
    let _edid = client.edid()?;
    let _proximity = client.proximity_enabled()?;
    let _stabilization = client.stabilization_enabled()?;
    let mut reports = ReportsClient::connect(&options)?;
    let mut imu_count = 0;
    for _ in 0..1000 {
        let report = reports.next_report()?;
        if report.report_type == 11 {
            imu_count += 1;
        }
    }
    assert!(imu_count > 0);
    Ok(())
}

#[cfg(feature = "hid")]
#[test]
#[ignore = "requires UNBOUND_HID_PATH naming the One control HID interface"]
fn live_one_usb_read_only() -> std::result::Result<(), Box<dyn std::error::Error>> {
    use unbound_protocol::{
        hid::HidTransport,
        usb::{Profile, UsbClient},
    };
    let path = std::ffi::CString::new(std::env::var("UNBOUND_HID_PATH")?)?;
    let api = hidapi::HidApi::new()?;
    let io = HidTransport::open(&api, &path, Profile::One)?;
    let mut client = UsbClient::new(io, Profile::One, std::time::Duration::from_secs(2))?;
    assert!(!client.mcu_version()?.is_empty());
    assert!(!client.dsp_version()?.is_empty());
    assert!(!client.serial()?.is_empty());
    let _config = client.usb_configuration()?;
    let _camera = client.camera_plugged()?;
    Ok(())
}
