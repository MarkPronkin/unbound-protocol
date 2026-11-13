use std::io::{self, Read};
use unbound_protocol::{
    Error,
    air::{ImuSample, SensorCommand},
    device::{self, Model},
    one::{SensorReport, opcode},
    protobuf,
    usb::{Function, Packet, Setting, UsbConfiguration},
    wire::{self, Frame, FrameReader},
};

#[test]
fn ieee_crc_check_vector() {
    assert_eq!(wire::crc32(b"123456789"), 0xcbf43926);
    assert_eq!(wire::crc32(b""), 0);
}

#[test]
fn usb_requests_match_synthetic_fixtures() {
    assert_eq!(
        Packet::request(0x26, 1, &[]).encode(64).unwrap(),
        include_bytes!("fixtures/usb_mcu_request.bin")
    );
    assert_eq!(
        Packet::request(4, 1, &[5]).encode(64).unwrap(),
        include_bytes!("fixtures/usb_brightness_request.bin")
    );
}

#[test]
fn usb_status_and_declared_length_exclude_padding() {
    let mut bytes = *include_bytes!("fixtures/usb_one_response.bin");
    bytes[1023] = 0xff;
    let packet = Packet::decode(&bytes).unwrap();
    assert_eq!((packet.request_id, packet.command), (1, 0x26));
    assert_eq!(packet.response_data().unwrap(), b"test-version");
    let rejected = Packet::request(0x26, 1, &[7]);
    assert!(matches!(
        rejected.response_data(),
        Err(Error::DeviceStatus(7))
    ));
    assert!(Packet::request(0x26, 1, &[]).response_data().is_err());
}

#[test]
fn usb_rejects_crc_underflow_truncation_and_oversize() {
    let original = *include_bytes!("fixtures/usb_mcu_request.bin");
    for i in 1..22 {
        let mut bad = original;
        bad[i] ^= 1;
        assert!(Packet::decode(&bad).is_err(), "accepted corrupt byte {i}");
    }
    let mut short = original;
    short[5..7].copy_from_slice(&16u16.to_le_bytes());
    assert!(Packet::decode(&short).is_err());
    let mut long = original;
    long[5..7].copy_from_slice(&100u16.to_le_bytes());
    assert!(matches!(
        Packet::decode(&long),
        Err(Error::Truncated { .. })
    ));
    assert!(Packet::request(1, 1, &[0; 43]).encode(64).is_err());
    assert!(Packet::request(1, 1, &[]).encode(1025).is_err());
}

#[test]
fn usb_configuration_preserves_unassigned_bits() {
    let config = UsbConfiguration(0xf0000000)
        .with(Function::Ncm, Setting::Enable)
        .with(Function::Ecm, Setting::Disable)
        .with(Function::Apply, Setting::Enable);
    assert_eq!(config.0, 0xf0010009);
    assert_eq!(config.get(Function::Ecm), 2);
    assert_eq!(config.with(Function::Ncm, Setting::Unchanged).0, 0xf0010008);
}

#[test]
fn mcu_events_do_not_strip_a_response_status_byte() {
    use unbound_protocol::usb::events::Event;
    let key = Packet::request(0x6c05, 0, &[2, 0, 0, 0, 3, 0, 0, 0, 4, 0, 0, 0]);
    assert_eq!(
        Event::decode(&key).unwrap(),
        Event::Key {
            key_type: 2,
            function: 3,
            parameter: 4
        }
    );
    assert_eq!(
        Event::decode(&Packet::request(0x6c04, 0, &[1, 0, 0, 0])).unwrap(),
        Event::Proximity(1)
    );
    assert!(Event::decode(&Packet::request(0x6c05, 0, &[1])).is_err());
    let unknown = Packet::request(0x6cff, 0, &[1]);
    assert_eq!(Event::decode(&unknown).unwrap(), Event::Unknown(unknown));
}

#[test]
fn sensor_commands_match_independent_fixture_and_check_crc() {
    let packet = SensorCommand {
        command: 0x19,
        payload: vec![1],
    };
    assert_eq!(
        packet.encode().unwrap(),
        *include_bytes!("fixtures/sensor_start_request.bin")
    );
    assert_eq!(
        SensorCommand::decode(&packet.encode().unwrap()).unwrap(),
        packet
    );
    let mut bad = packet.encode().unwrap();
    bad[8] ^= 1;
    assert!(matches!(
        SensorCommand::decode(&bad),
        Err(Error::Checksum { .. })
    ));
    assert!(
        SensorCommand {
            command: 1,
            payload: vec![0; 57]
        }
        .encode()
        .is_err()
    );
}

