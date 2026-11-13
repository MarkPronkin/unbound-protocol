//! Synthetic fixtures and mock peers; no connected hardware is opened.
use std::{collections::VecDeque, time::Duration};
use unbound_protocol::{
    Error, Result,
    device::{self, Model},
    grawoow, light, mad_gaze, rokid,
    usb::{
        ReportIo,
        control::{ControlIo, Setup},
    },
};
const TIMEOUT: Duration = Duration::from_millis(100);

#[derive(Default)]
struct Reports {
    reads: VecDeque<Vec<u8>>,
    writes: Vec<Vec<u8>>,
    invalid_length: bool,
}
impl ReportIo for Reports {
    fn write_report(&mut self, data: &[u8], timeout: Duration) -> Result<()> {
        assert!(!timeout.is_zero());
        self.writes.push(data.to_vec());
        Ok(())
    }
    fn read_report(&mut self, data: &mut [u8], timeout: Duration) -> Result<usize> {
        assert!(!timeout.is_zero());
        if self.invalid_length {
            return Ok(data.len() + 1);
        }
        let bytes = self.reads.pop_front().ok_or(Error::Timeout)?;
        data[..bytes.len()].copy_from_slice(&bytes);
        Ok(bytes.len())
    }
}
#[derive(Default)]
struct Controls {
    reads: VecDeque<Vec<u8>>,
    writes: Vec<(Setup, Vec<u8>)>,
    queries: Vec<Setup>,
    endpoints: Vec<u8>,
}
impl ControlIo for Controls {
    fn read_control(&mut self, setup: Setup, data: &mut [u8], timeout: Duration) -> Result<usize> {
        assert!(!timeout.is_zero());
        self.queries.push(setup);
        let bytes = self.reads.pop_front().ok_or(Error::Timeout)?;
        data[..bytes.len()].copy_from_slice(&bytes);
        Ok(bytes.len())
    }
    fn write_control(&mut self, setup: Setup, data: &[u8], timeout: Duration) -> Result<()> {
        assert!(!timeout.is_zero());
        self.writes.push((setup, data.to_vec()));
        Ok(())
    }
    fn read_interrupt(
        &mut self,
        endpoint: u8,
        data: &mut [u8],
        timeout: Duration,
    ) -> Result<usize> {
        self.endpoints.push(endpoint);
        self.read_control(
            Setup {
                request_type: 0,
                request: 0,
                value: 0,
                index: 0,
            },
            data,
            timeout,
        )
    }
}
fn light_reply(category: u8, command: u8, data: &[u8]) -> Vec<u8> {
    light::Packet::request(category, command, data)
        .encode()
        .unwrap()
        .to_vec()
}
fn mad_reply(command: [u8; 3], data: &[u8]) -> Vec<u8> {
    mad_gaze::Packet::request(command, data).encode().unwrap()
}

#[test]
fn identities_include_both_interfaces_without_guessing_rokid_model() {
    for (vid, pid, model) in [
        (0x0486, 0x573c, Model::Light),
        (0x05a9, 0x0680, Model::Light),
        (0x04d2, 0x162f, Model::RokidAirMax),
        (0x1ff7, 0x0ff4, Model::GrawoowG530),
        (0x05a9, 0x0f87, Model::GrawoowG530),
        (0x04b4, 2, Model::MadGazeGlow),
    ] {
        assert_eq!(
            device::identify(vid, pid).unwrap(),
            device::Identity {
                model,
                recovery: false
            }
        );
        assert!(!model.is_air());
        assert!(!model.is_one());
        assert_eq!(model.air_interfaces(), None);
    }
    for (vid, pid) in [(0x05a9, 0xffff), (0x04d2, 0x162d), (0x04b4, 3)] {
        assert!(device::identify(vid, pid).is_none());
    }
}

#[test]
fn light_mcu_matches_python_crc_fixture_and_rejects_corruption() {
    let fixture = include_bytes!("fixtures/light_serial_request.bin");
    let packet = light::Packet::request(b'3', b'C', b"x");
    assert_eq!(&packet.encode().unwrap(), fixture);
    assert_eq!(light::Packet::decode(fixture).unwrap(), packet);
    for i in 0..20 {
        let mut bad = *fixture;
        bad[i] ^= 1;
        assert!(light::Packet::decode(&bad).is_err(), "byte {i}");
    }
    for len in 0..20 {
        assert!(light::Packet::decode(&fixture[..len]).is_err());
    }
    assert!(light::Packet::request(b'1', b'3', b"a:b").encode().is_err());
    assert!(
        light::Packet::request(b'1', b'3', &[b'x'; 46])
            .encode()
            .is_err()
    );
    assert_eq!(
        light::Packet::request(b'1', b'3', &[b'x'; 45])
            .encode()
            .unwrap()
            .len(),
        64
    );
}

