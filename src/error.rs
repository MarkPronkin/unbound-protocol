use std::{fmt, io};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    Io(io::Error),
    Truncated {
        needed: usize,
        actual: usize,
    },
    Invalid(&'static str),
    /// The selected model or transport does not implement this operation.
    Unsupported(&'static str),
    TooLarge {
        size: usize,
        limit: usize,
    },
    Checksum {
        expected: u32,
        actual: u32,
    },
    DeviceStatus(u8),
    Timeout,
    QueueFull,
    Disconnected,
    #[cfg(feature = "hid")]
    Hid(hidapi::HidError),
    #[cfg(feature = "libusb")]
    Usb(rusb::Error),
    #[cfg(feature = "serial")]
    Serial(serialport::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O: {e}"),
            Self::Truncated { needed, actual } => {
                write!(f, "truncated: need {needed} bytes, got {actual}")
            }
            Self::Invalid(s) => write!(f, "invalid protocol data: {s}"),
            Self::Unsupported(operation) => write!(f, "unsupported operation: {operation}"),
            Self::TooLarge { size, limit } => write!(f, "size {size} exceeds limit {limit}"),
            Self::Checksum { expected, actual } => write!(
                f,
                "CRC32 mismatch: expected {expected:08x}, got {actual:08x}"
            ),
            Self::DeviceStatus(s) => write!(f, "device returned status {s}"),
            Self::Timeout => f.write_str("operation timed out"),
            Self::QueueFull => f.write_str("unsolicited event queue is full; drain events"),
            Self::Disconnected => f.write_str("connection closed or unusable; reconnect"),
            #[cfg(feature = "hid")]
            Self::Hid(e) => write!(f, "HID: {e}"),
            #[cfg(feature = "libusb")]
            Self::Usb(e) => write!(f, "USB: {e}"),
            #[cfg(feature = "serial")]
            Self::Serial(e) => write!(f, "serial: {e}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            #[cfg(feature = "hid")]
            Self::Hid(e) => Some(e),
            #[cfg(feature = "libusb")]
            Self::Usb(e) => Some(e),
            #[cfg(feature = "serial")]
            Self::Serial(e) => Some(e),
            _ => None,
        }
    }
}
#[cfg(feature = "libusb")]
impl From<rusb::Error> for Error {
    fn from(e: rusb::Error) -> Self {
        match e {
            rusb::Error::Timeout => Self::Timeout,
            rusb::Error::NoDevice => Self::Disconnected,
            _ => Self::Usb(e),
        }
    }
}
#[cfg(feature = "serial")]
impl From<serialport::Error> for Error {
    fn from(e: serialport::Error) -> Self {
        Self::Serial(e)
    }
}
impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        match e.kind() {
            io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => Self::Timeout,
            _ => Self::Io(e),
        }
    }
}
#[cfg(feature = "hid")]
impl From<hidapi::HidError> for Error {
    fn from(e: hidapi::HidError) -> Self {
        Self::Hid(e)
    }
}

pub(crate) fn need(data: &[u8], n: usize) -> Result<()> {
    if data.len() < n {
        Err(Error::Truncated {
            needed: n,
            actual: data.len(),
        })
    } else {
        Ok(())
    }
}
pub(crate) fn remaining(
    start: std::time::Instant,
    timeout: std::time::Duration,
) -> Result<std::time::Duration> {
    timeout
        .checked_sub(start.elapsed())
        .filter(|d| !d.is_zero())
        .ok_or(Error::Timeout)
}
