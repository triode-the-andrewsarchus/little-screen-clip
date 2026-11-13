# Bundled GoAT renderer

This data-only companion calls the pinned GoAT 0.5.1 ASCII and UTF-8 libraries.
Its only optional argument is the fixed `--utf8` mode. Source arrives on stdin;
SVG leaves stdout only after complete rendering. There are no filename, external
CSS, URL, import, script or arbitrary command arguments. Expressions are text.

The adapter caps input at 256 KiB, 256 columns and 128 rows, rejects tabs/NUL and
invalid UTF-8, and buffers at most 2 MiB of SVG. Go uses two runtime processors
and a 128 MiB soft GC memory target. Rust's existing CommandRequest owns the child,
parent-death handling, process group, timeout, output transport and hard process
limits. The disposable diagram worker still supplies outer cancellation/limits.
The Rust SVG composer blocks image resources and outlines bundled fonts.

## Build and deployment

Go 1.25+ is required at build time; `LITTLE_CLIP_GO` can choose the compiler.
Cargo's render build script runs a CGO-free, stripped, offline Go build using the
local module replacements. It disables toolchain downloads and module/network
lookups, uses one compiler job, and publishes a versioned `clip-goat-*` executable
beside the profile's desktop binaries. The binary is about 1.8 MiB on Linux amd64.
Keep that companion when distributing the application; neither Go nor a separate
GoAT installation is needed at runtime. Sibling discovery is tested with a
relocated application. A retained Cargo output path supports library/dev tests.

Versioned companions preserve the helper for already-running older applications
across rebuilds. They are intentionally retained with other bundled helpers;
ordinary Rust source edits do not rebuild Go unless its inputs change. Combined
original GoAT, CSS parser and Go runtime notices are available through the
application's `--diagram-licenses` output.

Offline helper tests:

```sh
GOPROXY=off GOSUMDB=off GOTOOLCHAIN=local go -C scripts/goat-renderer test -mod=readonly ./...
```

They cover both frontends, XML shape output, input/output budgets and the original
GoAT line-decoration fixture. Standalone and embedded examples are under
`docs/examples/diagrams/` and the user's `type_testing/md_extensions/` folder.

## Provenance and selection

Unmodified renderer packages and fixtures were imported in a separate commit from
GoAT tag `v0.5.1`, revision `6d4db359c601eae6efa1fcd94601fa1793e114d5`:
<https://github.com/blampe/goat>. The original CLI, Markdown/template commands and
file-based CSS loader are not part of this adapter. Library source is kept under
`vendor/goat` with its original MIT license.

The local CSS parser replacement contains unmodified required packages from
`github.com/tdewolff/parse/v2` v2.7.19, under its original MIT license. Its original
module checksums remain in `vendor/goat/go.sum`. Go runtime BSD license text is
retained from the official Go 1.25.0 distribution. No upstream engine code has
been rewritten or patched for this integration.

Compared Rust aasvg 1.0.0 (BSD-2-Clause, related Markdeep port), the existing
SvgBob 0.7.6 and upstream GoAT. `formulaa` is a math editor, not a diagram parser.
In the upstream line-decoration fixture, aasvg emitted only 11 arrow polygons and
left six markers as text; GoAT emitted 15 polygons and no text. Hollow-circle and
overlap fixtures also differed. This evidence rules out calling the related port
GoAT-compatible without a new maintenance-heavy algorithm port. The small compiled
companion preserves the actual upstream frontends without a browser engine,
interpreter, shared Go runtime, or application Rust unsafe code.