#[test]
fn light_matches_reply_category_and_preserves_events_and_unknowns() {
    let io = Reports {
        reads: [
            light_reply(b'5', b'K', b"UP"),
            light_reply(b'A', b'K', b"x"),
            light_reply(b'4', b'C', b"SN"),
        ]
        .into(),
        ..Default::default()
    };
    let mut client = light::ControlClient::new(io, TIMEOUT).unwrap();
    assert_eq!(client.serial().unwrap(), "SN");
    assert_eq!(
        light::Event::decode(client.pop_event().unwrap()).unwrap(),
        light::Event::KeyPress(0)
    );
    assert!(client.pop_event().is_none());
    assert_eq!(
        client.into_inner().writes[0],
        include_bytes!("fixtures/light_serial_request.bin")
    );
    assert_eq!(
        light::Event::decode(light::Packet::request(b'5', b'L', b"ff")).unwrap(),
        light::Event::AmbientLight(255)
    );
    assert!(light::Event::decode(light::Packet::request(b'5', b'L', b"not hex")).is_err());
    let unknown = light::Packet::request(b'5', b'?', b"raw");
    assert_eq!(
        light::Event::decode(unknown.clone()).unwrap(),
        light::Event::Unknown(unknown)
    );
}

#[test]
fn report_timeouts_and_invalid_adapter_lengths_require_reopen() {
    let mut client = light::ControlClient::new(Reports::default(), TIMEOUT).unwrap();
    assert!(matches!(client.serial(), Err(Error::Timeout)));
    assert!(matches!(client.serial(), Err(Error::Disconnected)));
    assert_eq!(client.into_inner().writes.len(), 1);
    let mut client = mad_gaze::Client::new(
        Reports {
            invalid_length: true,
            ..Default::default()
        },
        TIMEOUT,
    )
    .unwrap();
    assert!(matches!(client.serial(), Err(Error::Invalid(_))));
    assert!(matches!(client.serial(), Err(Error::Disconnected)));
}

#[test]
fn light_event_flood_is_bounded_and_cannot_reuse_exchange() {
    let io = Reports {
        reads: std::iter::repeat_n(light_reply(b'5', b'S', b"x"), 128).collect(),
        ..Default::default()
    };
    let mut client = light::ControlClient::new(io, TIMEOUT).unwrap();
    assert!(matches!(client.serial(), Err(Error::QueueFull)));
    let mut count = 0;
    while client.pop_event().is_some() {
        count += 1;
    }
    assert_eq!(count, 128);
    assert!(matches!(client.serial(), Err(Error::Disconnected)));
}

#[test]
fn light_imu_preserves_both_nanosecond_clocks_and_device_axes() {
    let bytes = include_bytes!("fixtures/light_imu.bin");
    let sample = light::ImuSample::decode(bytes).unwrap();
    assert_eq!(sample.gyro_timestamp_ns, 1234567890123);
    assert_eq!(sample.accel_timestamp_ns, 1234567890456);
    assert_eq!(sample.gyro_raw, [-123, 456, -789]);
    assert_eq!(sample.accel_raw, [-100, 200, 16384]);
    assert!((sample.gyro_rad_s[0] - (-123.0f32 * 2000.0 / 32768.0).to_radians()).abs() < 0.00001);
    assert!((sample.accel_m_s2[2] - 9.81).abs() < 0.00001);
    for offset in [56, 84] {
        let mut bad = *bytes;
        bad[offset..offset + 4].fill(0);
        assert!(light::ImuSample::decode(&bad).is_err());
    }
    assert!(light::ImuSample::decode(&bytes[..99]).is_err());
}

