# Protocol notes

All offsets are bytes from the start of the specified report/body. All commands
are firmware-sensitive; an ID in this document is not a guarantee that every
model accepts it. See [EVIDENCE.md](EVIDENCE.md) for source confidence.

## USB control: `0xfd`

Integers are little-endian. Air sends a 64-byte report; One sends a padded
1024-byte report. The codec also accepts an unpadded complete report.

| Offset | Size | Meaning |
| --- | --- | --- |
| 0 | 1 | `fd` magic |
| 1 | 4 | CRC-32/ISO-HDLC |
| 5 | 2 | Length from offset 5 through the payload, inclusive |
| 7 | 4 | Request ID |
| 11 | 4 | Timestamp, zero in current requests |
| 15 | 2 | Command ID |
| 17 | 5 | Reserved bytes, zero in current requests |
| 22 | variable | Payload |

The CRC covers `[5 .. 5 + length)`, excluding trailing report padding. Its
reflected polynomial is `edb88320`, initial register and final XOR `ffffffff`.
Requests have length `17 + payload_size`. Replies begin their payload with a
status byte: zero is success; subsequent bytes are command data. Unsolicited
events do not have that status prefix.

The Air request-ID behavior is less consistent across firmware. The Air profile
uses the community-observed constant `1337` and correlates by command. The One
profile increments request IDs and matches both ID and command. Failed USB
transactions require reopening the client; no implicit retries are made.

| ID | Operation | Request | Response after success status |
| --- | --- | --- | --- |
| `0003` | Read brightness | empty | level byte |
| `0004` | Write brightness | level byte | empty/firmware-specific extra bytes |
| `0007` | Read Air display mode | empty | mode byte |
| `0008` | Write Air display mode | mode byte | acknowledgement |
| `0015` | Read serial | empty | UTF-8 text |
| `0018` | Read DSP firmware | empty | UTF-8 text |
| `0026` | Read MCU firmware | empty | UTF-8 text |
| `00d2` | Read One USB configuration | empty | little-endian u32 |
| `00d3` | Write One USB configuration | little-endian u32 | acknowledgement; may disconnect |
| `00d5` | Read One camera attachment | empty | 0 plugged, 1 unplugged |
| `1213` | Read One pilot firmware | `02` | UTF-8 text |

On the tested One 1.9.1 firmware, USB brightness read `0003` returns status 4.
Its meaning is unverified; it is not translated to a brightness value. Most of
the older MCU command IDs are model-dependent, even though framing is shared.

Air display values: 1 mirror60, 3 SBS60, 4 SBS72, 5 mirror72, 8 half-SBS60,
9 SBS90, 10 mirror90, 11 mirror120. Availability depends on firmware/model.

One USB configuration is nine consecutive two-bit fields starting at bit zero:
NCM, ECM, USB audio, HID control, MTP, mass storage, camera0, camera1, apply.
Values are 0 unchanged, 1 enable, 2 disable; 3 is reserved. Higher bits are
preserved, not interpreted. The library never applies a guessed configuration.

### Air MCU events

The decoder recognizes these event IDs and payload widths. Integers stay raw
where a physical unit has not been verified on hardware.

| ID | Payload |
| --- | --- |
| `6c02` / `6c12` | temple / forehead temperature: i32 |
| `6c04` | proximity state: i32 |
| `6c05` | key type, function, parameter: three u32 |
| `6c07` | sleep state: i32 |
| `6c0b` | sequence and timestamp: two u64 |
| `6c18` | screen status: i32 |
| `6c19` | over-temperature state: i32 |
| `6c1a` | calibration kind/result: two i32 |

Any other event remains a raw `Packet`.

## Additional families

These layouts come from the MIT community reference pinned in EVIDENCE.md.
They have synthetic/mock validation here; see [GLASSES.md](GLASSES.md) for
selection, APIs, display modes and limitations.

### Light MCU and OV580

MCU HID reports are at most 64 bytes:
`02 : CATEGORY : COMMAND : PAYLOAD : TIMESTAMP : CRC : 03`, with zero padding
after the terminator. Category/command are single ASCII bytes; timestamp and CRC
are text. CRC is the same IEEE CRC32 as Air, over all bytes from `02` through
the colon preceding the CRC. The CRC field is eight lowercase hexadecimal
characters, left padded with spaces. Request timestamps are `0`; no-argument
requests use payload `x`. Payload fields cannot contain protocol delimiters.
Replies match the command and request category plus one.

