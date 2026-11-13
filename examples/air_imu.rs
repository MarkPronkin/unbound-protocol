#[cfg(feature = "hid")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use unbound_protocol::{air::SensorClient, device::identify, hid::HidTransport, usb::Profile};
    let path = std::env::args()
        .nth(1)
        .ok_or("pass the sensor HID path printed by unbound-protocol list")?;
    let api = hidapi::HidApi::new()?;
    let device = api
        .device_list()
        .find(|device| device.path().to_bytes() == path.as_bytes())
        .ok_or("HID path not found")?;
    let identity =
        identify(device.vendor_id(), device.product_id()).ok_or("not recognized glasses")?;
    let (_, interface) = identity
        .model
        .air_interfaces()
        .ok_or("not an Air sensor interface")?;
    if device.interface_number() >= 0 && device.interface_number() != interface {
        return Err("select the Air sensor interface, not the MCU interface".into());
    }
    let io = HidTransport::open(&api, device.path(), Profile::Air)?;
    let mut client = SensorClient::new(io, std::time::Duration::from_secs(2))?;
    client.set_streaming(true)?;
    let result = (|| -> unbound_protocol::Result<()> {
        for _ in 0..100 {
            println!("{:?}", client.next_sample()?);
        }
        Ok(())
    })();
    let stop = client.set_streaming(false);
    result?;
    stop?;
    Ok(())
}
#[cfg(not(feature = "hid"))]
fn main() {
    eprintln!("run with --features hid");
    std::process::exit(1);
}
