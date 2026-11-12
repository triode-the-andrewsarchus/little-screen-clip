// SPDX-License-Identifier: MPL-2.0
import assert from "node:assert/strict";
import { readOracleFixture } from "./oracle-fixture.mjs";

export function checkCancellationBoundary(runtime) {
  const fixture = { cases: readOracleFixture("layout-context-admission-master").cancellation };
  assert.equal(fixture.cases.length, 304);
  let previous;
  for (const item of fixture.cases) {
    const token = new runtime.WasmCancellationToken();
    if (item.input.cancel) token.cancel();
    let result, error = "";
    try {
      result = JSON.parse(runtime.layoutD2SourceWithCancellation(
        item.input.source, JSON.stringify(item.input), token,
      ));
    } catch (thrown) {
      error = thrown instanceof Error ? thrown.message : String(thrown);
    } finally {
      token.free();
    }
    assert.equal(error, item.error, item.input.name);
    if (item.input.cancel && error === "" && previous?.error === "") {
      // The source probe preserves every graph field for these paired calls.
      assert.deepEqual(result, previous.result, item.input.name);
    }
    previous = { error, result };
  }
}
