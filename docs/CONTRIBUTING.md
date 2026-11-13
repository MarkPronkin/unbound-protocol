# Contributing

New code and documentation are MIT licensed. Keep vendor libraries, firmware,
decompiled code, proprietary artwork, extracted symbol tables, and tools that
load a vendor library out of patches and packages. Facts from protocol analysis
belong in the evidence ledger with their provenance. This project does not claim
a formal clean-room process.

For a new model or protocol message, include the exact VID/PID, firmware version,
transport/interface, byte layout, field endianness, expected replies and the
observation that establishes them. Redact device identifiers from captures. Mark
constructed packets as synthetic; never label them as hardware captures.

Implement a pure decoder first, with explicit length and allocation bounds.
Preserve unknown values. Add independent golden vectors and rejection tests for
corrupt input. Then exercise the transport with a local peer or mock. Hardware
results should record which actual behavior was confirmed, including readback
and host display modes when testing display commands.

Run formatting, Clippy and both default/HID test configurations before submitting.
Do not add vendor SDK dependencies or make external research files a build input.
