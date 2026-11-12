// SPDX-License-Identifier: MPL-2.0
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { readdirSync } from "node:fs";
import { fileURLToPath } from "node:url";

// Exercise package self-references in a fresh process, so the export map and
// module cache follow the same rules as an installed package.
const require = createRequire(import.meta.url);
const fs = require("node:fs");
const originalRead = fs.readFileSync;
const OriginalModule = WebAssembly.Module;
let reads = 0;
let compilations = 0;
fs.readFileSync = function (path, ...args) {
  if (String(path).endsWith(".wasm")) reads++;
  return originalRead.call(this, path, ...args);
};
WebAssembly.Module = class extends OriginalModule {
  constructor(...args) {
    super(...args);
    compilations++;
  }
};
try {
  const cjs = require("@d2-rs/wasm");
  const esm = await import("@d2-rs/wasm");
  const raw = require("@d2-rs/wasm/raw");
  const rawEsm = await import("@d2-rs/wasm/raw");
  assert.equal(esm.default, cjs);
  assert.equal(rawEsm.default, raw);
  assert.equal(esm.D2, undefined, "Node ESM uses the documented default import");
  assert.equal(cjs.WasmCancellationToken, raw.WasmCancellationToken);
  assert.equal(cjs.version(), "0.9.0");
  assert.equal(cjs.jsVersion(), "HEAD");
  assert.equal(await cjs.load(), cjs);
  assert.equal(await cjs.raw(), cjs);
  assert.ok(JSON.parse(raw.getObjOrder("a -> b")).data);
  assert.ok(JSON.parse(cjs.getObjOrder("a -> b")).data);

  // Replacement instances retain their own bindings and allocator, but can
  // reuse the compiled module. A stale token must never reach a new heap.
  const createRuntime = require("../pkg/node/d2_runtime.cjs");
  const replacement = createRuntime();
  const next = createRuntime();
  assert.notEqual(replacement.WasmCancellationToken, raw.WasmCancellationToken);
  assert.notEqual(replacement.WasmCancellationToken, next.WasmCancellationToken);
  assert.equal(replacement.version(), raw.version());
  assert.equal(next.version(), raw.version());
  assert.equal(reads, 1, "one payload read for raw, facade and replacement instances");
  assert.equal(compilations, 1, "one compilation per Node isolate");
} finally {
  fs.readFileSync = originalRead;
  WebAssembly.Module = OriginalModule;
}

const pkg = fileURLToPath(new URL("../pkg/", import.meta.url));
const payloads = readdirSync(pkg, { recursive: true }).filter(path => path.endsWith(".wasm"));
assert.deepEqual(payloads, ["web/d2_rs_wasm_bg.wasm"], "publish one shared WASM payload");
console.log("Node CJS/ESM/raw exports share one payload read and compilation; replacement heaps stay isolated");
