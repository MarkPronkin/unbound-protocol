//! Rokid Air/Max vendor control and interrupt reports. Device axes are retained.
//! Protocol observations: badicsalex/ar-drivers-rs (MIT), see NOTICE.
use crate::{
    Error, Result,
    error::need,
    usb::control::{ControlIo, Setup},
};
use std::time::Duration;

pub const VID: u16 = 0x04d2;
pub const PID: u16 = 0x162f;
pub const ENDPOINT: u8 = 0x82;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum DisplayMode {
    Mirror = 0,
    Stereo = 1,
    MirrorHighRefresh = 3,
    StereoHighRefresh = 4,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Packet {
    Misc {
        sequence: u32,
        keys: u8,
        proximity: u8,
    },
    /// Sensor types 1/2/3 are acceleration (m/s²), gyro (rad/s), magnetometer (µT).
    /// Unknown sensor types and their vectors remain accessible.
    Sensor {
        sensor_type: u8,
        sequence: u32,
        timestamp_us: u64,
        vector: [f32; 3],
    },
    Combined {
        timestamp_ns: u64,
        accel_m_s2: [f32; 3],
        gyro_rad_s: [f32; 3],
        magnetometer_ut: [f32; 3],
        keys: u8,
        proximity: u8,
        vsync_timestamp: u64,
        brightness: u8,
        volume: u8,
    },
    Unknown(Vec<u8>),
}
impl Packet {
    pub fn decode(data: &[u8]) -> Result<Self> {
        need(data, 64)?;
        if data.len() != 64 {
            return Err(Error::Invalid("Rokid report length"));
        }
        let u32_at = |i| u32::from_le_bytes(data[i..i + 4].try_into().unwrap());
        let u64_at = |i| u64::from_le_bytes(data[i..i + 8].try_into().unwrap());
        let axes = |i| -> Result<[f32; 3]> {
            let v = std::array::from_fn(|axis| f32::from_bits(u32_at(i + 4 * axis)));
            if v.iter().any(|x| !x.is_finite()) {
                return Err(Error::Invalid("nonfinite Rokid sensor"));
            }
            Ok(v)
        };
        Ok(match data[0] {
            2 => Self::Misc {
                sequence: u32_at(1),
                keys: data[47],
                proximity: data[51],
            },
            4 => Self::Sensor {
                sensor_type: data[1],
                sequence: u32_at(2),
                timestamp_us: u64_at(9),
                vector: axes(21)?,
            },
            17 => Self::Combined {
                timestamp_ns: u64_at(1),
                accel_m_s2: axes(9)?,
                gyro_rad_s: axes(21)?,
                magnetometer_ut: axes(33)?,
                keys: data[45],
                proximity: data[46],
                vsync_timestamp: u64_at(48),
                brightness: data[59],
                volume: data[60],
            },
            _ => Self::Unknown(data.to_vec()),
        })
    }
}

pub struct Client<T> {
    io: T,
    timeout: Duration,
}
impl<T: ControlIo> Client<T> {
    pub fn new(io: T, timeout: Duration) -> Result<Self> {
        if timeout.is_zero() {
            return Err(Error::Invalid("zero Rokid timeout"));
        }
        Ok(Self { io, timeout })
    }
    pub fn into_inner(self) -> T {
        self.io
    }
    fn query(&mut self, value: u16, index: u16) -> Result<Vec<u8>> {
        let mut data = [0; 64];
        let n = self.io.read_control(
            Setup {
                request_type: 0xc0,
                request: 0x81,
                value,
                index,
            },
            &mut data,
            self.timeout,
        )?;
        if n > data.len() {
            return Err(Error::Invalid("Rokid control adapter length"));
        }
        need(&data[..n], 1)?;
        Ok(data[..n].to_vec())
    }
    pub fn serial(&mut self) -> Result<String> {
        let data = self.query(0x100, 0)?;
        let end = data.iter().position(|&b| b == 0).unwrap_or(data.len());
        String::from_utf8(data[..end].to_vec())
            .map_err(|_| Error::Invalid("Rokid serial is not UTF-8"))
    }
    /// Raw mode byte: 0 mirror, 1 stereo, 2 half-SBS, 3/4 high refresh.
    pub fn display_mode(&mut self) -> Result<u8> {
        let data = self.query(0, 1)?;
        need(&data, 2)?;
        Ok(data[1])
    }
    pub fn set_display_mode(&mut self, mode: DisplayMode) -> Result<()> {
        self.io.write_control(
            Setup {
                request_type: 0x40,
                request: 1,
                value: mode as u16,
                index: 1,
            },
            &[0],
            self.timeout,
        )
    }
    pub fn next_packet(&mut self) -> Result<Packet> {
        let mut data = [0; 64];
        let n = self.io.read_interrupt(ENDPOINT, &mut data, self.timeout)?;
        if n == 0 {
            return Err(Error::Timeout);
        }
        if n > data.len() {
            return Err(Error::Invalid("Rokid interrupt adapter length"));
        }
        Packet::decode(&data[..n])
    }
}
