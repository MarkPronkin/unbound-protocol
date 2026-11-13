//! Grawoow G530 / Metavision M53 MCU and OV580 sensor protocols.
//! Device axes and uncorrected samples are retained. See NOTICE for provenance.
use crate::{
    Error, Result,
    error::{need, remaining},
    usb::control::{ControlIo, Setup},
};
use std::time::{Duration, Instant};

pub const MCU_ID: (u16, u16) = (0x1ff7, 0x0ff4);
pub const SENSOR_ID: (u16, u16) = (0x05a9, 0x0f87);
pub const ENDPOINT: u8 = 0x89;
const WRITE: Setup = Setup {
    request_type: 0x21,
    request: 9,
    value: 0x201,
    index: 0,
};
const READ: Setup = Setup {
    request_type: 0xa1,
    request: 1,
    value: 0x102,
    index: 0,
};

pub fn encode_command(command: u16, payload: &[u8]) -> Result<Vec<u8>> {
    // The reply transfer has a 256-byte buffer, including six header bytes and checksum.
    if payload.len() > 249 {
        return Err(Error::TooLarge {
            size: payload.len(),
            limit: 249,
        });
    }
    let mut out = vec![
        0xaa,
        0xbb,
        (command >> 8) as u8,
        command as u8,
        0,
        payload.len() as u8,
    ];
    out.extend(payload);
    out.push(out[2..].iter().fold(0u8, |sum, &b| sum.wrapping_add(b)));
    Ok(out)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub command: u16,
    pub payload: Vec<u8>,
    pub checksum: u8,
}
impl Reply {
    /// The response checksum is preserved; its algorithm has no independent
    /// validation in the reference driver, so it is not guessed here.
    pub fn decode(data: &[u8]) -> Result<Self> {
        need(data, 7)?;
        if data.len() > 256 {
            return Err(Error::TooLarge {
                size: data.len(),
                limit: 256,
            });
        }
        if data[..2] != [0xaa, 0xbb] || data[4] != 0 {
            return Err(Error::Invalid("Grawoow reply header"));
        }
        let len = data[5] as usize;
        if len > 249 {
            return Err(Error::Invalid("Grawoow reply length"));
        }
        need(data, 7 + len)?;
        Ok(Self {
            command: u16::from_be_bytes([data[2], data[3]]),
            payload: data[6..6 + len].to_vec(),
            checksum: data[6 + len],
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImuSample {
    pub gyro_raw: [i32; 3],
    pub accel_raw: [i32; 3],
    pub gyro_rad_s: [f32; 3],
    pub accel_m_s2: [f32; 3],
}
impl ImuSample {
    /// No device timestamp has been established for this layout.
    pub fn decode(data: &[u8]) -> Result<Self> {
        need(data, 128)?;
        if data.len() != 128 {
            return Err(Error::Invalid("Grawoow IMU report length"));
        }
        let axes = |offset| {
            std::array::from_fn(|axis| {
                let i = offset + 4 * axis;
                i32::from_le_bytes(data[i..i + 4].try_into().unwrap())
            })
        };
        let gyro_raw: [i32; 3] = axes(0x3c);
        let accel_raw: [i32; 3] = axes(0x58);
        Ok(Self {
            gyro_raw,
            accel_raw,
            gyro_rad_s: gyro_raw.map(|x| (x as f32 / 16.4).to_radians()),
            accel_m_s2: accel_raw.map(|x| x as f32 * 9.81 / 16384.0),
        })
    }
}

pub struct ControlClient<T> {
    io: T,
    timeout: Duration,
    usable: bool,
}
impl<T: ControlIo> ControlClient<T> {
    pub fn new(io: T, timeout: Duration) -> Result<Self> {
        if timeout.is_zero() {
            return Err(Error::Invalid("zero Grawoow timeout"));
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
    pub fn command(&mut self, command: u16, payload: &[u8]) -> Result<Vec<u8>> {
        if !self.usable {
            return Err(Error::Disconnected);
        }
        let request = encode_command(command, payload)?;
        let start = Instant::now();
        let result = (|| {
            self.io.write_control(WRITE, &request, self.timeout)?;
            let mut buffer = [0; 256];
            let n = self
                .io
                .read_control(READ, &mut buffer, remaining(start, self.timeout)?)?;
            if n > buffer.len() {
                return Err(Error::Invalid("Grawoow adapter length"));
            }
            let reply = Reply::decode(&buffer[..n])?;
            if reply.command != command {
                return Err(Error::Invalid("Grawoow reply command mismatch"));
            }
            Ok(reply.payload)
        })();
        if result.is_err() {
            self.usable = false;
        }
        result
    }
    pub fn serial(&mut self) -> Result<String> {
        utf8(self.command(0x8005, &[])?)
    }
    /// Preserve unknown mode values; observed modes are 0 mirror and 1 stereo.
    pub fn display_mode(&mut self) -> Result<u8> {
        let data = self.command(0x8007, &[])?;
        if data.len() != 1 {
            return Err(Error::Invalid("Grawoow display mode length"));
        }
        Ok(data[0])
    }
    pub fn set_stereo(&mut self, enabled: bool) -> Result<()> {
        self.command(0x8008, &[u8::from(enabled)])?;
        Ok(())
    }
    pub fn read_configuration(&mut self) -> Result<String> {
        let mut bytes = Vec::new();
        loop {
            let offset = u16::try_from(bytes.len()).map_err(|_| Error::TooLarge {
                size: bytes.len(),
                limit: 65535,
            })?;
            let [hi, lo] = offset.to_be_bytes();
            let part = self.command(0x800a, &[0, 0, 0, hi, lo])?;
            if part.len() <= 6 {
                return utf8(bytes);
            }
            if bytes.len() + part.len() - 6 > 65535 {
                return Err(Error::TooLarge {
                    size: bytes.len() + part.len() - 6,
                    limit: 65535,
                });
            }
            bytes.extend_from_slice(&part[6..]);
        }
    }
}
fn utf8(bytes: Vec<u8>) -> Result<String> {
    String::from_utf8(bytes).map_err(|_| Error::Invalid("Grawoow text is not UTF-8"))
}

pub struct SensorClient<T> {
    io: T,
    timeout: Duration,
}
impl<T: ControlIo> SensorClient<T> {
    pub fn new(io: T, timeout: Duration) -> Result<Self> {
        if timeout.is_zero() {
            return Err(Error::Invalid("zero Grawoow sensor timeout"));
        }
        Ok(Self { io, timeout })
    }
    pub fn into_inner(self) -> T {
        self.io
    }
    pub fn next_sample(&mut self) -> Result<ImuSample> {
        let mut data = [0; 128];
        let n = self.io.read_interrupt(ENDPOINT, &mut data, self.timeout)?;
        if n == 0 {
            return Err(Error::Timeout);
        }
        if n > data.len() {
            return Err(Error::Invalid("Grawoow interrupt adapter length"));
        }
        ImuSample::decode(&data[..n])
    }
}
