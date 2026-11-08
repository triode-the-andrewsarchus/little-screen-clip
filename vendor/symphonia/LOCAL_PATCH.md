# Shared Opus registration

This is the Symphonia 0.5.5 facade from crates.io, corresponding to upstream
commit `6d533f26150953a882a6a111ebd13f0abf7129d5`. Its original MPL 2.0 license,
copyright headers, examples and original manifest are retained. Local changes
are also covered by MPL 2.0 and their corresponding source is supplied here.

An optional `opus` feature registers the existing
[`symphonia-adapter-libopus`](https://docs.rs/symphonia-adapter-libopus/0.2.9/symphonia_adapter_libopus/)
decoder in the same default codec registry as all other enabled codecs. It is
also included in `all-codecs`. This adapter supports mono and stereo Opus;
other channel layouts retain its explicit unsupported result. The 0.2 adapter
series is compatible with Symphonia 0.5; its own MIT/Apache licenses and the
bundled libopus license remain provided by those dependencies.

The workspace pins this facade version and its locally patched CAF, PCM and
Opus adapter dependencies so a registry release cannot silently bypass the
compatibility fixes.

Both core analysis/list previews and Rodio playback call this registry, so
registration cannot drift between the two paths. No format reader or existing
codec implementation is replaced by this facade change. The workspace enables
the feature explicitly and uses the adapter's bundled libopus by default.

The workbench's `audio_containers_decode_waveforms_and_export_without_overwriting`
test exercises stereo and mono Ogg Opus, Matroska Opus, gapless decoding,
seeking, whole-track preview coverage, full analysis, filtered export/readback
and source/no-overwrite safety. Remove this local registration after a coherent
upstream upgrade supplies equivalent shared support and passes those checks.