#[test]
fn light_sensor_commands_and_configuration_chunks_use_separate_framing() {
    let mut blob = vec![0; 0x28];
    blob.extend(b"\n\n{\"a\":1}\n\ntrailer");
    let mut part = vec![2, 1, blob.len() as u8];
    part.extend(&blob);
    let io = Reports {
        reads: [vec![2], vec![2], part, vec![2, 0]].into(),
        ..Default::default()
    };
    let mut client = light::SensorClient::new(io, TIMEOUT).unwrap();
    client.set_streaming(false).unwrap();
    assert_eq!(client.read_configuration().unwrap(), "{\"a\":1}");
    assert_eq!(
        client.into_inner().writes,
        [
            vec![2, 0x19, 0, 0, 0, 0, 0],
            vec![2, 0x14, 0, 0, 0, 0, 0],
            vec![2, 0x15, 0, 0, 0, 0, 0],
            vec![2, 0x15, 0, 0, 0, 0, 0]
        ]
    );
    for reply in [vec![2, 1, 0], vec![2, 1, 5, 1], vec![2]] {
        let io = Reports {
            reads: [vec![2], reply].into(),
            ..Default::default()
        };
        assert!(
            light::SensorClient::new(io, TIMEOUT)
                .unwrap()
                .read_configuration_blob()
                .is_err()
        );
    }
    assert!(light::configuration_json(&[]).is_err());
    assert!(light::configuration_json(&[0; 100]).is_err());
}

#[test]
fn light_sensor_skips_stream_samples_during_command_and_reads_samples() {
    let sample = include_bytes!("fixtures/light_imu.bin").to_vec();
    let io = Reports {
        reads: [sample.clone(), vec![2], sample].into(),
        ..Default::default()
    };
    let mut client = light::SensorClient::new(io, TIMEOUT).unwrap();
    client.set_streaming(true).unwrap();
    assert_eq!(client.next_sample().unwrap().gyro_raw[0], -123);
}

#[test]
fn rokid_reports_decode_all_layouts_and_preserve_unknown_packets() {
    let bytes = include_bytes!("fixtures/rokid_combined.bin");
    let rokid::Packet::Combined {
        timestamp_ns,
        accel_m_s2,
        gyro_rad_s,
        keys,
        proximity,
        vsync_timestamp,
        brightness,
        volume,
        ..
    } = rokid::Packet::decode(bytes).unwrap()
    else {
        panic!("wrong type")
    };
    assert_eq!(timestamp_ns, 1234567890123);
    assert_eq!(accel_m_s2, [1.0, -9.81, 3.0]);
    assert_eq!(gyro_rad_s, [-0.1, 0.2, -0.3]);
    assert_eq!(
        (keys, proximity, vsync_timestamp, brightness, volume),
        (0xa5, 1, 99887766, 7, 8)
    );
    let mut sensor = [0; 64];
    sensor[..2].copy_from_slice(&[4, 2]);
    sensor[2..6].copy_from_slice(&7u32.to_le_bytes());
    sensor[9..17].copy_from_slice(&999u64.to_le_bytes());
    for (i, value) in [1.0f32, -2.0, 3.0].iter().enumerate() {
        sensor[21 + 4 * i..25 + 4 * i].copy_from_slice(&value.to_le_bytes());
    }
    assert_eq!(
        rokid::Packet::decode(&sensor).unwrap(),
        rokid::Packet::Sensor {
            sensor_type: 2,
            sequence: 7,
            timestamp_us: 999,
            vector: [1.0, -2.0, 3.0]
        }
    );
    sensor[21..25].copy_from_slice(&f32::NAN.to_le_bytes());
    assert!(rokid::Packet::decode(&sensor).is_err());
    let mut misc = [0; 64];
    misc[0] = 2;
    misc[1] = 1;
    misc[47] = 3;
    misc[51] = 1;
    assert_eq!(
        rokid::Packet::decode(&misc).unwrap(),
        rokid::Packet::Misc {
            sequence: 1,
            keys: 3,
            proximity: 1
        }
    );
    let unknown = vec![0xff; 64];
    assert_eq!(
        rokid::Packet::decode(&unknown).unwrap(),
        rokid::Packet::Unknown(unknown)
    );
    assert!(rokid::Packet::decode(&bytes[..63]).is_err());
}

#[test]
fn rokid_vendor_requests_and_interrupt_endpoint_match_reference() {
    let io = Controls {
        reads: [
            b"SN\0padding".to_vec(),
            vec![0, 2],
            include_bytes!("fixtures/rokid_combined.bin").to_vec(),
        ]
        .into(),
        ..Default::default()
    };
    let mut client = rokid::Client::new(io, TIMEOUT).unwrap();
    assert_eq!(client.serial().unwrap(), "SN");
    assert_eq!(client.display_mode().unwrap(), 2);
    client.set_display_mode(rokid::DisplayMode::Stereo).unwrap();
    client.next_packet().unwrap();
    let io = client.into_inner();
    assert_eq!(
        io.queries[..2],
        [
            Setup {
                request_type: 0xc0,
                request: 0x81,
                value: 0x100,
                index: 0
            },
            Setup {
                request_type: 0xc0,
                request: 0x81,
                value: 0,
                index: 1
            }
        ]
    );
    assert_eq!(
        io.writes,
        [(
            Setup {
                request_type: 0x40,
                request: 1,
                value: 1,
                index: 1
            },
            vec![0]
        )]
    );
    assert_eq!(io.endpoints, [0x82]);
}

