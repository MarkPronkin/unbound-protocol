use std::{
    collections::VecDeque,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    thread,
    time::Duration,
};
use unbound_protocol::{
    Error, Result,
    air::{SensorClient, SensorCommand},
    one::{ControlClient, ReportsClient},
    usb::{Packet, Profile, ReportIo, UsbClient},
    wire::FrameReader,
};

const TIMEOUT: Duration = Duration::from_secs(2);
fn pair() -> (TcpStream, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let server = listener.accept().unwrap().0;
    server.set_read_timeout(Some(TIMEOUT)).unwrap();
    server.set_write_timeout(Some(TIMEOUT)).unwrap();
    (client, server)
}
fn response(op: u16, id: u32, value: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend(op.to_be_bytes());
    bytes.extend((4 + value.len() as u32).to_be_bytes());
    bytes.extend(id.to_be_bytes());
    bytes.extend(value);
    bytes
}

#[test]
fn tcp_correlates_id_and_opcode_and_preserves_unsolicited_frames() {
    let (client, mut server) = pair();
    let worker = thread::spawn(move || {
        let mut got = [0; 12];
        server.read_exact(&mut got).unwrap();
        assert_eq!(&got, include_bytes!("fixtures/tcp_version_request.bin"));
        // One 1.9.1 also emits unsolicited 0x278a bodies of nine bytes while
        // idle. This payload is synthetic; preserve it without assuming a
        // transaction-ID prefix or requiring a known event opcode.
        let unknown_event = [0x27, 0x8a, 0, 0, 0, 9, 0x80, 0, 0, 1, 5, 4, 3, 2, 1];
        server.write_all(&unknown_event).unwrap();
        let event = [0x27, 0x2e, 0, 0, 0, 0];
        server.write_all(&event).unwrap();
        server.write_all(&response(0x271c, 1, &[0x22, 0])).unwrap(); // same ID, wrong opcode
        server.write_all(&response(0x271d, 99, &[0x22, 0])).unwrap(); // stale ID
        for byte in response(0x271d, 1, &[0x22, 5, 0x12, 3, b'a', b'b', b'c']) {
            server.write_all(&[byte]).unwrap();
        }
        server.read_exact(&mut got).unwrap();
        assert_eq!(&got[6..10], &[0x80, 0, 0, 2]);
        server
            .write_all(&response(0x275e, 2, &[0x22, 2, 0x10, 9]))
            .unwrap();
    });
    let mut control = ControlClient::from_stream(client, TIMEOUT).unwrap();
    assert_eq!(control.software_version().unwrap(), "abc");
    let unknown = control.pop_event().unwrap();
    assert_eq!(unknown.opcode, 0x278a);
    assert_eq!(unknown.body, [0x80, 0, 0, 1, 5, 4, 3, 2, 1]);
    assert_eq!(control.pop_event().unwrap().opcode, 0x272e);
    assert_eq!(control.pop_event().unwrap().opcode, 0x271c);
    assert_eq!(control.pop_event().unwrap().body[..4], 99u32.to_be_bytes());
    assert!(control.pop_event().is_none());
    assert_eq!(control.edid().unwrap(), 9);
    worker.join().unwrap();
}

#[test]
fn timed_out_partial_reply_does_not_corrupt_next_transaction() {
    let (client, mut server) = pair();
    let worker = thread::spawn(move || {
        let mut got = [0; 12];
        server.read_exact(&mut got).unwrap();
        let old = response(0x275e, 1, &[0x22, 2, 0x10, 7]);
        server.write_all(&old[..3]).unwrap();
        // The next request is the synchronization signal; no timing-dependent sleep.
        server.read_exact(&mut got).unwrap();
        server.write_all(&old[3..]).unwrap();
        server
            .write_all(&response(0x275e, 2, &[0x22, 2, 0x10, 9]))
            .unwrap();
    });
    let mut control = ControlClient::from_stream(client, Duration::from_millis(100)).unwrap();
    assert!(matches!(control.edid(), Err(Error::Timeout)));
    assert_eq!(control.edid().unwrap(), 9);
    assert_eq!(&control.pop_event().unwrap().body[..4], &1u32.to_be_bytes());
    worker.join().unwrap();
}

