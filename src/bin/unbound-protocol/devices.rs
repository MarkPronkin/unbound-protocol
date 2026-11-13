#[cfg(any(feature = "libusb", feature = "serial"))]
use super::bounded;
#[cfg(any(feature = "hid", feature = "libusb", feature = "serial"))]
use super::{arity, positive};
use std::{error::Error, time::Duration};
type CliResult = std::result::Result<(), Box<dyn Error>>;

pub fn list_extra() -> CliResult {
    #[cfg(feature = "libusb")]
    for device in rusb::devices()?.iter() {
        let Ok(desc) = device.device_descriptor() else {
            continue;
        };
        if let Some(identity) =
            unbound_protocol::device::identify(desc.vendor_id(), desc.product_id())
        {
            println!(
                "USB {:?} {:04x}:{:04x} bus={} address={}",
                identity.model,
                desc.vendor_id(),
                desc.product_id(),
                device.bus_number(),
                device.address()
            );
        }
    }
    #[cfg(feature = "serial")]
    for port in serialport::available_ports()? {
        if let serialport::SerialPortType::UsbPort(info) = port.port_type {
            if (info.vid, info.pid)
                == (
                    unbound_protocol::mad_gaze::VID,
                    unbound_protocol::mad_gaze::PID,
                )
            {
                println!(
                    "SERIAL MadGazeGlow {:04x}:{:04x} port={}",
                    info.vid, info.pid, port.port_name
                );
            }
        }
    }
    #[cfg(not(any(feature = "hid", feature = "libusb", feature = "serial")))]
    return Err("rebuild with --features hid,libusb,serial".into());
    #[cfg(any(feature = "hid", feature = "libusb", feature = "serial"))]
    Ok(())
}

#[cfg(not(feature = "libusb"))]
pub fn usb(_: &[String], _: Duration) -> CliResult {
    Err("rebuild with --features libusb".into())
}
#[cfg(feature = "libusb")]
pub fn usb(args: &[String], timeout: Duration) -> CliResult {
    use unbound_protocol::{grawoow, libusb::UsbTransport, rokid};
    arity(args, 4, 5)?;
    let family = args[0].as_str();
    let bus = args[1].parse::<u8>()?;
    let address = args[2].parse::<u8>()?;
    let action = args[3].as_str();
    match (family, action) {
        ("rokid" | "grawoow", "info") | ("grawoow", "config") => arity(args, 4, 4)?,
        ("rokid" | "grawoow-sensor", "imu") => {
            arity(args, 5, 5)?;
            positive(&args[4])?;
        }
        ("rokid" | "grawoow", "mode") => {
            if args.len() == 5 {
                let value = bounded(&args[4], if family == "rokid" { 4 } else { 1 })?;
                if family == "rokid" && value == 2 {
                    return Err("Rokid half-SBS writes lack protocol evidence".into());
                }
            }
        }
        _ => return Err("unsupported USB family/action; run unbound-protocol --help".into()),
    }
    let expected = match family {
        "rokid" => (rokid::VID, rokid::PID),
        "grawoow" => grawoow::MCU_ID,
        _ => grawoow::SENSOR_ID,
    };
    let device = rusb::devices()?
        .iter()
        .find(|d| d.bus_number() == bus && d.address() == address)
        .ok_or("USB bus/address not found")?;
    let desc = device.device_descriptor()?;
    if (desc.vendor_id(), desc.product_id()) != expected {
        return Err("USB family does not match the selected device".into());
    }
    if family == "rokid" {
        let mut client = rokid::Client::new(
            UsbTransport::open_interrupt(bus, address, rokid::ENDPOINT)?,
            timeout,
        )?;
        match action {
            "info" => {
                println!("serial: {}", client.serial()?);
                println!("mode: {}", client.display_mode()?);
            }
            "imu" => {
                for _ in 0..positive(&args[4])? {
                    println!("{:?}", client.next_packet()?);
                }
            }
            "mode" => {
                if let Some(value) = args.get(4) {
                    client.set_display_mode(match value.parse::<u8>()? {
                        0 => rokid::DisplayMode::Mirror,
                        1 => rokid::DisplayMode::Stereo,
                        3 => rokid::DisplayMode::MirrorHighRefresh,
                        _ => rokid::DisplayMode::StereoHighRefresh,
                    })?;
                } else {
                    println!("{}", client.display_mode()?);
                }
            }
            _ => unreachable!(),
        }
    } else if family == "grawoow-sensor" {
        let mut client = grawoow::SensorClient::new(
            UsbTransport::open_interrupt(bus, address, grawoow::ENDPOINT)?,
            timeout,
        )?;
        for _ in 0..positive(&args[4])? {
            println!("{:?}", client.next_sample()?);
        }
    } else {
        let mut client =
            grawoow::ControlClient::new(UsbTransport::open_interface(bus, address, 0)?, timeout)?;
        match action {
            "info" => {
                println!("serial: {}", client.serial()?);
                println!("mode: {}", client.display_mode()?);
            }
            "config" => println!("{}", client.read_configuration()?),
            "mode" => {
                if let Some(value) = args.get(4) {
                    client.set_stereo(value.parse::<u8>()? == 1)?;
                } else {
                    println!("{}", client.display_mode()?);
                }
            }
            _ => unreachable!(),
        }
    }
    Ok(())
}

