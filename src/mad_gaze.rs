//! Mad Gaze Glow serial commands and BMI160 / AK09911 sensor reads.
//! Sensor axes are retained; gyro and acceleration are not swapped. Samples
//! have no fabricated device timestamp. See NOTICE for protocol provenance.
use crate::{Error, Result, error::need, report::Session, usb::ReportIo};
use std::time::Duration;

pub const VID: u16 = 0x04b4;
pub const PID: u16 = 0x0002;
pub const BAUD_RATE: u32 = 921600;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    pub command: [u8; 3],
    pub session: [u8; 2],
    pub payload: Vec<u8>,
    /// The reference uses zero checksum bytes. Their algorithm is unverified.
    pub checksum: [u8; 2],
    /// `ff` means no delimiter substitution. Other values are preserved by the
    /// codec but rejected by the client until their indexing is established.
    pub escape_index: u8,
}
impl Packet {
    pub fn request(command: [u8; 3], payload: &[u8]) -> Self {
        Self {
            command,
            session: [0xab, 0xcd],
            payload: payload.to_vec(),
            checksum: [0; 2],
            escape_index: 0xff,
        }
    }
    pub fn encode(&self) -> Result<Vec<u8>> {
        if self.payload.len() > 250 {
            return Err(Error::TooLarge {
                size: self.payload.len(),
                limit: 250,
            });
        }
        if self.command.contains(&b':') || self.payload.contains(&b':') {
            return Err(Error::Invalid(
                "Mad Gaze delimiter substitution is not implemented",
            ));
        }
        let mut bytes = vec![b':'];
        bytes.extend(self.command);
        bytes.push((self.payload.len() + 5) as u8);
        bytes.extend(self.session);
        bytes.extend(&self.payload);
        bytes.extend(self.checksum);
        bytes.push(self.escape_index);
        Ok(bytes)
    }
    pub fn decode(data: &[u8]) -> Result<Self> {
        need(data, 10)?;
        if data[0] != b':' || data[4] < 5 {
            return Err(Error::Invalid("Mad Gaze serial header"));
        }
        let len = 5 + data[4] as usize;
        need(data, len)?;
        if data.len() != len {
            return Err(Error::Invalid("Mad Gaze serial frame length"));
        }
        Ok(Self {
            command: data[1..4].try_into().unwrap(),
            session: data[5..7].try_into().unwrap(),
            payload: data[7..len - 3].to_vec(),
            checksum: data[len - 3..len - 1].try_into().unwrap(),
            escape_index: data[len - 1],
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImuSample {
    pub gyro_raw: [i16; 3],
    pub accel_raw: [i16; 3],
    pub gyro_rad_s: [f32; 3],
    pub accel_m_s2: [f32; 3],
}
impl ImuSample {
    /// Decode one 12-byte headerless BMI160 FIFO entry. The FIFO's -32768
    /// marker in the first gyro register denotes an empty entry.
    pub fn decode(data: &[u8]) -> Result<Option<Self>> {
        need(data, 12)?;
        if data.len() != 12 {
            return Err(Error::Invalid("BMI160 FIFO entry length"));
        }
        let axes = |offset| {
            std::array::from_fn(|axis| {
                let i = offset + 2 * axis;
                i16::from_le_bytes([data[i], data[i + 1]])
            })
        };
        let gyro_raw: [i16; 3] = axes(0);
        if gyro_raw[0] == i16::MIN {
            return Ok(None);
        }
        let accel_raw: [i16; 3] = axes(6);
        Ok(Some(Self {
            gyro_raw,
            accel_raw,
            gyro_rad_s: gyro_raw.map(|x| (x as f32 * 2000.0 / 32768.0).to_radians()),
            accel_m_s2: accel_raw.map(|x| x as f32 * (2.0 * 9.80665) / 32768.0),
        }))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MagnetometerSample {
    pub status: u8,
    pub raw: [i16; 3],
    pub microtesla: [f32; 3],
    pub status2: u8,
}
impl MagnetometerSample {
    pub fn decode(data: &[u8]) -> Result<Self> {
        need(data, 9)?;
        if data.len() != 9 {
            return Err(Error::Invalid("AK09911 register length"));
        }
        let raw = std::array::from_fn(|axis| {
            i16::from_le_bytes([data[1 + axis * 2], data[2 + axis * 2]])
        });
        Ok(Self {
            status: data[0],
            raw,
            microtesla: raw.map(|x| x as f32 * (4912.0 / 8190.0)),
            status2: data[8],
        })
    }
}

pub struct Client<T> {
    session: Session<T>,
}
impl<T: ReportIo> Client<T> {
    /// Construction does not reset sensors. Call `initialize_sensors` explicitly.
    pub fn new(io: T, timeout: Duration) -> Result<Self> {
        Ok(Self {
            session: Session::new(io, timeout)?,
        })
    }
    pub fn into_inner(self) -> T {
        self.session.io
    }
    pub fn command(&mut self, command: [u8; 3], payload: &[u8]) -> Result<Vec<u8>> {
        let request = Packet::request(command, payload).encode()?;
        self.session.exchange(&request, |data| {
            let reply = Packet::decode(data)?;
            if reply.command != command {
                return Ok(None);
            }
            if reply.escape_index != 0xff {
                return Err(Error::Invalid("unverified Mad Gaze delimiter substitution"));
            }
            // The reference does not establish whether firmware echoes the
            // session ID. Preserve it in Packet without assuming correlation.
            Ok(Some(reply.payload))
        })
    }
    pub fn serial(&mut self) -> Result<String> {
        String::from_utf8(self.command(*b"GSN", &[])?)
            .map_err(|_| Error::Invalid("Mad Gaze serial is not UTF-8"))
    }
    pub fn display_mode(&mut self) -> Result<u8> {
        let data = self.command(*b"G3D", &[])?;
        if data.len() != 1 {
            return Err(Error::Invalid("Mad Gaze display mode length"));
        }
        Ok(data[0])
    }
    pub fn set_stereo(&mut self, enabled: bool) -> Result<()> {
        ack(&self.command(*b"S3D", &[u8::from(enabled)])?)
    }
    pub fn set_brightness(&mut self, level: u8) -> Result<()> {
        if !(1..=7).contains(&level) {
            return Err(Error::Invalid("Mad Gaze brightness must be 1..7"));
        }
        ack(&self.command(*b"SLB", &[level])?)
    }
    pub fn read_i2c(&mut self, address: u8, register: u8, length: u8) -> Result<Vec<u8>> {
        if length == 0 || length > 245 {
            return Err(Error::Invalid("Mad Gaze I2C read length"));
        }
        let reply = self.command(*b"I2R", &[1, address, register, 0, length])?;
        need(&reply, 5)?;
        if reply.len() != 5 + length as usize {
            return Err(Error::Invalid("Mad Gaze I2C data length"));
        }
        Ok(reply[5..].to_vec())
    }
    pub fn write_i2c(&mut self, address: u8, register: u8, data: &[u8]) -> Result<()> {
        if data.is_empty() || data.len() > 245 {
            return Err(Error::Invalid("Mad Gaze I2C write length"));
        }
        let mut payload = vec![1, address, register, 0, data.len() as u8];
        payload.extend(data);
        let reply = self.command(*b"I2W", &payload)?;
        need(&reply, 5)?;
        if reply[4] != 0 {
            return Err(Error::DeviceStatus(reply[4]));
        }
        Ok(())
    }
    /// Resets and enables the AK09911 and BMI160, including headerless FIFO.
    pub fn initialize_sensors(&mut self) -> Result<()> {
        if self.read_i2c(12, 0, 2)? != [0x48, 0x05] {
            return Err(Error::Invalid("AK09911 identity"));
        }
        self.write_i2c(12, 0x32, &[1])?;
        std::thread::sleep(Duration::from_millis(10));
        self.write_i2c(12, 0x31, &[8])?;
        if self.read_i2c(104, 0, 1)? != [0xd1] {
            return Err(Error::Invalid("BMI160 identity"));
        }
        self.write_i2c(104, 0x7e, &[0xb6])?;
        std::thread::sleep(Duration::from_millis(10));
        for (register, value) in [(0x7e, 0x11), (0x7e, 0x15), (0x47, 0xc0)] {
            self.write_i2c(104, register, &[value])?;
        }
        Ok(())
    }
    /// Poll up to two FIFO entries. An empty FIFO returns an empty vector.
    pub fn read_imu(&mut self) -> Result<Vec<ImuSample>> {
        let bytes = self.read_i2c(104, 0x24, 24)?;
        let mut samples = Vec::new();
        for entry in bytes.chunks_exact(12) {
            match ImuSample::decode(entry)? {
                Some(sample) => samples.push(sample),
                None => break,
            }
        }
        Ok(samples)
    }
    pub fn read_magnetometer(&mut self) -> Result<MagnetometerSample> {
        MagnetometerSample::decode(&self.read_i2c(12, 0x10, 9)?)
    }
}
fn ack(data: &[u8]) -> Result<()> {
    if data == [0] {
        Ok(())
    } else if data.len() == 1 {
        Err(Error::DeviceStatus(data[0]))
    } else {
        Err(Error::Invalid("Mad Gaze acknowledgement"))
    }
}
