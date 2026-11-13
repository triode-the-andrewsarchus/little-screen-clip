// SPDX-License-Identifier: MPL-2.0

import { createRequire } from "node:module";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { Worker } from "node:worker_threads";
import { once } from "node:events";
import { fileURLToPath } from "node:url";
import { checkRuntimeRecovery } from "../tests/runtime-recovery.js";

const require = createRequire(import.meta.url);
const factoryPath = require.resolve("../pkg/node/d2_runtime.cjs");
const createRuntime = require(factoryPath);
const { recoverRuntime } = require("../src/recover-runtime.cjs");
await checkRuntimeRecovery(createRuntime, recoverRuntime);

// Test the public Node loader as well as the shared boundary. Replace one
// binding in the test process so it raises a trap exactly once. The next
// public request must use a fresh allocator and fresh binding state.
let instances = 0;
const initialRuntime = require("../pkg/node/d2_rs_wasm.cjs");
const initialGetObjOrder = initialRuntime.getObjOrder;
initialRuntime.getObjOrder = () => { throw new WebAssembly.RuntimeError("unreachable"); };
require.cache[factoryPath].exports = () => {
  instances++;
  return createRuntime();
};
try {
  const api = require("../src/node.cjs");
  const old = await api.raw();
  const OldToken = api.WasmCancellationToken;
  assert.equal(OldToken, initialRuntime.WasmCancellationToken);
  const token = new api.WasmCancellationToken();
  assert.equal(JSON.parse(api.getObjOrder("a")).error.code, 500);
  assert.ok(JSON.parse(api.getObjOrder("a")).data);
  assert.equal(instances, 1);
  assert.equal(await api.raw(), old);
  assert.notEqual(api.WasmCancellationToken, OldToken);
  const replacement = new api.WasmCancellationToken();
  token.cancel();
  token.free();
  assert.equal(replacement.is_cancelled(), false);
  replacement.free();
  assert.ok(JSON.parse(old.getObjOrder("a")).data);
} finally {
  require.cache[factoryPath].exports = createRuntime;
  initialRuntime.getObjOrder = initialGetObjOrder;
}

// Exercise the browser entry point under Node with its real ESM bindings.
// Inject the fault at instantiation, leaving its public loader untouched.
const instantiate = WebAssembly.instantiate;
let browserInstances = 0;
WebAssembly.instantiate = async (...args) => {
  const result = await instantiate(...args);
  // Node's Response implementation may instantiate its own HTTP parser.
  if (!result.instance?.exports.getObjOrder) return result;
  if (++browserInstances !== 1) return result;
  return {
    module: result.module,
    instance: { exports: { ...result.instance.exports,
      getObjOrder() { throw new WebAssembly.RuntimeError("unreachable"); },
    } },
  };
};
try {
  const api = await import("../src/index.js");
  // Response bodies are consumable. Recovery must clone the retained input.
  const input = new Response(readFileSync(new URL("../pkg/web/d2_rs_wasm_bg.wasm", import.meta.url)),
    { headers: { "Content-Type": "application/wasm" } });
  // Force the byte-buffer instantiation branch so the fault injection above
  // observes both instantiations, including the recovery from a cloned body.
  const streaming = WebAssembly.instantiateStreaming;
  WebAssembly.instantiateStreaming = undefined;
  try {
    const old = await api.init({ module_or_path: input });
    assert.equal(JSON.parse(await api.getObjOrder("a")).error.code, 500);
    assert.ok(JSON.parse(await api.getObjOrder("a")).data);
    assert.notEqual(await api.raw(), old);
    assert.equal(browserInstances, 2);
    assert.equal(input.bodyUsed, false);

    const fetch = globalThis.fetch;
    globalThis.fetch = async (request) => {
      assert.equal(await request.text(), "wasm request body");
      return input.clone();
    };
    try {
      const requestApi = await import("../src/index.js?request-recovery");
      const request = new Request("https://example.invalid/d2.wasm", {
        method: "POST", body: "wasm request body",
      });
      await requestApi.init(request);
      // The explicit raw module still holds the first faulted instance.
      assert.equal(JSON.parse(await requestApi.getObjOrder("a")).error.code, 500);
      assert.ok(JSON.parse(await requestApi.getObjOrder("a")).data);
      assert.equal(request.bodyUsed, false);
    } finally {
      globalThis.fetch = fetch;
    }
  } finally {
    WebAssembly.instantiateStreaming = streaming;
  }
} finally {
  WebAssembly.instantiate = instantiate;
}

// Run the production worker message handler, with one trapped binding in its
// initial module. The same worker must report the error and accept the next
// compile; restarting the worker would hide broken instance recovery.
const worker = new Worker(`
  const { workerData } = require("node:worker_threads");
  const runtime = require(workerData.bindings);
  runtime.prepare_d2_public_compile_request_json = () => {
    throw new WebAssembly.RuntimeError("unreachable");
  };
  import(workerData.entry);
`, { eval: true, workerData: {
  bindings: fileURLToPath(new URL("../pkg/node/d2_rs_wasm.cjs", import.meta.url)),
  entry: new URL("../src/d2-worker.node.mjs", import.meta.url).href,
} });
try {
  const signal = AbortSignal.timeout(30_000);
  assert.equal((await once(worker, "message", { signal }))[0].type, "ready");
  const request = { type: "compile", data: { fs: { index: "a -> b" }, options: {} } };
  worker.postMessage({ ...request, id: 1 });
  const fault = (await once(worker, "message", { signal }))[0];
  assert.equal(fault.id, 1);
  assert.equal(fault.type, "error");
  assert.match(fault.error, /^panic recovered: /);
  worker.postMessage({ ...request, id: 2 });
  const healthy = (await once(worker, "message", { signal }))[0];
  assert.equal(healthy.id, 2);
  assert.equal(healthy.type, "result");
  assert.equal(healthy.data.source, "a -> b");
  assert.ok(healthy.data.layout);
} finally {
  await worker.terminate();
}
console.log("WASM trap recovery and cancellation-token isolation pass");
