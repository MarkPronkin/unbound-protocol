//! Opt-in hardware stress tests. All device traffic in this file is read-only.
//! Run serially: cargo test --test hardware_stress --features hid -- --ignored --test-threads=1 --nocapture
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    thread,
    time::{Duration, Instant},
};
use unbound_protocol::{
    Error, Result,
    one::{ConnectionOptions, ControlClient, ReportsClient, SensorReport},
    wire::{Frame, FrameReader},
};

fn options() -> ConnectionOptions {
    let mut options = ConnectionOptions::default();
    if let Ok(source) = std::env::var("UNBOUND_SOURCE") {
        options.source = Some(
            source
                .parse()
                .expect("UNBOUND_SOURCE must be an IP address"),
        );
    }
    options
}

#[test]
#[ignore = "requires a connected One; 500 read requests on one TCP session"]
fn live_one_persistent_control_stress() -> Result<()> {
    let mut client = ControlClient::connect(&options())?;
    let baseline = (
        client.software_version()?,
        client.dsp_version()?,
        client.serial()?,
        client.edid()?,
        client.proximity_enabled()?,
        client.stabilization_enabled()?,
    );
    let mut latencies = Vec::new();
    let start = Instant::now();
    for i in 0..500 {
        let request = Instant::now();
        match i % 6 {
            0 => assert!(client.software_version()? == baseline.0),
            1 => assert!(client.dsp_version()? == baseline.1),
            2 => assert!(client.serial()? == baseline.2),
            3 => assert_eq!(client.edid()?, baseline.3),
            4 => assert_eq!(client.proximity_enabled()?, baseline.4),
            _ => assert_eq!(client.stabilization_enabled()?, baseline.5),
        }
        latencies.push(request.elapsed().as_micros());
    }
    latencies.sort_unstable();
    println!(
        "persistent_control requests=500 elapsed_ms={} median_us={} p99_us={} max_us={}",
        start.elapsed().as_millis(),
        latencies[250],
        latencies[494],
        latencies[499]
    );
    Ok(())
}

#[test]
#[ignore = "requires a connected One; repeated 190 KiB calibration responses"]
fn live_one_large_configuration_repeated() -> Result<()> {
    let mut client = ControlClient::connect(&options())?;
    let baseline = client.configuration()?;
    assert!(baseline.len() > 1024);
    for _ in 0..20 {
        let config = client.configuration()?;
        assert!(config == baseline, "configuration changed between reads");
        assert!(!client.software_version()?.is_empty());
    }
    println!(
        "configuration reads=21 bytes_per_response={}",
        baseline.len()
    );
    Ok(())
}

#[test]
#[ignore = "requires a connected One; 25 control reconnections"]
fn live_one_control_reconnects() -> Result<()> {
    let mut baseline = None;
    for _ in 0..25 {
        let mut client = ControlClient::connect(&options())?;
        let version = client.software_version()?;
        if let Some(expected) = &baseline {
            assert!(&version == expected);
        } else {
            baseline = Some(version);
        }
        let _ = client.edid()?;
    }
    println!("control_reconnects sessions=25");
    Ok(())
}