#[test]
fn air_v1_and_v2_offsets_scales_and_signed_values() {
    for (version, bytes, divisor) in [
        (1, include_bytes!("fixtures/air_imu_v1.bin"), 65536.0f32),
        (2, include_bytes!("fixtures/air_imu_v2.bin"), 16777216.0),
    ] {
        let sample = ImuSample::decode(bytes).unwrap();
        assert_eq!(sample.version, version);
        assert_eq!(sample.timestamp_ns, 123456789);
        assert_eq!(sample.sensor_timestamp_ns, 987654321);
        assert_eq!(sample.gyro_raw, [-123, 456, -789]);
        assert_eq!(sample.accel_raw, [-100, 200, 300]);
        assert!((sample.gyro_rad_s[0] - (-123.0 * 4000.0 / divisor).to_radians()).abs() < 0.0001);
        assert!((sample.accel_m_s2[1] - 200.0 * 32.0 / divisor * 9.8).abs() < 0.0001);
        assert_eq!(sample.magnetometer_raw, [1, 2, 3]);
        assert!(sample.magnetometer_updated);
    }
}

#[test]
fn air_rejects_bad_version_and_zero_divisors() {
    let original = *include_bytes!("fixtures/air_imu_v2.bin");
    for range in [14..18, 29..33] {
        let mut bad = original;
        bad[range].fill(0);
        assert!(ImuSample::decode(&bad).is_err());
    }
    let mut bad = original;
    bad[1] = 3;
    assert!(ImuSample::decode(&bad).is_err());
    assert!(ImuSample::decode(&original[..63]).is_err());
}

#[test]
fn one_sensor_uses_full_128_byte_body_and_preserves_device_axes() {
    let frame = Frame::decode(include_bytes!("fixtures/one_imu_frame.bin")).unwrap();
    let sample = SensorReport::decode(&frame).unwrap();
    assert_eq!(sample.device_id, 0x1234);
    assert_eq!(sample.timestamp_ns, 123456789);
    assert_eq!(sample.report_type, 11);
    assert_eq!(sample.gyro_rad_s, [-1.0, 2.0, -3.0]);
    assert_eq!(sample.accel_m_s2, [0.0, -9.8, 0.0]);
    assert_eq!(sample.frame_id, [1, 2, 3]);
    assert_eq!(sample.temperature_c, 25.0);
    assert_eq!(sample.raw.len(), 128);
    let mut short = frame.clone();
    short.body.truncate(78); // A previous community implementation consumed only 84 wire bytes.
    assert!(SensorReport::decode(&short).is_err());
    let mut bad = frame;
    bad.body[28..32].copy_from_slice(&f32::NAN.to_le_bytes());
    assert!(SensorReport::decode(&bad).is_err());
}

#[test]
fn protobuf_golden_requests_and_responses() {
    assert_eq!(protobuf::get_request(), [0x18, 0]);
    assert_eq!(protobuf::set_request(5), [0x1a, 2, 8, 5]);
    assert_eq!(protobuf::set_request(300), [0x1a, 3, 8, 0xac, 2]);
    assert_eq!(protobuf::numeric_response(&[0x22, 2, 0x10, 9]).unwrap(), 9);
    assert_eq!(protobuf::numeric_response(&[0x22, 0]).unwrap(), 0);
    assert_eq!(
        protobuf::string_response(&[0x22, 5, 0x12, 3, b'a', b'b', b'c']).unwrap(),
        "abc"
    );
    protobuf::acknowledgement(&[0x22, 0]).unwrap();
    assert!(protobuf::acknowledgement(&[0x22, 2, 0x10, 1]).is_err());
}

#[test]
fn protobuf_handles_unknown_fields_but_not_duplicate_or_malformed_values() {
    assert_eq!(
        protobuf::numeric_response(&[0x08, 0, 0x22, 2, 0x10, 1, 0x28, 3]).unwrap(),
        1
    );
    for bytes in [
        &[0x22, 0, 0x22, 0][..],
        &[0x22, 4, 0x10, 1, 0x10, 2],
        &[0x22, 2, 0x12, 0],
        &[0x22, 4, 0x10, 1],
        &[0],
        &[0x22, 0x80],
        &[0x0f],
        &[
            0x08, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 2,
        ],
    ] {
        assert!(protobuf::numeric_response(bytes).is_err(), "{bytes:x?}");
    }
}

