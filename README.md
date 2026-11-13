<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/unbound-logo-dark.svg">
    <img src="assets/unbound-logo.png" width="520" alt="Unbound Protocol — Dot Portal logo">
  </picture>
</p>

<h1 align="center">Unbound Protocol</h1>

<p align="center">
  <strong>Talk directly to your AR glasses.</strong><br>
  An independent Rust library and CLI for USB, HID, serial and TCP control and sensor streaming.
</p>

<p align="center">
  <a href="https://github.com/MarkPronkin/unbound-protocol/actions/workflows/ci.yml"><img src="https://github.com/MarkPronkin/unbound-protocol/actions/workflows/ci.yml/badge.svg" alt="Rust checks"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue" alt="MIT license"></a>
  <img src="https://img.shields.io/badge/Rust-1.85%2B-orange" alt="Rust 1.85 or newer">
</p>

<p align="center">
  <a href="#quick-start">Quick start</a> ·
  <a href="#use-it-in-rust">Rust API</a> ·
  <a href="docs/PROTOCOL.md">Wire protocol</a> ·
  <a href="docs/HARDWARE.md">Hardware tests</a>
</p>

Unbound Protocol gives applications direct access to glasses through ordinary
HID devices, USB transfers, serial ports and TCP sockets. Read firmware and
calibration data, receive IMU samples, inspect device events, and send supported
control commands. Everything
needed at runtime is open source; no vendor SDK, companion service, or Unity
runtime is required.

**Experimental, with real hardware testing.** The XREAL One path has been tested
on firmware **1.9.1**: 30,000 continuous sensor frames without counter gaps,
concurrent USB/TCP access, reconnects, timeout recovery, and reversible boolean
settings. Other devices and firmware need their own validation.

## What works

| Family | Implemented | Hardware evidence |
| --- | --- | --- |
| **XREAL One** | USB information/configuration; TCP control, calibration, events, IMU and magnetometer reports | Read paths and proximity/stabilization setters tested on One 1.9.1 |
| **Air / Air 2 / Air 2 Pro** | USB control and events; sensor start/stop, calibration reads, IMU revisions 1 and 2 | Wire layouts and synthetic fixtures; device testing pending |
| **Air 2 Ultra** | Air control/sensor subset with its interface mapping | Device testing pending; additional cameras/sensors not implemented |
| **Other One variants / ROG R1** | Exact USB identity recognition and shared protocol access | Recognition alone does not establish compatibility |
| **XREAL / Nreal Light** | HID serial/display control, events, stereo heartbeat, calibration reads and IMU | Community protocol evidence and synthetic fixtures; device testing pending |
| **Rokid Air / Max** | USB serial/display control, separate and combined sensor reports, raw key/proximity states | Community protocol evidence and synthetic fixtures; device testing pending |
| **Grawoow G530 / Metavision M53** | USB serial/display control, calibration reads and OV580 IMU | Community protocol evidence and synthetic fixtures; device testing pending |
| **Mad Gaze Glow** | Serial display/brightness control, I²C sensor initialization, BMI160 FIFO and AK09911 magnetometer | Community protocol evidence and synthetic fixtures; device testing pending |

Controls include firmware/serial reads, brightness, dimming, proximity,
stabilization, scene mode, EDID selection, and SBS input where the device accepts
them. [The hardware report](docs/HARDWARE.md) identifies which operations have
actual device readback validation.

Camera video and 6DoF, firmware flashing, sensor fusion, rendering, and
an OpenXR runtime are outside the current implementation. Video output continues
to use DisplayPort Alt Mode.

## Quick start

Requires **Rust 1.85+**. Build from source; the crate is not published to crates.io.

```sh
git clone https://github.com/MarkPronkin/unbound-protocol.git
cd unbound-protocol
cargo install --path . --locked --features hid
unbound-protocol --help
```

### XREAL One: read information and sensors

Enable TCP/IP on the glasses and connect their USB Ethernet interface. The OS
must provide a route to `169.254.2.1`; Unbound Protocol leaves host networking
under your control.

```sh
unbound-protocol info
unbound-protocol imu 100
unbound-protocol config
```

