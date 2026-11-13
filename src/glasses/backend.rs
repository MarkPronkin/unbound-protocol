use super::{Access, Capabilities, DisplayMode, RawReport, Report};
use crate::{
    Error, Result, air, grawoow, light, mad_gaze, one, rokid,
    usb::{
        Profile, ReportIo, UsbClient,
        control::{ControlIo, Setup},
    },
};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant},
};

pub(super) struct Reports(Box<dyn ReportIo + Send>);
impl ReportIo for Reports {
    fn write_report(&mut self, report: &[u8], timeout: Duration) -> Result<()> {
        self.0.write_report(report, timeout)
    }
    fn read_report(&mut self, buffer: &mut [u8], timeout: Duration) -> Result<usize> {
        self.0.read_report(buffer, timeout)
    }
}
pub(super) struct Controls(Box<dyn ControlIo + Send>);
impl ControlIo for Controls {
    fn read_control(&mut self, setup: Setup, data: &mut [u8], timeout: Duration) -> Result<usize> {
        self.0.read_control(setup, data, timeout)
    }
    fn write_control(&mut self, setup: Setup, data: &[u8], timeout: Duration) -> Result<()> {
        self.0.write_control(setup, data, timeout)
    }
    fn read_interrupt(
        &mut self,
        endpoint: u8,
        data: &mut [u8],
        timeout: Duration,
    ) -> Result<usize> {
        self.0.read_interrupt(endpoint, data, timeout)
    }
}

type LightControl = Arc<Mutex<light::ControlClient<Reports>>>;
fn lock_light(control: &LightControl) -> Result<MutexGuard<'_, light::ControlClient<Reports>>> {
    control.lock().map_err(|_| Error::Disconnected)
}

/// Service the MCU during every sensor exchange, including configuration and
/// start/stop acknowledgements. This shares ownership, without a worker thread.
pub(super) struct LightReports {
    sensor: Reports,
    control: LightControl,
}
impl ReportIo for LightReports {
    fn write_report(&mut self, report: &[u8], timeout: Duration) -> Result<()> {
        lock_light(&self.control)?.heartbeat()?;
        self.sensor.write_report(report, timeout)
    }
    fn read_report(&mut self, buffer: &mut [u8], timeout: Duration) -> Result<usize> {
        let start = Instant::now();
        loop {
            let heartbeat_wait = {
                let mut control = lock_light(&self.control)?;
                control.heartbeat()?;
                control.heartbeat_due_in()
            };
            let wait = crate::error::remaining(start, timeout)?.min(heartbeat_wait);
            if wait.is_zero() {
                continue;
            }
            match self.sensor.read_report(buffer, wait) {
                Err(Error::Timeout) | Ok(0) => {}
                result => return result,
            }
        }
    }
}

pub(super) enum Driver {
    OneTcp {
        control: one::ControlClient,
        reports: Option<one::ReportsClient>,
    },
    OneUsb(UsbClient<Reports>),
    Air {
        control: UsbClient<Reports>,
        sensor: Option<air::SensorClient<Reports>>,
        streaming: bool,
    },
    Light {
        control: LightControl,
        sensor: Option<light::SensorClient<LightReports>>,
        streaming: bool,
    },
    Rokid(rokid::Client<Controls>),
    Grawoow {
        control: grawoow::ControlClient<Controls>,
        sensor: Option<grawoow::SensorClient<Controls>>,
    },
    MadGaze {
        client: mad_gaze::Client<Reports>,
        initialized: bool,
        pending: VecDeque<mad_gaze::ImuSample>,
    },
}

const BASIC_MODES: &[DisplayMode] = &[DisplayMode::Mirror, DisplayMode::Stereo];
const AIR_MODES: &[DisplayMode] = &[
    DisplayMode::Mirror,
    DisplayMode::Stereo,
    DisplayMode::HalfStereo,
    DisplayMode::Mirror72,
    DisplayMode::Mirror90,
    DisplayMode::Mirror120,
    DisplayMode::Stereo72,
    DisplayMode::Stereo90,
];
const LIGHT_MODES: &[DisplayMode] = &[
    DisplayMode::Mirror,
    DisplayMode::Stereo,
    DisplayMode::HalfStereo,
    DisplayMode::Stereo72,
];
const ROKID_MODES: &[DisplayMode] = &[
    DisplayMode::Mirror,
    DisplayMode::Stereo,
    DisplayMode::MirrorHighRefresh,
    DisplayMode::StereoHighRefresh,
];

