// SPDX-License-Identifier: MPL-2.0

// Go's d2js/d2wasm/api.go wrapWASMCall recovers panics into code 500.
// A Rust wasm32 panic traps instead. Retire that instance, including its
// binding closure; replacing only its `wasm` variable would let old token
// finalizers free addresses in the replacement instance.
function recoverRuntime(runtime, retire) {
  const legacy = new Set([
    "encode", "decode", "version", "js_version", "getParentID",
    "getCompletions", "getBoardAtPosition", "getRefRanges", "getObjOrder", "getELKGraph",
  ]);
  let retired = false;
  const guarded = { ...runtime };
  for (const [name, method] of Object.entries(runtime)) {
    if (typeof method !== "function" || name === "WasmCancellationToken" ||
        name === "default" || name === "initSync") continue;
    guarded[name] = (...args) => {
      if (retired) throw new Error("WASM instance was retired after a panic; load a new runtime");
      try {
        return method(...args);
      } catch (error) {
        if (!(error instanceof WebAssembly.RuntimeError)) throw error;
        retired = true;
        retire();
        const message = `panic recovered: ${error.message}`;
        if (legacy.has(name)) return JSON.stringify({ error: { message, code: 500 } });
        throw new Error(message);
      }
    };
  }
  return guarded;
}

module.exports = { recoverRuntime };
