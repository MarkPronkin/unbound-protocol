//! Air MCU event payloads. Status is not stripped from unsolicited reports.
use super::Packet;
use crate::{Error, Result, error::need};

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Event {
    ForeheadTemperature(i32),
    TempleTemperature(i32),
    Proximity(i32),
    Key {
        key_type: u32,
        function: u32,
        parameter: u32,
    },
    Sleep(i32),
    Vsync {
        sequence: u64,
        timestamp_ns: u64,
    },
    ScreenStatus(i32),
    OverTemperature(i32),
    CalibrationResult {
        kind: i32,
        result: i32,
    },
    Unknown(Packet),
}
impl Event {
    pub fn decode(packet: &Packet) -> Result<Self> {
        let data = &packet.payload;
        let scalar = || -> Result<i32> {
            need(data, 4)?;
            Ok(i32::from_le_bytes(data[..4].try_into().unwrap()))
        };
        Ok(match packet.command {
            0x6c12 => Self::ForeheadTemperature(scalar()?),
            0x6c02 => Self::TempleTemperature(scalar()?),
            0x6c04 => Self::Proximity(scalar()?),
            0x6c05 => {
                need(data, 12)?;
                Self::Key {
                    key_type: u32::from_le_bytes(data[..4].try_into().unwrap()),
                    function: u32::from_le_bytes(data[4..8].try_into().unwrap()),
                    parameter: u32::from_le_bytes(data[8..12].try_into().unwrap()),
                }
            }
            0x6c07 => Self::Sleep(scalar()?),
            0x6c0b => {
                need(data, 16)?;
                Self::Vsync {
                    sequence: u64::from_le_bytes(data[..8].try_into().unwrap()),
                    timestamp_ns: u64::from_le_bytes(data[8..16].try_into().unwrap()),
                }
            }
            0x6c18 => Self::ScreenStatus(scalar()?),
            0x6c19 => Self::OverTemperature(scalar()?),
            0x6c1a => {
                need(data, 8)?;
                Self::CalibrationResult {
                    kind: scalar()?,
                    result: i32::from_le_bytes(data[4..8].try_into().unwrap()),
                }
            }
            _ => Self::Unknown(packet.clone()),
        })
    }
    pub fn is_known(&self) -> bool {
        !matches!(self, Self::Unknown(_))
    }
}
impl TryFrom<&Packet> for Event {
    type Error = Error;
    fn try_from(packet: &Packet) -> Result<Self> {
        Self::decode(packet)
    }
}
