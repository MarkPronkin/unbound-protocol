use std::{
    error::Error,
    fs::File,
    io::{self, Write},
    net::IpAddr,
    path::Path,
    time::Duration,
};
#[cfg(feature = "hid")]
use unbound_protocol::Result;
use unbound_protocol::{
    one::{ConnectionOptions, ControlClient, Dimmer, InputMode, ReportsClient},
    wire::{Frame, FrameReader},
};
#[path = "unbound-protocol/devices.rs"]
mod devices;

const HELP: &str = "unbound-protocol — independent AR glasses protocol tools

Usage:
  unbound-protocol [--host IP] [--source IP] [--timeout-ms N] info
  unbound-protocol [OPTIONS] config
  unbound-protocol [OPTIONS] brightness 0..9
  unbound-protocol [OPTIONS] dimmer 0..2
  unbound-protocol [OPTIONS] edid [SELECTOR]
  unbound-protocol [OPTIONS] input regular|sbs
  unbound-protocol [OPTIONS] proximity [on|off]
  unbound-protocol [OPTIONS] stabilization [on|off]
  unbound-protocol [OPTIONS] imu [COUNT]
  unbound-protocol [OPTIONS] capture FILE COUNT
  unbound-protocol decode FILE
  unbound-protocol list                 (enabled HID/USB/serial backends)
  unbound-protocol hid air|one|sensor PATH ACTION [VALUE]
  unbound-protocol hid light|light-sensor PATH ACTION [VALUE]
  unbound-protocol light MCU_PATH SENSOR_PATH imu COUNT
  unbound-protocol usb rokid|grawoow|grawoow-sensor BUS ADDRESS ACTION [VALUE]
  unbound-protocol mad-gaze PORT ACTION [VALUE]

HID actions: info, config (one/sensor), mode [BYTE] (air), brightness [LEVEL],
             usb-config (one), camera (one), imu COUNT (sensor).
PATH is an explicit HID interface path from list. Air MCU/IMU interfaces differ.
Light actions: info, mode [1..4], events COUNT; light-sensor: config.
USB actions: info, mode [BYTE], config (grawoow), imu COUNT (rokid/grawoow-sensor).
Mad Gaze actions: info, mode [0|1], brightness 1..7, imu COUNT, magnetometer.
Enable hid, libusb, and/or serial features for their respective transports.
Network commands use the One-family TCP protocol. Capture saves reassembled
wire frames, preserving unknown payloads; decode works offline. No automatic
USB reconfiguration, firmware writes, or network configuration is performed.";