If the default route selects another network, bind to the **actual local IP of
the glasses-facing interface**:

```sh
unbound-protocol --source 169.254.2.10 info
```

`169.254.2.10` is the address observed on the tested setup, not a universal host
address. `--host` and `--timeout-ms` are also configurable. Connecting does not
change display modes or write device settings.

### USB/HID: select an explicit interface

```sh
unbound-protocol list
unbound-protocol hid one 'CONTROL_PATH_FROM_LIST' info
unbound-protocol hid one 'CONTROL_PATH_FROM_LIST' usb-config
unbound-protocol hid air 'CONTROL_PATH_FROM_LIST' info
unbound-protocol hid sensor 'SENSOR_PATH_FROM_LIST' imu 100
```

Air/Air 2/Air 2 Pro use MCU/sensor interfaces **4/3**; Air 2 Ultra uses **0/2**.
Select the intended HID path, including its usage when the OS does not expose an
interface number. Linux also needs permission to access the HID node.

The default Cargo build includes TCP support. The optional `hid` feature enables
the open-source HID backend. `libusb` enables Rokid/Grawoow USB access and links
the shared system libusb (LGPL-2.1-or-later); install libusb 1.0 and pkg-config
before enabling it. `serial` enables Mad Gaze ports through `serialport`
(MPL-2.0). All protocol codecs and generic clients are available
without those optional backends. A custom `ReportIo` or `ControlIo` adapter can
integrate other platform transports; Android integrations can pass an already bound TCP stream
to `from_stream`.

### Light, Rokid, Grawoow and Mad Gaze

```sh
# The libusb feature needs a shared system libusb. See NOTICE.
cargo install --path . --locked --features hid,libusb,serial
unbound-protocol list
unbound-protocol hid light 'MCU_PATH' info
unbound-protocol light 'MCU_PATH' 'SENSOR_PATH' imu 100
unbound-protocol usb rokid BUS ADDRESS info
unbound-protocol usb rokid BUS ADDRESS imu 100
unbound-protocol usb grawoow MCU_BUS MCU_ADDRESS config
unbound-protocol usb grawoow-sensor SENSOR_BUS SENSOR_ADDRESS imu 100
unbound-protocol mad-gaze /dev/PORT info
unbound-protocol mad-gaze /dev/PORT imu 100
```

Replace paths and decimal bus/address values with the current inventory.
Light and Grawoow expose separate MCU and sensor USB identities. Rokid Air and
Max share one VID/PID, so identity alone cannot distinguish them. Light stereo
requires a continuing MCU heartbeat; the combined Light IMU command services it.
Mad Gaze sensor commands explicitly reset/enable its sensors before polling.
See [additional glasses](docs/GLASSES.md) for modes, APIs and remaining limitations.

## Use it in Rust

Add the repository as a Git dependency:

```toml
[dependencies]
unbound-protocol = { git = "https://github.com/MarkPronkin/unbound-protocol.git" }
```

```rust,no_run
use std::time::Duration;
use unbound_protocol::glasses::{Connection, Glasses};

fn main() -> unbound_protocol::Result<()> {
    // Default: XREAL One TCP. Select another explicit Connection for other models.
    let mut glasses = Glasses::connect(Connection::default(), Duration::from_secs(2))?;
    println!("{:?}: {}", glasses.model(), glasses.serial()?);

    if glasses.capabilities().brightness.can_write() {
        glasses.set_brightness(0.5)?; // 0.0..=1.0, mapped to native device steps
    }
    glasses.start_sensors()?;
    if let Some(sample) = glasses.next_report()?.sensors {
        println!("Gyro (rad/s): {:?}", sample.gyro_rad_s);
    }
    Ok(())
}
```

The [`glasses` API](docs/API.md) provides one unified interface for all supported
glasses families. It covers discovery, explicit connections, capabilities,
common display modes, normalized brightness, sensor lifecycle, and reports/events
with native data retained.
Capabilities account for missing interfaces and transport differences;
unsupported operations return `Error::Unsupported`. Existing `one`, `air`,
`light`, `rokid`, `grawoow`, `mad_gaze`, and `usb` APIs remain available for
protocol-specific commands and custom integrations.