#[cfg(not(feature = "serial"))]
pub fn mad_gaze(_: &[String], _: Duration) -> CliResult {
    Err("rebuild with --features serial".into())
}
#[cfg(feature = "serial")]
pub fn mad_gaze(args: &[String], timeout: Duration) -> CliResult {
    use unbound_protocol::{mad_gaze::Client, serial::SerialTransport};
    arity(args, 2, 3)?;
    let action = args[1].as_str();
    match action {
        "info" | "magnetometer" => arity(args, 2, 2)?,
        "mode" => {
            if args.len() == 3 {
                bounded(&args[2], 1)?;
            }
        }
        "brightness" => {
            arity(args, 3, 3)?;
            if bounded(&args[2], 7)? == 0 {
                return Err("brightness must be 1..7".into());
            }
        }
        "imu" => {
            arity(args, 3, 3)?;
            positive(&args[2])?;
        }
        _ => return Err("unsupported Mad Gaze action; run unbound-protocol --help".into()),
    }
    // Only a currently enumerated matching USB serial port is accepted.
    let found = serialport::available_ports()?.into_iter().any(|p| p.port_name == args[0] && matches!(p.port_type, serialport::SerialPortType::UsbPort(info) if (info.vid, info.pid) == (unbound_protocol::mad_gaze::VID, unbound_protocol::mad_gaze::PID)));
    if !found {
        return Err("select a Mad Gaze USB serial port from unbound-protocol list".into());
    }
    let mut client = Client::new(SerialTransport::open(&args[0], timeout)?, timeout)?;
    match action {
        "info" => {
            println!("serial: {}", client.serial()?);
            println!("mode: {}", client.display_mode()?);
        }
        "mode" => {
            if let Some(value) = args.get(2) {
                client.set_stereo(value.parse::<u8>()? == 1)?;
            } else {
                println!("{}", client.display_mode()?);
            }
        }
        "brightness" => client.set_brightness(args[2].parse()?)?,
        "magnetometer" => {
            client.initialize_sensors()?;
            println!("{:?}", client.read_magnetometer()?);
        }
        "imu" => {
            client.initialize_sensors()?;
            let mut left = positive(&args[2])?;
            let mut last_sample = std::time::Instant::now();
            while left > 0 {
                let samples = client.read_imu()?;
                if samples.is_empty() {
                    if last_sample.elapsed() >= timeout {
                        return Err(unbound_protocol::Error::Timeout.into());
                    }
                    std::thread::sleep(Duration::from_millis(1));
                } else {
                    for sample in samples.into_iter().take(left) {
                        println!("{sample:?}");
                        left -= 1;
                    }
                    last_sample = std::time::Instant::now();
                }
            }
        }
        _ => unreachable!(),
    }
    Ok(())
}

