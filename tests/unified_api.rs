use std::{
    collections::VecDeque,
    io::Write,
    net::{TcpListener, TcpStream},
    sync::{Arc, Mutex},
    time::Duration,
};
use unbound_protocol::{
    Error, Result, air,
    device::Model,
    glasses::{
        Access, Connection, DisplayMode, Event, Glasses, MagneticFieldUnit, RawReport, Report,
        Timestamp,
    },
    grawoow, light, mad_gaze, one, rokid,
    usb::{
        Packet, ReportIo,
        control::{ControlIo, Setup},
    },
    wire::{Frame, FrameReader},
};

const TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Default)]
struct State {
    reads: VecDeque<Vec<u8>>,
    writes: Vec<Vec<u8>>,
    setups: Vec<Setup>,
    endpoints: Vec<u8>,
    waits: Vec<Duration>,
}
#[derive(Clone, Default)]
struct Fake(Arc<Mutex<State>>);
impl Fake {
    fn with_reads(reads: impl IntoIterator<Item = Vec<u8>>) -> Self {
        let io = Self::default();
        io.0.lock().unwrap().reads.extend(reads);
        io
    }
    fn read(&self, buffer: &mut [u8]) -> Result<usize> {
        let bytes = self
            .0
            .lock()
            .unwrap()
            .reads
            .pop_front()
            .ok_or(Error::Timeout)?;
        buffer[..bytes.len()].copy_from_slice(&bytes);
        Ok(bytes.len())
    }
    fn writes(&self) -> Vec<Vec<u8>> {
        self.0.lock().unwrap().writes.clone()
    }
}
impl ReportIo for Fake {
    fn write_report(&mut self, report: &[u8], _: Duration) -> Result<()> {
        self.0.lock().unwrap().writes.push(report.to_vec());
        Ok(())
    }
    fn read_report(&mut self, buffer: &mut [u8], timeout: Duration) -> Result<usize> {
        self.0.lock().unwrap().waits.push(timeout);
        self.read(buffer)
    }
}
impl ControlIo for Fake {
    fn read_control(&mut self, setup: Setup, buffer: &mut [u8], _: Duration) -> Result<usize> {
        self.0.lock().unwrap().setups.push(setup);
        self.read(buffer)
    }
    fn write_control(&mut self, setup: Setup, data: &[u8], _: Duration) -> Result<()> {
        let mut state = self.0.lock().unwrap();
        state.setups.push(setup);
        state.writes.push(data.to_vec());
        Ok(())
    }
    fn read_interrupt(&mut self, endpoint: u8, buffer: &mut [u8], _: Duration) -> Result<usize> {
        self.0.lock().unwrap().endpoints.push(endpoint);
        self.read(buffer)
    }
}
fn usb_reply(command: u16, id: u32, data: &[u8]) -> Vec<u8> {
    Packet::request(command, id, &[&[0], data].concat())
        .encode(64)
        .unwrap()
}
fn sensor_reply(command: u8, payload: &[u8]) -> Vec<u8> {
    air::SensorCommand {
        command,
        payload: payload.to_vec(),
    }
    .encode()
    .unwrap()
    .to_vec()
}
fn light_reply(category: u8, command: u8, payload: &[u8]) -> Vec<u8> {
    light::Packet::request(category, command, payload)
        .encode()
        .unwrap()
        .to_vec()
}
fn mad_reply(command: [u8; 3], payload: &[u8]) -> Vec<u8> {
    mad_gaze::Packet::request(command, payload)
        .encode()
        .unwrap()
}
fn pair() -> (TcpStream, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let server = listener.accept().unwrap().0;
    server.set_read_timeout(Some(TIMEOUT)).unwrap();
    server.set_write_timeout(Some(TIMEOUT)).unwrap();
    (client, server)
}

#[test]
fn one_variants_share_one_handle_without_construction_io() {
    fn assert_send<T: Send>() {}
    assert_send::<Glasses>();
    for model in [
        Model::One,
        Model::OnePro,
        Model::OneS,
        Model::OneProPlus,
        Model::RogR1,
    ] {
        let io = Fake::with_reads([usb_reply(0x15, 1, b"SN")]);
        let mut glasses = Glasses::from_one_usb(model, io.clone(), TIMEOUT).unwrap();
        assert!(io.writes().is_empty());
        assert_eq!(glasses.model(), model);
        let caps = glasses.capabilities();
        assert_eq!(caps.brightness, Access::ReadWrite);
        assert!(!caps.sensors);
        assert!(matches!(glasses.next_report(), Err(Error::Unsupported(_))));
        assert!(matches!(
            glasses.set_display_mode(DisplayMode::Stereo),
            Err(Error::Unsupported(_))
        ));
        assert_eq!(glasses.serial().unwrap(), "SN");
        assert_eq!(io.writes().len(), 1);
    }
}

