//! One-family runtime control on TCP 52999 and sensor reports on TCP 52998.
use crate::{
    Error, Result,
    error::{need, remaining},
    protobuf,
    wire::{Frame, FrameReader},
};
use socket2::{Domain, Protocol, Socket, Type};
use std::{
    collections::VecDeque,
    io::{Read, Write},
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream},
    time::{Duration, Instant},
};

pub const GLASSES_IP: Ipv4Addr = Ipv4Addr::new(169, 254, 2, 1);
pub const CONTROL_PORT: u16 = 52999;
pub const REPORTS_PORT: u16 = 52998;
pub mod opcode {
    pub const SOFTWARE_VERSION: u16 = 0x271d;
    pub const DSP_VERSION: u16 = 0x272d;
    pub const SERIAL: u16 = 0x2729;
    pub const CONFIGURATION: u16 = 0x271f;
    pub const SET_BRIGHTNESS: u16 = 0x271c;
    pub const SET_DIMMER: u16 = 0x2727;
    pub const GET_PROXIMITY: u16 = 0x2718;
    pub const SET_PROXIMITY: u16 = 0x2719;
    pub const GET_EDID: u16 = 0x275e;
    pub const SET_EDID: u16 = 0x275f;
    pub const SET_INPUT_MODE: u16 = 0x2822;
    pub const SET_SCENE_MODE: u16 = 0x2829;
    pub const GET_STABILIZATION: u16 = 0x28a6;
    pub const SET_STABILIZATION: u16 = 0x28a7;
    pub const KEY_EVENT: u16 = 0x272e;
    pub const SENSOR_REPORT: u16 = 0x2836;
}

#[derive(Debug, Clone)]
pub struct ConnectionOptions {
    pub host: IpAddr,
    /// Bind the socket to the glasses-facing local address. Android callers can
    /// instead use `from_stream` after Network.bindSocket in their platform layer.
    pub source: Option<IpAddr>,
    pub timeout: Duration,
}
impl Default for ConnectionOptions {
    fn default() -> Self {
        Self {
            host: GLASSES_IP.into(),
            source: None,
            timeout: Duration::from_secs(2),
        }
    }
}
impl ConnectionOptions {
    pub fn connect(&self, port: u16) -> Result<TcpStream> {
        if self.timeout.is_zero() {
            return Err(Error::Invalid("zero TCP timeout"));
        }
        let socket = Socket::new(
            if self.host.is_ipv4() {
                Domain::IPV4
            } else {
                Domain::IPV6
            },
            Type::STREAM,
            Some(Protocol::TCP),
        )?;
        if let Some(source) = self.source {
            socket.bind(&SocketAddr::new(source, 0).into())?;
        }
        socket.connect_timeout(&SocketAddr::new(self.host, port).into(), self.timeout)?;
        let stream: TcpStream = socket.into();
        stream.set_nodelay(true)?;
        Ok(stream)
    }
}