The Cargo package and the executable are **`unbound-protocol`**; the Rust
import is **`unbound_protocol`**. See the
[unified example](examples/glasses.rs), [One example](examples/one_imu.rs),
and [Air example](examples/air_imu.rs).

Samples retain device coordinates and full device-clock timestamps. Air readings
are scaled from the packet's numerator/denominator; One readings already carry
floating-point sensor values. Bias correction and orientation estimation are
left to the application. Configuration reads return the device's own UTF-8 JSON.
Light retains separate gyro/acceleration nanosecond clocks. Rokid retains the
wire clock's original units for each packet type. Grawoow and Mad Gaze samples
do not claim a device timestamp where the reference protocol supplies none.

## Capture once, inspect offline

```sh
unbound-protocol capture session.bin 1000
unbound-protocol decode session.bin
unbound-protocol decode tests/fixtures/one_imu_frame.bin
```

Captures preserve complete TCP frames, including unknown opcodes and fields.
The format is a concatenated wire stream, not PCAP. Existing files are never
overwritten. Real captures may include device identifiers; the bundled fixtures
are synthetic.

## Behavior worth knowing

- **Readback matters.** Accepting an EDID or SBS command does not prove the host
  has selected a working video timing. EDID selectors vary by firmware.
- **Firmware can reject commands.** One 1.9.1 returns status 4 for USB brightness
  reads. The library returns that status rather than inventing a brightness value.
- **Timeouts do not trigger retries.** A setter may already have taken effect.
  TCP retains partial frames and late replies; failed USB exchanges require
  reopening the client. USB reconfiguration also requires rediscovery.
- **HID writes use OS deadlines.** HID reads and TCP operations are bounded by
  library timeouts; hidapi has no portable write-timeout API.

Detailed layouts, command IDs, and event behavior live in
[PROTOCOL.md](docs/PROTOCOL.md).

## Test and contribute

```sh
cargo test --locked --all-features
cargo fmt --all --check
cargo clippy --locked --all-features --all-targets -- -D warnings
```

CI checks Linux, macOS, Windows, and the minimum supported Rust version.
Hardware tests are **ignored by default**. The [hardware guide](docs/HARDWARE.md)
explains the read-only stress suite and separately enabled setting tests, which
save and restore their original values.

Contributions should include a reproducible observation, the affected device and
firmware, and tests for valid and malformed inputs. See
[CONTRIBUTING.md](docs/CONTRIBUTING.md) and the [evidence ledger](docs/EVIDENCE.md).

## Credits and license

Created and maintained by [MarkPronkin](https://github.com/MarkPronkin).
Protocol research builds on independent investigations in
[The Unbound Launcher](https://github.com/MarkPronkin/the-unbound-launcher),
[ar-drivers-rs](https://github.com/badicsalex/ar-drivers-rs), and
[xr-tools](https://github.com/0xcaff/xr-tools). Source-specific credits and license
notices are retained in [NOTICE](NOTICE) and [EVIDENCE.md](docs/EVIDENCE.md).

The Light, Rokid, Grawoow and Mad Gaze modules, and parts of the Air module,
were written with reference to
[ar-drivers-rs](https://github.com/badicsalex/ar-drivers-rs) by Alex Badics,
used under the MIT license. Its copyright and permission notice is reproduced
in [licenses/ar-drivers-rs-MIT.txt](licenses/ar-drivers-rs-MIT.txt).

Protocol research also builds on
[xr-tools](https://github.com/0xcaff/xr-tools) by Martin Charles
([0xcaff](https://github.com/0xcaff)), licensed under the MIT license.
Its copyright and permission notice is reproduced in
[licenses/xr-tools-MIT.txt](licenses/xr-tools-MIT.txt).

Source, documentation, and original branding are [MIT licensed](LICENSE).
Dependencies retain their own licenses. See [NOTICE](NOTICE).

XREAL, Nreal, Rokid, Grawoow, Metavision, Mad Gaze, Nubia, ROG, and OpenXR are
trademarks of their respective owners. Unbound Protocol is not sponsored by or
affiliated with them.
