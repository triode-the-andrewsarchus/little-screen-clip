# FFmpeg library worker

This executable hosts `ffmpeg-next` independently from the desktop binaries.
It does not spawn the FFmpeg CLI. The other workspace crates must communicate
with it through owned child processes rather than depend on this package.

Build with `cargo build -p clip-media-worker`. Development requires FFmpeg
development libraries, pkg-config and the Clang tooling needed by the bindings.
The initial dependency line is `ffmpeg-next` 9.x. Runtime deployment must include
compatible FFmpeg shared libraries or declare the appropriate system packages.

The initial command is:

```text
clip-media-worker probe FORMAT_WHITELIST /proc/self/fd/N
```

The parent must inherit a pinned ordinary input descriptor and retain child
ownership through exit. The worker duplicates the descriptor, probes through
seekable Rust stream I/O, denies FFmpeg URL protocols for secondary opens and
emits at most 64 KiB of JSON metadata. Parent-side cancellation and deadlines
remain mandatory; the library interrupt callback is additional protection.

This package is GPL-2.0-or-later. The complete GPL v2 text is retained at
[`scripts/timelens-GPL-2.0.txt`](../../scripts/timelens-GPL-2.0.txt). Preserve this
package's corresponding source, build instructions and notices when distributing
its executable, along with the obligations of the selected FFmpeg build. The
workspace's default MIT/Apache license is not this helper's license.

Migration status and the eventual playback-framework recommendation live in
[`media_backend_migration.md`](../../docs/audio/media_backend_migration.md).
