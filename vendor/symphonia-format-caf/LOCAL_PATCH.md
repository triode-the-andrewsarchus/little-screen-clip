# Local CAF reader patch

This is `symphonia-format-caf` 0.5.5 from crates.io, corresponding to
[Symphonia commit 6d533f2](https://github.com/pdeljanov/Symphonia/tree/6d533f26150953a882a6a111ebd13f0abf7129d5/symphonia-format-caf).
Original copyright headers and the MPL 2.0 license are retained. Local changes
are also covered by MPL 2.0; this directory supplies their corresponding source.

The reader unwraps legacy `frma`/`alac` codec cookies before passing the decoder
configuration to the ALAC codec. Modern configurations and optional channel
layouts remain unchanged. Both forms are described in Apple's
[ALAC cookie documentation](https://github.com/macosforge/alac/blob/master/ALACMagicCookieDescription.txt).
This fixes Apple Lossless CAF files emitted by FFmpeg without converting their
source bytes or introducing a separate playback decoder.

Compressed seeking searches the complete packet table, including after EOF,
so relative partition indices cannot select the wrong packet. Out-of-range
seeks fail; PCM reads past the declared data end finish normally. Unspecified
data chunk sizes extend to EOF, rather than interpreting sample bytes as more
chunks; ordinary files expose their remaining frame count.

Malformed headers cannot request unbounded cookie/table allocations or PCM
packet sizes. Cookies are capped at 1 MiB, tables at one million packets,
individual compressed packets at 16 MiB, and channel descriptions at 64.
PCM frame layouts, sample rates and channel counts are validated. Packet frame
and byte offset accumulation is checked. These are reader limits, not a complete
CAF conformance validator.

Validation: `cargo test -p symphonia-format-caf` and the workbench's
`audio_containers_decode_waveforms_and_export_without_overwriting` integration
test cover cookies, seeks, malformed headers, PCM/float/ALAC decoding, waveform
previews, source preservation and filtered no-overwrite exports. Remove this
patch after an upstream upgrade supplies equivalent behavior and passes those
checks.
