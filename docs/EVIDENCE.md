# Evidence ledger

This document records how the wire protocol described in
[PROTOCOL.md](PROTOCOL.md) was obtained, and what this repository includes.

## What this repository contains

Unbound Protocol is the MIT-licensed implementation in this tree: codecs,
clients, tests, synthetic fixtures, and these notes. [NOTICE](../NOTICE) states
the license boundary, dependency licenses, and device trademarks.

The tree does not contain a vendor SDK, firmware image, device capture, serial
number, or vendor symbol table. It does not contain a tool that loads a vendor
library. The build does not download vendor firmware or a vendor SDK.

Device makers did not license this implementation. The authors report that the
hardware used for the notes below was bought retail, that traffic was observed
on USB HID, USB interrupt, serial, and TCP interfaces the operating system
exposes, and that device firmware was not modified. This project does not claim
a formal clean-room process. The MIT license does not grant patent rights.
Readers decide for themselves whether their own contracts and local law allow
them to build and ship it.

Community corroboration is credited in NOTICE. Public MIT-licensed drivers are
cited there; their notices are retained. The ar-drivers-rs notice is reproduced
in [licenses/ar-drivers-rs-MIT.txt](../licenses/ar-drivers-rs-MIT.txt).

## Methodology

The protocol was derived in four complementary passes, each reproducible by any
party with the same retail hardware.

### 1. Passive capture — what the device emits on its own

Before sending a single command, the devices were connected and the traffic they
produce by themselves was recorded. The glasses continuously stream IMU samples,
magnetometer vectors, key/proximity events, temple/forehead temperature, and
heartbeat packets over their public endpoints — no query required. From these
unolicited streams the report layouts, field offsets, sizes, endianness, and
units were read directly. This is the strongest evidence for sensor formats,
because the structure is fully visible without needing to know any command.

Example: an Air IMU report is 64 bytes; version 1 and version 2 differ only in
signed-axis width and field offsets, both legible straight from the captured
frames. The One TCP sensor opcode `2836` and its body were likewise observed on
an idle control connection, not invented.

### 2. Active black-box probing — read-only first

With the read formats known, candidate control requests were sent over the open
interface and the round-trip responses recorded. The probing was deliberately
conservative:

- **Reads before writes.** Serial number, DSP/MCU/pilot firmware version,
  calibration JSON, USB configuration, and camera-attach state were all obtained
  with read-only commands that leave the device unchanged.
- **Setters last, and reversible.** Brightness, display mode, EDID selector,
  stabilization, and scene-mode writes were confirmed by reading back the value
  the device reports, and the documented setters save/restore original values
  (see [HARDWARE.md](HARDWARE.md)).
- **Behaviour, not secrets.** Each command's request/response pair was matched by
  transaction ID or command echo; timing and status bytes were noted, and
  firmware-specific rejections (e.g. One 1.9.1 USB brightness read returning
  status `4`) were recorded as observed rather than explained.

Black-box probing of a black box you own is standard protocol discovery and
requires no vendor cooperation.

### 3. Structural analysis — framing and checksums

The packet framing (magic bytes, length fields, terminators) and the checksums
were derived from structural regularities in the captured bytes, not guessed:

- The USB/`0xaa` CRC is the reflected CRC-32 with polynomial `edb88320` and
  `0xFFFFFFFF` init/XOR — the same standard `zlib.crc32` used everywhere. It was
  confirmed by reproducing the exact CRC bytes present in captured requests.
- Light's IEEE-CRC32 over `02 … <CRC>` and Grawoow's wrapping byte-sum checksum
  were recovered by matching the checksum field against the documented payload
  bytes across many samples.
- The One protobuf property envelopes (`18 00`, `1a 02 08 05`, `22 02 10 09`,
  `22 00`, …) are standard protobuf wire encoding; the byte strings are the
  literal encoding of field-number/wire-type messages, verifiable against any
  protobuf reference.

### 4. Cross-check against public community work

Independent MIT-licensed community drivers corroborate and extend the captures.
The library was written from wire facts, but where a layout is thin or a command
is ambiguous, the community reference is cited as the second source:

- **The Unbound Launcher** — Dot Portal branding and general protocol notes.
- **ar-drivers-rs** (Alex Badics, MIT) — Air/Light sensor and MCU layouts.
- **xr-tools** (0xcaff, MIT) — protocol observations; no source copied.

These are public and permissively licensed, so citing them needs no permission.

## Evidence tiers

Every fact in this project falls into one of three tiers, listed highest
confidence first. The confidence of a fact is the confidence of its weakest
tier.

| Tier | Meaning | Example |
| --- | --- | --- |
| **Live capture** | Raw wire traffic recorded from hardware owned by the authors, over an OS-exposed transport, device identifiers redacted. | One 1.9.1 IMU over TCP; Air IMU report layout |
| **Synthetic fixture** | A packet constructed from the documented layout by `tools/generate_fixtures.py` and checked by the codec tests. Proves the codec is self-consistent; it does not by itself prove device behaviour. | `tests/fixtures/*.bin` |
| **Community reference** | MIT-licensed public protocol observation, cited as a corroborating source. | Light OV580 streaming, Rokid report fields |

Captures are redacted of serial numbers and device identifiers before they enter
the repo, and constructed packets are always marked **synthetic** — never labeled
as hardware captures.

## Provenance by family

| Family | Primary evidence | Notes |
| --- | --- | --- |
| **XREAL One** | Live capture, One 1.9.1 | Read paths and proximity/stabilization setters tested on real hardware; see [HARDWARE.md](HARDWARE.md). |
| **Air / Air 2 / Air 2 Pro** | Live capture + synthetic fixtures | Sensor and MCU layouts captured; device testing pending for some models. |
| **Air 2 Ultra** | Community reference + Air subset | Interface mapping from community work; device testing pending. |
| **Rokid Air / Max** | Community reference + synthetic fixtures | Shared VID/PID; product string distinguishes them. |
| **Light / Light 2 / Light 2 Pro** | Community reference + synthetic fixtures | OV580 and MCU layouts from community work. |
| **Grawoow G530 / Metavision M53** | Community reference + synthetic fixtures | OV580 and MCU layouts from community work. |
| **Mad Gaze Glow** | Community reference + synthetic fixtures | Serial + I²C sensor init from community driver. |

Where a fact is only community- or fixture-tier, [PROTOCOL.md](PROTOCOL.md) and
[GLASSES.md](GLASSES.md) say so explicitly rather than implying live validation.

## Reproducing this yourself

Anyone can verify the methodology with the same retail hardware:

1. Buy the device and connect it over a normal transport (USB HID, serial, or the
   glasses' published TCP/IP interface). No special privileges beyond device
   access the OS already grants.
2. Record the device's own output (IMU, keys, temperature, heartbeats) before
   sending anything.
3. Send read-only queries first; confirm setters by reading back.
4. Reconstruct the documented packets and check them with
   `tools/generate_fixtures.py` and the codec tests.

The repository build does not download a vendor SDK or vendor firmware.
