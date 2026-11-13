//! Optional open-source hidapi backend. Select an explicit HID path from
//! `HidApi::device_list()`; never open the first matching VID/PID interface.
use crate::{
    Error, Result,
    usb::{Profile, ReportIo},
};
use hidapi::{HidApi, HidDevice};
use std::{ffi::CStr, time::Duration};

#[derive(Debug, Clone, Copy)]
pub enum ReportPrefix {
    ProtocolByte,
    Zero,
}
pub struct HidTransport {
    device: HidDevice,
    prefix: ReportPrefix,
}
impl HidTransport {
    pub fn open(api: &HidApi, path: &CStr, profile: Profile) -> Result<Self> {
        let prefix = if cfg!(target_os = "windows") && profile == Profile::Air {
            ReportPrefix::Zero
        } else {
            ReportPrefix::ProtocolByte
        };
        Self::open_with_prefix(api, path, prefix)
    }
    /// Open other report protocols with an explicit OS report-ID convention.
    /// Light uses `ProtocolByte` for both MCU and OV580 reports.
    pub fn open_with_prefix(api: &HidApi, path: &CStr, prefix: ReportPrefix) -> Result<Self> {
        #[cfg(target_os = "macos")]
        api.set_open_exclusive(false);
        Ok(Self {
            device: api.open_path(path)?,
            prefix,
        })
    }
    /// Override for HID backends that require an explicit zero report ID.
    pub fn with_prefix(mut self, prefix: ReportPrefix) -> Self {
        self.prefix = prefix;
        self
    }
}
impl ReportIo for HidTransport {
    fn write_report(&mut self, report: &[u8], timeout: Duration) -> Result<()> {
        if timeout.is_zero() {
            return Err(Error::Timeout);
        }
        let mut prefixed;
        let data = match self.prefix {
            ReportPrefix::ProtocolByte => report,
            ReportPrefix::Zero => {
                prefixed = Vec::with_capacity(report.len() + 1);
                prefixed.push(0);
                prefixed.extend(report);
                &prefixed
            }
        };
        // hidapi exposes no write deadline. Its platform driver's write timeout
        // applies here; reads and the rest of each exchange have SDK deadlines.
        let n = self.device.write(data)?;
        if n != data.len() {
            return Err(Error::Invalid("short HID report write"));
        }
        Ok(())
    }
    fn read_report(&mut self, buffer: &mut [u8], timeout: Duration) -> Result<usize> {
        if timeout.is_zero() {
            return Err(Error::Timeout);
        }
        let millis = timeout.as_millis().max(1).min(i32::MAX as u128) as i32;
        let n = self.device.read_timeout(buffer, millis)?;
        if n == 0 {
            return Err(Error::Timeout);
        }
        Ok(n)
    }
}
