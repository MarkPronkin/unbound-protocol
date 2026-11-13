# Unified application API

`unbound_protocol::glasses::Glasses` is a synchronous, non-generic handle for
every recognized family: Air variants, One variants and ROG R1, Light, Rokid
Air/Max, Grawoow G530/Metavision M53, and Mad Gaze Glow. Application code can
select a connection once and then use the same controls and sensor loop.
The handle is `Send`, so applications can move it to their own I/O worker.

This layer adapts the existing protocols. It adds no hardware compatibility
claims beyond the [hardware evidence](HARDWARE.md). A capability means the
selected transport implements the operation; firmware may still reject it.

## Connect

```rust,no_run
use std::time::Duration;
use unbound_protocol::glasses::{Connection, Glasses};

fn main() -> unbound_protocol::Result<()> {
let mut glasses = Glasses::connect(Connection::default(), Duration::from_secs(2))?;
println!("{:?}: {}", glasses.model(), glasses.serial()?);
let capabilities = glasses.capabilities();
if capabilities.brightness.can_write() {
    glasses.set_brightness(0.5)?;
}
if capabilities.sensors {
    glasses.start_sensors()?;
    let report = glasses.next_report()?;
    if let Some(sample) = report.sensors {
        if let Some(gyro) = sample.gyro_rad_s {
            println!("{:?}: {:?} rad/s", gyro.timestamp, gyro.value);
        }
    }
    if capabilities.stop_sensors {
        glasses.stop_sensors()?;
    }
}
Ok(())
}
```

The default connection is One TCP at `169.254.2.1`, with control and reports
sockets. It uses the existing host route. Override `host` and `source` through
`Connection::OneTcp`; set `reports: false` for a control-only connection.
Specify the actual One-family `Model` when connecting another variant.
The TCP connection cannot independently identify that variant.

Other connection recipes:

| Recipe | Required feature | Selected endpoints |
| --- | --- | --- |
| `Connection::Hid { model, control, sensor }` | `hid` | Owned `CString` paths; optional sensor path for Air/Light |
| `Connection::RokidUsb { device }` | `libusb` | `UsbAddress { bus, address }` |
| `Connection::GrawoowUsb { control, sensor }` | `libusb` | MCU address and optional separate sensor address |
| `Connection::MadGazeSerial { port }` | `serial` | Serial port path/name |

`Glasses::connect(connection, timeout)` rejects a zero timeout. Connection
recipes are available in all builds; connecting through a disabled backend
returns `Error::Unsupported` with the needed feature name. Protocol adapters
and TCP require no optional feature.

For example, attach selected Air 2 interfaces:

```rust,no_run
use std::{ffi::CString, time::Duration};
use unbound_protocol::{device::Model, glasses::{Connection, Glasses}};

fn main() -> Result<(), Box<dyn std::error::Error>> {
let connection = Connection::Hid {
    model: Model::Air2,
    control: CString::new("MCU_PATH_FROM_INVENTORY")?,
    sensor: Some(CString::new("SENSOR_PATH_FROM_INVENTORY")?),
};
let glasses = Glasses::connect(connection, Duration::from_secs(2))?;
Ok(())
}
```

## Discover and select devices

`glasses::discover()` returns a `Vec<DeviceInfo>` from enabled native backends.
Each record includes the exact USB identity, model/recovery state, endpoint,
interface role where known, and available descriptor strings. HID records
include their path, interface number, usage page, and usage. USB records use
bus/address. Serial records use the port name. No native features means an
empty list; TCP endpoints are configured explicitly.

Discovery does not open devices, probe the network, or choose the first match.
It lists interfaces, so multiple records can belong to one device. Separate
Light/Grawoow MCU and sensor identities must be paired explicitly. With multiple
pairs connected, let the application/user choose both endpoints. Rokid Air and
Max retain the shared `Model::RokidAirMax` identity.

HID connection validates the model, recovery state, and interface role whenever
the OS reports enough information. Air MCU/sensor serial strings must agree
when both are available. One HID control usages need explicit path selection;
One sensors require its TCP report connection. USB connections validate the
expected MCU/sensor VID/PID before claiming interfaces. Serial ports are explicit
application selections and can also refer to platform-provided bridges.

Opening/attaching a handle sends no control commands and does not change display
settings, enable/reset sensors, or configure host networking.

## Capabilities and controls

All handles provide `model()`, `capabilities()`, and `serial()`.
`Access` exposes `can_read()` and `can_write()` for asymmetric controls.

| Connection | Brightness | Display mode | Sensors | Configuration | Control events |
| --- | --- | --- | --- | --- | --- |
| One TCP | Write | Write input layout | With report socket | Yes | Yes |
| One HID | Read/write | Unsupported | Unsupported | Unsupported | Yes |
| Air HID | Read/write | Read/write | With sensor interface | With sensor interface | Yes |
| Light HID | Unsupported | Read/write | With sensor interface | With sensor interface | Yes; explicitly enable |
| Rokid USB | Unsupported | Read/write | Yes | Unsupported | In sensor reports |
| Grawoow USB | Unsupported | Read/write | With sensor interface | Yes | Unsupported |
| Mad Gaze serial | Write | Read/write | Explicit initialization | Unsupported | Unsupported |