| Category / command | Operation |
| --- | --- |
| `3` / `C` | Read serial |
| `3` / `3` | Read display mode description |
| `1` / `3` | Set display mode ASCII `1`..`4` |
| `@` / `3`, payload `1` | SDK presence for stereo |
| `@` / `K`, payload `x` | Heartbeat (reply may be omitted) |
| `1` / `L`, `1` / `N` | Enable ambient-light/vsync events with ASCII `1` |
| `5` / `K` | Key `UP`/`DN` |
| `5` / `P` | Proximity `near`/`away` |
| `5` / `L` | Ambient light, hexadecimal text |
| `5` / `S` | Vsync event |

Light OV580 commands are seven bytes: `[02, command, subcommand, 0, 0, 0, 0]`.
Streaming is command `19`, subcommand 0/1. Configuration starts with `14/0`,
then repeated `15/0`. Configuration chunks begin `[02, 01, length, bytes...]`;
a reply whose second byte differs from 1 terminates the blob. Empty continuation
chunks are invalid. The blob is bounded to 1 MiB. After the 40-byte header, a
JSON section begins at `\n\n{` and ends at `\n\n`. Raw blobs are also accessible.
Only report ID 2 identifies command replies; there is no echoed transaction ID.

IMU report ID is 1. Reports contain at least 100 and at most 128 bytes; unknown
trailing bytes are ignored. Gyro fields begin at 44 and acceleration at 72.
Each block is `u64 timestamp_ns`, `u32 numerator`, `u32 denominator`, `3 × i32`
in little endian. Gyro scales to degrees/s then radians/s; acceleration scales
to g then multiplies by 9.81. Denominators must be nonzero. Neither bias
subtraction nor the reference driver's axis remapping is applied.

### Rokid Air / Max

Interrupt IN endpoint `82` sends 64-byte reports. Integers/floats are LE.

| Report ID | Fields (offset / type) |
| --- | --- |
| 2 | sequence 1/u32, keys 47/u8, proximity 51/u8 |
| 4 | sensor type 1/u8, sequence 2/u32, timestamp 9/u64 (µs), vector 21/3×f32 |
| 17 | timestamp 1/u64 (ns), acceleration 9/3×f32, gyro 21/3×f32, magnetometer 33/3×f32, keys 45/u8, proximity 46/u8, vsync 48/u64, brightness 59/u8, volume 60/u8 |

Sensor types 1/2/3 denote acceleration, gyro and magnetometer, respectively.
Floats are already scaled to m/s², rad/s and µT in the community reference.
Unknown types are preserved. Key/proximity bytes remain raw states; there is
no fabricated key transition or clock conversion. Nonfinite vectors are rejected.

Vendor reads use setup `(bmRequestType=c0, bRequest=81)` with 64-byte buffers.
Serial uses `wValue=0100, wIndex=0`; display mode uses `wValue=0, wIndex=1` and
returns its value in byte 1. Mode writes use `(40, 01, mode, 1)`, payload `00`.
Air and Max share VID/PID; the product string provides the upstream distinction.

### Grawoow G530 / Metavision M53

MCU messages are `aa bb COMMAND_BE16 00 LENGTH_U8 PAYLOAD CHECKSUM_U8`.
Request checksum is the wrapping byte sum from command through payload.
Response checksum bytes are retained without validating an unverified algorithm.
Replies must match the command. Maximum payload is 249 bytes (256-byte transfer).
USB class/interface setup packets are `(21, 09, 0201, 0)` for write and
`(a1, 01, 0102, 0)` for read, on MCU interface zero.

Commands are `8005` serial, `8007` mode get, `8008` mode set (0 mirror, 1 stereo),
and `800a` calibration chunk. Calibration requests contain `[0,0,0,offset_hi,
offset_lo]`. The first six reply payload bytes are metadata; remaining bytes
append to the configuration. Payloads of at most six bytes terminate the read.
Offsets are byte counts, bounded to 65535; the returned JSON remains unmodified.

The OV580 sends 128-byte interrupt reports on endpoint `89`. Gyro is LE `3×i32`
at offset `3c`, acceleration LE `3×i32` at `58`. Scale gyro by `1/16.4` degrees/s
then convert to radians/s; acceleration by `9.81/16384`. There is no established
device clock in this layout; host elapsed time is not presented as device time.

### Mad Gaze Glow