#[test]
fn all_air_variants_map_brightness_and_display_modes() {
    for model in [Model::Air, Model::Air2, Model::Air2Pro, Model::Air2Ultra] {
        let io = Fake::with_reads([
            usb_reply(4, 0, &[]),
            usb_reply(3, 0, &[9]),
            usb_reply(8, 0, &[]),
            usb_reply(7, 0, &[9]),
            usb_reply(7, 0, &[0xee]),
        ]);
        let mut glasses = Glasses::from_air(model, io.clone(), None, TIMEOUT).unwrap();
        assert!(!glasses.capabilities().configuration);
        for value in [-0.1, 1.1, f32::NAN, f32::INFINITY] {
            assert!(glasses.set_brightness(value).is_err());
        }
        assert!(io.writes().is_empty());
        glasses.set_brightness(0.5).unwrap();
        assert_eq!(glasses.brightness().unwrap(), 1.0);
        glasses.set_display_mode(DisplayMode::Stereo90).unwrap();
        assert_eq!(glasses.display_mode().unwrap(), DisplayMode::Stereo90);
        assert_eq!(glasses.display_mode().unwrap(), DisplayMode::Unknown(0xee));
        let writes = io.writes();
        assert_eq!(Packet::decode(&writes[0]).unwrap().payload, [5]);
        assert_eq!(Packet::decode(&writes[2]).unwrap().payload, [9]);
    }
}

#[test]
fn air_sensor_lifecycle_retains_clocks_and_blocks_configuration_while_streaming() {
    let sensor = Fake::with_reads([
        sensor_reply(0x19, &[]),
        include_bytes!("fixtures/air_imu_v2.bin").to_vec(),
        sensor_reply(0x19, &[]),
        sensor_reply(0x14, &2u32.to_le_bytes()),
        sensor_reply(0x15, b"{}"),
    ]);
    let mut glasses = Glasses::from_air(
        Model::Air2,
        Fake::default(),
        Some(Box::new(sensor.clone())),
        TIMEOUT,
    )
    .unwrap();
    assert!(sensor.writes().is_empty());
    assert!(glasses.capabilities().sensors);
    assert!(matches!(glasses.next_report(), Err(Error::Invalid(_))));
    glasses.start_sensors().unwrap();
    glasses.start_sensors().unwrap();
    assert!(matches!(glasses.configuration(), Err(Error::Invalid(_))));
    assert_eq!(sensor.writes().len(), 1);
    let report = glasses.next_report().unwrap();
    let RawReport::Air(native) = report.raw else {
        panic!("wrong native report")
    };
    let sample = report.sensors.unwrap();
    assert_eq!(
        sample.gyro_rad_s.unwrap().timestamp,
        Some(Timestamp::nanoseconds(native.timestamp_ns))
    );
    assert_eq!(sample.accel_m_s2.unwrap().value, native.accel_m_s2);
    assert!(sample.temperature_c.is_none());
    assert!(sample.magnetometer.is_none());
    glasses.stop_sensors().unwrap();
    assert_eq!(glasses.configuration().unwrap(), "{}");
    assert!(matches!(glasses.next_report(), Err(Error::Invalid(_))));
}

#[test]
fn device_rejections_and_failed_exchanges_propagate_without_retries() {
    let io = Fake::with_reads([Packet::request(3, 1, &[4]).encode(64).unwrap()]);
    let mut glasses = Glasses::from_one_usb(Model::One, io.clone(), TIMEOUT).unwrap();
    assert!(matches!(glasses.brightness(), Err(Error::DeviceStatus(4))));
    assert!(matches!(glasses.brightness(), Err(Error::Disconnected)));
    assert_eq!(io.writes().len(), 1);
    let io = Fake::default();
    let mut glasses = Glasses::from_air(Model::Air, io.clone(), None, TIMEOUT).unwrap();
    assert!(matches!(glasses.set_brightness(0.5), Err(Error::Timeout)));
    assert!(matches!(
        glasses.set_brightness(0.5),
        Err(Error::Disconnected)
    ));
    assert_eq!(io.writes().len(), 1);
}

