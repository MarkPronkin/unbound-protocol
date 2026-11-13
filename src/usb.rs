//! USB control reports shared by Air and One. OS adapters implement `ReportIo`;
//! all bytes here exclude any OS-only report-ID prefix.
use crate::{
    Error, Result,
    error::{need, remaining},
    wire::crc32,
};
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

pub const HEADER_SIZE: usize = 22;
pub const MAX_REPORT: usize = 1024;

pub mod control;
pub mod events;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    pub request_id: u32,
    pub timestamp: u32,
    pub command: u16,
    pub reserved: [u8; 5],
    /// Responses have a status byte first; unsolicited events need not.
    pub payload: Vec<u8>,
}
impl Packet {
    pub fn request(command: u16, request_id: u32, payload: &[u8]) -> Self {
        Self {
            request_id,
            timestamp: 0,
            command,
            reserved: [0; 5],
            payload: payload.to_vec(),
        }
    }
    pub fn encode(&self, report_size: usize) -> Result<Vec<u8>> {
        if !(HEADER_SIZE..=MAX_REPORT).contains(&report_size) {
            return Err(Error::Invalid("USB report size"));
        }
        let len = HEADER_SIZE + self.payload.len();
        if len > report_size {
            return Err(Error::TooLarge {
                size: len,
                limit: report_size,
            });
        }
        let mut out = vec![0; report_size];
        out[0] = 0xfd;
        out[5..7].copy_from_slice(&((len - 5) as u16).to_le_bytes());
        out[7..11].copy_from_slice(&self.request_id.to_le_bytes());
        out[11..15].copy_from_slice(&self.timestamp.to_le_bytes());
        out[15..17].copy_from_slice(&self.command.to_le_bytes());
        out[17..22].copy_from_slice(&self.reserved);
        out[22..len].copy_from_slice(&self.payload);
        let crc = crc32(&out[5..len]);
        out[1..5].copy_from_slice(&crc.to_le_bytes());
        Ok(out)
    }
    /// Padding beyond the declared length is ignored; it is not checksummed.
    pub fn decode(data: &[u8]) -> Result<Self> {
        need(data, HEADER_SIZE)?;
        if data.len() > MAX_REPORT {
            return Err(Error::TooLarge {
                size: data.len(),
                limit: MAX_REPORT,
            });
        }
        if data[0] != 0xfd {
            return Err(Error::Invalid("USB control magic"));
        }
        let len = u16::from_le_bytes([data[5], data[6]]) as usize + 5;
        if len < HEADER_SIZE {
            return Err(Error::Invalid("USB control length"));
        }
        need(data, len)?;
        let expected = crc32(&data[5..len]);
        let actual = u32::from_le_bytes(data[1..5].try_into().unwrap());
        if expected != actual {
            return Err(Error::Checksum { expected, actual });
        }
        Ok(Self {
            request_id: u32::from_le_bytes(data[7..11].try_into().unwrap()),
            timestamp: u32::from_le_bytes(data[11..15].try_into().unwrap()),
            command: u16::from_le_bytes(data[15..17].try_into().unwrap()),
            reserved: data[17..22].try_into().unwrap(),
            payload: data[22..len].to_vec(),
        })
    }
    pub fn response_data(&self) -> Result<&[u8]> {
        let (&status, data) = self
            .payload
            .split_first()
            .ok_or(Error::Invalid("missing USB status"))?;
        if status != 0 {
            return Err(Error::DeviceStatus(status));
        }
        Ok(data)
    }
}

/// A read returns one report or `Error::Timeout`. A write must send the complete
/// report or fail. Read deadlines must be honored. The write timeout is advisory
/// for OS APIs (such as hidapi) that do not expose a write deadline.
pub trait ReportIo {
    fn write_report(&mut self, report: &[u8], timeout: Duration) -> Result<()>;
    fn read_report(&mut self, buffer: &mut [u8], timeout: Duration) -> Result<usize>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    Air,
    One,
}