fn main() {
    if let Err(e) = run() {
        if e.downcast_ref::<io::Error>()
            .is_some_and(|e| e.kind() == io::ErrorKind::BrokenPipe)
        {
            return;
        }
        eprintln!("unbound-protocol: {e}");
        std::process::exit(1);
    }
}
fn run() -> std::result::Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1).collect::<Vec<_>>();
    let mut options = ConnectionOptions::default();
    while args.first().is_some_and(|s| s.starts_with("--")) {
        let key = args.remove(0);
        if key == "--help" {
            println!("{HELP}");
            return Ok(());
        }
        let value = if args.is_empty() {
            return Err(format!("missing value for {key}").into());
        } else {
            args.remove(0)
        };
        match key.as_str() {
            "--host" => options.host = value.parse::<IpAddr>()?,
            "--source" => options.source = Some(value.parse::<IpAddr>()?),
            "--timeout-ms" => {
                options.timeout = Duration::from_millis(value.parse()?);
                if options.timeout.is_zero() {
                    return Err("timeout must be positive".into());
                }
            }
            _ => return Err(format!("unknown option {key}").into()),
        }
    }
    if args.is_empty() || args[0] == "help" {
        println!("{HELP}");
        return Ok(());
    }
    let command = args.remove(0);
    // Validate arguments before any connection or write.
    match command.as_str() {
        "decode" => {
            arity(&args, 1, 1)?;
            return decode(Path::new(&args[0]));
        }
        "list" => {
            arity(&args, 0, 0)?;
            return list();
        }
        "hid" => return hid(&args, options.timeout),
        "usb" => return devices::usb(&args, options.timeout),
        "mad-gaze" => return devices::mad_gaze(&args, options.timeout),
        "light" => return devices::light_imu(&args, options.timeout),
        "info" | "config" => arity(&args, 0, 0)?,
        "brightness" => {
            arity(&args, 1, 1)?;
            bounded(&args[0], 9)?;
        }
        "dimmer" => {
            arity(&args, 1, 1)?;
            bounded(&args[0], 2)?;
        }
        "edid" => {
            arity(&args, 0, 1)?;
            if let Some(value) = args.first() {
                value.parse::<u64>()?;
            }
        }
        "input" => {
            arity(&args, 1, 1)?;
            if !matches!(args[0].as_str(), "regular" | "sbs") {
                return Err("input must be regular or sbs".into());
            }
        }
        "proximity" | "stabilization" => {
            arity(&args, 0, 1)?;
            if let Some(value) = args.first() {
                onoff(value)?;
            }
        }
        "imu" => arity(&args, 0, 1)?,
        "capture" => arity(&args, 2, 2)?,
        _ => return Err(format!("unknown command {command}; run unbound-protocol --help").into()),
    }
    let stdout = io::stdout();
    let mut out = stdout.lock();
    if command == "imu" || command == "capture" {
        let count = if command == "capture" {
            positive(&args[1])?
        } else {
            args.first()
                .map(|s| positive(s))
                .transpose()?
                .unwrap_or(100)
        };
        // Refuse to overwrite an existing capture.
        let mut file = if command == "capture" {
            Some(
                File::options()
                    .write(true)
                    .create_new(true)
                    .open(&args[0])?,
            )
        } else {
            None
        };
        let mut client = ReportsClient::connect(&options)?;
        for _ in 0..count {
            let frame = client.next_frame()?;
            if let Some(file) = &mut file {
                file.write_all(&frame.encode()?)?;
            } else {
                display_frame(&mut out, &frame)?;
            }
        }
        return Ok(());
    }
    let mut client = ControlClient::connect(&options)?;
    match command.as_str() {
        "info" => {
            writeln!(out, "software: {}", client.software_version()?)?;
            writeln!(out, "dsp: {}", client.dsp_version()?)?;
            writeln!(out, "serial: {}", client.serial()?)?;
        }
        "config" => writeln!(out, "{}", client.configuration()?)?,
        "brightness" => client.set_brightness(args[0].parse()?)?,
        "dimmer" => client.set_dimmer(match args[0].parse::<u8>()? {
            0 => Dimmer::Clear,
            1 => Dimmer::Middle,
            _ => Dimmer::Dark,
        })?,
        "edid" => {
            if let Some(value) = args.first() {
                client.set_edid(value.parse()?)?;
            } else {
                writeln!(out, "{}", client.edid()?)?;
            }
        }
        "input" => client.set_input_mode(if args[0] == "sbs" {
            InputMode::SideBySide
        } else {
            InputMode::Regular
        })?,
        "proximity" => {
            if let Some(value) = args.first() {
                client.set_proximity_enabled(onoff(value)?)?;
            } else {
                writeln!(out, "{}", client.proximity_enabled()?)?;
            }
        }
        "stabilization" => {
            if let Some(value) = args.first() {
                client.set_stabilization_enabled(onoff(value)?)?;
            } else {
                writeln!(out, "{}", client.stabilization_enabled()?)?;
            }
        }
        _ => unreachable!(),
    }
    Ok(())
}
fn arity(args: &[String], min: usize, max: usize) -> std::result::Result<(), Box<dyn Error>> {
    if (min..=max).contains(&args.len()) {
        Ok(())
    } else {
        Err("wrong number of arguments; run unbound-protocol --help".into())
    }
}
fn bounded(value: &str, max: u8) -> std::result::Result<u8, Box<dyn Error>> {
    let value: u8 = value.parse()?;
    if value > max {
        return Err(format!("value must be 0..{max}").into());
    }
    Ok(value)
}
fn positive(value: &str) -> std::result::Result<usize, Box<dyn Error>> {
    let value = value.parse()?;
    if value == 0 {
        return Err("count must be positive".into());
    }
    Ok(value)
}
fn onoff(value: &str) -> std::result::Result<bool, Box<dyn Error>> {
    match value {
        "on" => Ok(true),
        "off" => Ok(false),
        _ => Err("expected on or off".into()),
    }
}
fn display_frame(out: &mut impl Write, frame: &Frame) -> std::result::Result<(), Box<dyn Error>> {
    if frame.opcode == unbound_protocol::one::opcode::SENSOR_REPORT {
        writeln!(
            out,
            "{:?}",
            unbound_protocol::one::SensorReport::decode(frame)?
        )?;
    } else {
        writeln!(out, "opcode={:04x} body={:02x?}", frame.opcode, frame.body)?;
    }
    Ok(())
}
fn decode(path: &Path) -> std::result::Result<(), Box<dyn Error>> {
    let mut file = File::open(path)?;
    let size = file.metadata()?.len();
    let mut reader = FrameReader::default();
    let mut consumed = 0;
    let stdout = io::stdout();
    let mut out = stdout.lock();
    while consumed < size {
        let frame = reader.read(&mut file)?;
        consumed += 6 + frame.body.len() as u64;
        display_frame(&mut out, &frame)?;
    }
    Ok(())
}