#[test]
fn grawoow_control_matches_fixture_and_checks_actual_response_length() {
    assert_eq!(
        grawoow::encode_command(0x8005, &[]).unwrap(),
        include_bytes!("fixtures/grawoow_serial_request.bin")
    );
    assert!(grawoow::encode_command(0x8005, &[0; 250]).is_err());
    let reply = grawoow::encode_command(0x8005, b"SN").unwrap();
    let io = Controls {
        reads: [reply.clone()].into(),
        ..Default::default()
    };
    let mut client = grawoow::ControlClient::new(io, TIMEOUT).unwrap();
    assert_eq!(client.serial().unwrap(), "SN");
    let io = client.into_inner();
    assert_eq!(
        io.writes,
        [(
            Setup {
                request_type: 0x21,
                request: 9,
                value: 0x201,
                index: 0
            },
            include_bytes!("fixtures/grawoow_serial_request.bin").to_vec()
        )]
    );
    assert_eq!(
        io.queries,
        [Setup {
            request_type: 0xa1,
            request: 1,
            value: 0x102,
            index: 0
        }]
    );
    for len in 0..reply.len() {
        assert!(grawoow::Reply::decode(&reply[..len]).is_err());
    }
    let wrong = Controls {
        reads: [grawoow::encode_command(0x8007, &[0]).unwrap()].into(),
        ..Default::default()
    };
    let mut client = grawoow::ControlClient::new(wrong, TIMEOUT).unwrap();
    assert!(client.serial().is_err());
    assert!(matches!(client.serial(), Err(Error::Disconnected)));
}

#[test]
fn grawoow_calibration_uses_big_endian_byte_offsets() {
    let part = |p: &[u8]| grawoow::encode_command(0x800a, p).unwrap();
    let io = Controls {
        reads: [
            part(b"\0\0\0\0\0\0{\""),
            part(b"\0\0\0\0\0\0a\":1}"),
            part(&[]),
        ]
        .into(),
        ..Default::default()
    };
    let mut client = grawoow::ControlClient::new(io, TIMEOUT).unwrap();
    assert_eq!(client.read_configuration().unwrap(), "{\"a\":1}");
    let io = client.into_inner();
    assert_eq!(&io.writes[1].1[6..11], &[0, 0, 0, 0, 2]);
    assert_eq!(&io.writes[2].1[6..11], &[0, 0, 0, 0, 7]);
}

#[test]
fn grawoow_sensors_keep_unbiased_device_axes_without_inventing_a_clock() {
    let fixture = include_bytes!("fixtures/grawoow_imu.bin");
    let sample = grawoow::ImuSample::decode(fixture).unwrap();
    assert_eq!(sample.gyro_raw, [-164, 328, -492]);
    assert_eq!(sample.accel_raw, [-16384, 8192, 4096]);
    assert!((sample.gyro_rad_s[0] - (-10.0f32).to_radians()).abs() < 0.00001);
    assert!((sample.accel_m_s2[0] + 9.81).abs() < 0.00001);
    let io = Controls {
        reads: [fixture.to_vec()].into(),
        ..Default::default()
    };
    let mut client = grawoow::SensorClient::new(io, TIMEOUT).unwrap();
    assert_eq!(client.next_sample().unwrap(), sample);
    assert_eq!(client.into_inner().endpoints, [0x89]);
}

