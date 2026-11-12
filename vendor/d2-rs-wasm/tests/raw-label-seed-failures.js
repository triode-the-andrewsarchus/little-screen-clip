// SPDX-License-Identifier: MPL-2.0
import assert from "node:assert/strict";
import { readOracleFixture } from "../scripts/oracle-fixture.mjs";

export function checkRawLabelSeedFailures(runtime) {
  const { cases } = readOracleFixture("tala-raw-label-seed-failures-source-v090");
  assert.equal(cases.length, 12);
  for (const item of cases) {
    const options = JSON.stringify({ seeds: item.seeds, max_concurrency: item.concurrency });
    let caught;
    try { runtime.layoutD2Json(JSON.stringify(item.graph), options); }
    catch (error) { caught = error; }
    assert.notEqual(caught, undefined, item.name);
    assert.ok(!(caught instanceof WebAssembly.RuntimeError), item.name);
    assert.equal(String(caught), `D2 layout failed: ${item.error}`, item.name);
    // Reuse this same WASM instance after every failure. A trap can leave its
    // allocator/stack damaged even when the host catches RuntimeError.
    const healthy = structuredClone(item.graph);
    for (const edge of healthy.edges) if (edge.attributes) delete edge.attributes.label;
    const result = JSON.parse(runtime.layoutD2Json(JSON.stringify(healthy), options));
    assert.equal(result.objects.length, healthy.objects.length, item.name);
    assert.ok(result.objects.every(object => object.box.TopLeft && Number.isFinite(object.box.TopLeft.x) && Number.isFinite(object.box.TopLeft.y)), item.name);
    assert.ok(result.edges.every(edge => Array.isArray(edge.route) && edge.route.length >= 2), item.name);
  }
  return { sourceErrors: cases.length, sameInstanceRecovery: cases.length };
}