Serial is 921600 baud, 8N1. A frame is `: COMMAND_3 LENGTH_U8 SESSION_2 PAYLOAD
CHECKSUM_2 ESCAPE_INDEX_U8`. Length counts all bytes after itself, minimum 5,
maximum 255. Requests use session `ab cd`, checksum `00 00`, escape index `ff`.
Checksums/session echo behavior are unverified in the reference. The codec
preserves them; clients correlate commands. Substitution indexing is unverified:
outbound colon payloads and non-`ff` inbound escape indices are explicitly rejected.

Commands: `GSN` serial, `G3D` mode get, `S3D` mode set (0/1), `SLB` brightness
(1..7); setters require payload `00` in their acknowledgement. I²C reads `I2R`
send `[1,address,register,0,length]`; reply payload has five header bytes then
exactly the requested data. I²C writes `I2W` use the same header plus data;
reply byte 4 is status (0 success).

AK09911 address 12 identifies as `48 05`; reset is register `32=01`, continuous
100 Hz is `31=08`. Reading nine bytes at `10` yields status, three LE i16 axes,
an unassigned byte and status2; scaling is `4912/8190` µT per count. Status and
overflow bits are preserved for the application.

BMI160 address 104 identifies as `d1`; reset `7e=b6`, acceleration enable
`7e=11`, gyro enable `7e=15`, headerless FIFO `47=c0`. Reset waits are 10 ms,
matching the reference. Reading 24 bytes at `24` polls two FIFO entries, each
three LE i16 gyro values then three LE i16 acceleration values. A first gyro
register of -32768 marks an empty FIFO. Gyro scale is `2000/32768` degrees/s,
acceleration `2*9.80665/32768` m/s². The upstream driver's swapped acceleration
and gyro outputs and synthetic 10 ms timestamp are not reproduced.

## Air sensor control: `0xaa`

| Offset | Size | Meaning |
| --- | --- | --- |
| 0 | 1 | `aa` magic |
| 1 | 4 | Same CRC32 as USB control |
| 5 | 2 | Little-endian length: 3 + payload size |
| 7 | 1 | Command |
| 8 | variable | Payload |

Reports are padded to 64 bytes. CRC coverage begins at offset 5 and ends at
`5 + length`. Start/stop is command `19`, payload `01`/`00`. Configuration
length is command `14`, returning a u32; repeated command `15` reads chunks.
The library bounds the advertised length to 1 MiB and rejects empty chunks and
chunks exceeding the advertised size. Firmware that pads the final chunk will
currently need a separately evidenced adapter policy, rather than silent truncation.

Replies must echo the command ID. Streaming samples arriving while waiting for
a command reply are consumed. No concurrent command transactions are supported.

## Air IMU reports

The first bytes are report ID 1, version 1 or 2, and a two-byte temperature
register. Each report is 64 bytes. Timestamp at offset 4 is a little-endian u64.

| Field | Version 1 offset / size | Version 2 offset / size |
| --- | --- | --- |
| Gyro numerator / denominator | 12 / u16, 14 / u32 | same |
| Gyro axes | 18,20,22 / signed i16 | 18,21,24 / signed i24 |
| Accel numerator / denominator | 24 / u16, 26 / u32 | 27 / u16, 29 / u32 |
| Accel axes | 30,32,34 / signed i16 | 33,36,39 / signed i24 |
| Magnetometer offset / denominator | 36 / u16, 38 / u32 | 42 / u16, 44 / u32 |
| Magnetometer axes | 42,44,46 / u16 | 48,50,52 / u16 |
| Sensor timestamp | 48 / u64 | 54 / u64 |
| Magnetometer updated flag | 56 / u8 | 62 / u8 |

Gyro: `raw * numerator / denominator` degrees/second, converted to radians/second.
Acceleration: `raw * numerator / denominator * 9.8` metres/second², using the
local schema's gravity. The packet carries its own scale; benchmark numerator
values are not hardcoded. Zero denominators are rejected. The schema dated
2026-03-23 changes benchmark scales but retains version-2 offsets.

There is no verified CRC field in this sensor-report layout. The library does
not invent one. Temperature and magnetometer registers remain raw, and the SDK
does not assume a signed temperature interpretation or magnetometer rotation.
All timestamps retain full nanosecond integers; they are device clocks, not UTC.

## One TCP frame envelope