#[test]
fn brightness_readback_outside_native_range_is_rejected() {
    let mut glasses = Glasses::from_air(
        Model::Air,
        Fake::with_reads([usb_reply(3, 0, &[10])]),
        None,
        TIMEOUT,
    )
    .unwrap();
    assert!(matches!(glasses.brightness(), Err(Error::Invalid(_))));
}

#[test]
fn light_stereo_handshake_events_and_independent_sensor_clocks() {
    let control = Fake::with_reads([
        light_reply(b'A', b'3', b"1"),
        light_reply(b'2', b'3', b"3"),
        light_reply(b'2', b'L', b"1"),
        light_reply(b'2', b'N', b"1"),
        light_reply(b'5', b'P', b"near"),
    ]);
    let sensor = Fake::with_reads([vec![2], include_bytes!("fixtures/light_imu.bin").to_vec()]);
    let mut glasses =
        Glasses::from_light(control.clone(), Some(Box::new(sensor.clone())), TIMEOUT).unwrap();
    assert!(control.writes().is_empty());
    glasses.set_display_mode(DisplayMode::Stereo).unwrap();
    glasses.set_events_enabled(true).unwrap();
    let event = glasses.next_event().unwrap();
    assert_eq!(
        event.events,
        [Event::Proximity {
            raw: None,
            near: Some(true)
        }]
    );
    glasses.start_sensors().unwrap();
    let sample = glasses.next_report().unwrap().sensors.unwrap();
    assert_eq!(
        sample.gyro_rad_s.unwrap().timestamp,
        Some(Timestamp::nanoseconds(1234567890123))
    );
    assert_eq!(
        sample.accel_m_s2.unwrap().timestamp,
        Some(Timestamp::nanoseconds(1234567890456))
    );
    let writes = control.writes();
    assert_eq!(
        light::Packet::decode(&writes[0]).unwrap(),
        light::Packet::request(b'@', b'3', b"1")
    );
    assert_eq!(
        light::Packet::decode(&writes[1]).unwrap(),
        light::Packet::request(b'1', b'3', b"3")
    );
    assert_eq!(sensor.writes(), [vec![2, 0x19, 1, 0, 0, 0, 0]]);
}

#[test]
fn light_sensor_ack_wait_services_heartbeat_and_keeps_the_full_timeout() {
    struct DelayedSensor {
        start: std::time::Instant,
        waits: Arc<Mutex<Vec<Duration>>>,
    }
    impl ReportIo for DelayedSensor {
        fn write_report(&mut self, _: &[u8], _: Duration) -> Result<()> {
            Ok(())
        }
        fn read_report(&mut self, buffer: &mut [u8], timeout: Duration) -> Result<usize> {
            self.waits.lock().unwrap().push(timeout);
            let Some(left) = Duration::from_millis(300).checked_sub(self.start.elapsed()) else {
                buffer[0] = 2;
                return Ok(1);
            };
            std::thread::sleep(timeout.min(left));
            Err(Error::Timeout)
        }
    }
    let control = Fake::default();
    let waits = Arc::new(Mutex::new(Vec::new()));
    let sensor = DelayedSensor {
        start: std::time::Instant::now(),
        waits: Arc::clone(&waits),
    };
    let mut glasses = Glasses::from_light(
        control.clone(),
        Some(Box::new(sensor)),
        Duration::from_millis(600),
    )
    .unwrap();
    glasses.start_sensors().unwrap();
    assert!(control.writes().iter().any(|bytes| {
        light::Packet::decode(bytes).unwrap() == light::Packet::request(b'@', b'K', b"x")
    }));
    assert!(
        waits
            .lock()
            .unwrap()
            .iter()
            .all(|wait| *wait <= Duration::from_millis(250))
    );
}