#[test]
fn mad_gaze_serial_mode_matches_fixture_and_bounds_lengths() {
    let fixture = include_bytes!("fixtures/mad_gaze_mode_request.bin");
    assert_eq!(
        mad_gaze::Packet::request(*b"S3D", &[1]).encode().unwrap(),
        fixture
    );
    assert_eq!(mad_gaze::Packet::decode(fixture).unwrap().payload, [1]);
    for len in 0..fixture.len() {
        assert!(mad_gaze::Packet::decode(&fixture[..len]).is_err());
    }
    let mut bad = *fixture;
    bad[4] = 4;
    assert!(mad_gaze::Packet::decode(&bad).is_err());
    assert!(
        mad_gaze::Packet::request(*b"GSN", &[0; 251])
            .encode()
            .is_err()
    );
    assert!(mad_gaze::Packet::request(*b"GSN", b":").encode().is_err());
    let io = Reports {
        reads: [mad_reply(*b"GSN", b"unrelated"), mad_reply(*b"S3D", &[0])].into(),
        ..Default::default()
    };
    let mut client = mad_gaze::Client::new(io, TIMEOUT).unwrap();
    client.set_stereo(true).unwrap();
    assert_eq!(client.into_inner().writes, [fixture.to_vec()]);
}

#[test]
fn mad_gaze_brightness_ack_and_i2c_lengths_are_validated() {
    let io = Reports {
        reads: [
            mad_reply(*b"SLB", &[0]),
            mad_reply(*b"I2R", &[1, 104, 0, 0, 1, 0xd1]),
            mad_reply(*b"I2W", &[1, 104, 0x7e, 0, 0]),
        ]
        .into(),
        ..Default::default()
    };
    let mut client = mad_gaze::Client::new(io, TIMEOUT).unwrap();
    assert!(client.set_brightness(0).is_err());
    assert!(client.set_brightness(8).is_err());
    client.set_brightness(7).unwrap();
    assert_eq!(client.read_i2c(104, 0, 1).unwrap(), [0xd1]);
    client.write_i2c(104, 0x7e, &[0x11]).unwrap();
    let io = client.into_inner();
    assert_eq!(io.writes.len(), 3);
    assert_eq!(
        mad_gaze::Packet::decode(&io.writes[1]).unwrap().payload,
        [1, 104, 0, 0, 1]
    );
    let io = Reports {
        reads: [mad_reply(*b"I2R", &[0; 4])].into(),
        ..Default::default()
    };
    assert!(
        mad_gaze::Client::new(io, TIMEOUT)
            .unwrap()
            .read_i2c(104, 0, 1)
            .is_err()
    );
    let io = Reports {
        reads: [mad_reply(*b"SLB", &[5])].into(),
        ..Default::default()
    };
    assert!(matches!(
        mad_gaze::Client::new(io, TIMEOUT)
            .unwrap()
            .set_brightness(1),
        Err(Error::DeviceStatus(5))
    ));
}

#[test]
fn mad_gaze_fifo_has_correct_units_and_does_not_swap_sensor_fields() {
    let sample = mad_gaze::ImuSample::decode(include_bytes!("fixtures/mad_gaze_fifo.bin"))
        .unwrap()
        .unwrap();
    assert_eq!(sample.gyro_raw, [-123, 456, -789]);
    assert_eq!(sample.accel_raw, [-100, 200, 16384]);
    assert!((sample.gyro_rad_s[0] - (-123.0f32 * 2000.0 / 32768.0).to_radians()).abs() < 0.00001);
    assert!((sample.accel_m_s2[2] - 9.80665).abs() < 0.00001);
    let mut empty = [0; 12];
    empty[..2].copy_from_slice(&i16::MIN.to_le_bytes());
    assert_eq!(mad_gaze::ImuSample::decode(&empty).unwrap(), None);
    let sample = mad_gaze::MagnetometerSample::decode(&[1, 0xff, 0xff, 2, 0, 3, 0, 0, 8]).unwrap();
    assert_eq!(sample.raw, [-1, 2, 3]);
    assert_eq!((sample.status, sample.status2), (1, 8));
    assert!(sample.microtesla[0] < 0.0);
}