The glasses IP is `169.254.2.1`. Control is TCP port **52999**; reports are
**52998**. A frame is a two-byte big-endian opcode, four-byte big-endian body
length, then exactly that many body bytes. Length does not include the header.
There is no checksum or delimiter in this envelope. Unknown opcodes are legal.
An individual body is limited to 1 MiB by this SDK, before allocation.

Control request bodies begin with a big-endian u32 transaction ID, with bit 31
set. Replies use the corresponding ID with bit 31 clear. The remaining body is
protobuf wire data. Unsolicited frames, including key events, have their own
body layouts and must not have four bytes stripped unconditionally.

Live One 1.9.1 testing also observed unsolicited opcode `278a` with a nine-byte
body on an otherwise idle control connection. Its semantics have not been
established. It remains a raw frame; absence of a known opcode does not make it
a framing error, and idle reads are not guaranteed to time out immediately.

TCP packet boundaries do not delimit frames. `FrameReader` retains partial data
across timeouts and handles multiple frames in one OS read. EOF, oversize frames
and permanent I/O errors invalidate the reader. There is no unsafe delimiter
search for `28 36`: those bytes can occur inside payloads.

### Property envelopes

- Get request: field 3 varint zero (`18 00`).
- Numeric set request: field 3 length-delimited, containing field 1 varint.
  Setting 5 is `1a 02 08 05`.
- Response: field 4 length-delimited, containing field 2 of the value's wire type.
  Numeric 9 is `22 02 10 09`; string `abc` is `22 05 12 03 61 62 63`.
- Setter acknowledgement: empty field 4 (`22 00`). Empty scalar response also
  represents protobuf's default zero. Outer and inner lengths are enforced.

This is a narrow, observed schema. The wire parser supports varint, fixed32,
fixed64 and length-delimited fields, bounds the number of fields, and rejects
overflowing varints, duplicate property values and unsupported group wire types.
Unknown outer fields can be inspected with `protobuf::fields`. Error/status
schemas outside the observed envelope are not guessed; raw `request` preserves
the full reply for investigation. Typed responses fail without field 4.

| Opcode | Operation |
| --- | --- |
| `2718` / `2719` | get/set proximity enable |
| `271c` | set brightness, observed range 0..9 |
| `271d` | get software version |
| `271f` | get calibration/configuration JSON |
| `2727` | set dimmer: 0 clear, 1 middle, 2 dark |
| `2729` | get glasses identifier |
| `272d` | get DSP version |
| `272e` | unsolicited key event |
| `275e` / `275f` | get/set EDID selector |
| `2822` | set input mode: 0 regular, 1 SBS |
| `2829` | scene mode: 0 buttons enabled, 1 buttons disabled |
| `2836` | unsolicited sensor report |
| `28a6` / `28a7` | get/set stabilization enable |

EDID selector values vary by firmware. The local Nubia evidence records selector
5 for SBS60 and selector 9 for restoring 1080p120. Older community tables use
different selectors for some 2D modes. No universal resolution-to-selector enum
is supplied. Video itself uses DisplayPort Alt Mode, not these TCP sockets.

### One sensor body, opcode `2836`

The supported body is exactly **128 bytes** (134 with the TCP header).

| Offset | Type | Meaning |
| --- | --- | --- |
| 0 | u64 LE | device ID |
| 8 | u64 LE | device timestamp, nanoseconds |
| 16 | 8 bytes | unassigned, preserved |
| 24 | u32 LE | type: 11 IMU, 4 magnetometer |
| 28 | 3 × f32 LE | gyro, rad/s |
| 40 | 3 × f32 LE | acceleration, m/s² |
| 52 | 3 × f32 LE | magnetometer, units not independently validated |
| 64 | f32 LE | temperature °C |
| 68 | u8 | IMU ID |
| 69 | 3 bytes | frame counter, preserved byte order |
| 72 | 56 bytes | unassigned, preserved |

Only the active sensor fields are checked for finite values. Unknown report
types remain readable. No gravity-magnitude filter is applied: legitimate
motion and freefall must not be classified as corrupted framing. A historical
community parser consumed only the first 84 wire bytes despite the header's
128-byte body length; Unbound Protocol always consumes the full declared frame.

Key-event bodies have a 64-byte observed layout. At offsets 0,4,8 are LE u32
key type, key state and a device time field whose units are not confirmed here.
Observed keys 1..4 refer to the lower button, front rocker, back rocker and upper
button; state 1 down, 2 up. Unknown integer values are preserved.
