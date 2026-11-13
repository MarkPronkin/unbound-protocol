#!/usr/bin/env python3
"""Independent, synthetic wire vectors. Uses only Python's standard library.

These are constructed fixtures, not hardware captures, and not output from a
vendor library. The codec tests check them.
"""
import pathlib
import struct
import zlib

ROOT = pathlib.Path(__file__).resolve().parents[1]
DEST = ROOT / "tests" / "fixtures"
DEST.mkdir(parents=True, exist_ok=True)


def usb(command, payload=b"", request_id=1, size=64):
    tail = struct.pack("<HIIH5x", 17 + len(payload), request_id, 0, command) + payload
    wire = b"\xfd" + struct.pack("<I", zlib.crc32(tail)) + tail
    return wire.ljust(size, b"\0")


def tcp(opcode, body):
    return struct.pack(">HI", opcode, len(body)) + body


vectors = {
    "usb_mcu_request.bin": usb(0x26),
    "usb_brightness_request.bin": usb(4, bytes([5])),
    "usb_one_response.bin": usb(0x26, b"\0test-version", size=1024),
    "tcp_version_request.bin": tcp(0x271d, bytes.fromhex("800000011800")),
    "tcp_brightness_request.bin": tcp(0x271c, bytes.fromhex("800000011a020805")),
}
tail = bytes.fromhex("04001901")  # length=4 LE, command=19, streaming=1
vectors["sensor_start_request.bin"] = (b"\xaa" + struct.pack("<I", zlib.crc32(tail)) + tail).ljust(64, b"\0")

for version in (1, 2):
    data = bytearray(64)
    struct.pack_into("<BBHQHI", data, 0, 1, version, 50, 123456789, 4000, 65536 if version == 1 else 16777216)
    acc, mag, ts, flag = (24, 36, 48, 56) if version == 1 else (27, 42, 54, 62)
    width = 2 if version == 1 else 3
    for i, value in enumerate((-123, 456, -789)):
        data[18 + i * width:18 + (i + 1) * width] = value.to_bytes(width, "little", signed=True)
    struct.pack_into("<HI", data, acc, 32, 65536 if version == 1 else 16777216)
    for i, value in enumerate((-100, 200, 300)):
        data[acc + 6 + i * width:acc + 6 + (i + 1) * width] = value.to_bytes(width, "little", signed=True)
    struct.pack_into("<HIHHH", data, mag, 32768, 100, 1, 2, 3)
    struct.pack_into("<Q", data, ts, 987654321)
    data[flag] = 1
    vectors[f"air_imu_v{version}.bin"] = bytes(data)

report = bytearray(128)
struct.pack_into("<QQ", report, 0, 0x1234, 123456789)
struct.pack_into("<I10fB3s", report, 24, 11, -1.0, 2.0, -3.0, 0.0, -9.8, 0.0, 4.0, 5.0, 6.0, 25.0, 1, bytes.fromhex("010203"))
vectors["one_imu_frame.bin"] = tcp(0x2836, report)

# Additional families: independently packed from the documented wire offsets.
light_prefix = b"\x02:3:C:x:0:"
vectors["light_serial_request.bin"] = (
    light_prefix + f"{zlib.crc32(light_prefix):8x}".encode() + b":\x03"
).ljust(64, b"\0")
light = bytearray(128)
light[0] = 1
struct.pack_into("<QII3i", light, 44, 1234567890123, 2000, 32768, -123, 456, -789)
struct.pack_into("<QII3i", light, 72, 1234567890456, 2, 32768, -100, 200, 16384)
vectors["light_imu.bin"] = light
rokid = bytearray(64)
struct.pack_into("<BQ9fBBBQ3xBB", rokid, 0, 17, 1234567890123,
                 1.0, -9.81, 3.0, -0.1, 0.2, -0.3, 4.0, 5.0, -6.0,
                 0xa5, 1, 0, 99887766, 7, 8)
vectors["rokid_combined.bin"] = rokid
grawoow = bytearray(128)
struct.pack_into("<3i", grawoow, 0x3c, -164, 328, -492)
struct.pack_into("<3i", grawoow, 0x58, -16384, 8192, 4096)
vectors["grawoow_imu.bin"] = grawoow
vectors["grawoow_serial_request.bin"] = bytes.fromhex("aa bb 80 05 00 00 85")
vectors["mad_gaze_mode_request.bin"] = bytes.fromhex("3a 53 33 44 06 ab cd 01 00 00 ff")
vectors["mad_gaze_fifo.bin"] = struct.pack("<6h", -123, 456, -789, -100, 200, 16384)

for name, data in vectors.items():
    (DEST / name).write_bytes(data)
print(f"wrote {len(vectors)} synthetic fixtures")
