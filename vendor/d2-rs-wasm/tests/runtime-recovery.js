// SPDX-License-Identifier: MPL-2.0

// Exercise an actual WASM trap with real d2-rs bindings and allocators. The
// tiny module's only function is `unreachable`; no production fault hook is
// needed and no large or hostile source is processed by the test runner.
export async function checkRuntimeRecovery(createRuntime, recoverRuntime) {
  const check = (condition, message) => {
    if (!condition) throw new Error(message);
  };
  const trap = new WebAssembly.Instance(new WebAssembly.Module(Uint8Array.from([
    0, 97, 115, 109, 1, 0, 0, 0,
    1, 4, 1, 96, 0, 0,
    3, 2, 1, 0,
    7, 8, 1, 4, 116, 114, 97, 112, 0, 0,
    10, 5, 1, 3, 0, 0, 11,
  ]))).exports.trap;
  const first = await createRuntime();
  first.getObjOrder = trap;
  let retired = 0;
  const old = recoverRuntime(first, () => { retired++; });
  const oldToken = new old.WasmCancellationToken();
  const staleToken = new old.WasmCancellationToken();
  const response = JSON.parse(old.getObjOrder("a -> b"));
  check(response.error.code === 500, "trap must return Go's error code");
  check(response.error.message.startsWith("panic recovered: "), "trap must return a recovered panic");
  check(retired === 1, "trap must retire its runtime once");
  try {
    old.parse_d2_source("a", "{}");
    throw new Error("retired runtime accepted another request");
  } catch (error) {
    check(error.message.includes("instance was retired"), "retired runtime must reject subsequent calls");
  }

  const second = await createRuntime();
  const healthy = recoverRuntime(second, () => { retired++; });
  const newToken = new healthy.WasmCancellationToken();
  check(first.WasmCancellationToken !== second.WasmCancellationToken, "replacement must have its own token class");
  oldToken.cancel();
  check(!newToken.is_cancelled(), "old token must not cancel the replacement's token");
  oldToken.free();
  check(!newToken.is_cancelled(), "old token free must not free the replacement's token");
  try {
    healthy.layout_d2_source_with_cancellation("a", "{}", staleToken);
    throw new Error("replacement accepted a stale token");
  } catch (error) {
    check(String(error).includes("expected instance of WasmCancellationToken"), "replacement must reject foreign token pointers");
  } finally {
    staleToken.free();
  }
  newToken.cancel();
  check(newToken.is_cancelled(), "new token must remain usable");
  newToken.free();
  check(JSON.parse(healthy.getObjOrder("a -> b")).data, "healthy request after a trap must succeed");
  check(healthy.parse_d2_source("a", "{}").length > 0, "native helper after a trap must succeed");
  check(retired === 1, "ordinary token type errors must not retire a healthy runtime");

  // Native Rust helpers use exceptions, whereas the Go-compatible exports
  // above return JSON. Preserve both existing API shapes for recovered faults.
  const native = recoverRuntime({ parse_d2_source: trap }, () => { retired++; });
  try {
    native.parse_d2_source("a", "{}");
    throw new Error("native trap was not recovered");
  } catch (error) {
    check(error instanceof Error && error.message.startsWith("panic recovered: "), "native trap must throw a recovered Error");
  }
  const validation = new TypeError("invalid argument");
  const ordinary = recoverRuntime({ decode() { throw validation; } }, () => { retired++; });
  try { ordinary.decode(); } catch (error) { check(error === validation, "ordinary error must be preserved"); }
  check(retired === 2, "ordinary validation errors must not retire a runtime");
}
