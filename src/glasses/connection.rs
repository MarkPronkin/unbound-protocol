use super::Glasses;
use crate::{
    Error, Result,
    device::{Identity, Model},
    one,
};
use std::{ffi::CString, net::IpAddr, time::Duration};

/// An explicit current USB location. Rediscover after unplug/re-enumeration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsbAddress {
    pub bus: u8,
    pub address: u8,
}

/// Connection recipes use owned endpoints and are available with any feature
/// set. A missing native backend produces `Error::Unsupported` on connect.
/// Separate MCU/sensor devices must be paired explicitly by the application.
#[derive(Debug, Clone)]
pub enum Connection {
    OneTcp {
        model: Model,
        host: IpAddr,
        source: Option<IpAddr>,
        reports: bool,
    },
    Hid {
        model: Model,
        control: CString,
        sensor: Option<CString>,
    },
    RokidUsb {
        device: UsbAddress,
    },
    GrawoowUsb {
        control: UsbAddress,
        sensor: Option<UsbAddress>,
    },
    MadGazeSerial {
        port: String,
    },
}
impl Default for Connection {
    fn default() -> Self {
        Self::OneTcp {
            model: Model::One,
            host: one::GLASSES_IP.into(),
            source: None,
            reports: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterfaceRole {
    Control,
    Sensor,
    ControlAndSensor,
    /// In particular, One composite HID usages must be selected explicitly.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    Hid {
        path: CString,
        interface: i32,
        usage_page: u16,
        usage: u16,
    },
    Usb(UsbAddress),
    Serial(String),
}

/// One enumerated endpoint, not necessarily one physical pair of glasses.
/// Recognition and capabilities do not establish hardware/firmware validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    pub identity: Identity,
    pub vendor_id: u16,
    pub product_id: u16,
    pub product_name: Option<String>,
    pub serial_number: Option<String>,
    pub role: InterfaceRole,
    pub endpoint: Endpoint,
}

/// Enumerate recognized local endpoints through enabled `hid`, `libusb`, and
/// `serial` backends. Does not open devices, probe TCP, or change any settings.
/// With no native features this returns an empty inventory. Multiple backends
/// can list the same hardware; records are never guessed into physical pairs.
pub fn discover() -> Result<Vec<DeviceInfo>> {
    #[allow(unused_mut)]
    let mut devices = Vec::new();
    #[cfg(feature = "hid")]
    {
        let api = hidapi::HidApi::new()?;
        for device in api.device_list() {
            let Some(identity) = crate::device::identify(device.vendor_id(), device.product_id())
            else {
                continue;
            };
            // Other families use USB control/interrupt or serial access here.
            if !identity.model.is_air()
                && !identity.model.is_one()
                && identity.model != Model::Light
            {
                continue;
            }
            devices.push(DeviceInfo {
                identity,
                vendor_id: device.vendor_id(),
                product_id: device.product_id(),
                product_name: device.product_string().map(str::to_owned),
                serial_number: device.serial_number().map(str::to_owned),
                role: hid_role(
                    identity.model,
                    device.product_id(),
                    device.interface_number(),
                ),
                endpoint: Endpoint::Hid {
                    path: device.path().to_owned(),
                    interface: device.interface_number(),
                    usage_page: device.usage_page(),
                    usage: device.usage(),
                },
            });
        }
    }
    #[cfg(feature = "libusb")]
    for device in rusb::devices()?.iter() {
        let Ok(desc) = device.device_descriptor() else {
            continue;
        };
        let Some(identity) = crate::device::identify(desc.vendor_id(), desc.product_id()) else {
            continue;
        };
        let role = match (desc.vendor_id(), desc.product_id()) {
            (crate::rokid::VID, crate::rokid::PID) => InterfaceRole::ControlAndSensor,
            crate::grawoow::MCU_ID => InterfaceRole::Control,
            crate::grawoow::SENSOR_ID => InterfaceRole::Sensor,
            _ => continue,
        };
        devices.push(DeviceInfo {
            identity,
            vendor_id: desc.vendor_id(),
            product_id: desc.product_id(),
            product_name: None,
            serial_number: None,
            role,
            endpoint: Endpoint::Usb(UsbAddress {
                bus: device.bus_number(),
                address: device.address(),
            }),
        });
    }
    #[cfg(feature = "serial")]
    for port in serialport::available_ports()? {
        let serialport::SerialPortType::UsbPort(info) = port.port_type else {
            continue;
        };
        if (info.vid, info.pid) != (crate::mad_gaze::VID, crate::mad_gaze::PID) {
            continue;
        }
        devices.push(DeviceInfo {
            identity: Identity {
                model: Model::MadGazeGlow,
                recovery: false,
            },
            vendor_id: info.vid,
            product_id: info.pid,
            product_name: info.product,
            serial_number: info.serial_number,
            role: InterfaceRole::ControlAndSensor,
            endpoint: Endpoint::Serial(port.port_name),
        });
    }
    Ok(devices)
}

pub(super) fn connect(connection: Connection, timeout: Duration) -> Result<Glasses> {
    match connection {
        Connection::OneTcp {
            model,
            host,
            source,
            reports,
        } => {
            super::require_one(model)?;
            let options = one::ConnectionOptions {
                host,
                source,
                timeout,
            };
            let control = one::ControlClient::connect(&options)?;
            let reports = reports
                .then(|| one::ReportsClient::connect(&options))
                .transpose()?;
            Glasses::from_one_tcp(model, control, reports)
        }
        Connection::Hid {
            model,
            control,
            sensor,
        } => connect_hid(model, control, sensor, timeout),
        Connection::RokidUsb { device } => connect_rokid(device, timeout),
        Connection::GrawoowUsb { control, sensor } => connect_grawoow(control, sensor, timeout),
        Connection::MadGazeSerial { port } => connect_serial(port, timeout),
    }
}

#[cfg(feature = "hid")]
fn hid_role(model: Model, product: u16, interface: i32) -> InterfaceRole {
    if let Some((control, sensor)) = model.air_interfaces() {
        if interface == control {
            return InterfaceRole::Control;
        }
        if interface == sensor {
            return InterfaceRole::Sensor;
        }
    }
    if model == Model::Light {
        return if product == crate::light::MCU_ID.1 {
            InterfaceRole::Control
        } else {
            InterfaceRole::Sensor
        };
    }
    InterfaceRole::Unknown
}

#[cfg(feature = "hid")]
fn connect_hid(
    model: Model,
    control: CString,
    sensor: Option<CString>,
    timeout: Duration,
) -> Result<Glasses> {
    use crate::{
        hid::{HidTransport, ReportPrefix},
        usb::{Profile, ReportIo},
    };
    if !model.is_air() && !model.is_one() && model != Model::Light {
        return Err(Error::Unsupported("HID connection for this model"));
    }
    if model.is_one() && sensor.is_some() {
        return Err(Error::Unsupported("One sensors require TCP reports"));
    }
    if sensor.as_ref() == Some(&control) {
        return Err(Error::Invalid("MCU and sensor HID paths must differ"));
    }
    let api = hidapi::HidApi::new()?;
    let validate = |path: &CString, role| -> Result<&hidapi::DeviceInfo> {
        let device = api
            .device_list()
            .find(|device| device.path() == path.as_c_str())
            .ok_or(Error::Disconnected)?;
        let identity = crate::device::identify(device.vendor_id(), device.product_id())
            .ok_or(Error::Invalid("unrecognized HID device"))?;
        if identity.model != model || identity.recovery {
            return Err(Error::Invalid("HID model mismatch or recovery device"));
        }
        let actual = hid_role(model, device.product_id(), device.interface_number());
        if actual != InterfaceRole::Unknown && actual != role {
            return Err(Error::Invalid("wrong HID interface role"));
        }
        Ok(device)
    };
    let control_info = validate(&control, InterfaceRole::Control)?;
    if let Some(path) = &sensor {
        let sensor_info = validate(path, InterfaceRole::Sensor)?;
        if model.is_air() {
            if let (Some(a), Some(b)) = (control_info.serial_number(), sensor_info.serial_number())
            {
                if !a.is_empty() && !b.is_empty() && a != b {
                    return Err(Error::Invalid("Air HID serial numbers differ"));
                }
            }
        }
    }
    if model == Model::Light {
        let control = HidTransport::open_with_prefix(&api, &control, ReportPrefix::ProtocolByte)?;
        let sensor = sensor
            .map(|path| {
                HidTransport::open_with_prefix(&api, &path, ReportPrefix::ProtocolByte)
                    .map(|io| Box::new(io) as Box<dyn ReportIo + Send>)
            })
            .transpose()?;
        Glasses::from_light(control, sensor, timeout)
    } else {
        let profile = if model.is_one() {
            Profile::One
        } else {
            Profile::Air
        };
        let control = HidTransport::open(&api, &control, profile)?;
        if model.is_one() {
            return Glasses::from_one_usb(model, control, timeout);
        }
        let sensor = sensor
            .map(|path| {
                HidTransport::open(&api, &path, profile)
                    .map(|io| Box::new(io) as Box<dyn ReportIo + Send>)
            })
            .transpose()?;
        Glasses::from_air(model, control, sensor, timeout)
    }
}
#[cfg(not(feature = "hid"))]
fn connect_hid(_: Model, _: CString, _: Option<CString>, _: Duration) -> Result<Glasses> {
    Err(Error::Unsupported("HID backend; enable the hid feature"))
}

#[cfg(feature = "libusb")]
fn validate_usb(address: UsbAddress, expected: (u16, u16)) -> Result<()> {
    let device = rusb::devices()?
        .iter()
        .find(|device| device.bus_number() == address.bus && device.address() == address.address)
        .ok_or(Error::Disconnected)?;
    let desc = device.device_descriptor()?;
    if (desc.vendor_id(), desc.product_id()) != expected {
        return Err(Error::Invalid("USB family/interface mismatch"));
    }
    Ok(())
}
#[cfg(feature = "libusb")]
fn connect_rokid(device: UsbAddress, timeout: Duration) -> Result<Glasses> {
    validate_usb(device, (crate::rokid::VID, crate::rokid::PID))?;
    Glasses::from_rokid(
        crate::libusb::UsbTransport::open_interrupt(
            device.bus,
            device.address,
            crate::rokid::ENDPOINT,
        )?,
        timeout,
    )
}
#[cfg(not(feature = "libusb"))]
fn connect_rokid(_: UsbAddress, _: Duration) -> Result<Glasses> {
    Err(Error::Unsupported("USB backend; enable the libusb feature"))
}
#[cfg(feature = "libusb")]
fn connect_grawoow(
    control: UsbAddress,
    sensor: Option<UsbAddress>,
    timeout: Duration,
) -> Result<Glasses> {
    use crate::{libusb::UsbTransport, usb::control::ControlIo};
    validate_usb(control, crate::grawoow::MCU_ID)?;
    if let Some(address) = sensor {
        validate_usb(address, crate::grawoow::SENSOR_ID)?;
    }
    let control = UsbTransport::open_interface(control.bus, control.address, 0)?;
    let sensor = sensor
        .map(|address| {
            UsbTransport::open_interrupt(address.bus, address.address, crate::grawoow::ENDPOINT)
                .map(|io| Box::new(io) as Box<dyn ControlIo + Send>)
        })
        .transpose()?;
    Glasses::from_grawoow(control, sensor, timeout)
}
#[cfg(not(feature = "libusb"))]
fn connect_grawoow(_: UsbAddress, _: Option<UsbAddress>, _: Duration) -> Result<Glasses> {
    Err(Error::Unsupported("USB backend; enable the libusb feature"))
}
#[cfg(feature = "serial")]
fn connect_serial(port: String, timeout: Duration) -> Result<Glasses> {
    Glasses::from_mad_gaze(
        crate::serial::SerialTransport::open(&port, timeout)?,
        timeout,
    )
}
#[cfg(not(feature = "serial"))]
fn connect_serial(_: String, _: Duration) -> Result<Glasses> {
    Err(Error::Unsupported(
        "serial backend; enable the serial feature",
    ))
}
