//! Boundary/error cases are injected locally, never sent malformed to glasses.
use std::{
    collections::VecDeque,
    io::{self, Read},
    time::Duration,
};
use unbound_protocol::{
    Error, Result,
    one::SensorReport,
    protobuf::{self, Value},
    usb::{Packet, Profile, ReportIo, UsbClient},
    wire::{Frame, FrameReader, MAX_BODY},
};

#[test]
fn usb_maximum_report_and_reserved_fields() {
    let mut packet = Packet::request(0xffff, u32::MAX, &vec![0; 1002]);
    packet.timestamp = u32::MAX;
    packet.reserved = [1, 2, 3, 4, 5];
    let bytes = packet.encode(1024).unwrap();
    assert_eq!(bytes.len(), 1024);
    assert_eq!(Packet::decode(&bytes).unwrap(), packet);
    packet.payload.push(1);
    assert!(matches!(packet.encode(1024), Err(Error::TooLarge { .. })));
}

#[test]
fn tcp_body_limit_and_zero_length_unknown_frames() {
    let max = Frame {
        opcode: 0xffff,
        body: vec![0x80; MAX_BODY],
    };
    let encoded = max.encode().unwrap();
    assert_eq!(
        FrameReader::default()
            .read(&mut encoded.as_slice())
            .unwrap(),
        max
    );
    assert!(
        Frame {
            opcode: 0xffff,
            body: vec![0; MAX_BODY + 1]
        }
        .encode()
        .is_err()
    );
    let empty = Frame {
        opcode: 0,
        body: vec![],
    };
    let bytes = [&empty.encode().unwrap()[..], &encoded[..]].concat();
    let mut input = bytes.as_slice();
    let mut reader = FrameReader::default();
    assert_eq!(reader.read(&mut input).unwrap(), empty);
    assert_eq!(reader.read(&mut input).unwrap(), max);
}

#[test]
fn protobuf_field_limit_and_full_width_integers() {
    assert_eq!(protobuf::fields(&[8, 0].repeat(4096)).unwrap().len(), 4096);
    assert!(matches!(
        protobuf::fields(&[8, 0].repeat(4097)),
        Err(Error::TooLarge { .. })
    ));
    let mut response = vec![0x22, 11, 0x10];
    response.extend([0xff; 9]);
    response.push(1);
    assert_eq!(protobuf::numeric_response(&response).unwrap(), u64::MAX);
    let request = protobuf::set_request(u64::MAX);
    let outer = protobuf::fields(&request).unwrap();
    let Value::Bytes(inner) = outer[0].value else {
        panic!("expected nested field");
    };
    assert_eq!(
        protobuf::fields(inner).unwrap()[0].value,
        Value::Varint(u64::MAX)
    );
    assert!(protobuf::string_response(&[0x22, 3, 0x12, 1, 0xff]).is_err());
}

#[test]
fn one_sensor_checks_only_active_fields_and_preserves_unknown_types() {
    let mut frame = Frame::decode(include_bytes!("fixtures/one_imu_frame.bin")).unwrap();
    frame.body[24..28].copy_from_slice(&4u32.to_le_bytes());
    frame.body[28..32].copy_from_slice(&f32::NAN.to_le_bytes());
    assert!(SensorReport::decode(&frame).is_ok()); // Inactive gyro bytes are not IMU data.
    frame.body[52..56].copy_from_slice(&f32::INFINITY.to_le_bytes());
    assert!(SensorReport::decode(&frame).is_err());
    frame.body[24..28].copy_from_slice(&99u32.to_le_bytes());
    let unknown = SensorReport::decode(&frame).unwrap();
    assert_eq!(unknown.report_type, 99);
    assert_eq!(unknown.raw, frame.body);
    for length in 0..128 {
        assert!(
            SensorReport::decode(&Frame {
                opcode: frame.opcode,
                body: frame.body[..length].to_vec()
            })
            .is_err()
        );
    }
}

struct InterruptedReader<'a> {
    bytes: &'a [u8],
    interrupt: bool,
}
impl Read for InterruptedReader<'_> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        self.interrupt = !self.interrupt;
        if self.interrupt {
            return Err(io::ErrorKind::Interrupted.into());
        }
        let n = out.len().min(self.bytes.len()).min(3);
        out[..n].copy_from_slice(&self.bytes[..n]);
        self.bytes = &self.bytes[n..];
        Ok(n)
    }
}
#[test]
fn interrupted_reads_retry_without_losing_frame_bytes() {
    let bytes = include_bytes!("fixtures/one_imu_frame.bin");
    let mut input = InterruptedReader {
        bytes,
        interrupt: false,
    };
    assert_eq!(
        FrameReader::default()
            .read(&mut input)
            .unwrap()
            .encode()
            .unwrap(),
        bytes
    );
}

struct UsbReplies {
    reads: VecDeque<Vec<u8>>,
    writes: usize,
}
impl ReportIo for UsbReplies {
    fn write_report(&mut self, _: &[u8], _: Duration) -> Result<()> {
        self.writes += 1;
        Ok(())
    }
    fn read_report(&mut self, buffer: &mut [u8], _: Duration) -> Result<usize> {
        let bytes = self.reads.pop_front().ok_or(Error::Timeout)?;
        buffer[..bytes.len()].copy_from_slice(&bytes);
        Ok(bytes.len())
    }
}
#[test]
fn usb_rejected_device_reply_requires_reopen_without_resending() {
    let rejected = Packet::request(3, 1, &[4]).encode(1024).unwrap();
    let mut client = UsbClient::new(
        UsbReplies {
            reads: [rejected].into(),
            writes: 0,
        },
        Profile::One,
        Duration::from_secs(1),
    )
    .unwrap();
    assert!(matches!(client.brightness(), Err(Error::DeviceStatus(4))));
    assert!(matches!(client.mcu_version(), Err(Error::Disconnected)));
    assert_eq!(client.into_inner().writes, 1);
}

#[test]
fn usb_validation_rejects_before_writing_and_preserves_client() {
    let reply = Packet::request(0x26, 2, b"\0version").encode(1024).unwrap();
    let mut client = UsbClient::new(
        UsbReplies {
            reads: [reply].into(),
            writes: 0,
        },
        Profile::One,
        Duration::from_secs(1),
    )
    .unwrap();
    assert!(matches!(client.set_brightness(10), Err(Error::Invalid(_))));
    assert!(matches!(
        client.command(0x26, &vec![0; 1003]),
        Err(Error::TooLarge { .. })
    ));
    assert_eq!(client.mcu_version().unwrap(), "version");
    assert_eq!(client.into_inner().writes, 1);
}