impl Driver {
    pub(super) fn one_usb(io: impl ReportIo + Send + 'static, timeout: Duration) -> Result<Self> {
        Ok(Self::OneUsb(UsbClient::new(
            Reports(Box::new(io)),
            Profile::One,
            timeout,
        )?))
    }
    pub(super) fn air(
        control: impl ReportIo + Send + 'static,
        sensor: Option<Box<dyn ReportIo + Send>>,
        timeout: Duration,
    ) -> Result<Self> {
        Ok(Self::Air {
            control: UsbClient::new(Reports(Box::new(control)), Profile::Air, timeout)?,
            sensor: sensor
                .map(|io| air::SensorClient::new(Reports(io), timeout))
                .transpose()?,
            streaming: false,
        })
    }
    pub(super) fn light(
        control: impl ReportIo + Send + 'static,
        sensor: Option<Box<dyn ReportIo + Send>>,
        timeout: Duration,
    ) -> Result<Self> {
        let control = Arc::new(Mutex::new(light::ControlClient::new(
            Reports(Box::new(control)),
            timeout,
        )?));
        let sensor = sensor
            .map(|io| {
                light::SensorClient::new(
                    LightReports {
                        sensor: Reports(io),
                        control: Arc::clone(&control),
                    },
                    timeout,
                )
            })
            .transpose()?;
        Ok(Self::Light {
            control,
            sensor,
            streaming: false,
        })
    }
    pub(super) fn rokid(io: impl ControlIo + Send + 'static, timeout: Duration) -> Result<Self> {
        Ok(Self::Rokid(rokid::Client::new(
            Controls(Box::new(io)),
            timeout,
        )?))
    }
    pub(super) fn grawoow(
        control: impl ControlIo + Send + 'static,
        sensor: Option<Box<dyn ControlIo + Send>>,
        timeout: Duration,
    ) -> Result<Self> {
        Ok(Self::Grawoow {
            control: grawoow::ControlClient::new(Controls(Box::new(control)), timeout)?,
            sensor: sensor
                .map(|io| grawoow::SensorClient::new(Controls(io), timeout))
                .transpose()?,
        })
    }
    pub(super) fn mad_gaze(io: impl ReportIo + Send + 'static, timeout: Duration) -> Result<Self> {
        Ok(Self::MadGaze {
            client: mad_gaze::Client::new(Reports(Box::new(io)), timeout)?,
            initialized: false,
            pending: VecDeque::new(),
        })
    }

    pub(super) fn capabilities(&self) -> Capabilities {
        use Access::{ReadWrite, WriteOnly};
        let mut caps = Capabilities::default();
        match self {
            Self::OneTcp { reports, .. } => {
                caps.firmware_version = true;
                caps.configuration = true;
                caps.brightness = WriteOnly;
                caps.display_mode = WriteOnly;
                caps.display_modes = BASIC_MODES;
                caps.sensors = reports.is_some();
                caps.magnetometer = reports.is_some();
                caps.events = true;
                caps.proximity_setting = ReadWrite;
                caps.stabilization = ReadWrite;
            }
            Self::OneUsb(_) => {
                caps.firmware_version = true;
                caps.brightness = ReadWrite;
                caps.events = true;
            }
            Self::Air { sensor, .. } => {
                caps.firmware_version = true;
                caps.configuration = sensor.is_some();
                caps.brightness = ReadWrite;
                caps.display_mode = ReadWrite;
                caps.display_modes = AIR_MODES;
                caps.sensors = sensor.is_some();
                caps.stop_sensors = sensor.is_some();
                caps.events = true;
            }
            Self::Light { sensor, .. } => {
                caps.configuration = sensor.is_some();
                caps.display_mode = ReadWrite;
                caps.display_modes = LIGHT_MODES;
                caps.sensors = sensor.is_some();
                caps.stop_sensors = sensor.is_some();
                caps.events = true;
                caps.event_control = true;
                caps.requires_service = true;
            }
            Self::Rokid(_) => {
                caps.display_mode = ReadWrite;
                caps.display_modes = ROKID_MODES;
                caps.sensors = true;
                caps.magnetometer = true;
            }
            Self::Grawoow { sensor, .. } => {
                caps.configuration = true;
                caps.display_mode = ReadWrite;
                caps.display_modes = BASIC_MODES;
                caps.sensors = sensor.is_some();
            }
            Self::MadGaze { .. } => {
                caps.brightness = WriteOnly;
                caps.display_mode = ReadWrite;
                caps.display_modes = BASIC_MODES;
                caps.sensors = true;
                caps.magnetometer = true;
                caps.magnetometer_poll = true;
            }
        }
        caps
    }

