# Opus rewind delay correction

This is `symphonia-adapter-libopus` 0.2.9 from crates.io. Original MIT and
Apache 2.0 license files, source and package metadata are retained.

The decoder keeps the original header pre-skip across resets and reapplies it
only when restarting at timestamp zero. Seeking into the stream does not discard
another header delay. This corrects an observed one-second Ogg Opus replay that
returned 48,312 frames instead of the original 48,000 at 48 kHz.

Header delay and demuxer start trimming are combined as bounds on the same
leading samples, not added twice. Delay larger than one packet is carried into
following packets. The libopus calls, fixed maximum output buffers, supported
mono/stereo layouts and existing channel conversion remain unchanged.

Validation: `cargo test -p symphonia-adapter-libopus --lib` covers rewinds,
later seeks, demuxer/header trimming and delay spanning packets. The workbench
container test decodes a complete gapless Ogg Opus track before and after an EOF
rewind and verifies exact sample counts, in addition to waveform/export checks.
Remove the patch after an upstream adapter upgrade passes these regressions.
