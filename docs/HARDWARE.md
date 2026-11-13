# Local hardware validation

Date: 2026-11-13. Device: **XREAL One**, USB `3318:0438`.
Host: macOS, ARM64. No XREAL SDK or proprietary library was loaded by the Rust
process. Firmware, networking, display modes, brightness, dimming, scene mode
and USB configuration were left unchanged. The extended suite temporarily
toggled proximity and stabilization, verified each readback, and restored both
original values. Device serials and raw captures are not included in the package.

## Successful read paths

| Operation | Observed result |
| --- | --- |
| HID enumeration | Vendor control collection at interface 0, usage `ff00:0001`; USB consumer-control collection at interface 8 was not opened |
| USB MCU firmware query | `15.1.03.603_20260914` |
| USB DSP firmware query | `15.A.00.069_20241211` |
| USB serial query | Nonempty; matched the TCP identifier (redacted) |
| USB configuration query | `0000aa55` |
| USB camera attachment query | false; no Eye attached |
| TCP software version | `1.9.1.20260914175625` |
| TCP DSP version | `15.A.00.069_20241211`; agrees with USB |
| TCP configuration query | Valid JSON with IMU, display, distortion and camera-calibration sections |
| TCP EDID readback | 9 |
| TCP proximity enable readback | true |
| TCP stabilization enable readback | true |
| TCP sensor stream | 1,000 complete `2836` frames, each with a 128-byte body (134,000 capture bytes total) |

The Ethernet interface had `169.254.2.10/24`. Both the default host route and
explicit source-address binding worked. Active sensor fields in the captured
frames contained no non-finite values. Device timestamps were nondecreasing.
This validates the framing and implemented read commands on this particular
One/firmware combination. It does not establish sensor accuracy, coordinate
alignment with a renderer, or support on another firmware/model/OS.

Raw development captures and the device's calibration JSON remain under
`target/hardware/`, which is ignored and excluded from the crate package.

## Extended stress and edge cases

All 12 tests in `tests/hardware_stress.rs` passed against the same connected
One/firmware. They use valid, read-only requests. Malformed frames and invalid
payloads are tested locally; they are not fuzzed against the device.

| Test | Verified result |
| --- | --- |
| Persistent control session | 500 mixed requests; stable versions, identifier and settings |
| Large configuration responses | 21 identical UTF-8 responses of 193,584 bytes, interleaved with version reads |
| Control reconnects | 25 sessions, successful version and EDID query on each |
| Concurrent control | 3 simultaneous sessions; 180 successful requests |
| Continuous report integrity | 30,000 frames: 21,429 IMU and 8,571 magnetometer; no backwards timestamps or frame-counter discontinuities per sensor |
| Slow report consumer | 300 ms pause during that continuous session; resumed without missing counters |
| Control while streaming | 200 DSP queries during the 30,000-frame session |
| Abort and reopen report streams | Dropped connections after 1,3,5,6,7,17,64,133 bytes; each replacement decoded 100 valid frames |
| Small application I/O chunks | 12 control queries written in 1–5 byte chunks; 700 reports read in chunks down to one byte |
| Injected late replies | A local relay withheld real responses after byte 3,7,11 until the client timed out; subsequent request succeeded and the late reply retained its original ID |
| Idle timeout and local rejection | 20 consecutive sessions: idle timeout, brightness 10 rejection, oversized-request rejection and successful subsequent version query |
| USB stress/reopen | 310 successful queries across one sustained session and ten reopen cycles |
| USB device rejection | Brightness read returned status 4; client required reopening, after which firmware queries succeeded |
| All transports together | 200 USB queries, 100 TCP queries and 5,000 decoded reports with workers running concurrently |

The frame counter is interpreted as a little-endian 24-bit counter, compared
separately for `(report_type, imu_id)` with wraparound. These counts establish
continuity in the tested sessions, not a guarantee against loss in every host
or device condition. Small application reads/writes test stream framing; they
do not prove particular Ethernet packet segmentation. Idle checks allow valid
unsolicited frames before requiring a timeout, rather than assuming silence.

The repeated idle check observed an unsolicited opcode **`278a`**, body length
**9**, in one session. Its semantics are unknown; the SDK returns the complete
raw frame. An initial test that required the very next read to be a timeout
failed. That assertion was corrected to accept valid events, and both the full
stress rerun and all 20 repeated idle/recovery sessions passed. A synthetic
regression now verifies retaining this opcode/body shape while matching a
different pending reply; it is not presented as a captured payload.

Both tests in `tests/hardware_settings.rs` also passed:

- Proximity and stabilization accepted false and true, with readback after every
  setter. This exercises actual protobuf zero/default-value responses.
- A deliberate operation error after changing both settings triggered cleanup.
  Reconnection, device-identity checking and readback confirmed restoration.

Both original values were **true** and both final values are **true**. A baseline
journal is written before the first setter and marked restored after successful
readback. Cleanup is explicit on test errors, with an additional best-effort
Drop fallback; abrupt process termination or physical removal can still prevent
restoration. The journal is in `target/hardware/settings-restoration.json`.

Seven new offline regressions cover maximum USB/TCP payloads, empty/unknown TCP
frames, reserved bytes, protobuf field limits and full-width varints, interrupted
reads, inactive/unknown sensor fields, and the observed status-4 failure path.

## Repeat validation

```sh
cargo test --features hid --test hardware live_one_tcp_read_only -- --ignored
UNBOUND_HID_PATH='PATH_FROM_LIST' cargo test --features hid --test hardware live_one_usb_read_only -- --ignored
```

Set `UNBOUND_SOURCE` to the actual local glasses-facing IP if necessary. Tests
are ignored by default and must be selected explicitly. They issue queries and
read the existing report stream; they do not enable Ethernet or change modes.

Run the expanded read-only suite **serially** so tests do not consume one
another's replies or interfere with session baselines:

```sh
UNBOUND_HID_PATH='PATH_FROM_LIST' cargo test --features hid --test hardware_stress -- --ignored --test-threads=1 --nocapture
```

The separate settings suite performs temporary writes and requires a second,
explicit environment opt-in. It does not write brightness, dimming, display
modes, scene mode, USB configuration or firmware:

```sh
UNBOUND_ALLOW_SETTINGS_TEST=1 cargo test --test hardware_settings -- --ignored --test-threads=1 --nocapture
```

Logs from this run are in `target/hardware/extended-read-only-final.log` and
`target/hardware/extended-settings.log`; the 20-session idle follow-up is in
`target/hardware/extended-idle-repeat.log`. All hardware tests remain ignored by
ordinary `cargo test` and CI. Run settings and stress suites separately.

## Firmware limitation and untested behavior

USB brightness query `0003` consistently returned device status **4** on this
One 1.9.1 firmware. The meaning of status 4 is not established. This command
cannot currently supply a reliable brightness baseline on this device; no
brightness or dimmer writes were attempted. The generic USB API reports the
device status without inventing a value or silently retrying.

Only proximity and stabilization setters have live readback validation. Other
setters remain tested against local peers or fixtures. Display-mode changes,
physical unplug/replug, suspend/wake and OS routing changes were not exercised.
Air-family devices and other One variants have not been attached for this work.
There is no Eye attached, and its video and pose protocols remain unimplemented.
