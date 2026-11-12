# d2-rs

A Rust port of the [D2](https://github.com/d2lang/d2) toolchain: the
compiler, the Dagre, ELK and TALA layout engines, the renderers and exporters,
and the command line, for the D2 diagram scripting language. The command is
installed as `d2-rs` and takes the same flags as Go's `d2`. Everything runs
in process, and it builds as a native CLI and as a WebAssembly package.

d2-rs targets byte-identical output with upstream Go D2 `master` at the commit
pinned in [`reference-manifest.json`](https://github.com/nelsonjchen/d2-rs/blob/main/reference-manifest.json) (6a7b48378,
2026-10-11). Upstream's own test suites are the oracle.

Announced in D2's
[Show and tell](https://github.com/d2lang/d2/discussions/2958) discussions;
questions and feedback are welcome there.

## Status

**Parity:** all 944 of upstream's e2e golden boards match byte for byte
(933 SVGs and 11 expected errors) across Dagre, ELK and TALA, apart from the
XML declaration, the version stamp and the trailing newline, which the goldens
differ in by how they were generated.

**Speed and memory:** across every e2e case and output format, d2-rs is
faster and smaller than Go D2 on the median, with output identical to Go's.
Rust/Go ratios on Linux arm64, lower is better:

| Output | Median wall time | Median peak RSS | Rows slower than Go |
|---|---:|---:|---:|
| SVG, Dagre / ELK | 0.21× / 0.21× | 0.60× / 0.60× | 0 of ~295 each |
| SVG, TALA | 0.47× | 0.68× | 0 of 296 |
| PNG / PDF / PPTX / GIF | 0.46× / 0.45× / 0.44× / 0.15× | 0.55–0.82× | 0 per format |
| WebAssembly vs `d2js`, per engine (36 rows) | 0.29–0.40× | 0.32–0.61× | 0 of 36 |

The WebAssembly row gives the range of the three engines' median ratios for
compile and render in Node; over all 36 rows, total wall time is 0.27–0.33×
Go's. The WebAssembly module is about half the size of `d2js` (31.3 MB against
60.0 MB uncompressed, 10.1 against 15.3 MB gzipped) and starts in about a
tenth of the time. The native CLI is the other way round: stripped on Linux
arm64, it is 48.9 MB against Go's 40.8 MB (1.20×). See
[benchmarks](https://github.com/nelsonjchen/d2-rs/blob/main/docs/BENCHMARKS.md) for x86-64, the generated scale sweep, the
method and caveats.

**CLI:** all 67 applicable assertions in upstream's `e2etests-cli` suite
pass, and every retained golden that the pinned Go still reproduces (51 of
66) matches byte for byte.

**Platforms:** parity is qualified on arm64 (Apple Silicon, Linux arm64) and
x86-64 (Linux), each against Go built for that architecture, since Go's float
output differs slightly between them
([details](https://github.com/nelsonjchen/d2-rs/blob/main/docs/FIXTURES.md#host-architecture)). Benchmarks were measured on
arm64 and in Node. Windows passes the default test tier and all 944 e2e
boards natively (MSVC) and under Wine; CI builds Windows but does not run its
tests.

**Differential runs:** the pinned Go CLI and d2-rs render the same upstream
inputs and mutations of them under the same options; all 5,024 original jobs
match. The only exceptions are 5 mutated inputs that crash the pinned Go
itself, where d2-rs reports an error instead.

**Known issues** are tracked in
[GitHub issues](https://github.com/nelsonjchen/d2-rs/issues).

## Install

Prebuilt `d2-rs` binaries for Linux, macOS and Windows on x86-64 (amd64) and
arm64 are attached to each
[GitHub release](https://github.com/nelsonjchen/d2-rs/releases), packaged
like Go D2's release archives as `d2-rs-<version>-<os>-<arch>.tar.gz`, with a
`SHA256SUMS` file. Each archive holds `bin/d2-rs` (`bin/d2-rs.exe` on
Windows), the license and third-party notices, and a `Makefile` that installs
the binary:

```sh
curl -fsSLO https://github.com/nelsonjchen/d2-rs/releases/download/v0.1.7/d2-rs-v0.1.7-linux-amd64.tar.gz
tar -xzf d2-rs-v0.1.7-linux-amd64.tar.gz
make -C d2-rs-v0.1.7 install   # to /usr/local, or ~/.local if that is not writable
```

The Linux binaries need glibc 2.35 or newer. On macOS, a binary downloaded with
a browser is quarantined; clear that with
`xattr -d com.apple.quarantine bin/d2-rs` (`curl` downloads are not affected).

On Windows, the release also has an installer for each architecture,
`d2-rs-<version>-windows-amd64.msi` and `-windows-arm64.msi`. It asks whether
to install just for you, the default, which needs no administrator rights and
puts `d2-rs` in `%LOCALAPPDATA%\Programs\d2-rs` on your `PATH`, or for all
users, in `Program Files\d2-rs` on the system `PATH`. The installer is
unsigned, so SmartScreen asks before running it. Silent installs work the same
way (add `ALLUSERS=1` for all users):

```powershell
msiexec /i d2-rs-<version>-windows-amd64.msi /qn
```

Or the command line, from [crates.io](https://crates.io/crates/d2-rs). It needs
Rust 1.98.1 or newer:

```sh
cargo install d2-rs
d2-rs input.d2 output.svg
d2-rs --layout=tala input.d2 output.png
```

The installed command is `d2-rs`, and it takes the same subcommands and flags
as Go's `d2`:
- `fmt`, `validate`, `--theme`, `--sketch`, `--pad` and `--scale`;
- multiboard output;
- PNG, PDF, PPTX, GIF and ASCII export.

For reproducible native raster and paged exports, `D2RS_SYSTEM_FONT_DIR` can
restrict system font fallback to one directory, searched recursively. Bundled
fonts still take precedence. Without it, d2-rs uses the host's system fonts.

The WebAssembly package, from [npm](https://www.npmjs.com/package/@d2-rs/wasm),
for Node and browsers. Its `D2` class has the same `compile` / `render` shape
as D2.js ([usage](https://github.com/nelsonjchen/d2-rs/blob/main/crates/d2-rs-wasm/README.md)):

```sh
npm install @d2-rs/wasm
```

As a Rust library, there is no single high-level crate yet: each stage of the
pipeline is its own crate, mirroring D2's Go packages, and the crates keep
Go-shaped APIs that may change before 1.0. Depend on the stages you need:
- [`d2-rs-language`](https://crates.io/crates/d2-rs-language) to parse,
  compile and format D2 source;
- [`d2-rs-layout`](https://crates.io/crates/d2-rs-layout) to lay out a
  compiled graph with Dagre, ELK or TALA, including grids, sequence diagrams
  and `near` placement;
- [`d2-rs-render`](https://crates.io/crates/d2-rs-render) for SVG and sketch
  rendering;
- [`d2-rs-export`](https://crates.io/crates/d2-rs-export) for PNG, PDF, PPTX,
  GIF and ASCII export.

[`d2-rs-lib`](https://crates.io/crates/d2-rs-lib) is only the serialized graph
JSON boundary between the compiler and the layout engines (D2's `d2graph`
serialization), not a general entry point. The `d2-rs` command line's source
shows how the stages fit together, and the
[repository map](https://github.com/nelsonjchen/d2-rs#repository-map) lists
every crate. Releases are listed in the
[changelog](https://github.com/nelsonjchen/d2-rs/blob/main/CHANGELOG.md).

### From source

```sh
cargo build --release --locked -p d2-rs
target/release/d2-rs input.d2 output.svg
cd crates/d2-rs-wasm && pnpm build   # needs wasm-bindgen 0.2.128 and pnpm
```

## How it was built

The port started with static comparison against the Go source. A divergence
was fixed at its earliest point, never by matching a fixture's output.
Scoreboards choose the work:
- `scripts/e2e-scoreboard.py` covers upstream's e2e goldens;
- other scripts cover raster, CLI and language suites;
- `scripts/perf-bench.py` covers speed and memory.

Every change must keep every scoreboard unchanged. Go D2 is a test oracle
only: no Go code is linked into d2-rs.

Most of the code was written by AI coding agents, in two stages:
- **From 2026-09-09:** OpenAI's Codex, running GPT-6.1 Sol and GPT-6 Astra,
  started the port and worked on it alone.
- **From 2026-10-22:** Anthropic's Claude Opus 5.5 took the lead on the same
  codebase. It chose the work by scoreboard
  ([docs/SCOREBOARD-WORKFLOW.md](https://github.com/nelsonjchen/d2-rs/blob/main/docs/SCOREBOARD-WORKFLOW.md)), wrote code
  itself, and supervised parallel Codex lanes running GPT-6.1 Sol, GPT-6.1
  Terra and GPT-6 Astra, reviewing and verifying their patches before each
  commit.

## Go behavior, ported

Byte-identical output rules out most Rust equivalents. A different deflate
encoder, float formatter, sort order or random generator changes the bytes,
even when the result looks the same. So beyond D2 itself, d2-rs ports the Go
code that D2's output depends on, matching each to its pinned Go source:

- **Go standard library:** `strconv` float parsing and formatting, `sort`,
  `math` (including its per-architecture routines) and `math/rand`,
  `compress/flate`, `compress/zlib`, `compress/lzw`, `hash/adler32`,
  `image/png`, `image/gif` and JPEG decoding, `archive/zip`, `encoding/xml`
  name rules, `encoding/json` string quoting, `net/url`, `html` and
  `unicode/utf8`.
- **Go libraries D2 uses:** `goldmark` (Markdown), `chroma` (code
  highlighting), `mathjax-go` (LaTeX), `go-text/typesetting` (text shaping,
  through a fork of Rustybuzz in `vendor/`), `rough-go` (sketch mode),
  `dagro` (Dagre), `elk-go` (ELK), Go-PDF/FPDF (PDF), `go-quantize` (GIF
  palettes), `csscolorparser`, `golang.org/x/net/html` name folding, the
  Brotli reader's errors, a TrueType font subsetter and Workiva's Fibonacci
  heap.
- **The Go compiler's arithmetic:** on arm64, Go fuses `x*y+z` into one
  rounding step, so d2-rs does too there, and plain arithmetic on x86-64.

Each port keeps its upstream license notice in
[THIRD_PARTY_NOTICES.md](https://github.com/nelsonjchen/d2-rs/blob/main/THIRD_PARTY_NOTICES.md).

These ports are useful beyond D2: any Go-to-Rust port that has to reproduce
Go's output exactly needs the same pieces. `crates/d2-rs-goflate` (Go's
`compress/flate` and `compress/zlib`) is already a standalone crate. Most
others are still modules inside the crates that needed them, for example
`d2-rs-core/src/go_float.rs` (`strconv`), `go_sort.rs` (`sort`),
`d2-rs-export/src/go_png.rs` and `go_zip.rs`, `d2-rs-elk/src/go_math.rs` and
`go_trig.rs`, and `d2-rs-tala/src/internal/runtime/go_rng.rs`
(`math/rand`).

## Repository map

The workspace mirrors upstream D2's package layout. Each crate ports the Go
packages named beside it; [docs/PORT-MAP.md](https://github.com/nelsonjchen/d2-rs/blob/main/docs/PORT-MAP.md) maps every
upstream Go package to its Rust location.

**Language**

| Crate | Ports |
|---|---|
| `d2-rs-language` | `d2parser`, `d2ir`, `d2compiler`, `d2format`: parsing, compilation and formatting |
| `d2-rs-editor` | `d2oracle`, `d2lsp`: source edits and completions |
| `d2-rs-core` | `d2ast`, plus the shared graph, IR, geometry and layout result types |
| `d2-rs-lib` | the serialized graph JSON boundary between the compiler and the layout engines |

**Layout**

| Crate | Ports |
|---|---|
| `d2-rs-layout` | `d2layouts`: engine selection, grids, sequence diagrams, `near` placement |
| `d2-rs-tala` | TALA, the layout engine bundled in D2 |
| `d2-rs-dagre` | the Dagre adapter and `dagro` |
| `d2-rs-elk` | the ELK adapter and `elk-go` |

**Text and math**

| Crate | Ports |
|---|---|
| `d2-rs-text` | text measurement, Markdown (`goldmark`) and code highlighting (`chroma`) |
| `d2-rs-fontface` | D2's internal fontface package: parsing, shaping, glyph outlines, COLRv1 |
| `d2-rs-math` | `d2latex` and `mathjax-go`: LaTeX layout and SVG |

**Rendering**

| Crate | Ports |
|---|---|
| `d2-rs-target` | `d2target`: the diagram model and theme resolution |
| `d2-rs-scene` | `d2renderers/d2scene`: the renderer-neutral scene model |
| `d2-rs-scenebuild` | `d2renderers/d2scenebuild`: diagram to scene |
| `d2-rs-render` | `d2renderers/d2svg` and `d2renderers/d2sketch`: the SVG and sketch renderers |
| `d2-rs-sketch` | `rough-go`: the rough drawing primitives behind sketch mode |
| `d2-rs-isometric` | `d2renderers/d2isometric`: the isometric renderer |
| `d2-rs-raster` | `d2raster`: scene to pixels |

**Export and front ends**

| Crate | Ports |
|---|---|
| `d2-rs-export` | PNG, PDF, PPTX, GIF and ASCII export (`lib/png`, `lib/pdf`, `lib/pptx`, ...) |
| `d2-rs-goflate` | Go's `compress/flate` and `compress/zlib`, byte for byte |
| `d2-rs-cli` | `d2cli`: the `d2-rs` binary |
| `d2-rs-wasm` | `d2js`: the WebAssembly package |
| `d2-rs-testdata` | test-only access to the regenerated oracle fixtures |

**Elsewhere**

| Path | What |
|---|---|
| `vendor/` | `rustybuzz-go-text`: Rustybuzz 0.20.1 modified so text shaping matches Go's `go-text/typesetting` (GPOS pair adjustment, coverage digests, Unicode 17 property tables); see its [SOURCE.md](https://github.com/nelsonjchen/d2-rs/blob/main/vendor/rustybuzz-go-text/SOURCE.md) |
| `scripts/` | scoreboards, differential harnesses, benchmarks, fixture runner |
| `tools/` | fixture recipes and hooks, upstream patches, oracle programs |
| `tests/` | hand-written D2 inputs |
| `docs/` | [status](https://github.com/nelsonjchen/d2-rs/blob/main/docs/STATUS.md), [fixtures](https://github.com/nelsonjchen/d2-rs/blob/main/docs/FIXTURES.md), [benchmarks](https://github.com/nelsonjchen/d2-rs/blob/main/docs/BENCHMARKS.md), [references](https://github.com/nelsonjchen/d2-rs/blob/main/docs/REFERENCES.md), [provenance](https://github.com/nelsonjchen/d2-rs/blob/main/docs/PROVENANCE.md) |

## Testing

Plan for about 20 GB free. A debug test build of the workspace takes about
12 GB in `target/` and grows with rebuilds; the oracle tier adds the pinned
upstream checkout, Go's build cache and the regenerated fixtures, about 5 GB
together.

```sh
cargo test --workspace --locked
uv run python scripts/references.py verify
# needs the pinned e2e case dump (reference-manifest.json `e2e_cases`);
# see docs/SCOREBOARD-WORKFLOW.md
cargo build --locked --profile diagnostic -p d2-rs
uv run python scripts/e2e-scoreboard.py --output e2e.json
```

Normal builds compile diagnostic tracing out. It is enabled with
`--features diagnostic-traces`.

## License

MPL-2.0, the same license as D2. This is a derivative work of D2, Copyright
2022 Terrastruct Inc. Third-party material keeps its notices: see
[THIRD_PARTY_NOTICES.md](https://github.com/nelsonjchen/d2-rs/blob/main/THIRD_PARTY_NOTICES.md). Cargo dependency notices
for distributions are generated with `node scripts/sync-licenses.mjs`; see
[SOURCE.md](https://github.com/nelsonjchen/d2-rs/blob/main/SOURCE.md).