pub struct ControlClient {
    stream: TcpStream,
    reader: FrameReader,
    next_id: u32,
    timeout: Duration,
    events: VecDeque<Frame>,
    event_bytes: usize,
    usable: bool,
}
impl ControlClient {
    pub fn connect(options: &ConnectionOptions) -> Result<Self> {
        Self::from_stream(options.connect(CONTROL_PORT)?, options.timeout)
    }
    pub fn from_stream(stream: TcpStream, timeout: Duration) -> Result<Self> {
        if timeout.is_zero() {
            return Err(Error::Invalid("zero TCP timeout"));
        }
        stream.set_nonblocking(false)?;
        stream.set_nodelay(true)?;
        Ok(Self {
            stream,
            reader: FrameReader::default(),
            next_id: 0,
            timeout,
            events: VecDeque::new(),
            event_bytes: 0,
            usable: true,
        })
    }
    pub fn pop_event(&mut self) -> Option<Frame> {
        let frame = self.events.pop_front()?;
        self.event_bytes -= frame.body.len();
        Some(frame)
    }
    fn queue(&mut self, frame: Frame) -> Result<()> {
        if self.events.len() >= 128 || self.event_bytes + frame.body.len() > crate::wire::MAX_BODY {
            self.usable = false;
            return Err(Error::QueueFull);
        }
        self.event_bytes += frame.body.len();
        self.events.push_back(frame);
        Ok(())
    }
    /// Sends one request. Reply correlation uses both opcode and transaction ID.
    /// Late replies and unsolicited messages are retained in the bounded queue.
    /// A timeout is never retried automatically (a setter may have succeeded).
    pub fn request(&mut self, opcode: u16, payload: &[u8]) -> Result<Vec<u8>> {
        if !self.usable {
            return Err(Error::Disconnected);
        }
        if self.events.len() >= 128 {
            return Err(Error::QueueFull);
        }
        if payload.len() > crate::wire::MAX_BODY - 4 {
            return Err(Error::TooLarge {
                size: payload.len(),
                limit: crate::wire::MAX_BODY - 4,
            });
        }
        self.next_id = (self.next_id % 0x7fff_ffff) + 1;
        let id = self.next_id;
        let mut body = Vec::with_capacity(4 + payload.len());
        body.extend((id | 0x8000_0000).to_be_bytes());
        body.extend(payload);
        let packet = Frame { opcode, body }.encode()?;
        let start = Instant::now();
        let result = (|| {
            let mut offset = 0;
            while offset < packet.len() {
                self.stream
                    .set_write_timeout(Some(remaining(start, self.timeout)?))?;
                match self.stream.write(&packet[offset..]) {
                    Ok(0) => return Err(Error::Disconnected),
                    Ok(n) => offset += n,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(e) => return Err(e.into()),
                }
            }
            Ok(())
        })();
        if let Err(e) = result {
            self.usable = false;
            return Err(e);
        }
        loop {
            let frame = self.read_frame(start)?;
            if frame.opcode == opcode
                && frame.body.len() >= 4
                && u32::from_be_bytes(frame.body[..4].try_into().unwrap()) == id
            {
                return Ok(frame.body[4..].to_vec());
            }
            self.queue(frame)?;
        }
    }
    fn read_frame(&mut self, start: Instant) -> Result<Frame> {
        let result = self.reader.read(&mut DeadlineReader {
            stream: &mut self.stream,
            start,
            timeout: self.timeout,
        });
        if result.is_err() && !matches!(result, Err(Error::Timeout)) {
            self.usable = false;
        }
        result
    }
    pub fn next_event(&mut self) -> Result<Frame> {
        if let Some(frame) = self.pop_event() {
            return Ok(frame);
        }
        if !self.usable {
            return Err(Error::Disconnected);
        }
        self.read_frame(Instant::now())
    }
    pub fn get_number(&mut self, opcode: u16) -> Result<u64> {
        protobuf::numeric_response(&self.request(opcode, &protobuf::get_request())?)
    }
    pub fn get_string(&mut self, opcode: u16) -> Result<String> {
        protobuf::string_response(&self.request(opcode, &protobuf::get_request())?)
    }
    pub fn set_number(&mut self, opcode: u16, value: u64) -> Result<()> {
        protobuf::acknowledgement(&self.request(opcode, &protobuf::set_request(value))?)
    }
    pub fn software_version(&mut self) -> Result<String> {
        self.get_string(opcode::SOFTWARE_VERSION)
    }
    pub fn dsp_version(&mut self) -> Result<String> {
        self.get_string(opcode::DSP_VERSION)
    }
    pub fn serial(&mut self) -> Result<String> {
        self.get_string(opcode::SERIAL)
    }
    pub fn configuration(&mut self) -> Result<String> {
        self.get_string(opcode::CONFIGURATION)
    }
    /// Firmware-dependent EDID selector; preserve readback for restoration.
    pub fn edid(&mut self) -> Result<u64> {
        self.get_number(opcode::GET_EDID)
    }
    pub fn set_edid(&mut self, value: u64) -> Result<()> {
        self.set_number(opcode::SET_EDID, value)
    }
    pub fn set_input_mode(&mut self, mode: InputMode) -> Result<()> {
        self.set_number(opcode::SET_INPUT_MODE, mode as u64)
    }
    pub fn set_brightness(&mut self, level: u8) -> Result<()> {
        if level > 9 {
            return Err(Error::Invalid("brightness must be 0..9"));
        }
        self.set_number(opcode::SET_BRIGHTNESS, level.into())
    }
    pub fn set_dimmer(&mut self, level: Dimmer) -> Result<()> {
        self.set_number(opcode::SET_DIMMER, level as u64)
    }
    pub fn proximity_enabled(&mut self) -> Result<bool> {
        boolean(self.get_number(opcode::GET_PROXIMITY)?)
    }
    pub fn set_proximity_enabled(&mut self, enabled: bool) -> Result<()> {
        self.set_number(opcode::SET_PROXIMITY, u64::from(enabled))
    }
    pub fn stabilization_enabled(&mut self) -> Result<bool> {
        boolean(self.get_number(opcode::GET_STABILIZATION)?)
    }
    pub fn set_stabilization_enabled(&mut self, enabled: bool) -> Result<()> {
        self.set_number(opcode::SET_STABILIZATION, u64::from(enabled))
    }
    pub fn set_scene_mode(&mut self, mode: SceneMode) -> Result<()> {
        self.set_number(opcode::SET_SCENE_MODE, mode as u64)
    }
}
fn boolean(value: u64) -> Result<bool> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(Error::Invalid("non-boolean property value")),
    }
}
#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum InputMode {
    Regular = 0,
    SideBySide = 1,
}
#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum Dimmer {
    Clear = 0,
    Middle = 1,
    Dark = 2,
}
#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum SceneMode {
    ButtonsEnabled = 0,
    ButtonsDisabled = 1,
}

