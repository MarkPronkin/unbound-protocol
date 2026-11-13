//! Optional serial backend for Mad Gaze's length-delimited reports.
use crate::{Error, Result, error::remaining, mad_gaze::BAUD_RATE, usb::ReportIo};
use serialport::SerialPort;
use std::{
    io::{Read, Write},
    time::{Duration, Instant},
};

pub struct SerialTransport {
    port: Box<dyn SerialPort>,
}
impl SerialTransport {
    /// Explicit port path, 921600 baud, 8N1. Does not clear host/device buffers.
    pub fn open(path: &str, timeout: Duration) -> Result<Self> {
        if timeout.is_zero() {
            return Err(Error::Invalid("zero serial timeout"));
        }
        Ok(Self::from_port(
            serialport::new(path, BAUD_RATE)
                .data_bits(serialport::DataBits::Eight)
                .stop_bits(serialport::StopBits::One)
                .parity(serialport::Parity::None)
                .timeout(timeout)
                .open()?,
        ))
    }
    pub fn from_port(port: Box<dyn SerialPort>) -> Self {
        Self { port }
    }
    fn read_exact_until(
        &mut self,
        mut data: &mut [u8],
        start: Instant,
        timeout: Duration,
    ) -> Result<()> {
        while !data.is_empty() {
            self.port.set_timeout(remaining(start, timeout)?)?;
            match self.port.read(data) {
                Ok(0) => return Err(Error::Disconnected),
                Ok(n) => {
                    data = &mut data[n..];
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }
}
impl ReportIo for SerialTransport {
    fn write_report(&mut self, mut data: &[u8], timeout: Duration) -> Result<()> {
        if timeout.is_zero() {
            return Err(Error::Timeout);
        }
        let start = Instant::now();
        while !data.is_empty() {
            self.port.set_timeout(remaining(start, timeout)?)?;
            match self.port.write(data) {
                Ok(0) => return Err(Error::Disconnected),
                Ok(n) => data = &data[n..],
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }
    fn read_report(&mut self, data: &mut [u8], timeout: Duration) -> Result<usize> {
        let start = Instant::now();
        let mut header = [0; 5];
        loop {
            self.read_exact_until(&mut header[..1], start, timeout)?;
            if header[0] == b':' {
                break;
            }
        }
        self.read_exact_until(&mut header[1..], start, timeout)?;
        if header[4] < 5 {
            return Err(Error::Invalid("Mad Gaze serial body length"));
        }
        let len = 5 + header[4] as usize;
        if len > data.len() {
            return Err(Error::TooLarge {
                size: len,
                limit: data.len(),
            });
        }
        data[..5].copy_from_slice(&header);
        self.read_exact_until(&mut data[5..len], start, timeout)?;
        Ok(len)
    }
}
