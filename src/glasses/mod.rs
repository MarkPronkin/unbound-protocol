//! Application-facing API shared by all supported glasses families.
//!
//! Connections select explicit endpoints. Capabilities describe the connected
//! transport, not a promise that every firmware implements a command. Opening
//! a connection never changes display settings or initializes sensors.
//!
//! ```no_run
//! use std::time::Duration;
//! use unbound_protocol::{Result, glasses::{Connection, Glasses}};
//! # fn main() -> Result<()> {
//! let mut glasses = Glasses::connect(Connection::default(), Duration::from_secs(2))?;
//! println!("{:?}: {}", glasses.model(), glasses.serial()?);
//! if glasses.capabilities().brightness.can_write() {
//!     glasses.set_brightness(0.5)?;
//! }
//! glasses.start_sensors()?;
//! let report = glasses.next_report()?;
//! if let Some(sample) = report.sensors {
//!     println!("gyro (rad/s): {:?}", sample.gyro_rad_s);
//! }
//! # Ok(()) }
//! ```

mod backend;
mod connection;
mod report;

pub use connection::{Connection, DeviceInfo, Endpoint, InterfaceRole, UsbAddress, discover};
pub use report::{
    Event, MagneticField, MagneticFieldUnit, RawReport, Report, SensorSample, TimeUnit,
    TimedVector, Timestamp,
};

use crate::{Error, Result, device::Model, one, usb::ReportIo, usb::control::ControlIo};
use backend::Driver;
use std::time::Duration;

/// Implemented access through this particular connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Access {
    #[default]
    Unsupported,
    ReadOnly,
    WriteOnly,
    ReadWrite,
}
impl Access {
    pub fn can_read(self) -> bool {
        matches!(self, Self::ReadOnly | Self::ReadWrite)
    }
    pub fn can_write(self) -> bool {
        matches!(self, Self::WriteOnly | Self::ReadWrite)
    }
}

/// Common display layouts. Modes without a refresh suffix select the family's
/// ordinary mirror/stereo timing; they do not imply a universal refresh rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DisplayMode {
    Mirror,
    Stereo,
    HalfStereo,
    Mirror72,
    Mirror90,
    Mirror120,
    Stereo72,
    Stereo90,
    /// Rokid's high-refresh selector; its exact rate is firmware dependent.
    MirrorHighRefresh,
    StereoHighRefresh,
    /// Unrecognized native readback. Cannot be written through this API.
    Unknown(u8),
}

/// Availability reflects both the family and the interfaces actually attached.
/// Device rejection/status errors are still returned unchanged by operations.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Capabilities {
    pub firmware_version: bool,
    pub configuration: bool,
    pub brightness: Access,
    pub display_mode: Access,
    pub display_modes: &'static [DisplayMode],
    pub sensors: bool,
    /// Scaled magnetic readings, with their unit specified in each report.
    pub magnetometer: bool,
    /// Mad Gaze exposes a separate magnetometer poll rather than a stream.
    pub magnetometer_poll: bool,
    /// An explicit command can disable sensor streaming on this connection.
    pub stop_sensors: bool,
    /// A separate MCU event channel is available through `next_event`.
    /// Rokid device state instead accompanies `next_report`.
    pub events: bool,
    /// Light can explicitly enable/disable MCU event generation.
    pub event_control: bool,
    pub proximity_setting: Access,
    pub stabilization: Access,
    /// Light needs `service` at least every 250 ms, including when idle.
    pub requires_service: bool,
}

/// One non-generic handle for application code. Operations are synchronous and
/// serialized by `&mut self`. The handle can be moved to an application worker.
pub struct Glasses {
    model: Model,
    driver: Driver,
}

impl Glasses {
    /// Open only the selected endpoints. No display/sensor commands are sent.
    pub fn connect(connection: Connection, timeout: Duration) -> Result<Self> {
        if timeout.is_zero() {
            return Err(Error::Invalid("zero glasses timeout"));
        }
        connection::connect(connection, timeout)
    }

    /// Attach an already connected One TCP control client and optional reports
    /// client, including streams bound by an Android/platform integration.
    pub fn from_one_tcp(
        model: Model,
        control: one::ControlClient,
        reports: Option<one::ReportsClient>,
    ) -> Result<Self> {
        require_one(model)?;
        Ok(Self {
            model,
            driver: Driver::OneTcp { control, reports },
        })
    }

    /// Use a custom HID/report transport for any One variant or ROG R1.
    pub fn from_one_usb(
        model: Model,
        control: impl ReportIo + Send + 'static,
        timeout: Duration,
    ) -> Result<Self> {
        require_one(model)?;
        Ok(Self {
            model,
            driver: Driver::one_usb(control, timeout)?,
        })
    }

    /// Attach explicit Air MCU and optional sensor transports. The model must
    /// be an Air variant. Use `Some(Box::new(io))` to attach a sensor interface.
    pub fn from_air(
        model: Model,
        control: impl ReportIo + Send + 'static,
        sensor: Option<Box<dyn ReportIo + Send>>,
        timeout: Duration,
    ) -> Result<Self> {
        if !model.is_air() {
            return Err(Error::Invalid("an Air model is required"));
        }
        Ok(Self {
            model,
            driver: Driver::air(control, sensor, timeout)?,
        })
    }

    pub fn from_light(
        control: impl ReportIo + Send + 'static,
        sensor: Option<Box<dyn ReportIo + Send>>,
        timeout: Duration,
    ) -> Result<Self> {
        Ok(Self {
            model: Model::Light,
            driver: Driver::light(control, sensor, timeout)?,
        })
    }