struct DeadlineReader<'a> {
    stream: &'a mut TcpStream,
    start: Instant,
    timeout: Duration,
}
impl Read for DeadlineReader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let timeout = self
            .timeout
            .checked_sub(self.start.elapsed())
            .filter(|d| !d.is_zero())
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::TimedOut))?;
        self.stream.set_read_timeout(Some(timeout))?;
        self.stream.read(buffer)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SensorReport {
    pub device_id: u64,
    pub timestamp_ns: u64,
    /// 11: gyro/accelerometer, 4: magnetometer. Other types are preserved.
    pub report_type: u32,
    pub gyro_rad_s: [f32; 3],
    pub accel_m_s2: [f32; 3],
    pub magnetometer: [f32; 3],
    pub temperature_c: f32,
    pub imu_id: u8,
    pub frame_id: [u8; 3],
    /// Complete body, including currently unassigned fields.
    pub raw: Vec<u8>,
}
impl SensorReport {
    pub fn decode(frame: &Frame) -> Result<Self> {
        if frame.opcode != opcode::SENSOR_REPORT {
            return Err(Error::Invalid("One sensor opcode"));
        }
        let data = &frame.body;
        if data.len() != 128 {
            return Err(Error::Invalid(
                "unsupported One sensor body size (expected 128)",
            ));
        }
        let number = |o| f32::from_le_bytes(data[o..o + 4].try_into().unwrap());
        let axes = |o| [number(o), number(o + 4), number(o + 8)];
        let report = Self {
            device_id: u64::from_le_bytes(data[..8].try_into().unwrap()),
            timestamp_ns: u64::from_le_bytes(data[8..16].try_into().unwrap()),
            report_type: u32::from_le_bytes(data[24..28].try_into().unwrap()),
            gyro_rad_s: axes(28),
            accel_m_s2: axes(40),
            magnetometer: axes(52),
            temperature_c: number(64),
            imu_id: data[68],
            frame_id: data[69..72].try_into().unwrap(),
            raw: data.clone(),
        };
        let active: &[f32] = match report.report_type {
            11 => &report.raw_finite_imu(),
            4 => &report.magnetometer,
            _ => &[],
        };
        if active.iter().any(|x| !x.is_finite()) {
            return Err(Error::Invalid("non-finite sensor reading"));
        }
        Ok(report)
    }
    fn raw_finite_imu(&self) -> [f32; 6] {
        [
            self.gyro_rad_s[0],
            self.gyro_rad_s[1],
            self.gyro_rad_s[2],
            self.accel_m_s2[0],
            self.accel_m_s2[1],
            self.accel_m_s2[2],
        ]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyEvent {
    pub key: u32,
    pub state: u32,
    pub device_time_raw: u32,
}
impl KeyEvent {
    pub fn decode(frame: &Frame) -> Result<Self> {
        if frame.opcode != opcode::KEY_EVENT {
            return Err(Error::Invalid("key event opcode"));
        }
        need(&frame.body, 64)?;
        Ok(Self {
            key: u32::from_le_bytes(frame.body[..4].try_into().unwrap()),
            state: u32::from_le_bytes(frame.body[4..8].try_into().unwrap()),
            device_time_raw: u32::from_le_bytes(frame.body[8..12].try_into().unwrap()),
        })
    }
}
pub struct ReportsClient {
    stream: TcpStream,
    reader: FrameReader,
    timeout: Duration,
}
impl ReportsClient {
    pub fn connect(options: &ConnectionOptions) -> Result<Self> {
        Self::from_stream(options.connect(REPORTS_PORT)?, options.timeout)
    }
    pub fn from_stream(stream: TcpStream, timeout: Duration) -> Result<Self> {
        if timeout.is_zero() {
            return Err(Error::Invalid("zero TCP timeout"));
        }
        stream.set_nonblocking(false)?;
        Ok(Self {
            stream,
            reader: FrameReader::default(),
            timeout,
        })
    }
    /// Includes unknown report opcodes, allowing lossless capture and research.
    pub fn next_frame(&mut self) -> Result<Frame> {
        self.reader.read(&mut DeadlineReader {
            stream: &mut self.stream,
            start: Instant::now(),
            timeout: self.timeout,
        })
    }
    pub fn next_report(&mut self) -> Result<SensorReport> {
        SensorReport::decode(&self.next_frame()?)
    }
}