#[test]
fn rokid_reports_keep_state_partial_samples_and_unknown_sensor_types() {
    let io = Fake::with_reads([
        include_bytes!("fixtures/rokid_combined.bin").to_vec(),
        vec![0xee; 64],
        vec![0, 2],
    ]);
    let mut glasses = Glasses::from_rokid(io.clone(), TIMEOUT).unwrap();
    glasses.start_sensors().unwrap();
    let report = glasses.next_report().unwrap();
    assert_eq!(
        report.events,
        [
            Event::Buttons { raw: 0xa5 },
            Event::Proximity {
                raw: Some(1),
                near: None
            }
        ]
    );
    let sample = report.sensors.unwrap();
    assert_eq!(
        sample.gyro_rad_s.unwrap().timestamp,
        Some(Timestamp::nanoseconds(1234567890123))
    );
    assert_eq!(
        sample.magnetometer.unwrap().unit,
        MagneticFieldUnit::Microtesla
    );
    let unknown = glasses.next_report().unwrap();
    assert!(unknown.sensors.is_none());
    assert_eq!(
        unknown.raw,
        RawReport::Rokid(rokid::Packet::Unknown(vec![0xee; 64]))
    );
    assert_eq!(glasses.display_mode().unwrap(), DisplayMode::HalfStereo);
    assert!(matches!(
        glasses.set_display_mode(DisplayMode::HalfStereo),
        Err(Error::Unsupported(_))
    ));
    glasses.set_display_mode(DisplayMode::Stereo).unwrap();
    assert_eq!(io.0.lock().unwrap().endpoints, [rokid::ENDPOINT; 2]);
    assert_eq!(io.0.lock().unwrap().setups.last().unwrap().value, 1);

    for sensor_type in [1, 2, 3, 99] {
        let raw = RawReport::Rokid(rokid::Packet::Sensor {
            sensor_type,
            sequence: 7,
            timestamp_us: u64::MAX,
            vector: [1.0, 2.0, 3.0],
        });
        let report = Report::from_raw(raw.clone()).unwrap();
        assert_eq!(report.raw, raw);
        if sensor_type == 99 {
            assert!(report.sensors.is_none());
            continue;
        }
        let sample = report.sensors.unwrap();
        let vectors = [
            &sample.accel_m_s2,
            &sample.gyro_rad_s,
            &sample.magnetometer.as_ref().map(|m| m.vector.clone()),
        ];
        assert_eq!(vectors.iter().filter(|v| v.is_some()).count(), 1);
        assert_eq!(
            vectors[(sensor_type - 1) as usize]
                .as_ref()
                .unwrap()
                .timestamp,
            Some(Timestamp::microseconds(u64::MAX))
        );
    }
}

#[test]
fn grawoow_uses_separate_transports_and_never_invents_a_timestamp() {
    let control = Fake::with_reads([
        grawoow::encode_command(0x8005, b"SN").unwrap(),
        grawoow::encode_command(0x8008, &[]).unwrap(),
    ]);
    let sensor = Fake::with_reads([include_bytes!("fixtures/grawoow_imu.bin").to_vec()]);
    let mut glasses =
        Glasses::from_grawoow(control.clone(), Some(Box::new(sensor.clone())), TIMEOUT).unwrap();
    assert_eq!(glasses.serial().unwrap(), "SN");
    glasses.set_display_mode(DisplayMode::Stereo).unwrap();
    glasses.start_sensors().unwrap();
    let sample = glasses.next_report().unwrap().sensors.unwrap();
    assert!(sample.gyro_rad_s.unwrap().timestamp.is_none());
    assert!(sample.accel_m_s2.unwrap().timestamp.is_none());
    assert_eq!(sensor.0.lock().unwrap().endpoints, [grawoow::ENDPOINT]);
    assert_eq!(
        control.writes()[0],
        include_bytes!("fixtures/grawoow_serial_request.bin")
    );
    assert_eq!(
        grawoow::Reply::decode(&control.writes()[1])
            .unwrap()
            .payload,
        [1]
    );
}

fn mad_initialization() -> Vec<Vec<u8>> {
    let read = |data: &[u8]| mad_reply(*b"I2R", &[&[0; 5], data].concat());
    let write = || mad_reply(*b"I2W", &[0; 5]);
    vec![
        read(&[0x48, 5]),
        write(),
        write(),
        read(&[0xd1]),
        write(),
        write(),
        write(),
        write(),
    ]
}

#[test]
fn mad_gaze_brightness_maps_to_native_range_and_readback_is_unsupported() {
    let io = Fake::with_reads(vec![mad_reply(*b"SLB", &[0]); 3]);
    let mut glasses = Glasses::from_mad_gaze(io.clone(), TIMEOUT).unwrap();
    assert!(io.writes().is_empty());
    assert!(matches!(glasses.brightness(), Err(Error::Unsupported(_))));
    for value in [0.0, 0.5, 1.0] {
        glasses.set_brightness(value).unwrap();
    }
    let levels: Vec<_> = io
        .writes()
        .iter()
        .map(|bytes| mad_gaze::Packet::decode(bytes).unwrap().payload[0])
        .collect();
    assert_eq!(levels, [1, 4, 7]);
}

