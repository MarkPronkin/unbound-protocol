use crate::{Result, air, grawoow, light, mad_gaze, one, rokid, usb, wire::Frame};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeUnit {
    Nanoseconds,
    Microseconds,
}

/// An integer device-clock value, never host time or UTC. Units and clocks
/// must be considered before comparing samples from different sources.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timestamp {
    pub ticks: u64,
    pub unit: TimeUnit,
}
impl Timestamp {
    pub fn nanoseconds(ticks: u64) -> Self {
        Self {
            ticks,
            unit: TimeUnit::Nanoseconds,
        }
    }
    pub fn microseconds(ticks: u64) -> Self {
        Self {
            ticks,
            unit: TimeUnit::Microseconds,
        }
    }
}

/// Axes retain the device's coordinate system and have no bias correction or
/// sensor fusion. The enclosing field names specify the physical units.
#[derive(Debug, Clone, PartialEq)]
pub struct TimedVector {
    pub value: [f32; 3],
    pub timestamp: Option<Timestamp>,
}
impl TimedVector {
    fn new(value: [f32; 3], timestamp: Option<Timestamp>) -> Self {
        Self { value, timestamp }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MagneticFieldUnit {
    Microtesla,
    /// One's floating-point magnetic readings have no independently validated
    /// physical unit. They must not be interpreted as microtesla.
    DeviceNative,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MagneticField {
    pub vector: TimedVector,
    pub unit: MagneticFieldUnit,
}

/// Only fields actually supplied by this report are present. Separate Rokid
/// sensor packets are never silently combined; Light retains two sensor clocks.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SensorSample {
    pub gyro_rad_s: Option<TimedVector>,
    pub accel_m_s2: Option<TimedVector>,
    pub magnetometer: Option<MagneticField>,
    pub temperature_c: Option<f32>,
}
impl SensorSample {
    fn imu(gyro: [f32; 3], accel: [f32; 3], timestamp: Option<Timestamp>) -> Self {
        Self {
            gyro_rad_s: Some(TimedVector::new(gyro, timestamp)),
            accel_m_s2: Some(TimedVector::new(accel, timestamp)),
            ..Self::default()
        }
    }
}

/// Common event concepts. Key codes, states, light levels, and raw proximity
/// values remain device specific; no undocumented meaning is assigned to them.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Event {
    Key {
        code: u32,
        state: Option<u32>,
    },
    Buttons {
        raw: u32,
    },
    Proximity {
        /// Present only when the protocol supplies a numeric state.
        raw: Option<i32>,
        near: Option<bool>,
    },
    AmbientLight(u16),
    Vsync {
        sequence: Option<u64>,
        timestamp: Option<Timestamp>,
    },
}

/// Native decoded data is retained alongside common fields. Unknown packet
/// kinds, unvalidated registers, counters, and secondary clocks stay available.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum RawReport {
    One(Frame),
    XrealUsb(usb::Packet),
    Air(air::ImuSample),
    Light(light::ImuSample),
    LightEvent(light::Event),
    Rokid(rokid::Packet),
    Grawoow(grawoow::ImuSample),
    MadGaze(mad_gaze::ImuSample),
    MadGazeMagnetometer(mad_gaze::MagnetometerSample),
    Empty,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub sensors: Option<SensorSample>,
    pub events: Vec<Event>,
    pub raw: RawReport,
}
impl Report {
    /// Interpret an already decoded native report using the same rules as a
    /// live connection. Unknown data is retained without inventing samples.
    pub fn from_raw(raw: RawReport) -> Result<Self> {
        let mut report = Self {
            sensors: None,
            events: Vec::new(),
            raw,
        };
        match &report.raw {
            RawReport::One(frame) => match frame.opcode {
                one::opcode::SENSOR_REPORT => {
                    let native = one::SensorReport::decode(frame)?;
                    let timestamp = Some(Timestamp::nanoseconds(native.timestamp_ns));
                    match native.report_type {
                        11 => {
                            let mut sample =
                                SensorSample::imu(native.gyro_rad_s, native.accel_m_s2, timestamp);
                            sample.temperature_c = native
                                .temperature_c
                                .is_finite()
                                .then_some(native.temperature_c);
                            report.sensors = Some(sample);
                        }
                        4 => {
                            report.sensors = Some(SensorSample {
                                magnetometer: Some(MagneticField {
                                    vector: TimedVector::new(native.magnetometer, timestamp),
                                    unit: MagneticFieldUnit::DeviceNative,
                                }),
                                ..SensorSample::default()
                            })
                        }
                        _ => {}
                    }
                }
                one::opcode::KEY_EVENT => {
                    let key = one::KeyEvent::decode(frame)?;
                    report.events.push(Event::Key {
                        code: key.key,
                        state: Some(key.state),
                    });
                }
                _ => {}
            },
            RawReport::XrealUsb(packet) => match usb::events::Event::decode(packet)? {
                usb::events::Event::Key { key_type, .. } => {
                    report.events.push(Event::Key {
                        code: key_type,
                        state: None,
                    });
                }
                usb::events::Event::Proximity(raw) => {
                    report.events.push(Event::Proximity {
                        raw: Some(raw),
                        near: None,
                    });
                }
                usb::events::Event::Vsync {
                    sequence,
                    timestamp_ns,
                } => {
                    report.events.push(Event::Vsync {
                        sequence: Some(sequence),
                        timestamp: Some(Timestamp::nanoseconds(timestamp_ns)),
                    });
                }
                _ => {}
            },
            RawReport::Air(native) => {
                report.sensors = Some(SensorSample::imu(
                    native.gyro_rad_s,
                    native.accel_m_s2,
                    Some(Timestamp::nanoseconds(native.timestamp_ns)),
                ));
            }
            RawReport::Light(native) => {
                report.sensors = Some(SensorSample {
                    gyro_rad_s: Some(TimedVector::new(
                        native.gyro_rad_s,
                        Some(Timestamp::nanoseconds(native.gyro_timestamp_ns)),
                    )),
                    accel_m_s2: Some(TimedVector::new(
                        native.accel_m_s2,
                        Some(Timestamp::nanoseconds(native.accel_timestamp_ns)),
                    )),
                    ..SensorSample::default()
                });
            }
            RawReport::LightEvent(event) => match event {
                light::Event::KeyPress(code) => report.events.push(Event::Key {
                    code: (*code).into(),
                    state: None,
                }),
                light::Event::ProximityNear => report.events.push(Event::Proximity {
                    raw: None,
                    near: Some(true),
                }),
                light::Event::ProximityFar => report.events.push(Event::Proximity {
                    raw: None,
                    near: Some(false),
                }),
                light::Event::AmbientLight(value) => {
                    report.events.push(Event::AmbientLight(*value))
                }
                light::Event::VSync => report.events.push(Event::Vsync {
                    sequence: None,
                    timestamp: None,
                }),
                light::Event::Unknown(_) => {}
            },
            RawReport::Rokid(packet) => match packet {
                rokid::Packet::Misc {
                    keys, proximity, ..
                } => {
                    rokid_events(&mut report.events, *keys, *proximity);
                }
                rokid::Packet::Sensor {
                    sensor_type,
                    timestamp_us,
                    vector,
                    ..
                } => {
                    let vector =
                        TimedVector::new(*vector, Some(Timestamp::microseconds(*timestamp_us)));
                    let mut sample = SensorSample::default();
                    match sensor_type {
                        1 => sample.accel_m_s2 = Some(vector),
                        2 => sample.gyro_rad_s = Some(vector),
                        3 => {
                            sample.magnetometer = Some(MagneticField {
                                vector,
                                unit: MagneticFieldUnit::Microtesla,
                            })
                        }
                        _ => return Ok(report),
                    }
                    report.sensors = Some(sample);
                }
                rokid::Packet::Combined {
                    timestamp_ns,
                    gyro_rad_s,
                    accel_m_s2,
                    magnetometer_ut,
                    keys,
                    proximity,
                    ..
                } => {
                    let timestamp = Some(Timestamp::nanoseconds(*timestamp_ns));
                    let mut sample = SensorSample::imu(*gyro_rad_s, *accel_m_s2, timestamp);
                    sample.magnetometer = Some(MagneticField {
                        vector: TimedVector::new(*magnetometer_ut, timestamp),
                        unit: MagneticFieldUnit::Microtesla,
                    });
                    report.sensors = Some(sample);
                    rokid_events(&mut report.events, *keys, *proximity);
                }
                rokid::Packet::Unknown(_) => {}
            },
            RawReport::Grawoow(native) => {
                report.sensors = Some(SensorSample::imu(
                    native.gyro_rad_s,
                    native.accel_m_s2,
                    None,
                ));
            }
            RawReport::MadGaze(native) => {
                report.sensors = Some(SensorSample::imu(
                    native.gyro_rad_s,
                    native.accel_m_s2,
                    None,
                ));
            }
            RawReport::MadGazeMagnetometer(native) => {
                if native.status & 1 != 0 && native.status2 & 8 == 0 {
                    report.sensors = Some(SensorSample {
                        magnetometer: Some(MagneticField {
                            vector: TimedVector::new(native.microtesla, None),
                            unit: MagneticFieldUnit::Microtesla,
                        }),
                        ..SensorSample::default()
                    });
                }
            }
            RawReport::Empty => {}
        }
        Ok(report)
    }
}

fn rokid_events(events: &mut Vec<Event>, keys: u8, proximity: u8) {
    events.push(Event::Buttons { raw: keys.into() });
    events.push(Event::Proximity {
        raw: Some(proximity.into()),
        near: None,
    });
}
