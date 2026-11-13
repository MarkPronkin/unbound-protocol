# Additional AR glasses

Applications can use the common [`Glasses` API](API.md) for all families below.
The family-specific APIs documented here remain available for native controls,
protocol research, and custom transport integrations.

The core controls and sensor protocols for every family listed by
[ar-drivers-rs at 56bfa48](https://github.com/badicsalex/ar-drivers-rs/tree/56bfa489ab697706ea95b36c2e43c5ca15d6a263)
are implemented independently here. Air support already existed. The added
families below have synthetic fixtures and mock transport coverage, **not new
hardware validation**. The upstream source is MIT licensed; see NOTICE.

| Family | Module | Transport feature | USB identities |
| --- | --- | --- | --- |
| XREAL / Nreal Light | `light` | `hid` | MCU `0486:573c`, sensor `05a9:0680` |
| Rokid Air / Max | `rokid` | `libusb` | `04d2:162f` (shared; product string distinguishes models) |
| Grawoow G530 / Metavision M53 | `grawoow` | `libusb` | MCU `1ff7:0ff4`, sensor `05a9:0f87` |
| Mad Gaze Glow | `mad_gaze` | `serial` | USB serial `04b4:0002`, 921600 baud, 8N1 |

The protocol modules, decoders and clients are always available. Optional
features provide OS transports. `ReportIo` supports custom HID/serial adapters;
`usb::control::ControlIo` supports USB setup/interrupt adapters. The `libusb`
feature links the shared system libusb and does not bundle it. Install libusb
1.0 before building that feature; see NOTICE. Linux requires access to the
selected USB/HID/serial nodes; Windows libusb access requires an appropriate
USB driver for the selected interface. Existing One TCP/HID and Air APIs retain
their behavior.

## Selection and display controls

`unbound-protocol list` lists identities using the enabled backends. Select the complete
HID path, decimal USB bus/address, or serial port path explicitly. USB addresses
can change on reconnect. Do not pair MCU/sensor paths belonging to different
physical glasses. The CLI checks that each path has the correct role's VID/PID;
physical pairing remains the caller's responsibility.

```sh
unbound-protocol hid light MCU_PATH mode           # read the raw ASCII response
unbound-protocol hid light MCU_PATH mode 3         # stereo 1080; requires ongoing heartbeat
unbound-protocol hid light MCU_PATH events 100     # MCU events and continuing heartbeat
unbound-protocol hid light-sensor SENSOR_PATH config
unbound-protocol usb rokid BUS ADDRESS mode 1
unbound-protocol usb grawoow BUS ADDRESS mode 1
unbound-protocol mad-gaze PORT mode 1
unbound-protocol mad-gaze PORT brightness 7
```

Light modes are 1 mirror, 2 half-SBS, 3 full stereo, 4 stereo 72 Hz. Rokid setters
accept 0 mirror, 1 stereo, 3 high-refresh mirror, 4 high-refresh stereo; its
half-SBS read value 2 is preserved but has no evidenced setter in the reference.
Grawoow/Mad Gaze use 0 mirror, 1 stereo. Mad Gaze brightness is 1..7. Mode getters
retain unknown device responses. A successful command does not establish host
video timing or firmware compatibility.

Light requires an SDK-presence command before stereo and a continuing heartbeat
every 250 ms. A one-shot mode command does not keep the display alive after it
exits. Run a continuing event/IMU reader or service `ControlClient::heartbeat`
from the application. The combined `unbound-protocol light MCU_PATH SENSOR_PATH imu COUNT`
command reads sensors with short waits so stalled sensors do not starve the MCU.
It leaves display mode selection explicit and stops the IMU stream on completion.

## Rust API

Enable the backends needed by the application:

```toml
unbound-protocol = { git = "https://github.com/MarkPronkin/unbound-protocol.git", features = ["hid", "libusb", "serial"] }
```

Rokid example, with explicit bus/address from inventory:

```rust,no_run
use std::time::Duration;
use unbound_protocol::{libusb::UsbTransport, rokid};

fn read(bus: u8, address: u8) -> unbound_protocol::Result<()> {
    let io = UsbTransport::open_interrupt(bus, address, rokid::ENDPOINT)?;
    let mut glasses = rokid::Client::new(io, Duration::from_secs(1))?;
    println!("{}", glasses.serial()?);
    println!("{:?}", glasses.next_packet()?);
    Ok(())
}
```

For Light, open MCU and sensor HID paths with
`HidTransport::open_with_prefix(..., ReportPrefix::ProtocolByte)`, then construct
`light::ControlClient` and `light::SensorClient`. Stop streaming before reading
configuration. Call `enable_stereo` before selecting stereo and service
`heartbeat` while reading sensors. `next_event` also services heartbeats during
its wait. Sensor command replies lack a transaction ID; do not issue commands
concurrently on one interface.

Grawoow uses `ControlClient` on `UsbTransport::open_interface(bus, address, 0)`
and `SensorClient` on `UsbTransport::open_interrupt(bus, address, 0x89)`.
Calibration reads return device JSON unchanged; the binary Light calibration
envelope is also available through `read_configuration_blob`.

Mad Gaze uses `mad_gaze::Client::new(SerialTransport::open(path, timeout)?, timeout)`.
`initialize_sensors` explicitly resets/enables the sensors; `read_imu` polls up
to two BMI160 FIFO entries and returns an empty vector for an empty FIFO.
`read_magnetometer` retains AK09911 status flags, including data-ready/overflow
bits for callers to interpret.

## Data and failure behavior

Samples retain device axes without bias correction, axis rotation, fusion or
optical calibration. Gyro is rad/s and acceleration is m/s²; exposed raw fields
allow independent interpretation. Rokid/Mad Gaze magnetometer scaling follows
the community reference. Separate Rokid sensor reports are returned individually
with their timestamps, rather than silently merging mismatched samples.

Light preserves both full nanosecond timestamps. Rokid separate-sensor reports
carry microseconds, combined reports nanoseconds; vsync is retained without an
assumed unit. Grawoow has no established device timestamp. Mad Gaze does not
fabricate one by counting FIFO samples. Its BMI160 layout is gyro then
acceleration: the upstream reference's swapped output fields are corrected.

Commands use bounded transfers and do not retry setters. Light MCU/sensor,
Grawoow MCU and Mad Gaze clients require reopening after a failed wire exchange
because late replies cannot be correlated safely. Pure argument validation does
not invalidate a client. Light MCU unsolicited packets are queued up to 128;
unknown events and Rokid report types remain accessible. Continuous sensor reads
can recover from timeouts when no command is in flight.

Grawoow response checksums and Mad Gaze checksum bytes are preserved without
claiming an algorithm absent from the reference. Mad Gaze delimiter-substitution
indexing is unverified: outbound `:` payloads and inbound substituted frames are
rejected explicitly. Normal unescaped frames are supported. These limitations
need device traces before they can be extended safely.

Camera video, SLAM, camera descriptor math and optical/display transforms from
the upstream library are outside this SDK's control/sensor scope. Calibration
data is available to applications, but no 6DoF or OpenXR runtime is provided.