#[test]
fn mad_gaze_initializes_once_drains_both_fifo_entries_and_preserves_empty_reads() {
    let mut fifo = include_bytes!("fixtures/mad_gaze_fifo.bin").to_vec();
    let mut second = fifo.clone();
    second[..2].copy_from_slice(&123i16.to_le_bytes());
    fifo.extend(second);
    let mut empty = [0; 24];
    empty[..2].copy_from_slice(&i16::MIN.to_le_bytes());
    let mut replies = mad_initialization();
    replies.extend([
        mad_reply(*b"I2R", &[&[0; 5], &fifo[..]].concat()),
        mad_reply(*b"I2R", &[&[0; 5], &empty[..]].concat()),
    ]);
    let io = Fake::with_reads(replies);
    let mut glasses = Glasses::from_mad_gaze(io.clone(), TIMEOUT).unwrap();
    assert!(matches!(glasses.next_report(), Err(Error::Invalid(_))));
    assert!(matches!(
        glasses.poll_magnetometer(),
        Err(Error::Invalid(_))
    ));
    assert!(io.writes().is_empty());
    glasses.start_sensors().unwrap();
    let initialized_writes = io.writes().len();
    glasses.start_sensors().unwrap();
    assert_eq!(io.writes().len(), initialized_writes);
    let first = glasses.next_report().unwrap();
    let writes = io.writes().len();
    let second = glasses.next_report().unwrap();
    assert_eq!(writes, io.writes().len());
    assert!(first.sensors.unwrap().gyro_rad_s.unwrap().value[0] < 0.0);
    assert!(second.sensors.unwrap().gyro_rad_s.unwrap().value[0] > 0.0);
    let empty = glasses.next_report().unwrap();
    assert!(empty.sensors.is_none());
    assert_eq!(empty.raw, RawReport::Empty);
}

#[test]
fn magnetometer_availability_follows_status_and_keeps_native_units() {
    for (status, status2, valid) in [(0, 0, false), (1, 8, false), (1, 0, true)] {
        let native = mad_gaze::MagnetometerSample {
            status,
            status2,
            raw: [1, 2, 3],
            microtesla: [1.0, 2.0, 3.0],
        };
        let raw = RawReport::MadGazeMagnetometer(native);
        let report = Report::from_raw(raw.clone()).unwrap();
        assert_eq!(report.raw, raw);
        assert_eq!(report.sensors.is_some(), valid);
    }
    let frame = Frame::decode(include_bytes!("fixtures/one_imu_frame.bin")).unwrap();
    let mut magnetometer = frame.clone();
    magnetometer.body[24..28].copy_from_slice(&4u32.to_le_bytes());
    let sample = Report::from_raw(RawReport::One(magnetometer))
        .unwrap()
        .sensors
        .unwrap();
    assert!(sample.gyro_rad_s.is_none());
    assert!(sample.accel_m_s2.is_none());
    assert_eq!(
        sample.magnetometer.unwrap().unit,
        MagneticFieldUnit::DeviceNative
    );
    let mut unknown = frame;
    unknown.body[24..28].copy_from_slice(&99u32.to_le_bytes());
    let report = Report::from_raw(RawReport::One(unknown.clone())).unwrap();
    assert!(report.sensors.is_none());
    assert_eq!(report.raw, RawReport::One(unknown));
}

#[test]
fn one_tcp_uses_common_controls_and_does_not_change_settings_on_attach() {
    let (control, mut peer) = pair();
    let worker = std::thread::spawn(move || {
        let mut reader = FrameReader::default();
        for (opcode, response, expected_value) in [
            (one::opcode::SERIAL, vec![0x12, 2, b'S', b'N'], None),
            (one::opcode::SET_BRIGHTNESS, vec![], Some(5)),
            (one::opcode::SET_INPUT_MODE, vec![], Some(1)),
            (one::opcode::GET_PROXIMITY, vec![0x10, 1], None),
        ] {
            let request = reader.read(&mut peer).unwrap();
            assert_eq!(request.opcode, opcode);
            if let Some(value) = expected_value {
                assert_eq!(request.body.last(), Some(&value));
            }
            let id = u32::from_be_bytes(request.body[..4].try_into().unwrap()) & 0x7fff_ffff;
            let body = [
                &id.to_be_bytes()[..],
                &[0x22, response.len() as u8],
                &response,
            ]
            .concat();
            peer.write_all(&Frame { opcode, body }.encode().unwrap())
                .unwrap();
        }
    });
    let mut glasses = Glasses::from_one_tcp(
        Model::OnePro,
        one::ControlClient::from_stream(control, TIMEOUT).unwrap(),
        None,
    )
    .unwrap();
    assert!(!glasses.capabilities().sensors);
    assert_eq!(glasses.capabilities().brightness, Access::WriteOnly);
    assert!(matches!(glasses.brightness(), Err(Error::Unsupported(_))));
    assert!(matches!(glasses.display_mode(), Err(Error::Unsupported(_))));
    assert_eq!(glasses.serial().unwrap(), "SN");
    glasses.set_brightness(0.5).unwrap();
    glasses.set_display_mode(DisplayMode::Stereo).unwrap();
    assert!(glasses.proximity_enabled().unwrap());
    worker.join().unwrap();
}

