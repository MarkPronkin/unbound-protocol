//! USB setup packets and interrupt transfers, independent of libusb.
use crate::Result;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Setup {
    pub request_type: u8,
    pub request: u8,
    pub value: u16,
    pub index: u16,
}

/// Implementations must honor nonzero deadlines and return the actual length.
/// Writes send all bytes or fail. Zero duration means timeout, never infinite.
pub trait ControlIo {
    fn read_control(&mut self, setup: Setup, data: &mut [u8], timeout: Duration) -> Result<usize>;
    fn write_control(&mut self, setup: Setup, data: &[u8], timeout: Duration) -> Result<()>;
    fn read_interrupt(&mut self, endpoint: u8, data: &mut [u8], timeout: Duration)
    -> Result<usize>;
}
