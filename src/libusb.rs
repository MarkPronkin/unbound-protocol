//! Optional libusb backend. Links the shared system libusb; see `NOTICE`.
//! Open an explicit bus/address and interface/endpoint;
//! no first-device selection or USB configuration changes are performed.
use crate::{
    Error, Result,
    usb::control::{ControlIo, Setup},
};
use rusb::{DeviceHandle, GlobalContext};
use std::time::Duration;

pub struct UsbTransport {
    handle: DeviceHandle<GlobalContext>,
}
impl UsbTransport {
    pub fn open_interface(bus: u8, address: u8, interface: u8) -> Result<Self> {
        let mut handle = find(bus, address)?.open()?;
        detach(&mut handle)?;
        handle.claim_interface(interface)?;
        Ok(Self { handle })
    }
    /// Select the interface and alternate setting declaring this interrupt IN endpoint.
    pub fn open_interrupt(bus: u8, address: u8, endpoint: u8) -> Result<Self> {
        let device = find(bus, address)?;
        let config = device.active_config_descriptor()?;
        let mut selected = None;
        for interface in config.interfaces() {
            for desc in interface.descriptors() {
                if desc.endpoint_descriptors().any(|ep| {
                    ep.address() == endpoint
                        && ep.transfer_type() == rusb::TransferType::Interrupt
                        && ep.direction() == rusb::Direction::In
                }) {
                    if selected.is_some() {
                        return Err(Error::Invalid("ambiguous USB interrupt endpoint"));
                    }
                    selected = Some((interface.number(), desc.setting_number()));
                }
            }
        }
        let (interface, setting) =
            selected.ok_or(Error::Invalid("USB interrupt endpoint not found"))?;
        let transport = Self::open_interface(bus, address, interface)?;
        if setting != 0 {
            transport.handle.set_alternate_setting(interface, setting)?;
        }
        Ok(transport)
    }
}
fn find(bus: u8, address: u8) -> Result<rusb::Device<GlobalContext>> {
    rusb::devices()?
        .iter()
        .find(|d| d.bus_number() == bus && d.address() == address)
        .ok_or(Error::Disconnected)
}
fn detach(handle: &mut DeviceHandle<GlobalContext>) -> Result<()> {
    match handle.set_auto_detach_kernel_driver(true) {
        Ok(()) | Err(rusb::Error::NotSupported) => Ok(()),
        Err(e) => Err(e.into()),
    }
}
fn deadline(timeout: Duration) -> Result<()> {
    if timeout.is_zero() {
        Err(Error::Timeout)
    } else {
        Ok(())
    }
}
impl ControlIo for UsbTransport {
    fn read_control(&mut self, s: Setup, data: &mut [u8], timeout: Duration) -> Result<usize> {
        deadline(timeout)?;
        Ok(self
            .handle
            .read_control(s.request_type, s.request, s.value, s.index, data, timeout)?)
    }
    fn write_control(&mut self, s: Setup, data: &[u8], timeout: Duration) -> Result<()> {
        deadline(timeout)?;
        let n = self.handle.write_control(
            s.request_type,
            s.request,
            s.value,
            s.index,
            data,
            timeout,
        )?;
        if n != data.len() {
            return Err(Error::Invalid("short USB control write"));
        }
        Ok(())
    }
    fn read_interrupt(
        &mut self,
        endpoint: u8,
        data: &mut [u8],
        timeout: Duration,
    ) -> Result<usize> {
        deadline(timeout)?;
        let n = self.handle.read_interrupt(endpoint, data, timeout)?;
        if n == 0 {
            return Err(Error::Timeout);
        }
        Ok(n)
    }
}