`firmware_version()` returns the One TCP software version or XREAL USB MCU
version. One TCP also provides proximity and stabilization getters/setters.
Unimplemented operations return `Error::Unsupported`, without sending commands.
Native device status, timeout, and disconnect errors are preserved. A failed
setter is never automatically retried or rolled back.

`set_brightness(f32)` accepts finite values in `0.0..=1.0`, rounding to the nearest
native step: XREAL `0..9`, Mad Gaze `1..7`. `brightness()` returns a normalized
native readback; write-only connections never substitute cached values.

`DisplayMode::Mirror`, `Stereo`, and `HalfStereo` describe layouts; ordinary
modes do not promise a universal refresh rate. Air/Light expose supported exact
refresh variants; Rokid has firmware-dependent `*HighRefresh` selectors.
Consult `capabilities().display_modes` before setting. Unsupported modes fail
before any I/O, and unknown native readbacks remain `DisplayMode::Unknown(u8)`.
Rokid half-SBS is readable but cannot be written without protocol evidence.
One's layout setter changes input mode and leaves its EDID selector untouched.
Applications still need host display timing support for the requested mode.

## Sensors and events

`start_sensors()` explicitly enables Air/Light streams or initializes Mad Gaze's
BMI160/AK09911 sensors. It is idempotent after success. One/Rokid/Grawoow streams
need no startup commands. `stop_sensors()` disables Air/Light streams; other
families return `Error::Unsupported`. Dropping a handle closes transports and
does not send cleanup commands. Applications should explicitly stop streams
when `capabilities().stop_sensors` is true, including on error paths.

`next_report()` reads the sensor channel into a `Report`:

- `sensors: Option<SensorSample>` supplies only readings present in that report.
  `gyro_rad_s` and `accel_m_s2` are optional `TimedVector` values, with full integer
  device timestamps when known. Coordinates remain in each device's axes.
- `events: Vec<Event>` contains common key, button, proximity, ambient-light,
  and vsync concepts. Numeric codes/states remain device specific. Proximity
  `near` is populated only where the protocol establishes its meaning.
- `raw: RawReport` retains native decoded data, including unknown packet kinds,
  counters, raw temperature/magnetic registers, and extra clocks/state.

One report type 11 supplies IMU values; type 4 supplies magnetic values in
`MagneticFieldUnit::DeviceNative`. Rokid/Mad Gaze magnetic readings use
`Microtesla`. Air's unvalidated magnetic registers remain in `raw`.
Mad Gaze magnetic polling uses `poll_magnetometer()` after startup; unavailable
or overflowed magnetic samples have no normalized reading but retain all flags.

Light retains separate gyro/acceleration nanosecond clocks. Air's report clock
is attached to both normalized vectors; its second sensor clock remains in
`raw`. Rokid separate sensor packets retain microseconds, while combined packets
retain nanoseconds. Separate packets are never silently merged. Grawoow and Mad
Gaze have no fabricated timestamp. These clocks are not UTC and do not establish
a shared coordinate frame or synchronization between devices.

Mad Gaze FIFO polling drains both returned entries in order without discarding
the second sample or issuing a redundant poll. An empty FIFO returns
`RawReport::Empty` with no sensors. Applications should pace FIFO polling to
their needs. Unknown reports also have no normalized sensors; they remain raw
reports rather than being silently skipped.

`next_event()` reads the separate One/Air/Light MCU event channel and drains
events queued during control exchanges. Light requires
`set_events_enabled(true)` to request MCU event generation. Rokid state travels
with `next_report()`. The two read methods do not multiplex independent channels;
choose which channel to read in the application. `Report::from_raw` applies the
same normalization to decoded fixtures/offline native reports.

Light's stereo-presence handshake is performed when setting a stereo mode.
Call `service()` at least every 250 ms while using Light, including while idle.
The adapter also services its heartbeat throughout control/sensor waits and
configuration exchanges. It uses the full requested operation timeout.
No background maintenance thread is started. `service()` is a no-op for other
families. Air/Light `configuration()` requires streaming to be stopped and will
not implicitly stop/restart it.

## Custom transports and native APIs

`Glasses::from_air`, `from_one_usb`, `from_light`, and `from_mad_gaze` accept
custom `ReportIo + Send + 'static` transports. `from_rokid` and `from_grawoow`
accept `ControlIo + Send + 'static`. Optional sensor transports use
`Some(Box::new(sensor_io))`. These constructors are available without native
backend features and make platform integration and mock testing possible.

`from_one_tcp` accepts existing `one::ControlClient` and optional
`one::ReportsClient`, including platform-bound TCP streams. Existing native
protocol clients remain available for controls that have no common equivalent,
such as EDID, dimming, USB configuration, raw commands, and calibration envelopes.
The unified layer does not implement fusion, rendering, cameras, or 6DoF.