    pub fn from_rokid(io: impl ControlIo + Send + 'static, timeout: Duration) -> Result<Self> {
        Ok(Self {
            model: Model::RokidAirMax,
            driver: Driver::rokid(io, timeout)?,
        })
    }

    pub fn from_grawoow(
        control: impl ControlIo + Send + 'static,
        sensor: Option<Box<dyn ControlIo + Send>>,
        timeout: Duration,
    ) -> Result<Self> {
        Ok(Self {
            model: Model::GrawoowG530,
            driver: Driver::grawoow(control, sensor, timeout)?,
        })
    }

    pub fn from_mad_gaze(io: impl ReportIo + Send + 'static, timeout: Duration) -> Result<Self> {
        Ok(Self {
            model: Model::MadGazeGlow,
            driver: Driver::mad_gaze(io, timeout)?,
        })
    }

    pub fn model(&self) -> Model {
        self.model
    }

    pub fn capabilities(&self) -> Capabilities {
        self.driver.capabilities()
    }

    pub fn serial(&mut self) -> Result<String> {
        self.driver.serial()
    }

    /// One software version, or the MCU firmware version for USB connections.
    pub fn firmware_version(&mut self) -> Result<String> {
        self.driver.firmware_version()
    }

    /// Device-provided configuration/calibration JSON. Stop Air/Light sensor
    /// streaming before calling; this method never stops/restarts it implicitly.
    pub fn configuration(&mut self) -> Result<String> {
        self.driver.configuration()
    }

    /// Normalized brightness in 0.0..=1.0. The device's native discrete range
    /// is mapped linearly. Unsupported reads never return a cached value.
    pub fn brightness(&mut self) -> Result<f32> {
        self.driver.brightness()
    }

    /// Map a finite 0.0..=1.0 value to the nearest native brightness step.
    /// XREAL uses 0..9; Mad Gaze uses 1..7. No retries are performed.
    pub fn set_brightness(&mut self, brightness: f32) -> Result<()> {
        if !brightness.is_finite() || !(0.0..=1.0).contains(&brightness) {
            return Err(Error::Invalid("brightness must be finite and in 0..1"));
        }
        self.driver.set_brightness(brightness)
    }

    pub fn display_mode(&mut self) -> Result<DisplayMode> {
        self.driver.display_mode()
    }

    /// Validate against this connection's supported modes before sending any
    /// commands. One selects the input layout, leaving its EDID unchanged.
    /// Light's stereo-presence handshake is performed when required.
    pub fn set_display_mode(&mut self, mode: DisplayMode) -> Result<()> {
        if !self.capabilities().display_modes.contains(&mode) {
            return Err(Error::Unsupported("display mode"));
        }
        self.driver.set_display_mode(mode)
    }

    /// Enable Air/Light streaming, initialize Mad Gaze sensors, or do nothing
    /// for already streaming One/Rokid/Grawoow sensors. Explicitly opt in here;
    /// construction does not reset sensors. Repeated successful starts are
    /// idempotent, including Mad Gaze's sensor reset sequence.
    pub fn start_sensors(&mut self) -> Result<()> {
        self.driver.start_sensors()
    }

    /// Disable Air/Light streams. Other families return `Error::Unsupported`.
    pub fn stop_sensors(&mut self) -> Result<()> {
        self.driver.stop_sensors()
    }

    /// Read one sensor-channel report, including unknown packets and Rokid
    /// device-state packets. Mad Gaze's two-entry FIFO is drained in order;
    /// an empty FIFO returns a report with no sensors and `RawReport::Empty`.
    /// Call `start_sensors` first on Air, Light, and Mad Gaze.
    pub fn next_report(&mut self) -> Result<Report> {
        self.driver.next_report()
    }

    /// Read the separate control event channel. This does not multiplex sensor
    /// reads; Rokid state and One report-port events arrive via `next_report`.
    pub fn next_event(&mut self) -> Result<Report> {
        self.driver.next_event()
    }

    /// Enable/disable Light MCU event generation. Other protocols expose no
    /// corresponding switch and return `Error::Unsupported`.
    pub fn set_events_enabled(&mut self, enabled: bool) -> Result<()> {
        self.driver.set_events_enabled(enabled)
    }

    /// Poll Mad Gaze's separate magnetometer. Data-ready and overflow flags
    /// govern sample availability and are always retained in `Report::raw`.
    pub fn poll_magnetometer(&mut self) -> Result<Report> {
        self.driver.poll_magnetometer()
    }

    /// Maintain Light's heartbeat. Call at least every 250 ms while using it;
    /// `next_report` and `next_event` also service it during bounded waits.
    /// Other families require no maintenance and return success.
    pub fn service(&mut self) -> Result<()> {
        self.driver.service()
    }

    pub fn proximity_enabled(&mut self) -> Result<bool> {
        self.driver.proximity_enabled()
    }
    pub fn set_proximity_enabled(&mut self, enabled: bool) -> Result<()> {
        self.driver.set_proximity_enabled(enabled)
    }
    pub fn stabilization_enabled(&mut self) -> Result<bool> {
        self.driver.stabilization_enabled()
    }
    pub fn set_stabilization_enabled(&mut self, enabled: bool) -> Result<()> {
        self.driver.set_stabilization_enabled(enabled)
    }
}

fn require_one(model: Model) -> Result<()> {
    if model.is_one() {
        Ok(())
    } else {
        Err(Error::Invalid("a One-family model is required"))
    }
}