#[cfg(not(feature = "hid"))]
fn list() -> std::result::Result<(), Box<dyn Error>> {
    devices::list_extra()
}
#[cfg(feature = "hid")]
fn list() -> std::result::Result<(), Box<dyn Error>> {
    let api = hidapi::HidApi::new()?;
    for device in api.device_list() {
        if let Some(identity) =
            unbound_protocol::device::identify(device.vendor_id(), device.product_id())
        {
            println!(
                "{:?} recovery={} {:04x}:{:04x} interface={} usage={:04x}:{:04x} path={}",
                identity.model,
                identity.recovery,
                device.vendor_id(),
                device.product_id(),
                device.interface_number(),
                device.usage_page(),
                device.usage(),
                device.path().to_string_lossy()
            );
        }
    }
    devices::list_extra()
}
#[cfg(not(feature = "hid"))]
fn hid(_: &[String], _: Duration) -> std::result::Result<(), Box<dyn Error>> {
    Err("rebuild with --features hid".into())
}
#[cfg(feature = "hid")]
fn hid(args: &[String], timeout: Duration) -> std::result::Result<(), Box<dyn Error>> {
    use unbound_protocol::{
        air::{DisplayMode, SensorClient},
        hid::HidTransport,
        usb::{Profile, UsbClient},
    };
    if args
        .first()
        .is_some_and(|s| matches!(s.as_str(), "light" | "light-sensor"))
    {
        return devices::light_hid(args, timeout);
    }
    arity(args, 3, 4)?;
    let profile = match args[0].as_str() {
        "air" | "sensor" => Profile::Air,
        "one" => Profile::One,
        _ => return Err("HID family must be air, one, or sensor".into()),
    };
    let action = args[2].as_str();
    match (args[0].as_str(), action) {
        ("sensor", "imu") => {
            arity(args, 4, 4)?;
            positive(&args[3])?;
        }
        ("sensor", "config")
        | ("one", "config" | "usb-config" | "camera")
        | ("air" | "one", "info") => arity(args, 3, 3)?,
        ("air", "mode") => {
            if args.len() == 4 {
                air_mode(&args[3])?;
            }
        }
        ("air" | "one", "brightness") => {
            if args.len() == 4 {
                bounded(&args[3], 9)?;
            }
        }
        _ => {
            return Err(
                "unsupported HID action for this family; run unbound-protocol --help".into(),
            );
        }
    }
    let api = hidapi::HidApi::new()?;
    let path = std::ffi::CString::new(args[1].as_str())?;
    let device = api
        .device_list()
        .find(|device| device.path() == path.as_c_str())
        .ok_or("HID path is not in the current device inventory")?;
    let identity = unbound_protocol::device::identify(device.vendor_id(), device.product_id())
        .ok_or("unrecognized glasses USB identity")?;
    if identity.recovery {
        return Err("recovery/bootloader protocols are not implemented".into());
    }
    if (profile == Profile::Air && !identity.model.is_air())
        || (profile == Profile::One && !identity.model.is_one())
    {
        return Err("HID family does not match the selected device".into());
    }
    if let Some((mcu, sensor)) = identity.model.air_interfaces() {
        let expected = if args[0] == "sensor" { sensor } else { mcu };
        if device.interface_number() >= 0 && device.interface_number() != expected {
            return Err(format!("select HID interface {expected} for this action").into());
        }
    }
    let transport = HidTransport::open(&api, &path, profile)?;
    if args[0] == "sensor" {
        let mut client = SensorClient::new(transport, timeout)?;
        if action == "config" {
            client.set_streaming(false)?;
            println!("{}", client.read_configuration()?);
        } else {
            client.set_streaming(true)?;
            let result = (|| -> Result<()> {
                for _ in 0..positive(&args[3]).unwrap() {
                    println!("{:?}", client.next_sample()?);
                }
                Ok(())
            })();
            let stop = client.set_streaming(false);
            result?;
            stop?;
        }
        return Ok(());
    }
    let mut client = UsbClient::new(transport, profile, timeout)?;
    match action {
        "info" => {
            println!("mcu: {}", client.mcu_version()?);
            println!("dsp: {}", client.dsp_version()?);
            println!("serial: {}", client.serial()?);
        }
        "config" | "usb-config" => println!("{:08x}", client.usb_configuration()?.0),
        "camera" => println!("{}", client.camera_plugged()?),
        "brightness" => {
            if args.len() == 4 {
                client.set_brightness(args[3].parse()?)?;
            } else {
                println!("{}", client.brightness()?);
            }
        }
        "mode" => {
            if args.len() == 4 {
                client.set_air_display_mode(air_mode(&args[3])?)?;
            } else {
                println!("{}", client.air_display_mode()?);
            }
        }
        _ => unreachable!(),
    }
    fn air_mode(value: &str) -> std::result::Result<DisplayMode, Box<dyn Error>> {
        Ok(match value.parse::<u8>()? {
            1 => DisplayMode::Mirror60,
            3 => DisplayMode::Stereo60,
            4 => DisplayMode::Stereo72,
            5 => DisplayMode::Mirror72,
            8 => DisplayMode::HalfStereo60,
            9 => DisplayMode::Stereo90,
            10 => DisplayMode::Mirror90,
            11 => DisplayMode::Mirror120,
            _ => return Err("unknown Air display mode".into()),
        })
    }
    Ok(())
}