pub struct UsbClient<T> {
    io: T,
    profile: Profile,
    request_id: u32,
    timeout: Duration,
    events: VecDeque<Packet>,
    usable: bool,
}
impl<T: ReportIo> UsbClient<T> {
    pub fn new(io: T, profile: Profile, timeout: Duration) -> Result<Self> {
        if timeout.is_zero() {
            return Err(Error::Invalid("zero USB timeout"));
        }
        Ok(Self {
            io,
            profile,
            request_id: 0,
            timeout,
            events: VecDeque::new(),
            usable: true,
        })
    }
    pub fn into_inner(self) -> T {
        self.io
    }
    pub fn pop_event(&mut self) -> Option<Packet> {
        self.events.pop_front()
    }
    /// Read an unsolicited report while no command is in flight.
    pub fn next_event(&mut self) -> Result<Packet> {
        if let Some(packet) = self.pop_event() {
            return Ok(packet);
        }
        if !self.usable {
            return Err(Error::Disconnected);
        }
        let mut buffer = [0; MAX_REPORT];
        let n = self.io.read_report(&mut buffer, self.timeout)?;
        if n == 0 {
            return Err(Error::Timeout);
        }
        if n > buffer.len() {
            return Err(Error::Invalid("USB adapter returned invalid length"));
        }
        Packet::decode(&buffer[..n])
    }
    pub fn command(&mut self, command: u16, payload: &[u8]) -> Result<Vec<u8>> {
        if !self.usable {
            return Err(Error::Disconnected);
        }
        if self.events.len() >= 128 {
            return Err(Error::QueueFull);
        }
        self.request_id = self.request_id.wrapping_add(1).max(1);
        let id = if self.profile == Profile::Air {
            0x1337
        } else {
            self.request_id
        };
        let size = if self.profile == Profile::Air {
            64
        } else {
            1024
        };
        let report = Packet::request(command, id, payload).encode(size)?;
        let start = Instant::now();
        if let Err(e) = self.io.write_report(&report, self.timeout) {
            self.usable = false;
            return Err(e);
        }
        let result = (|| {
            loop {
                let mut buffer = [0; MAX_REPORT];
                let n = self
                    .io
                    .read_report(&mut buffer, remaining(start, self.timeout)?)?;
                if n == 0 {
                    return Err(Error::Timeout);
                }
                if n > buffer.len() {
                    return Err(Error::Invalid("USB adapter returned invalid length"));
                }
                let packet = Packet::decode(&buffer[..n])?;
                if packet.command == command
                    && (self.profile == Profile::Air || packet.request_id == id)
                {
                    return Ok(packet.response_data()?.to_vec());
                }
                self.events.push_back(packet);
                if self.events.len() >= 128 {
                    return Err(Error::QueueFull);
                }
            }
        })();
        // Air firmware may not echo request IDs, so a late reply cannot be
        // distinguished from a new command. Reopen after any failed exchange.
        if result.is_err() {
            self.usable = false;
        }
        result
    }
    pub fn mcu_version(&mut self) -> Result<String> {
        text(&self.command(0x26, &[])?)
    }
    pub fn dsp_version(&mut self) -> Result<String> {
        text(&self.command(0x18, &[])?)
    }
    pub fn serial(&mut self) -> Result<String> {
        text(&self.command(0x15, &[])?)
    }
    pub fn brightness(&mut self) -> Result<u8> {
        byte(&self.command(0x03, &[])?)
    }
    pub fn set_brightness(&mut self, level: u8) -> Result<()> {
        if level > 9 {
            return Err(Error::Invalid("brightness must be 0..9"));
        }
        self.command(0x04, &[level])?;
        Ok(())
    }
    pub fn air_display_mode(&mut self) -> Result<u8> {
        self.require(Profile::Air)?;
        byte(&self.command(0x07, &[])?)
    }
    pub fn set_air_display_mode(&mut self, mode: crate::air::DisplayMode) -> Result<()> {
        self.require(Profile::Air)?;
        self.command(0x08, &[mode as u8])?;
        Ok(())
    }
    pub fn camera_plugged(&mut self) -> Result<bool> {
        self.require(Profile::One)?;
        match byte(&self.command(0xd5, &[])?)? {
            0 => Ok(true),
            1 => Ok(false),
            _ => Err(Error::Invalid("camera status")),
        }
    }
    pub fn pilot_version(&mut self) -> Result<String> {
        self.require(Profile::One)?;
        text(&self.command(0x1213, &[2])?)
    }
    pub fn usb_configuration(&mut self) -> Result<UsbConfiguration> {
        self.require(Profile::One)?;
        let data = self.command(0xd2, &[])?;
        Ok(UsbConfiguration(u32::from_le_bytes(
            data.as_slice()
                .try_into()
                .map_err(|_| Error::Invalid("USB configuration length"))?,
        )))
    }
    /// May cause USB re-enumeration. Drop this client and rediscover afterward.
    pub fn set_usb_configuration(&mut self, config: UsbConfiguration) -> Result<()> {
        self.require(Profile::One)?;
        let result = self.command(0xd3, &config.0.to_le_bytes()).map(|_| ());
        self.usable = false;
        result
    }
    fn require(&self, profile: Profile) -> Result<()> {
        if self.profile == profile {
            Ok(())
        } else {
            Err(Error::Invalid("command is for another USB protocol family"))
        }
    }
}
fn byte(data: &[u8]) -> Result<u8> {
    if data.len() == 1 {
        Ok(data[0])
    } else {
        Err(Error::Invalid("expected one-byte USB value"))
    }
}
fn text(data: &[u8]) -> Result<String> {
    let end = data.iter().position(|&b| b == 0).unwrap_or(data.len());
    String::from_utf8(data[..end].to_vec()).map_err(|_| Error::Invalid("USB string is not UTF-8"))
}

/// Two-bit fields: 0 leave unchanged, 1 enable, 2 disable, 3 reserved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UsbConfiguration(pub u32);
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Function {
    Ncm,
    Ecm,
    Audio,
    HidControl,
    Mtp,
    MassStorage,
    Camera0,
    Camera1,
    Apply,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Setting {
    Unchanged = 0,
    Enable = 1,
    Disable = 2,
}
impl UsbConfiguration {
    pub fn get(self, function: Function) -> u8 {
        ((self.0 >> (function as u32 * 2)) & 3) as u8
    }
    pub fn with(mut self, function: Function, setting: Setting) -> Self {
        let shift = function as u32 * 2;
        self.0 = (self.0 & !(3 << shift)) | ((setting as u32) << shift);
        self
    }
}
