//! Independent AR glasses wire protocols. See `docs/PROTOCOL.md` for layouts
//! and evidence. No vendor SDK, firmware, Unity runtime, or Android service is used.
//! Applications can use [`glasses::Glasses`] for a common interface across all
//! supported families, with explicit connections and transport capabilities.
//!
//! ```no_run
//! use unbound_protocol::one::{ControlClient, ConnectionOptions};
//! # fn main() -> unbound_protocol::Result<()> {
//! let mut glasses = ControlClient::connect(&ConnectionOptions::default())?;
//! println!("{}", glasses.software_version()?);
//! glasses.set_brightness(5)?;
//! # Ok(()) }
//! ```
pub mod air;
pub mod device;
mod error;
pub mod glasses;
pub mod grawoow;
#[cfg(feature = "hid")]
pub mod hid;
#[cfg(feature = "libusb")]
pub mod libusb;
pub mod light;
pub mod mad_gaze;
pub mod one;
pub mod protobuf;
mod report;
pub mod rokid;
#[cfg(feature = "serial")]
pub mod serial;
pub mod usb;
pub mod wire;

pub use error::{Error, Result};
