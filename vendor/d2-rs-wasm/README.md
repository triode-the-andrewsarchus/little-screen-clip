# @d2-rs/wasm

WebAssembly build of [d2-rs](https://github.com/nelsonjchen/d2-rs/blob/main/README.md), a Rust port of the
[D2](https://d2lang.com) diagram toolchain whose output is byte-identical to
upstream Go D2. This package runs the whole pipeline (parse, compile, layout
with dagre, ELK or TALA, and SVG rendering) in the browser or in Node, with no
server, filesystem or network access. Its high-level `D2` class has the same
API shape as D2.js, Go D2's own WASM package (`d2js`), and renders the same
SVGs.

```sh
npm install @d2-rs/wasm
```

## Build from the repository

The npm package ships the built `pkg/` files, so installing it needs no
build. These steps are for building it from a checkout of the
[d2-rs repository](https://github.com/nelsonjchen/d2-rs); the build and test
scripts they run are in the repository, not in the npm package.

You need:

- a Rust toolchain with the `wasm32-unknown-unknown` target
  (`rustup target add wasm32-unknown-unknown`);
- the `wasm-bindgen` CLI on your `PATH`, at the same version as the
  `wasm-bindgen` crate in the workspace `Cargo.lock` (currently 0.2.128):
  `cargo install wasm-bindgen-cli --version 0.2.128 --locked`;
- Node 18 or newer, and pnpm.

Then, in `crates/d2-rs-wasm` of the checkout:

```sh
pnpm build
```

`pnpm build` runs `scripts/build-wasm.mjs`. It runs
`cargo build --locked --release --target wasm32-unknown-unknown -p d2-rs-wasm`,
then `wasm-bindgen` twice: once with `--target web` into `pkg/web/` and once
with `--target nodejs` into `pkg/node/`. It also generates the browser ESM
copy of the `D2` facade. Both loaders share the single WASM payload in
`pkg/web/`; Node reads and compiles it once per isolate, including when a
replacement instance is needed after a trap. Workers have separate isolates.
`pkg/` is not checked in, so rebuild it for every
distribution. `pnpm test` runs the Node contract tests against the built
package. `pnpm test:types` checks the browser declarations and real Node
CommonJS and ESM imports, including `@d2-rs/wasm/raw`.

To use a local build from another project, depend on this directory, for
example `pnpm add file:path/to/d2-rs/crates/d2-rs-wasm`.

## Usage

`compile` takes D2 source and returns the laid-out diagram. `render` turns
that diagram into an SVG string. Each `D2` instance runs its work in a worker
(a `worker_threads` worker in Node, a module `Worker` in the browser). Call
`dispose()` when you are done with it.

### Node

The Node entry point is CommonJS, so from an ES module take `D2` from the
default import. A named import (`import { D2 } from "@d2-rs/wasm"`) fails in
Node:

```js
import d2rs from "@d2-rs/wasm";

const d2 = new d2rs.D2();
const result = await d2.compile("x -> y: hello", { layout: "elk", sketch: true });
const svg = await d2.render(result.diagram, result.renderOptions);
await d2.dispose();
```

From CommonJS, use `const { D2 } = require("@d2-rs/wasm")`. TypeScript picks
up the matching types for each entry point (`src/node.d.cts` in Node,
`src/index.d.ts` elsewhere).
The generated raw bindings use TypeScript's `ESNext.Disposable` library for
explicit token disposal. Include it alongside `ES2022` and `DOM` in `lib`
when your project does not already include it through its Node types.

### Browser

The browser entry point is an ES module with named exports and no default
export:

```js
import { D2 } from "@d2-rs/wasm";

const d2 = new D2();
const result = await d2.compile("x -> y", { layout: "tala", themeID: 200 });
document.body.innerHTML = await d2.render(result.diagram, {
  ...result.renderOptions,
  pad: 20,
});
```

The browser entry point (`src/index.js`) loads `src/d2-worker.browser.js`
and the generated `pkg/web/` modules and `d2_rs_wasm_bg.wasm` relative to its
own URL, so serve or bundle `src/` and `pkg/web/` together with your app.

### Imports and multiple files

Pass a virtual filesystem instead of a source string:

```js
const result = await d2.compile({
  fs: { "index.d2": "...@shared\nx -> y", "shared.d2": "x.shape: circle" },
  inputPath: "index.d2",
  options: { layout: "dagre" },
});
```

Imports only read files in `fs`.

## API

The `D2` class (see `src/index.d.ts` for the full types):

| Method | Returns |
|---|---|
| `compile(input: string \| CompileRequest, options?: CompileOptions)` | `Promise<CompileResponse>` |
| `render(diagram: Diagram, options?: RenderOptions)` | `Promise<string>` (SVG, or text for ASCII) |
| `encode(script: string)` / `decode(encoded: string)` | `Promise<string>`: D2 playground URL encoding |
| `version()` / `jsVersion()` | `Promise<string>` |
| `dispose()` | `Promise<void>`; rejects any pending requests |

`CompileRequest` is `{ fs: Record<string, string>, inputPath?: string, options?: CompileOptions }`.

`version()` returns `"0.9.0"`, the pinned D2 compatibility version, rather
than this npm package's version. Upstream d2js returns Go's build-injected
version (`"v0.8.1-HEAD"` in an unstamped source build, `"v0.9.0"` in our
stamped oracle); the existing d2-rs API omits the `v` prefix.
`jsVersion()` returns `"HEAD"`, matching d2js's default for a source build.
Upstream d2js's release build injects its own npm version into that field.
The installed d2-rs package version is recorded separately in `package.json`.

`CompileOptions` extends `RenderOptions` with:

- `layout`: `"dagre"`, `"elk"` or `"tala"`;
- `fontRegular`, `fontItalic`, `fontBold`, `fontSemibold`: custom font bytes,
  as base64 strings, number arrays (as in Go's JSON `[]byte`) or
  `Uint8Array`s, used to measure text. Go's `d2js` does not accept
  `Uint8Array`.

`RenderOptions` (all optional):

- `themeID`, `darkThemeID`, `themeOverrides`, `darkThemeOverrides`;
- `sketch`, `isometric`, `center`, `pad`, `scale`, `forceAppendix`, `salt`,
  `noXMLTag`;
- `target` (a board path such as `layers.name`, or `*` for all boards) and
  `animateInterval` (milliseconds, for animated multi-board SVGs);
- `ascii` and `asciiMode` (`"standard"` for plain characters instead of box
  glyphs);
- `assetData`: a map from an icon or image path to base64 bytes, embedded
  as data URIs.

`CompileResponse` has:

- `diagram`: the laid-out `Diagram` (`shapes`, `connections`, `root`, and
  any `layers`, `scenarios` and `steps`), which you pass to `render`;
- `renderOptions`: the effective render options after the source's `vars.d2-config`
  and defaults are applied;
- `graph`: the compiled graph (objects, edges, boards and their metadata);
- `fs` and `inputPath`: the input files, with the input file formatted.

### Lower-level exports

The package also exports functions that take source and a JSON options
string and return JSON or SVG strings, for callers that don't want the
worker facade. In the browser they are async and load the module on first
call; in Node they are the synchronous wasm-bindgen functions and return
strings directly. They include `parseD2Source`, `formatD2Source`,
`compileD2Source`, `layoutD2Source`, `renderD2Source` and editor helpers such
as `completeD2Source`, `getCompletions`, `getRefRanges` and `moveObjectJson`.
In the browser, `init()` (alias `load()`) initializes the module and returns
the raw wasm-bindgen `Runtime`. In Node the module initializes synchronously
on `require`, there is no `init()`, and `load()` resolves to the package
object itself. `WasmCancellationToken` and the `*WithCancellation` variants
let a worker cancel a long layout cooperatively; Node exports the token class
directly, while browser code gets it from `await raw()` or
`@d2-rs/wasm/raw`. The generated
bindings themselves are available as `@d2-rs/wasm/raw`.

If the WASM module traps, the package entry points report it as a
`panic recovered: ...` error and load a fresh instance for the next call.
Go-compatible calls return an error object with code 500, and the native
helpers throw an `Error`. In the browser, get a new `raw()` handle and new
cancellation tokens after a trap: old handles reject calls, and old tokens
don't work with the new instance. Node's `D2` facade keeps working and
forwards to the new instance. The generated `@d2-rs/wasm/raw` bindings have
no recovery wrapper.

## Compared with Go's d2js

- **API:** the same `D2` class shape: `compile`, `render`, `encode`, `decode`,
  `version`, `jsVersion`.
- **Output:** on 12 upstream e2e cases with all 3 layout engines, all 36 SVGs
  are byte-identical to `d2js`'s.
- **Speed:** the per-case median Rust/Go wall-time ratio for compile and
  render is 0.30x with dagre, 0.29x with ELK and 0.40x with TALA; total wall
  time over all 12 cases is 0.31x, 0.33x and 0.27x. Median peak memory is
  0.33x, 0.32x and 0.61x.

| | Go `d2js` | `@d2-rs/wasm` |
|---|---:|---:|
| Module size, raw / gzip -9 | 60.0 / 15.3 MB | 31.3 / 10.1 MB |
| Startup (instantiate + init) | ~405 ms | ~43 ms |

These numbers were measured in Node 22 only, not in browsers, with both
packages run in-process without workers. See
[BENCHMARKS.md](https://github.com/nelsonjchen/d2-rs/blob/main/docs/BENCHMARKS.md) for the method and caveats.

## License

MPL-2.0. Any distribution of this package must include `LICENSE`,
`THIRD_PARTY_NOTICES.md`, `SOURCE.md` and `DEPENDENCY_NOTICES.md`. Run
`node scripts/sync-licenses.mjs` at the root of a repository checkout to
generate them into `crates/d2-rs-wasm`; the published package already
includes them.