#[test]
#[ignore = "requires a connected One; 3 concurrent control sessions"]
fn live_one_concurrent_control_sessions() -> Result<()> {
    let clients = (0..3)
        .map(|_| ControlClient::connect(&options()))
        .collect::<Result<Vec<_>>>()?;
    let results = thread::scope(|scope| {
        clients
            .into_iter()
            .map(|mut client| {
                scope.spawn(move || -> Result<()> {
                    for _ in 0..30 {
                        assert!(!client.software_version()?.is_empty());
                        let _ = client.edid()?;
                    }
                    Ok(())
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|worker| worker.join().expect("control worker panicked"))
            .collect::<Vec<_>>()
    });
    for result in results {
        result?;
    }
    println!("concurrent_control sessions=3 requests=180");
    Ok(())
}

#[test]
#[ignore = "requires a connected One; 30,000 sensor frames with control traffic"]
fn live_one_stream_integrity_under_control_load() -> Result<()> {
    let mut reports = ReportsClient::connect(&options())?;
    let start = Instant::now();
    let control = thread::spawn(|| -> Result<()> {
        let mut client = ControlClient::connect(&options())?;
        for _ in 0..200 {
            assert!(!client.dsp_version()?.is_empty());
        }
        Ok(())
    });
    let mut states = BTreeMap::<(u32, u8), (u64, u32, u64, u64, u64)>::new();
    let read_result = (|| -> Result<()> {
        for i in 0..30_000 {
            if i == 5_000 {
                thread::sleep(Duration::from_millis(300));
            } // Exercise a slow consumer and queued frames.
            let sample = reports.next_report()?;
            let key = (sample.report_type, sample.imu_id);
            let counter = u32::from_le_bytes([
                sample.frame_id[0],
                sample.frame_id[1],
                sample.frame_id[2],
                0,
            ]);
            if let Some(state) = states.get_mut(&key) {
                assert!(
                    sample.timestamp_ns >= state.0,
                    "device timestamp moved backwards"
                );
                let delta = counter.wrapping_sub(state.1) & 0x00ff_ffff;
                if delta != 1 {
                    state.3 += 1;
                }
                state.4 = state.4.max(sample.timestamp_ns - state.0);
                state.0 = sample.timestamp_ns;
                state.1 = counter;
                state.2 += 1;
            } else {
                states.insert(key, (sample.timestamp_ns, counter, 1, 0, 0));
            }
        }
        Ok(())
    })();
    let control_result = control.join().expect("control worker panicked");
    read_result?;
    control_result?;
    assert!(states.keys().any(|&(kind, _)| kind == 11));
    println!(
        "stream frames=30000 elapsed_ms={}",
        start.elapsed().as_millis()
    );
    for ((kind, imu), (_, _, count, discontinuities, max_dt)) in states {
        println!(
            "stream_type type={kind} imu={imu} frames={count} counter_discontinuities={discontinuities} max_timestamp_delta_ns={max_dt}"
        );
        assert_eq!(
            discontinuities, 0,
            "frame counter discontinuity in a continuous session"
        );
    }
    Ok(())
}

#[test]
#[ignore = "requires a connected One; abort report connections at partial frame boundaries"]
fn live_one_reports_aborted_mid_frame_reconnect() -> Result<()> {
    let options = options();
    for cut in [1, 3, 5, 6, 7, 17, 64, 133] {
        let mut raw = options.connect(unbound_protocol::one::REPORTS_PORT)?;
        raw.set_read_timeout(Some(options.timeout))?;
        raw.read_exact(&mut vec![0; cut])?;
        drop(raw);
        let mut next = ReportsClient::connect(&options)?;
        for _ in 0..100 {
            next.next_report()?;
        }
    }
    println!("report_abort_reconnect cuts=8 verified_frames=800");
    Ok(())
}

struct LimitedReader<'a> {
    stream: &'a mut TcpStream,
    chunk: usize,
}
impl Read for LimitedReader<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        let n = bytes.len().min(self.chunk);
        self.stream.read(&mut bytes[..n])
    }
}
#[test]
#[ignore = "requires a connected One; tiny application reads/writes against real streams"]
fn live_one_tiny_chunks() -> Result<()> {
    let options = options();
    let mut stream = options.connect(unbound_protocol::one::CONTROL_PORT)?;
    stream.set_read_timeout(Some(options.timeout))?;
    stream.set_write_timeout(Some(options.timeout))?;
    let mut reader = FrameReader::default();
    for id in 1..=12_u32 {
        let request = Frame {
            opcode: unbound_protocol::one::opcode::SOFTWARE_VERSION,
            body: [&(id | 0x80000000).to_be_bytes()[..], &[0x18, 0]].concat(),
        }
        .encode()?;
        for part in request.chunks((id as usize % 5) + 1) {
            stream.write_all(part)?;
        }
        let response = reader.read(&mut LimitedReader {
            stream: &mut stream,
            chunk: (id as usize % 7) + 1,
        })?;
        assert_eq!(
            response.opcode,
            unbound_protocol::one::opcode::SOFTWARE_VERSION
        );
        assert_eq!(&response.body[..4], &id.to_be_bytes());
        assert!(!unbound_protocol::protobuf::string_response(&response.body[4..])?.is_empty());
    }
    let mut stream = options.connect(unbound_protocol::one::REPORTS_PORT)?;
    stream.set_read_timeout(Some(options.timeout))?;
    let mut reader = FrameReader::default();
    for chunk in [1, 2, 3, 5, 7, 31, 133] {
        for _ in 0..100 {
            SensorReport::decode(&reader.read(&mut LimitedReader {
                stream: &mut stream,
                chunk,
            })?)?;
        }
    }
    println!("tiny_chunks control_requests=12 report_frames=700 smallest_read=1");
    Ok(())
}