    pub(super) fn serial(&mut self) -> Result<String> {
        match self {
            Self::OneTcp { control, .. } => control.serial(),
            Self::OneUsb(control) | Self::Air { control, .. } => control.serial(),
            Self::Light { control, .. } => lock_light(control)?.serial(),
            Self::Rokid(client) => client.serial(),
            Self::Grawoow { control, .. } => control.serial(),
            Self::MadGaze { client, .. } => client.serial(),
        }
    }
    pub(super) fn firmware_version(&mut self) -> Result<String> {
        match self {
            Self::OneTcp { control, .. } => control.software_version(),
            Self::OneUsb(control) | Self::Air { control, .. } => control.mcu_version(),
            _ => Err(Error::Unsupported("firmware version")),
        }
    }
    pub(super) fn configuration(&mut self) -> Result<String> {
        match self {
            Self::OneTcp { control, .. } => control.configuration(),
            Self::Air {
                sensor: Some(sensor),
                streaming,
                ..
            } => {
                require_stopped(*streaming)?;
                sensor.read_configuration()
            }
            Self::Light {
                sensor: Some(sensor),
                streaming,
                ..
            } => {
                require_stopped(*streaming)?;
                sensor.read_configuration()
            }
            Self::Grawoow { control, .. } => control.read_configuration(),
            _ => Err(Error::Unsupported("configuration")),
        }
    }
    pub(super) fn brightness(&mut self) -> Result<f32> {
        match self {
            Self::OneUsb(control) | Self::Air { control, .. } => {
                let level = control.brightness()?;
                if level > 9 {
                    return Err(Error::Invalid("brightness readback outside 0..9"));
                }
                Ok(f32::from(level) / 9.0)
            }
            _ => Err(Error::Unsupported("brightness read")),
        }
    }
    pub(super) fn set_brightness(&mut self, brightness: f32) -> Result<()> {
        let level = (brightness * 9.0).round() as u8;
        match self {
            Self::OneTcp { control, .. } => control.set_brightness(level),
            Self::OneUsb(control) | Self::Air { control, .. } => control.set_brightness(level),
            Self::MadGaze { client, .. } => {
                client.set_brightness(1 + (brightness * 6.0).round() as u8)
            }
            _ => Err(Error::Unsupported("brightness write")),
        }
    }
    pub(super) fn display_mode(&mut self) -> Result<DisplayMode> {
        use DisplayMode::*;
        Ok(match self {
            Self::Air { control, .. } => match control.air_display_mode()? {
                1 => Mirror,
                3 => Stereo,
                4 => Stereo72,
                5 => Mirror72,
                8 => HalfStereo,
                9 => Stereo90,
                10 => Mirror90,
                11 => Mirror120,
                raw => Unknown(raw),
            },
            Self::Light { control, .. } => {
                let value = lock_light(control)?.display_mode()?;
                if value.len() != 1 {
                    return Err(Error::Invalid("Light display mode length"));
                }
                match value[0] {
                    b'1' => Mirror,
                    b'2' => HalfStereo,
                    b'3' => Stereo,
                    b'4' => Stereo72,
                    raw => Unknown(raw),
                }
            }
            Self::Rokid(client) => match client.display_mode()? {
                0 => Mirror,
                1 => Stereo,
                2 => HalfStereo,
                3 => MirrorHighRefresh,
                4 => StereoHighRefresh,
                raw => Unknown(raw),
            },
            Self::Grawoow { control, .. } => binary_mode(control.display_mode()?),
            Self::MadGaze { client, .. } => binary_mode(client.display_mode()?),
            _ => return Err(Error::Unsupported("display mode read")),
        })
    }
    pub(super) fn set_display_mode(&mut self, mode: DisplayMode) -> Result<()> {
        use DisplayMode::*;
        match self {
            Self::OneTcp { control, .. } => control.set_input_mode(match mode {
                Mirror => one::InputMode::Regular,
                Stereo => one::InputMode::SideBySide,
                _ => return Err(Error::Unsupported("display mode")),
            }),
            Self::Air { control, .. } => control.set_air_display_mode(match mode {
                Mirror => air::DisplayMode::Mirror60,
                Stereo => air::DisplayMode::Stereo60,
                HalfStereo => air::DisplayMode::HalfStereo60,
                Mirror72 => air::DisplayMode::Mirror72,
                Mirror90 => air::DisplayMode::Mirror90,
                Mirror120 => air::DisplayMode::Mirror120,
                Stereo72 => air::DisplayMode::Stereo72,
                Stereo90 => air::DisplayMode::Stereo90,
                _ => return Err(Error::Unsupported("display mode")),
            }),
            Self::Light { control, .. } => {
                let mut control = lock_light(control)?;
                let native = match mode {
                    Mirror => light::DisplayMode::Mirror,
                    Stereo => light::DisplayMode::Stereo,
                    HalfStereo => light::DisplayMode::HalfStereo,
                    Stereo72 => light::DisplayMode::Stereo72,
                    _ => return Err(Error::Unsupported("display mode")),
                };
                if mode != Mirror {
                    control.enable_stereo()?;
                }
                control.set_display_mode(native)
            }
            Self::Rokid(client) => client.set_display_mode(match mode {
                Mirror => rokid::DisplayMode::Mirror,
                Stereo => rokid::DisplayMode::Stereo,
                MirrorHighRefresh => rokid::DisplayMode::MirrorHighRefresh,
                StereoHighRefresh => rokid::DisplayMode::StereoHighRefresh,
                _ => return Err(Error::Unsupported("display mode")),
            }),
            Self::Grawoow { control, .. } => control.set_stereo(mode == Stereo),
            Self::MadGaze { client, .. } => client.set_stereo(mode == Stereo),
            _ => Err(Error::Unsupported("display mode write")),
        }
    }

