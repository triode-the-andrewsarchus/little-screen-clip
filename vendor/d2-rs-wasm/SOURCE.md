# Corresponding source

d2-rs is MPL-2.0 (see `LICENSE`). Its corresponding source is this
repository at the revision an artifact was built from; distributed binaries
and WASM packages must name that revision.

- **Upstream:** the pinned Go D2 commit, its Go toolchain and module versions
  are in `reference-manifest.json`. `scripts/references.py` acquires and
  verifies the references (`docs/REFERENCES.md`).
- **Oracle fixtures:** Go and JS oracle outputs are regenerated, never
  committed: `scripts/regen-fixtures.py` rebuilds them from the pin, the
  patches listed in the manifest's `trace_patches` and the recipes in
  `tools/fixtures/` (`docs/FIXTURES.md`).
- **Generated tables:** source-derived Rust tables are produced by the
  `scripts/generate-*.py` scripts from the pinned upstream modules (math
  glyphs, compounds and command registry, source Unicode tables, grapheme
  boundaries).
- **Notices:** third-party attributions are in `THIRD_PARTY_NOTICES.md`.
  Cargo dependency notices are generated with `node scripts/sync-licenses.mjs`
  into `DEPENDENCY_NOTICES.md` (not committed) and must accompany every
  binary or WASM distribution, together with `LICENSE` and
  `THIRD_PARTY_NOTICES.md`.