#[test]
fn one_report_port_preserves_unknown_frames_and_delivers_normalized_imu() {
    let (control, _control_peer) = pair();
    let (reports, mut peer) = pair();
    let unknown = Frame {
        opcode: 0xffff,
        body: vec![1, 2, 3],
    };
    peer.write_all(&unknown.encode().unwrap()).unwrap();
    peer.write_all(include_bytes!("fixtures/one_imu_frame.bin"))
        .unwrap();
    let mut glasses = Glasses::from_one_tcp(
        Model::One,
        one::ControlClient::from_stream(control, TIMEOUT).unwrap(),
        Some(one::ReportsClient::from_stream(reports, TIMEOUT).unwrap()),
    )
    .unwrap();
    assert!(glasses.capabilities().sensors);
    glasses.start_sensors().unwrap();
    let report = glasses.next_report().unwrap();
    assert!(report.sensors.is_none());
    assert_eq!(report.raw, RawReport::One(unknown));
    let sample = glasses.next_report().unwrap().sensors.unwrap();
    assert!(sample.magnetometer.is_none());
    assert_eq!(
        sample.gyro_rad_s.unwrap().timestamp,
        Some(Timestamp::nanoseconds(123456789))
    );
}

#[test]
fn invalid_model_and_zero_timeout_are_rejected_before_transport_io() {
    let io = Fake::default();
    assert!(Glasses::from_air(Model::One, io.clone(), None, TIMEOUT).is_err());
    assert!(Glasses::from_one_usb(Model::Air, io.clone(), TIMEOUT).is_err());
    assert!(Glasses::from_air(Model::Air, io.clone(), None, Duration::ZERO).is_err());
    assert!(Glasses::from_light(io.clone(), None, Duration::ZERO).is_err());
    assert!(Glasses::from_rokid(io.clone(), Duration::ZERO).is_err());
    assert!(Glasses::from_grawoow(io.clone(), None, Duration::ZERO).is_err());
    assert!(Glasses::from_mad_gaze(io.clone(), Duration::ZERO).is_err());
    assert!(Glasses::connect(Connection::default(), Duration::ZERO).is_err());
    let wrong_tcp = Connection::OneTcp {
        model: Model::Light,
        host: "127.0.0.1".parse().unwrap(),
        source: None,
        reports: true,
    };
    assert!(matches!(
        Glasses::connect(wrong_tcp, TIMEOUT),
        Err(Error::Invalid(_))
    ));
    assert!(io.writes().is_empty());
}

#[cfg(not(feature = "hid"))]
#[test]
fn absent_hid_backend_returns_a_typed_unsupported_error() {
    let connection = Connection::Hid {
        model: Model::Air,
        control: std::ffi::CString::new("unused").unwrap(),
        sensor: None,
    };
    assert!(matches!(
        Glasses::connect(connection, TIMEOUT),
        Err(Error::Unsupported(_))
    ));
}
#[cfg(not(feature = "libusb"))]
#[test]
fn absent_usb_backend_returns_a_typed_unsupported_error() {
    use unbound_protocol::glasses::UsbAddress;
    let connection = Connection::RokidUsb {
        device: UsbAddress { bus: 0, address: 0 },
    };
    assert!(matches!(
        Glasses::connect(connection, TIMEOUT),
        Err(Error::Unsupported(_))
    ));
}
#[cfg(not(feature = "serial"))]
#[test]
fn absent_serial_backend_returns_a_typed_unsupported_error() {
    let connection = Connection::MadGazeSerial {
        port: "unused".into(),
    };
    assert!(matches!(
        Glasses::connect(connection, TIMEOUT),
        Err(Error::Unsupported(_))
    ));
}