#[test]
fn tcp_golden_requests() {
    assert_eq!(
        Frame::decode(include_bytes!("fixtures/tcp_version_request.bin")).unwrap(),
        Frame {
            opcode: opcode::SOFTWARE_VERSION,
            body: vec![0x80, 0, 0, 1, 0x18, 0]
        }
    );
    assert_eq!(
        Frame {
            opcode: opcode::SET_BRIGHTNESS,
            body: vec![0x80, 0, 0, 1, 0x1a, 2, 8, 5]
        }
        .encode()
        .unwrap(),
        include_bytes!("fixtures/tcp_brightness_request.bin")
    );
}

struct Chunks<'a> {
    data: &'a [u8],
    size: usize,
    until_timeout: Option<usize>,
}
impl Read for Chunks<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.until_timeout == Some(0) {
            self.until_timeout = None;
            return Err(io::ErrorKind::TimedOut.into());
        }
        let n = buffer
            .len()
            .min(self.size)
            .min(self.data.len())
            .min(self.until_timeout.unwrap_or(usize::MAX));
        buffer[..n].copy_from_slice(&self.data[..n]);
        self.data = &self.data[n..];
        if let Some(count) = &mut self.until_timeout {
            *count -= n;
        }
        Ok(n)
    }
}

#[test]
fn fragmented_and_coalesced_tcp_frames() {
    let original = include_bytes!("fixtures/one_imu_frame.bin");
    let double = [original.as_slice(), original.as_slice()].concat();
    for size in 1..=double.len() {
        let mut input = Chunks {
            data: &double,
            size,
            until_timeout: None,
        };
        let mut reader = FrameReader::default();
        for _ in 0..2 {
            assert_eq!(reader.read(&mut input).unwrap().encode().unwrap(), original);
        }
    }
}

#[test]
fn partial_tcp_frame_survives_timeout_at_every_boundary() {
    let original = include_bytes!("fixtures/one_imu_frame.bin");
    for timeout_at in 0..original.len() {
        let mut input = Chunks {
            data: original,
            size: 3,
            until_timeout: Some(timeout_at),
        };
        let mut reader = FrameReader::default();
        assert!(
            matches!(reader.read(&mut input), Err(Error::Timeout)),
            "offset {timeout_at}"
        );
        assert_eq!(reader.read(&mut input).unwrap().encode().unwrap(), original);
    }
}

#[test]
fn invalid_framing_and_eof_poison_the_reader() {
    let mut reader = FrameReader::default();
    let mut oversized = &b"\x28\x36\xff\xff\xff\xff"[..];
    assert!(matches!(
        reader.read(&mut oversized),
        Err(Error::TooLarge { .. })
    ));
    assert!(matches!(
        reader.read(&mut &b""[..]),
        Err(Error::Disconnected)
    ));
    assert!(Frame::decode(&[0, 0, 0, 0, 0, 1]).is_err());
    assert!(Frame::decode(&[0, 0, 0, 0, 0, 0, 0]).is_err());
    let mut reader = FrameReader::default();
    assert!(matches!(
        reader.read(&mut &b"\x28\x36"[..]),
        Err(Error::Disconnected)
    ));
}

#[test]
fn exact_usb_id_recognition_excludes_unrelated_asus_and_xreal_devices() {
    assert_eq!(device::identify(0x3318, 0x0438).unwrap().model, Model::One);
    assert!(device::identify(0x3318, 0x0437).unwrap().recovery);
    assert!(device::identify(0x0b05, 0x1d9d).is_some());
    assert!(device::identify(0x0b05, 0x1234).is_none());
    assert!(device::identify(0x3318, 0xffff).is_none());
    assert_eq!(Model::Air2Ultra.air_interfaces(), Some((0, 2)));
}

#[test]
fn arbitrary_inputs_never_panic() {
    let mut seed = 0x9234abcd_u32;
    for len in 0..1100 {
        let bytes = (0..len)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                seed as u8
            })
            .collect::<Vec<_>>();
        let _ = Packet::decode(&bytes);
        let _ = SensorCommand::decode(&bytes);
        let _ = ImuSample::decode(&bytes);
        let _ = Frame::decode(&bytes);
        let _ = SensorReport::decode(&Frame {
            opcode: 0x2836,
            body: bytes.clone(),
        });
        let _ = protobuf::fields(&bytes);
    }
}