#[test]
fn mad_gaze_sensor_initialization_and_fifo_poll_match_reference_sequence() {
    let read = |data: &[u8]| mad_reply(*b"I2R", &[&[0; 5], data].concat());
    let write = || mad_reply(*b"I2W", &[0; 5]);
    let mut fifo = include_bytes!("fixtures/mad_gaze_fifo.bin").to_vec();
    fifo.extend(i16::MIN.to_le_bytes());
    fifo.extend([0; 10]);
    let io = Reports {
        reads: [
            read(&[0x48, 5]),
            write(),
            write(),
            read(&[0xd1]),
            write(),
            write(),
            write(),
            write(),
            read(&fifo),
        ]
        .into(),
        ..Default::default()
    };
    let mut client = mad_gaze::Client::new(io, TIMEOUT).unwrap();
    client.initialize_sensors().unwrap();
    let samples = client.read_imu().unwrap();
    assert_eq!(samples.len(), 1);
    assert_eq!(samples[0].accel_raw[2], 16384);
    let writes: Vec<_> = client
        .into_inner()
        .writes
        .iter()
        .map(|bytes| {
            let packet = mad_gaze::Packet::decode(bytes).unwrap();
            (packet.command, packet.payload)
        })
        .collect();
    assert_eq!(
        writes,
        [
            (*b"I2R", vec![1, 12, 0, 0, 2]),
            (*b"I2W", vec![1, 12, 0x32, 0, 1, 1]),
            (*b"I2W", vec![1, 12, 0x31, 0, 1, 8]),
            (*b"I2R", vec![1, 104, 0, 0, 1]),
            (*b"I2W", vec![1, 104, 0x7e, 0, 1, 0xb6]),
            (*b"I2W", vec![1, 104, 0x7e, 0, 1, 0x11]),
            (*b"I2W", vec![1, 104, 0x7e, 0, 1, 0x15]),
            (*b"I2W", vec![1, 104, 0x47, 0, 1, 0xc0]),
            (*b"I2R", vec![1, 104, 0x24, 0, 24]),
        ]
    );
}

#[test]
fn new_clients_reject_zero_timeouts_without_io() {
    assert!(light::ControlClient::new(Reports::default(), Duration::ZERO).is_err());
    assert!(light::SensorClient::new(Reports::default(), Duration::ZERO).is_err());
    assert!(mad_gaze::Client::new(Reports::default(), Duration::ZERO).is_err());
    assert!(rokid::Client::new(Controls::default(), Duration::ZERO).is_err());
    assert!(grawoow::ControlClient::new(Controls::default(), Duration::ZERO).is_err());
    assert!(grawoow::SensorClient::new(Controls::default(), Duration::ZERO).is_err());
}

#[test]
fn new_parsers_reject_truncation_without_panicking_on_arbitrary_bytes() {
    let mut seed = 0x9234abcd_u32;
    for len in 0..1100 {
        let bytes: Vec<_> = (0..len)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                seed as u8
            })
            .collect();
        let _ = light::Packet::decode(&bytes);
        let _ = light::ImuSample::decode(&bytes);
        let _ = light::configuration_json(&bytes);
        let _ = rokid::Packet::decode(&bytes);
        let _ = grawoow::Reply::decode(&bytes);
        let _ = grawoow::ImuSample::decode(&bytes);
        let _ = mad_gaze::Packet::decode(&bytes);
        let _ = mad_gaze::ImuSample::decode(&bytes);
        let _ = mad_gaze::MagnetometerSample::decode(&bytes);
    }
}

#[cfg(all(feature = "serial", unix))]
#[test]
fn serial_backend_handles_noise_fragmentation_and_coalesced_frames() {
    use serialport::SerialPort;
    use std::io::{Read, Write};
    let (mut peer, port) = serialport::TTYPort::pair().unwrap();
    let mut transport = unbound_protocol::serial::SerialTransport::from_port(Box::new(port));
    peer.set_timeout(Duration::from_secs(2)).unwrap();
    let worker = std::thread::spawn(move || {
        let mut request = [0; 10];
        peer.read_exact(&mut request).unwrap();
        assert_eq!(request, mad_reply(*b"GSN", &[]).as_slice());
        peer.write_all(b"noise").unwrap();
        for byte in mad_reply(*b"GSN", b"SN") {
            peer.write_all(&[byte]).unwrap();
        }
        peer.write_all(&mad_reply(*b"G3D", &[1])).unwrap();
        // Keep the pseudo-terminal alive until both replies are consumed.
        let mut done = [0];
        peer.read_exact(&mut done).unwrap();
    });
    transport
        .write_report(&mad_reply(*b"GSN", &[]), TIMEOUT)
        .unwrap();
    let mut data = [0; 260];
    let n = transport
        .read_report(&mut data, Duration::from_secs(2))
        .unwrap();
    assert_eq!(mad_gaze::Packet::decode(&data[..n]).unwrap().payload, b"SN");
    let n = transport
        .read_report(&mut data, Duration::from_secs(2))
        .unwrap();
    assert_eq!(mad_gaze::Packet::decode(&data[..n]).unwrap().payload, [1]);
    transport.write_report(&[0], TIMEOUT).unwrap();
    worker.join().unwrap();
}
