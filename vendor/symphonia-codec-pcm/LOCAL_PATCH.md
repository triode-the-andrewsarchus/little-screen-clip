# Local PCM decoder patch

This is `symphonia-codec-pcm` 0.5.5 from crates.io, corresponding to
[Symphonia commit 6d533f2](https://github.com/pdeljanov/Symphonia/tree/6d533f26150953a882a6a111ebd13f0abf7129d5/symphonia-codec-pcm).
The original MPL 2.0 license and copyright headers are retained. The embedded
Sun Microsystems notice for the G.711 routines remains unchanged. Local changes
are covered by MPL 2.0 and their corresponding source is supplied here.

The decoder starts with an empty sample buffer and grows it from actual PCM
packet data rather than requiring `max_frames_per_packet`. Matroska does not
provide that hint. This also handles growing packet sizes without truncating
samples or allocating an untrusted advertised maximum up front. The existing
sample-format dispatch and conversion routines remain in use by analysis,
waveform previews and Rodio playback.

Encoded packets are limited to 16 MiB before buffer growth; incomplete channel
frames fail explicitly. G.711 uses its encoded one-byte sample width while
retaining the existing decoded 16-bit output. Failed packets clear previous
samples through the original decoder error path.

Validation: `cargo test -p symphonia-codec-pcm` exercises unknown/huge hints,
growing and shrinking stereo packets, floating-point and companded samples,
truncated frames and the allocation limit. The workbench's
`audio_containers_decode_waveforms_and_export_without_overwriting` test covers
PCM Matroska and existing containers end to end. Remove this patch after an
upstream update passes equivalent checks.
