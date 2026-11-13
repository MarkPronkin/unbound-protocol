//! Shared deadline and failed-exchange rules for report-based protocols.
use crate::{Error, Result, error::remaining, usb::ReportIo};
use std::time::{Duration, Instant};

pub(crate) fn read<T: ReportIo>(io: &mut T, buffer: &mut [u8], timeout: Duration) -> Result<usize> {
    let n = io.read_report(buffer, timeout)?;
    if n == 0 {
        return Err(Error::Timeout);
    }
    if n > buffer.len() {
        return Err(Error::Invalid("report adapter length"));
    }
    Ok(n)
}

pub(crate) struct Session<T> {
    pub io: T,
    pub timeout: Duration,
    usable: bool,
}
impl<T: ReportIo> Session<T> {
    pub fn new(io: T, timeout: Duration) -> Result<Self> {
        if timeout.is_zero() {
            return Err(Error::Invalid("zero report timeout"));
        }
        Ok(Self {
            io,
            timeout,
            usable: true,
        })
    }
    pub fn check(&self) -> Result<()> {
        if self.usable {
            Ok(())
        } else {
            Err(Error::Disconnected)
        }
    }
    pub fn write(&mut self, request: &[u8]) -> Result<()> {
        self.check()?;
        let result = self.io.write_report(request, self.timeout);
        if result.is_err() {
            self.usable = false;
        }
        result
    }
    pub fn exchange<R>(
        &mut self,
        request: &[u8],
        mut accept: impl FnMut(&[u8]) -> Result<Option<R>>,
    ) -> Result<R> {
        self.transaction(|io, timeout| {
            let start = Instant::now();
            io.write_report(request, timeout)?;
            loop {
                let mut buffer = [0; 1024];
                let n = read(io, &mut buffer, remaining(start, timeout)?)?;
                if let Some(reply) = accept(&buffer[..n])? {
                    return Ok(reply);
                }
            }
        })
    }
    pub fn transaction<R>(
        &mut self,
        operation: impl FnOnce(&mut T, Duration) -> Result<R>,
    ) -> Result<R> {
        self.check()?;
        let result = operation(&mut self.io, self.timeout);
        // No transaction IDs: a late reply cannot safely be reused.
        if result.is_err() {
            self.usable = false;
        }
        result
    }
}
