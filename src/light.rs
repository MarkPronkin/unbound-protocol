//! XREAL / Nreal Light MCU and OV580 HID protocols (not the Air protocol).
//! Stereo mode requires `ControlClient::heartbeat` at least every 250 ms,
//! including while reading the separate sensor interface. See NOTICE.
use crate::{
    Error, Result,
    error::{need, remaining},
    report::{self, Session},
    usb::ReportIo,
    wire::crc32,
};
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

pub const MCU_ID: (u16, u16) = (0x0486, 0x573c);
pub const SENSOR_ID: (u16, u16) = (0x05a9, 0x0680);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DisplayMode {
    Mirror = b'1',
    HalfStereo = b'2',
    Stereo = b'3',
    Stereo72 = b'4',
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    pub category: u8,
    pub command: u8,
    pub payload: Vec<u8>,
    /// ASCII device timestamp, preserved without assuming a unit or radix.
    pub timestamp: Vec<u8>,
}
impl Packet {
    pub fn request(category: u8, command: u8, payload: &[u8]) -> Self {
        Self {
            category,
            command,
            payload: payload.to_vec(),
            timestamp: b"0".to_vec(),
        }
    }
    pub fn encode(&self) -> Result<[u8; 64]> {
        let valid = |s: &[u8]| !s.iter().any(|b| matches!(b, 2 | 3 | b':'));
        if !valid(&[self.category, self.command])
            || !valid(&self.payload)
            || !valid(&self.timestamp)
        {
            return Err(Error::Invalid("Light packet contains a delimiter"));
        }
        let len = 18usize
            .saturating_add(self.payload.len())
            .saturating_add(self.timestamp.len());
        if len > 64 {
            return Err(Error::TooLarge {
                size: len,
                limit: 64,
            });
        }
        let mut bytes = vec![2, b':', self.category, b':', self.command, b':'];
        bytes.extend(&self.payload);
        bytes.push(b':');
        bytes.extend(&self.timestamp);
        bytes.push(b':');
        bytes.extend(format!("{:>8x}", crc32(&bytes)).bytes());
        bytes.extend([b':', 3]);
        let mut out = [0; 64];
        out[..bytes.len()].copy_from_slice(&bytes);
        Ok(out)
    }
    pub fn decode(data: &[u8]) -> Result<Self> {
        need(data, 18)?;
        if data.len() > 64 || data[0] != 2 {
            return Err(Error::Invalid("Light MCU report header"));
        }
        let end = data
            .iter()
            .position(|&b| b == 3)
            .ok_or(Error::Invalid("Light packet terminator"))?;
        let fields: Vec<_> = data[1..end].split(|&b| b == b':').collect();
        if fields.len() != 7
            || !fields[0].is_empty()
            || !fields[6].is_empty()
            || fields[1].len() != 1
            || fields[2].len() != 1
            || fields[5].len() != 8
        {
            return Err(Error::Invalid("Light MCU packet fields"));
        }
        let crc_text =
            std::str::from_utf8(fields[5]).map_err(|_| Error::Invalid("Light checksum text"))?;
        let actual = u32::from_str_radix(crc_text.trim_start(), 16)
            .map_err(|_| Error::Invalid("Light checksum value"))?;
        let crc_start = end - 9;
        let expected = crc32(&data[..crc_start]);
        if expected != actual {
            return Err(Error::Checksum { expected, actual });
        }
        Ok(Self {
            category: fields[1][0],
            command: fields[2][0],
            payload: fields[3].to_vec(),
            timestamp: fields[4].to_vec(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    KeyPress(u8),
    ProximityNear,
    ProximityFar,
    AmbientLight(u16),
    VSync,
    Unknown(Packet),
}
impl Event {
    pub fn decode(packet: Packet) -> Result<Self> {
        Ok(
            match (packet.category, packet.command, packet.payload.as_slice()) {
                (b'5', b'K', b"UP") => Self::KeyPress(0),
                (b'5', b'K', b"DN") => Self::KeyPress(1),
                (b'5', b'P', b"near") => Self::ProximityNear,
                (b'5', b'P', b"away") => Self::ProximityFar,
                (b'5', b'L', data) => Self::AmbientLight(
                    u16::from_str_radix(
                        std::str::from_utf8(data)
                            .map_err(|_| Error::Invalid("Light ambient light text"))?,
                        16,
                    )
                    .map_err(|_| Error::Invalid("Light ambient light value"))?,
                ),
                (b'5', b'S', _) => Self::VSync,
                _ => Self::Unknown(packet),
            },
        )
    }
}

pub struct ControlClient<T> {
    session: Session<T>,
    events: VecDeque<Packet>,
    last_heartbeat: Instant,
}
impl<T: ReportIo> ControlClient<T> {
    /// Opens no streams and changes no settings. Call `enable_stereo` explicitly.
    pub fn new(io: T, timeout: Duration) -> Result<Self> {
        Ok(Self {
            session: Session::new(io, timeout)?,
            events: VecDeque::new(),
            last_heartbeat: Instant::now(),
        })
    }
    pub fn into_inner(self) -> T {
        self.session.io
    }
    pub fn pop_event(&mut self) -> Option<Packet> {
        self.events.pop_front()
    }
    pub fn command(&mut self, category: u8, command: u8, payload: &[u8]) -> Result<Vec<u8>> {
        let response_category = category
            .checked_add(1)
            .ok_or(Error::Invalid("Light request category"))?;
        if self.events.len() >= 128 {
            return Err(Error::QueueFull);
        }
        let request = Packet::request(category, command, payload).encode()?;
        let events = &mut self.events;
        let last_heartbeat = &mut self.last_heartbeat;
        self.session.transaction(|io, timeout| {
            let start = Instant::now();
            io.write_report(&request, timeout)?;
            loop {
                send_heartbeat(io, last_heartbeat, remaining(start, timeout)?)?;
                let wait = remaining(start, timeout)?.min(heartbeat_wait(*last_heartbeat));
                if wait.is_zero() {
                    continue;
                }
                let mut buffer = [0; 64];
                let n = match report::read(io, &mut buffer, wait) {
                    Err(Error::Timeout) => continue,
                    result => result?,
                };
                let packet = Packet::decode(&buffer[..n])?;
                if packet.category == response_category && packet.command == command {
                    return Ok(packet.payload);
                }
                if (packet.category, packet.command) != (b'A', b'K') {
                    events.push_back(packet);
                    if events.len() >= 128 {
                        return Err(Error::QueueFull);
                    }
                }
            }
        })
    }
    pub fn serial(&mut self) -> Result<String> {
        String::from_utf8(self.command(b'3', b'C', b"x")?)
            .map_err(|_| Error::Invalid("Light serial is not UTF-8"))
    }
    pub fn display_mode(&mut self) -> Result<Vec<u8>> {
        self.command(b'3', b'3', b"x")
    }
    /// Send the SDK-presence command required by Light's stereo display.
    pub fn enable_stereo(&mut self) -> Result<()> {
        self.command(b'@', b'3', b"1")?;
        Ok(())
    }
    pub fn set_display_mode(&mut self, mode: DisplayMode) -> Result<()> {
        let reply = self.command(b'1', b'3', &[mode as u8])?;
        if reply.first() != Some(&(mode as u8)) {
            return Err(Error::Invalid("Light display mode acknowledgement"));
        }
        Ok(())
    }
    pub fn set_events_enabled(&mut self, enabled: bool) -> Result<()> {
        let value = if enabled { b"1" } else { b"0" };
        self.command(b'1', b'L', value)?;
        self.command(b'1', b'N', value)?;
        Ok(())
    }
    /// Call regularly even when sensor reads occur on another interface. Does
    /// not wait for an acknowledgement: Light sometimes omits that reply.
    pub fn heartbeat(&mut self) -> Result<()> {
        self.session.check()?;
        if self.last_heartbeat.elapsed() >= Duration::from_millis(250) {
            self.session
                .write(&Packet::request(b'@', b'K', b"x").encode()?)?;
            self.last_heartbeat = Instant::now();
        }
        Ok(())
    }
    pub(crate) fn heartbeat_due_in(&self) -> Duration {
        heartbeat_wait(self.last_heartbeat)
    }
    pub fn next_event(&mut self) -> Result<Event> {
        let start = Instant::now();
        loop {
            self.heartbeat()?;
            if let Some(packet) = self.pop_event() {
                return Event::decode(packet);
            }
            let timeout =
                remaining(start, self.session.timeout)?.min(heartbeat_wait(self.last_heartbeat));
            if timeout.is_zero() {
                continue;
            }
            let mut buffer = [0; 64];
            let n = match report::read(&mut self.session.io, &mut buffer, timeout) {
                Err(Error::Timeout) => continue,
                result => result?,
            };
            let packet = Packet::decode(&buffer[..n])?;
            if (packet.category, packet.command) != (b'A', b'K') {
                return Event::decode(packet);
            }
        }
    }
}

fn heartbeat_wait(last: Instant) -> Duration {
    Duration::from_millis(250).saturating_sub(last.elapsed())
}
fn send_heartbeat<T: ReportIo>(io: &mut T, last: &mut Instant, timeout: Duration) -> Result<()> {
    if heartbeat_wait(*last).is_zero() {
        io.write_report(&Packet::request(b'@', b'K', b"x").encode()?, timeout)?;
        *last = Instant::now();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct HeartbeatIo {
        writes: Vec<Vec<u8>>,
    }
    impl ReportIo for HeartbeatIo {
        fn write_report(&mut self, data: &[u8], _: Duration) -> Result<()> {
            self.writes.push(data.to_vec());
            Ok(())
        }
        fn read_report(&mut self, data: &mut [u8], _: Duration) -> Result<usize> {
            let reply = Packet::request(b'4', b'C', b"SN").encode()?;
            data[..64].copy_from_slice(&reply);
            Ok(64)
        }
    }
    #[test]
    fn heartbeat_is_rate_limited_and_does_not_wait_for_reply() {
        let mut client =
            ControlClient::new(HeartbeatIo::default(), Duration::from_secs(1)).unwrap();
        client.last_heartbeat = Instant::now() - Duration::from_millis(251);
        client.heartbeat().unwrap();
        client.heartbeat().unwrap();
        assert_eq!(client.session.io.writes.len(), 1);
        assert_eq!(
            Packet::decode(&client.session.io.writes[0]).unwrap(),
            Packet::request(b'@', b'K', b"x")
        );
    }
    #[test]
    fn waiting_for_command_also_services_due_heartbeat() {
        let mut client =
            ControlClient::new(HeartbeatIo::default(), Duration::from_secs(1)).unwrap();
        client.last_heartbeat = Instant::now() - Duration::from_millis(251);
        assert_eq!(client.serial().unwrap(), "SN");
        assert_eq!(client.session.io.writes.len(), 2);
        assert_eq!(
            Packet::decode(&client.session.io.writes[1])
                .unwrap()
                .command,
            b'K'
        );
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImuSample {
    pub gyro_timestamp_ns: u64,
    pub accel_timestamp_ns: u64,
    pub gyro_raw: [i32; 3],
    pub accel_raw: [i32; 3],
    pub gyro_rad_s: [f32; 3],
    pub accel_m_s2: [f32; 3],
}
impl ImuSample {
    pub fn decode(data: &[u8]) -> Result<Self> {
        need(data, 100)?;
        if data.len() > 128 || data[0] != 1 {
            return Err(Error::Invalid("Light IMU report header/length"));
        }
        let u32_at = |i| u32::from_le_bytes(data[i..i + 4].try_into().unwrap());
        let axes = |offset| std::array::from_fn(|axis| u32_at(offset + axis * 4) as i32);
        let gyro_raw: [i32; 3] = axes(60);
        let accel_raw: [i32; 3] = axes(88);
        let gyro_div = u32_at(56);
        let accel_div = u32_at(84);
        if gyro_div == 0 || accel_div == 0 {
            return Err(Error::Invalid("zero Light IMU scale denominator"));
        }
        Ok(Self {
            gyro_timestamp_ns: u64::from_le_bytes(data[44..52].try_into().unwrap()),
            accel_timestamp_ns: u64::from_le_bytes(data[72..80].try_into().unwrap()),
            gyro_raw,
            accel_raw,
            gyro_rad_s: gyro_raw
                .map(|x| (x as f32 * (u32_at(52) as f32 / gyro_div as f32)).to_radians()),
            accel_m_s2: accel_raw.map(|x| x as f32 * (u32_at(80) as f32 / accel_div as f32) * 9.81),
        })
    }
}

pub struct SensorClient<T> {
    session: Session<T>,
}
impl<T: ReportIo> SensorClient<T> {
    pub fn new(io: T, timeout: Duration) -> Result<Self> {
        Ok(Self {
            session: Session::new(io, timeout)?,
        })
    }
    pub fn into_inner(self) -> T {
        self.session.io
    }
    pub fn command(&mut self, command: u8, subcommand: u8) -> Result<Vec<u8>> {
        self.session
            .exchange(&[2, command, subcommand, 0, 0, 0, 0], |data| {
                match data[0] {
                    2 => {
                        if data.len() > 128 {
                            return Err(Error::Invalid("Light sensor reply length"));
                        }
                        Ok(Some(data.to_vec()))
                    }
                    1 => {
                        ImuSample::decode(data)?;
                        Ok(None)
                    }
                    _ => Err(Error::Invalid("Light sensor report ID")),
                }
            })
    }
    pub fn set_streaming(&mut self, enabled: bool) -> Result<()> {
        self.command(0x19, u8::from(enabled))?;
        Ok(())
    }
    /// Call while streaming is stopped. The binary configuration envelope is
    /// retained, including camera calibration and unknown sections.
    pub fn read_configuration_blob(&mut self) -> Result<Vec<u8>> {
        self.command(0x14, 0)?;
        let mut bytes = Vec::new();
        loop {
            let reply = self.command(0x15, 0)?;
            need(&reply, 2)?;
            if reply[1] != 1 {
                return Ok(bytes);
            }
            need(&reply, 3)?;
            let len = reply[2] as usize;
            if len == 0 {
                return Err(Error::Invalid("empty Light configuration chunk"));
            }
            need(&reply, 3 + len)?;
            if bytes.len() + len > crate::wire::MAX_BODY {
                return Err(Error::TooLarge {
                    size: bytes.len() + len,
                    limit: crate::wire::MAX_BODY,
                });
            }
            bytes.extend_from_slice(&reply[3..3 + len]);
        }
    }
    pub fn read_configuration(&mut self) -> Result<String> {
        configuration_json(&self.read_configuration_blob()?)
    }
    pub fn next_sample(&mut self) -> Result<ImuSample> {
        self.session.check()?;
        let start = Instant::now();
        loop {
            let mut buffer = [0; 128];
            let n = report::read(
                &mut self.session.io,
                &mut buffer,
                remaining(start, self.session.timeout)?,
            )?;
            if buffer[0] == 1 {
                return ImuSample::decode(&buffer[..n]);
            }
            if buffer[0] != 2 {
                return Err(Error::Invalid("Light sensor report ID"));
            }
        }
    }
}

/// Extract the reference driver's double-newline-delimited JSON section.
pub fn configuration_json(blob: &[u8]) -> Result<String> {
    if blob.len() > crate::wire::MAX_BODY {
        return Err(Error::TooLarge {
            size: blob.len(),
            limit: crate::wire::MAX_BODY,
        });
    }
    let tail = blob
        .get(0x28..)
        .ok_or(Error::Invalid("Light configuration header"))?;
    let start = tail
        .windows(3)
        .position(|w| w == b"\n\n{")
        .ok_or(Error::Invalid("Light configuration start marker"))?
        + 2;
    let end = tail[start..]
        .windows(2)
        .position(|w| w == b"\n\n")
        .ok_or(Error::Invalid("Light configuration end marker"))?
        + start;
    String::from_utf8(tail[start..end].to_vec())
        .map_err(|_| Error::Invalid("Light configuration is not UTF-8"))
}