    pub(super) fn start_sensors(&mut self) -> Result<()> {
        match self {
            Self::Air {
                sensor: Some(sensor),
                streaming,
                ..
            } => {
                if !*streaming {
                    sensor.set_streaming(true)?;
                    *streaming = true;
                }
                Ok(())
            }
            Self::Light {
                control,
                sensor: Some(sensor),
                streaming,
                ..
            } => {
                lock_light(control)?.heartbeat()?;
                if !*streaming {
                    sensor.set_streaming(true)?;
                    *streaming = true;
                }
                Ok(())
            }
            Self::MadGaze {
                client,
                initialized,
                ..
            } => {
                if !*initialized {
                    client.initialize_sensors()?;
                    *initialized = true;
                }
                Ok(())
            }
            Self::OneTcp {
                reports: Some(_), ..
            }
            | Self::Rokid(_)
            | Self::Grawoow {
                sensor: Some(_), ..
            } => Ok(()),
            _ => Err(Error::Unsupported("sensors")),
        }
    }
    pub(super) fn stop_sensors(&mut self) -> Result<()> {
        match self {
            Self::Air {
                sensor: Some(sensor),
                streaming,
                ..
            } => {
                sensor.set_streaming(false)?;
                *streaming = false;
                Ok(())
            }
            Self::Light {
                control,
                sensor: Some(sensor),
                streaming,
                ..
            } => {
                lock_light(control)?.heartbeat()?;
                sensor.set_streaming(false)?;
                *streaming = false;
                Ok(())
            }
            _ => Err(Error::Unsupported("stop sensors")),
        }
    }
    pub(super) fn next_report(&mut self) -> Result<Report> {
        let raw = match self {
            Self::OneTcp {
                reports: Some(reports),
                ..
            } => RawReport::One(reports.next_frame()?),
            Self::Air {
                sensor: Some(sensor),
                streaming,
                ..
            } => {
                require_started(*streaming)?;
                RawReport::Air(sensor.next_sample()?)
            }
            Self::Light {
                sensor: Some(sensor),
                streaming,
                ..
            } => {
                require_started(*streaming)?;
                RawReport::Light(sensor.next_sample()?)
            }
            Self::Rokid(client) => RawReport::Rokid(client.next_packet()?),
            Self::Grawoow {
                sensor: Some(sensor),
                ..
            } => RawReport::Grawoow(sensor.next_sample()?),
            Self::MadGaze {
                client,
                initialized,
                pending,
            } => {
                require_started(*initialized)?;
                if pending.is_empty() {
                    pending.extend(client.read_imu()?);
                }
                match pending.pop_front() {
                    Some(sample) => RawReport::MadGaze(sample),
                    None => RawReport::Empty,
                }
            }
            _ => return Err(Error::Unsupported("sensor reports")),
        };
        Report::from_raw(raw)
    }
    pub(super) fn next_event(&mut self) -> Result<Report> {
        let raw = match self {
            Self::OneTcp { control, .. } => RawReport::One(control.next_event()?),
            Self::OneUsb(control) | Self::Air { control, .. } => {
                RawReport::XrealUsb(control.next_event()?)
            }
            Self::Light { control, .. } => {
                RawReport::LightEvent(lock_light(control)?.next_event()?)
            }
            _ => {
                return Err(Error::Unsupported(
                    "control events; read reports for Rokid state",
                ));
            }
        };
        Report::from_raw(raw)
    }
    pub(super) fn set_events_enabled(&mut self, enabled: bool) -> Result<()> {
        match self {
            Self::Light { control, .. } => lock_light(control)?.set_events_enabled(enabled),
            _ => Err(Error::Unsupported("event control")),
        }
    }
    pub(super) fn poll_magnetometer(&mut self) -> Result<Report> {
        match self {
            Self::MadGaze {
                client,
                initialized,
                ..
            } => {
                require_started(*initialized)?;
                Report::from_raw(RawReport::MadGazeMagnetometer(client.read_magnetometer()?))
            }
            _ => Err(Error::Unsupported("magnetometer poll")),
        }
    }
    pub(super) fn service(&mut self) -> Result<()> {
        match self {
            Self::Light { control, .. } => lock_light(control)?.heartbeat(),
            _ => Ok(()),
        }
    }
    pub(super) fn proximity_enabled(&mut self) -> Result<bool> {
        match self {
            Self::OneTcp { control, .. } => control.proximity_enabled(),
            _ => Err(Error::Unsupported("proximity setting")),
        }
    }
    pub(super) fn set_proximity_enabled(&mut self, enabled: bool) -> Result<()> {
        match self {
            Self::OneTcp { control, .. } => control.set_proximity_enabled(enabled),
            _ => Err(Error::Unsupported("proximity setting")),
        }
    }
    pub(super) fn stabilization_enabled(&mut self) -> Result<bool> {
        match self {
            Self::OneTcp { control, .. } => control.stabilization_enabled(),
            _ => Err(Error::Unsupported("stabilization")),
        }
    }
    pub(super) fn set_stabilization_enabled(&mut self, enabled: bool) -> Result<()> {
        match self {
            Self::OneTcp { control, .. } => control.set_stabilization_enabled(enabled),
            _ => Err(Error::Unsupported("stabilization")),
        }
    }
}

fn binary_mode(value: u8) -> DisplayMode {
    match value {
        0 => DisplayMode::Mirror,
        1 => DisplayMode::Stereo,
        raw => DisplayMode::Unknown(raw),
    }
}
fn require_stopped(streaming: bool) -> Result<()> {
    if streaming {
        Err(Error::Invalid("stop sensors before reading configuration"))
    } else {
        Ok(())
    }
}
fn require_started(started: bool) -> Result<()> {
    if started {
        Ok(())
    } else {
        Err(Error::Invalid("call start_sensors before reading sensors"))
    }
}