#[test]
fn eof_is_reported_and_connection_cannot_be_reused() {
    let (client, mut server) = pair();
    let worker = thread::spawn(move || {
        let mut got = [0; 12];
        server.read_exact(&mut got).unwrap();
    });
    let mut control = ControlClient::from_stream(client, TIMEOUT).unwrap();
    assert!(matches!(control.edid(), Err(Error::Disconnected)));
    assert!(matches!(control.edid(), Err(Error::Disconnected)));
    worker.join().unwrap();
}

#[test]
fn setters_use_golden_request_and_validate_ack() {
    let (client, mut server) = pair();
    let worker = thread::spawn(move || {
        let mut got = [0; 14];
        server.read_exact(&mut got).unwrap();
        assert_eq!(&got, include_bytes!("fixtures/tcp_brightness_request.bin"));
        server.write_all(&response(0x271c, 1, &[0x22, 0])).unwrap();
    });
    let mut control = ControlClient::from_stream(client, TIMEOUT).unwrap();
    assert!(control.set_brightness(10).is_err()); // must not send anything
    control.set_brightness(5).unwrap();
    worker.join().unwrap();
}

#[test]
fn reports_read_multiple_frames_and_return_eof() {
    let (client, mut server) = pair();
    let worker = thread::spawn(move || {
        let bytes = include_bytes!("fixtures/one_imu_frame.bin");
        server.write_all(bytes).unwrap();
        server.write_all(bytes).unwrap();
    });
    let mut reports = ReportsClient::from_stream(client, TIMEOUT).unwrap();
    for _ in 0..2 {
        assert_eq!(reports.next_report().unwrap().timestamp_ns, 123456789);
    }
    assert!(matches!(reports.next_frame(), Err(Error::Disconnected)));
    worker.join().unwrap();
}

#[test]
fn event_flood_is_bounded() {
    let (client, mut server) = pair();
    let worker = thread::spawn(move || {
        let _ = FrameReader::default().read(&mut server).unwrap();
        let events = [0x27, 0x2e, 0, 0, 0, 0].repeat(129);
        server.write_all(&events).unwrap();
    });
    let mut control = ControlClient::from_stream(client, TIMEOUT).unwrap();
    assert!(matches!(control.edid(), Err(Error::QueueFull)));
    let mut count = 0;
    while control.pop_event().is_some() {
        count += 1;
    }
    assert_eq!(count, 128);
    assert!(matches!(control.edid(), Err(Error::Disconnected)));
    worker.join().unwrap();
}

#[derive(Default)]
struct FakeHid {
    reads: VecDeque<Vec<u8>>,
    writes: Vec<Vec<u8>>,
}
impl ReportIo for FakeHid {
    fn write_report(&mut self, report: &[u8], _: Duration) -> Result<()> {
        self.writes.push(report.to_vec());
        Ok(())
    }
    fn read_report(&mut self, buffer: &mut [u8], _: Duration) -> Result<usize> {
        let data = self.reads.pop_front().ok_or(Error::Timeout)?;
        buffer[..data.len()].copy_from_slice(&data);
        Ok(data.len())
    }
}
fn usb_reply(command: u16, id: u32, data: &[u8]) -> Vec<u8> {
    Packet::request(command, id, &[&[0], data].concat())
        .encode(1024)
        .unwrap()
}

#[test]
fn one_usb_correlates_replies_and_queues_events() {
    let io = FakeHid {
        reads: [usb_reply(0x26, 99, b"old"), usb_reply(0x26, 1, b"v1")].into(),
        ..Default::default()
    };
    let mut client = UsbClient::new(io, Profile::One, TIMEOUT).unwrap();
    assert_eq!(client.mcu_version().unwrap(), "v1");
    assert_eq!(client.pop_event().unwrap().request_id, 99);
    let io = client.into_inner();
    assert_eq!(io.writes[0].len(), 1024);
    assert_eq!(Packet::decode(&io.writes[0]).unwrap().request_id, 1);
}