#[test]
#[ignore = "requires a connected One; inject local reply timeouts through a TCP relay"]
fn live_one_late_reply_after_injected_timeout() -> Result<()> {
    for cut in [3, 7, 11] {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let mut upstream = options().connect(unbound_protocol::one::CONTROL_PORT)?;
        upstream.set_read_timeout(Some(Duration::from_secs(2)))?;
        upstream.set_write_timeout(Some(Duration::from_secs(2)))?;
        let client = TcpStream::connect(listener.local_addr()?)?;
        let (mut downstream, _) = listener.accept()?;
        downstream.set_read_timeout(Some(Duration::from_secs(2)))?;
        downstream.set_write_timeout(Some(Duration::from_secs(2)))?;
        let relay = thread::spawn(move || -> Result<()> {
            let mut reader = FrameReader::default();
            upstream.write_all(&reader.read(&mut downstream)?.encode()?)?;
            let first = FrameReader::default().read(&mut upstream)?.encode()?;
            downstream.write_all(&first[..cut])?;
            // The second request proves the client observed its deadline.
            let second = reader.read(&mut downstream)?;
            downstream.write_all(&first[cut..])?;
            upstream.write_all(&second.encode()?)?;
            downstream.write_all(&FrameReader::default().read(&mut upstream)?.encode()?)?;
            Ok(())
        });
        let mut control = ControlClient::from_stream(client, Duration::from_millis(250))?;
        let first = control.software_version();
        let result = (|| -> Result<()> {
            assert!(
                matches!(first, Err(Error::Timeout)),
                "first request should time out"
            );
            assert!(!control.software_version()?.is_empty());
            let late = control.pop_event().expect("late reply must be preserved");
            assert_eq!(&late.body[..4], &1u32.to_be_bytes());
            Ok(())
        })();
        drop(control);
        let relay_result = relay.join().expect("relay panicked");
        result?;
        relay_result?;
    }
    println!("injected_timeouts partial_header_and_body_cases=3 recovery=ok");
    Ok(())
}

#[test]
#[ignore = "requires a connected One; idle event timeout and rejected client inputs"]
fn live_one_timeouts_and_invalid_inputs_leave_session_usable() -> Result<()> {
    for cycle in 0..20 {
        let stream = options().connect(unbound_protocol::one::CONTROL_PORT)?;
        let mut client = ControlClient::from_stream(stream, Duration::from_millis(250))?;
        let start = Instant::now();
        let mut unsolicited = 0;
        loop {
            match client.next_event() {
                Err(Error::Timeout) => break,
                Ok(frame) => {
                    println!(
                        "idle_unsolicited opcode={:04x} bytes={}",
                        frame.opcode,
                        frame.body.len()
                    );
                    unsolicited += 1;
                    assert!(
                        unsolicited <= 32 && start.elapsed() < Duration::from_secs(3),
                        "idle event traffic never became quiet"
                    );
                }
                Err(error) => return Err(error),
            }
        }
        assert!(start.elapsed() < Duration::from_secs(4));
        assert!(matches!(client.set_brightness(10), Err(Error::Invalid(_))));
        assert!(matches!(
            client.request(0x271d, &vec![0; unbound_protocol::wire::MAX_BODY]),
            Err(Error::TooLarge { .. })
        ));
        assert!(!client.software_version()?.is_empty());
        println!(
            "timeout_and_invalid_inputs cycle={cycle} idle_timeout_recovered=true unsolicited_frames={unsolicited} oversized_and_range_rejections=ok"
        );
    }
    Ok(())
}

