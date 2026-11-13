//! Air sensor command framing and raw, device-coordinate sensor samples.
use crate::{
    Error, Result,
    error::{need, remaining},
    usb::ReportIo,
    wire::crc32,
};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DisplayMode {
    Mirror60 = 1,
    Stereo60 = 3,
    Stereo72 = 4,
    Mirror72 = 5,
    HalfStereo60 = 8,
    Stereo90 = 9,
    Mirror90 = 10,
    Mirror120 = 11,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SensorCommand {
    pub command: u8,
    pub payload: Vec<u8>,
}
impl SensorCommand {
    pub fn encode(&self) -> Result<[u8; 64]> {
        if self.payload.len() > 56 {
            return Err(Error::TooLarge {
                size: self.payload.len(),
                limit: 56,
            });
        }
        let len = self.payload.len() + 8;
        let mut out = [0; 64];
        out[0] = 0xaa;
        out[5..7].copy_from_slice(&((len - 5) as u16).to_le_bytes());
        out[7] = self.command;
        out[8..len].copy_from_slice(&self.payload);
        let crc = crc32(&out[5..len]);
        out[1..5].copy_from_slice(&crc.to_le_bytes());
        Ok(out)
    }
    pub fn decode(data: &[u8]) -> Result<Self> {
        need(data, 8)?;
        if data[0] != 0xaa {
            return Err(Error::Invalid("sensor command magic"));
        }
        let len = u16::from_le_bytes([data[5], data[6]]) as usize + 5;
        if !(8..=64).contains(&len) {
            return Err(Error::Invalid("sensor command length"));
        }
        need(data, len)?;
        let expected = crc32(&data[5..len]);
        let actual = u32::from_le_bytes(data[1..5].try_into().unwrap());
        if expected != actual {
            return Err(Error::Checksum { expected, actual });
        }
        Ok(Self {
            command: data[7],
            payload: data[8..len].to_vec(),
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImuSample {
    pub version: u8,
    pub timestamp_ns: u64,
    pub sensor_timestamp_ns: u64,
    /// Unconverted temperature register; conversion varies by sensor revision.
    pub temperature_raw: u16,
    pub gyro_raw: [i32; 3],
    pub accel_raw: [i32; 3],
    /// Device axes, radians/second. No bias correction or axis remapping.
    pub gyro_rad_s: [f32; 3],
    /// Device axes, metres/second² (using the local schema's gravity 9.8).
    pub accel_m_s2: [f32; 3],
    pub magnetometer_raw: [u16; 3],
    pub magnetometer_offset: u16,
    pub magnetometer_denominator: u32,
    pub magnetometer_updated: bool,
}
impl ImuSample {
    pub fn decode(data: &[u8]) -> Result<Self> {
        need(data, 64)?;
        if data[0] != 1 {
            return Err(Error::Invalid("Air IMU report ID"));
        }
        let version = data[1];
        let (width, acc, mag, timestamp, updated) = match version {
            1 => (2, 24, 36, 48, 56),
            2 => (3, 27, 42, 54, 62),
            _ => return Err(Error::Invalid("unsupported Air IMU version")),
        };
        let gyro_div = le32(data, 14);
        let accel_div = le32(data, acc + 2);
        if gyro_div == 0 || accel_div == 0 {
            return Err(Error::Invalid("zero IMU scale denominator"));
        }
        let read_axes =
            |start: usize| std::array::from_fn(|i| signed(&data[start + i * width..][..width]));
        let gyro_raw: [i32; 3] = read_axes(18);
        let accel_raw: [i32; 3] = read_axes(acc + 6);
        let gyro_scale = (le16(data, 12) as f32 / gyro_div as f32).to_radians();
        let accel_scale = le16(data, acc) as f32 / accel_div as f32 * 9.8;
        Ok(Self {
            version,
            timestamp_ns: le64(data, 4),
            sensor_timestamp_ns: le64(data, timestamp),
            temperature_raw: le16(data, 2),
            gyro_raw,
            accel_raw,
            gyro_rad_s: gyro_raw.map(|x| x as f32 * gyro_scale),
            accel_m_s2: accel_raw.map(|x| x as f32 * accel_scale),
            magnetometer_raw: std::array::from_fn(|i| le16(data, mag + 6 + 2 * i)),
            magnetometer_offset: le16(data, mag),
            magnetometer_denominator: le32(data, mag + 2),
            magnetometer_updated: data[updated] != 0,
        })
    }
}
fn signed(data: &[u8]) -> i32 {
    match data.len() {
        2 => i16::from_le_bytes(data.try_into().unwrap()) as i32,
        3 => i32::from_le_bytes([
            data[0],
            data[1],
            data[2],
            if data[2] & 0x80 != 0 { 0xff } else { 0 },
        ]),
        _ => unreachable!(),
    }
}
fn le16(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(data[offset..offset + 2].try_into().unwrap())
}
fn le32(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap())
}
fn le64(data: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(data[offset..offset + 8].try_into().unwrap())
}

pub struct SensorClient<T> {
    io: T,
    timeout: Duration,
    usable: bool,
}
impl<T: ReportIo> SensorClient<T> {
    pub fn new(io: T, timeout: Duration) -> Result<Self> {
        if timeout.is_zero() {
            return Err(Error::Invalid("zero sensor timeout"));
        }
        Ok(Self {
            io,
            timeout,
            usable: true,
        })
    }
    pub fn into_inner(self) -> T {
        self.io
    }
    /// Sensor replies use the same framing as requests. The observed API has no
    /// transaction ID, so only one exchange is allowed at a time. Reports seen
    /// during a command are consumed; use `next_sample` for streaming afterward.
    pub fn command(&mut self, command: u8, payload: &[u8]) -> Result<SensorCommand> {
        if !self.usable {
            return Err(Error::Disconnected);
        }
        let report = SensorCommand {
            command,
            payload: payload.to_vec(),
        }
        .encode()?;
        let start = Instant::now();
        let result = (|| {
            self.io.write_report(&report, self.timeout)?;
            loop {
                let mut buffer = [0; 512];
                let n = self
                    .io
                    .read_report(&mut buffer, remaining(start, self.timeout)?)?;
                if n == 0 {
                    return Err(Error::Timeout);
                }
                if n > buffer.len() {
                    return Err(Error::Invalid("sensor adapter length"));
                }
                match buffer[0] {
                    0xaa => {
                        let reply = SensorCommand::decode(&buffer[..n])?;
                        if reply.command != command {
                            return Err(Error::Invalid("sensor reply command mismatch"));
                        }
                        return Ok(reply);
                    }
                    1 => {
                        ImuSample::decode(&buffer[..n])?;
                    }
                    _ => return Err(Error::Invalid("sensor report magic")),
                }
            }
        })();
        if result.is_err() {
            self.usable = false;
        }
        result
    }
    pub fn set_streaming(&mut self, enabled: bool) -> Result<()> {
        self.command(0x19, &[u8::from(enabled)])?;
        Ok(())
    }
    /// Call while streaming is stopped. Returns the device's own calibration
    /// JSON without rewriting it or imposing a vendor-specific calibration model.
    pub fn read_configuration(&mut self) -> Result<String> {
        let data = self.command(0x14, &[])?.payload;
        let len = u32::from_le_bytes(
            data.as_slice()
                .try_into()
                .map_err(|_| Error::Invalid("configuration length response"))?,
        ) as usize;
        if len > crate::wire::MAX_BODY {
            return Err(Error::TooLarge {
                size: len,
                limit: crate::wire::MAX_BODY,
            });
        }
        let mut bytes = Vec::with_capacity(len);
        while bytes.len() < len {
            let part = self.command(0x15, &[])?.payload;
            if part.is_empty() {
                return Err(Error::Invalid("empty calibration chunk"));
            }
            if part.len() > len - bytes.len() {
                return Err(Error::Invalid(
                    "calibration chunk exceeds advertised length",
                ));
            }
            bytes.extend(part);
        }
        String::from_utf8(bytes).map_err(|_| Error::Invalid("calibration is not UTF-8"))
    }
    pub fn next_sample(&mut self) -> Result<ImuSample> {
        if !self.usable {
            return Err(Error::Disconnected);
        }
        let start = Instant::now();
        loop {
            let mut buffer = [0; 512];
            let n = self
                .io
                .read_report(&mut buffer, remaining(start, self.timeout)?)?;
            if n == 0 {
                return Err(Error::Timeout);
            }
            if n > buffer.len() {
                return Err(Error::Invalid("sensor adapter length"));
            }
            if buffer[0] == 1 {
                return ImuSample::decode(&buffer[..n]);
            }
            SensorCommand::decode(&buffer[..n])?;
        }
    }
}
