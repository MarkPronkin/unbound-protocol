//! Shared CRC and bounded TCP framing. TCP uses a two-byte big-endian opcode
//! followed by a four-byte big-endian body length.
use crate::{Error, Result, error::need};
use std::io::Read;

pub const MAX_BODY: usize = 1024 * 1024;

/// CRC-32/ISO-HDLC (IEEE, reflected 0xedb88320, init/final xor 0xffffffff).
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub opcode: u16,
    pub body: Vec<u8>,
}

impl Frame {
    pub fn encode(&self) -> Result<Vec<u8>> {
        if self.body.len() > MAX_BODY {
            return Err(Error::TooLarge {
                size: self.body.len(),
                limit: MAX_BODY,
            });
        }
        let mut out = Vec::with_capacity(6 + self.body.len());
        out.extend(self.opcode.to_be_bytes());
        out.extend((self.body.len() as u32).to_be_bytes());
        out.extend(&self.body);
        Ok(out)
    }
    /// Decode exactly one complete frame; trailing bytes are rejected.
    pub fn decode(data: &[u8]) -> Result<Self> {
        need(data, 6)?;
        let len = body_length(&data[..6])?;
        need(data, 6 + len)?;
        if data.len() != 6 + len {
            return Err(Error::Invalid("trailing TCP frame bytes"));
        }
        Ok(Self {
            opcode: u16::from_be_bytes([data[0], data[1]]),
            body: data[6..].to_vec(),
        })
    }
}

fn body_length(header: &[u8]) -> Result<usize> {
    let len = u32::from_be_bytes(header[2..6].try_into().unwrap()) as usize;
    if len > MAX_BODY {
        return Err(Error::TooLarge {
            size: len,
            limit: MAX_BODY,
        });
    }
    Ok(len)
}

/// Retains partially read headers/bodies across I/O timeouts. Invalid framing,
/// EOF, or a permanent I/O error poisons the reader; callers must reconnect.
#[derive(Debug, Default)]
pub struct FrameReader {
    header: [u8; 6],
    header_read: usize,
    body: Vec<u8>,
    body_read: usize,
    poisoned: bool,
}
impl FrameReader {
    pub fn read(&mut self, reader: &mut impl Read) -> Result<Frame> {
        if self.poisoned {
            return Err(Error::Disconnected);
        }
        let result = self.read_inner(reader);
        if result.is_err() && !matches!(result, Err(Error::Timeout)) {
            self.poisoned = true;
        }
        result
    }
    fn read_inner(&mut self, reader: &mut impl Read) -> Result<Frame> {
        fill(reader, &mut self.header, &mut self.header_read)?;
        let len = body_length(&self.header)?;
        self.body.resize(len, 0);
        fill(reader, &mut self.body, &mut self.body_read)?;
        let frame = Frame {
            opcode: u16::from_be_bytes([self.header[0], self.header[1]]),
            body: std::mem::take(&mut self.body),
        };
        self.header_read = 0;
        self.body_read = 0;
        Ok(frame)
    }
}
fn fill(reader: &mut impl Read, data: &mut [u8], offset: &mut usize) -> Result<()> {
    while *offset < data.len() {
        match reader.read(&mut data[*offset..]) {
            Ok(0) => return Err(Error::Disconnected),
            Ok(n) => *offset += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}