#[cfg(feature = "hid")]
fn usb()
-> std::result::Result<unbound_protocol::usb::UsbClient<unbound_protocol::hid::HidTransport>, Error>
{
    let path = std::ffi::CString::new(
        std::env::var("UNBOUND_HID_PATH")
            .map_err(|_| Error::Invalid("UNBOUND_HID_PATH is required"))?,
    )
    .map_err(|_| Error::Invalid("NUL in HID path"))?;
    let api = hidapi::HidApi::new()?;
    unbound_protocol::usb::UsbClient::new(
        unbound_protocol::hid::HidTransport::open(
            &api,
            &path,
            unbound_protocol::usb::Profile::One,
        )?,
        unbound_protocol::usb::Profile::One,
        Duration::from_secs(2),
    )
}

#[cfg(feature = "hid")]
#[test]
#[ignore = "requires One control HID path; USB, TCP control and reports at the same time"]
fn live_one_usb_tcp_and_reports_concurrent() -> Result<()> {
    let mut reports = ReportsClient::connect(&options())?;
    let usb_worker = thread::spawn(|| -> Result<()> {
        let mut client = usb()?;
        for _ in 0..100 {
            assert!(!client.mcu_version()?.is_empty());
            let _ = client.camera_plugged()?;
        }
        Ok(())
    });
    let tcp_worker = thread::spawn(|| -> Result<()> {
        let mut client = ControlClient::connect(&options())?;
        for _ in 0..100 {
            assert!(!client.dsp_version()?.is_empty());
        }
        Ok(())
    });
    let read_result = (|| -> Result<()> {
        for _ in 0..5000 {
            reports.next_report()?;
        }
        Ok(())
    })();
    let usb_result = usb_worker.join().expect("USB worker panicked");
    let tcp_result = tcp_worker.join().expect("TCP worker panicked");
    read_result?;
    usb_result?;
    tcp_result?;
    println!("all_transports_concurrent usb_requests=200 tcp_requests=100 report_frames=5000");
    Ok(())
}

#[cfg(feature = "hid")]
#[test]
#[ignore = "requires One control HID path; 300 USB requests and 10 reopen cycles"]
fn live_one_usb_stress_and_reopen() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut client = usb()?;
    let baseline = (
        client.mcu_version()?,
        client.dsp_version()?,
        client.serial()?,
        client.usb_configuration()?,
        client.camera_plugged()?,
    );
    let start = Instant::now();
    for _ in 0..60 {
        assert!(client.mcu_version()? == baseline.0);
        assert!(client.dsp_version()? == baseline.1);
        assert!(client.serial()? == baseline.2);
        assert_eq!(client.usb_configuration()?, baseline.3);
        assert_eq!(client.camera_plugged()?, baseline.4);
    }
    drop(client);
    for _ in 0..10 {
        let mut client = usb()?;
        assert!(client.mcu_version()? == baseline.0);
    }
    println!(
        "usb_stress requests=310 reopen_cycles=10 elapsed_ms={}",
        start.elapsed().as_millis()
    );
    Ok(())
}

#[cfg(feature = "hid")]
#[test]
#[ignore = "requires One control HID path; rejected USB brightness query and recovery"]
fn live_one_usb_rejection_and_recovery() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut client = usb()?;
    let version = client.mcu_version()?;
    match client.brightness() {
        Ok(level) => println!("usb_brightness firmware_accepts_read=true level={level}"),
        Err(Error::DeviceStatus(status)) => {
            println!("usb_brightness rejected_status={status}");
            assert!(matches!(client.mcu_version(), Err(Error::Disconnected)));
            drop(client);
            assert!(usb()?.mcu_version()? == version);
            println!("usb_rejection_reopen recovered=true");
        }
        Err(error) => return Err(error.into()),
    }
    Ok(())
}