#[cfg(feature = "hid")]
fn light_transport(
    path: &str,
    sensor: bool,
) -> std::result::Result<unbound_protocol::hid::HidTransport, Box<dyn Error>> {
    use unbound_protocol::{
        hid::{HidTransport, ReportPrefix},
        light,
    };
    let api = hidapi::HidApi::new()?;
    let path = std::ffi::CString::new(path)?;
    let device = api
        .device_list()
        .find(|d| d.path() == path.as_c_str())
        .ok_or("HID path not found")?;
    let expected = if sensor {
        light::SENSOR_ID
    } else {
        light::MCU_ID
    };
    if (device.vendor_id(), device.product_id()) != expected {
        return Err("select the correct Light MCU/sensor path from unbound-protocol list".into());
    }
    Ok(HidTransport::open_with_prefix(
        &api,
        &path,
        ReportPrefix::ProtocolByte,
    )?)
}

#[cfg(feature = "hid")]
pub fn light_hid(args: &[String], timeout: Duration) -> CliResult {
    use unbound_protocol::light;
    arity(args, 3, 4)?;
    match (args[0].as_str(), args[2].as_str()) {
        ("light", "info") | ("light-sensor", "config") => arity(args, 3, 3)?,
        ("light", "mode") => {
            if args.len() == 4 {
                light_mode(&args[3])?;
            }
        }
        ("light", "events") => {
            arity(args, 4, 4)?;
            positive(&args[3])?;
        }
        _ => return Err("unsupported Light action; run unbound-protocol --help".into()),
    }
    let sensor = args[0] == "light-sensor";
    let transport = light_transport(&args[1], sensor)?;
    if sensor {
        let mut client = light::SensorClient::new(transport, timeout)?;
        client.set_streaming(false)?;
        println!("{}", client.read_configuration()?);
    } else {
        let mut client = light::ControlClient::new(transport, timeout)?;
        match args[2].as_str() {
            "info" => {
                println!("serial: {}", client.serial()?);
                println!("mode: {}", String::from_utf8_lossy(&client.display_mode()?));
            }
            "mode" => {
                if args.len() == 4 {
                    let mode = light_mode(&args[3])?;
                    if mode != light::DisplayMode::Mirror {
                        client.enable_stereo()?;
                    }
                    client.set_display_mode(mode)?;
                } else {
                    println!("{}", String::from_utf8_lossy(&client.display_mode()?));
                }
            }
            "events" => {
                client.set_events_enabled(true)?;
                for _ in 0..positive(&args[3])? {
                    println!("{:?}", client.next_event()?);
                }
            }
            _ => unreachable!(),
        }
    }
    Ok(())
}
#[cfg(feature = "hid")]
fn light_mode(
    value: &str,
) -> std::result::Result<unbound_protocol::light::DisplayMode, Box<dyn Error>> {
    use unbound_protocol::light::DisplayMode;
    Ok(match value.parse::<u8>()? {
        1 => DisplayMode::Mirror,
        2 => DisplayMode::HalfStereo,
        3 => DisplayMode::Stereo,
        4 => DisplayMode::Stereo72,
        _ => return Err("Light mode must be 1..4".into()),
    })
}
#[cfg(not(feature = "hid"))]
pub fn light_imu(_: &[String], _: Duration) -> CliResult {
    Err("rebuild with --features hid".into())
}
#[cfg(feature = "hid")]
pub fn light_imu(args: &[String], timeout: Duration) -> CliResult {
    use unbound_protocol::{Error as ProtocolError, Result, light};
    arity(args, 4, 4)?;
    if args[2] != "imu" {
        return Err("expected light MCU_PATH SENSOR_PATH imu COUNT".into());
    }
    let count = positive(&args[3])?;
    let mut control = light::ControlClient::new(light_transport(&args[0], false)?, timeout)?;
    // Short sensor waits allow heartbeat service even when reports stop.
    let mut sensor = light::SensorClient::new(
        light_transport(&args[1], true)?,
        timeout.min(Duration::from_millis(100)),
    )?;
    sensor.set_streaming(true)?;
    let result = (|| -> Result<()> {
        let mut last_sample = std::time::Instant::now();
        let mut left = count;
        while left > 0 {
            control.heartbeat()?;
            match sensor.next_sample() {
                Ok(sample) => {
                    println!("{sample:?}");
                    left -= 1;
                    last_sample = std::time::Instant::now();
                }
                Err(ProtocolError::Timeout) if last_sample.elapsed() < timeout => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    })();
    let stop = sensor.set_streaming(false);
    result?;
    stop?;
    Ok(())
}
