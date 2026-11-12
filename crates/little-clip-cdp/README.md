# Isolated CDP8 Audio & Spectral Processing Helper

This crate is an **LGPL-2.1-only** standalone executable (`little-clip-cdp`) that
orchestrates the [Composers Desktop Project (CDP8)](https://github.com/ComposersDesktop/CDP8)
command-line tools (pinned revision `28bc42c72c1a7cb0fab933acd1c433be958a787b`).

## License Separation

Application crates (`little-clip-ops`, `clip-workbench`, `little-clipd`, etc.)
remain `MIT OR Apache-2.0`. They do **not** link against this crate or CDP8 C
libraries. Instead, `little-clip-ops` communicates with the `little-clip-cdp`
helper binary via bounded JSON over standard input/output.

The process catalog metadata in `cdp_processes.json` is derived from
[SoundThread](https://github.com/j-p-higgins/SoundThread) (MIT License,
revision `a33198a84fa44d8ddc1ea7469653cbe0906eb3c5`).

`COPYING.LESSER` contains the full GNU Lesser General Public License v2.1 text.

## Building and Packaging

Build and package with:

```bash
python3 scripts/build_cdp_helper.py
```

This builds `little-clip-cdp` and the CDP8 C executables into
`target/release/cdp-helper/` alongside `COPYING.LESSER`, `README.md`, and
rebuildable source. Set `LITTLE_CLIP_CDP_HELPER` or `LITTLE_CLIP_CDP_BIN` to
override helper discovery.