#[test]
fn air_accepts_command_only_reply_and_uses_64_byte_report() {
    let io = FakeHid {
        reads: [usb_reply(7, 0, &[3])].into(),
        ..Default::default()
    };
    let mut client = UsbClient::new(io, Profile::Air, TIMEOUT).unwrap();
    assert_eq!(client.air_display_mode().unwrap(), 3);
    let io = client.into_inner();
    assert_eq!(io.writes[0].len(), 64);
    assert_eq!(Packet::decode(&io.writes[0]).unwrap().request_id, 0x1337);
}

#[test]
fn usb_timeout_requires_reopen_and_no_automatic_retry() {
    let mut client = UsbClient::new(FakeHid::default(), Profile::Air, TIMEOUT).unwrap();
    assert!(matches!(client.mcu_version(), Err(Error::Timeout)));
    assert!(matches!(client.mcu_version(), Err(Error::Disconnected)));
    assert_eq!(client.into_inner().writes.len(), 1);
}

#[test]
fn sensor_configuration_reassembles_chunks_and_stream_command() {
    let answer = |command, data: &[u8]| {
        SensorCommand {
            command,
            payload: data.to_vec(),
        }
        .encode()
        .unwrap()
        .to_vec()
    };
    let io = FakeHid {
        reads: [
            answer(0x19, &[]),
            answer(0x14, &7u32.to_le_bytes()),
            answer(0x15, b"{\"a"),
            answer(0x15, b"\":1}"),
        ]
        .into(),
        ..Default::default()
    };
    let mut client = SensorClient::new(io, TIMEOUT).unwrap();
    client.set_streaming(false).unwrap();
    assert_eq!(client.read_configuration().unwrap(), "{\"a\":1}");
    assert_eq!(client.into_inner().writes.len(), 4);
}

#[test]
fn sensor_stream_decode_and_empty_calibration_chunk_rejection() {
    let io = FakeHid {
        reads: [include_bytes!("fixtures/air_imu_v2.bin").to_vec()].into(),
        ..Default::default()
    };
    assert_eq!(
        SensorClient::new(io, TIMEOUT)
            .unwrap()
            .next_sample()
            .unwrap()
            .gyro_raw,
        [-123, 456, -789]
    );
    let answer = |command, data: &[u8]| {
        SensorCommand {
            command,
            payload: data.to_vec(),
        }
        .encode()
        .unwrap()
        .to_vec()
    };
    let io = FakeHid {
        reads: [answer(0x14, &7u32.to_le_bytes()), answer(0x15, &[])].into(),
        ..Default::default()
    };
    assert!(
        SensorClient::new(io, TIMEOUT)
            .unwrap()
            .read_configuration()
            .is_err()
    );
}

#[test]
fn sensor_rejects_reply_for_another_command() {
    let wrong = SensorCommand {
        command: 0x14,
        payload: vec![],
    }
    .encode()
    .unwrap()
    .to_vec();
    let io = FakeHid {
        reads: [wrong].into(),
        ..Default::default()
    };
    let mut client = SensorClient::new(io, TIMEOUT).unwrap();
    assert!(client.set_streaming(true).is_err());
    assert!(matches!(
        client.set_streaming(true),
        Err(Error::Disconnected)
    ));
}

#[test]
fn connection_options_bind_requested_source_address() {
    use unbound_protocol::one::ConnectionOptions;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let options = ConnectionOptions {
        host: "127.0.0.1".parse().unwrap(),
        source: Some("127.0.0.1".parse().unwrap()),
        timeout: TIMEOUT,
    };
    let stream = options
        .connect(listener.local_addr().unwrap().port())
        .unwrap();
    assert_eq!(stream.local_addr().unwrap().ip(), options.source.unwrap());
    let (_server, peer) = listener.accept().unwrap();
    assert_eq!(peer.ip(), options.source.unwrap());
}
