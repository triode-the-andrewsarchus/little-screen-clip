# MinedMap renderer

This in-memory renderer comes from [MinedMap](https://github.com/neocturne/MinedMap) under the MIT license. The [upstream revision](../../scripts/minedmap/upstream-revision) pins the base for this source tree. Normal Cargo builds use these vendored files directly.

## Local patches

- [Embedded terrain renderer](../../scripts/minedmap/0001-embed-terrain-renderer.patch) packages the region processor and tile renderer as a library. It takes a caller-owned reader and cancellation callback, smooths biome colors within each region, and returns an RGBA tile. Block and biome data and coordinate types come from the published MinedMap crates.
- [Bounded NBT reads](../../scripts/minedmap/0002-bound-nbt-reads.patch) caps decompression and nesting, adds gzip and uncompressed chunk support, and lets the caller skip unreadable chunks while retaining usable terrain.

## Updating

Change the pinned revision and replay the patches from the repository root.

```sh
scripts/regenerate_minedmap.py
scripts/regenerate_minedmap.py --check
cargo test --locked -p little-clip-core minecraft::tests
```

Regeneration stages the selected upstream files and applies the numbered patches before replacing the vendored source. If a patch fails to apply, refresh it against the selected upstream paths listed in the script. Review the pin, patch, and generated source changes together.

Pass a local MinedMap Git checkout as the final argument to regenerate or check without fetching. The checkout must contain the pinned commit. Regeneration uses Python 3.12 or later, Git, and rsync.
