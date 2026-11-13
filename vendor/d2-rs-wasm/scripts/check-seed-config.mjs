import assert from "node:assert/strict";
import { readOracleFixture } from "./oracle-fixture.mjs";

export async function checkSeedConfig(d2) {
  const deferred = readOracleFixture("tala-deferred-seeds-source-v090");
  assert.equal(deferred.cases.length, 100);
  for (const item of deferred.cases) {
    let message = "";
    try { await d2.compile(item.input.source, { layout: "tala", seeds: item.input.seeds }); }
    catch (error) { message = error instanceof Error ? error.message : String(error); }
    assert.equal(message, item.error, item.input.name);
  }
  const fixture = readOracleFixture("tala-seed-config-source-v090");
  assert.equal(fixture.cases.length, 72);
  for (const item of fixture.cases) {
    const { source, engine, seeds } = item.input;
    let result, message = "";
    try { result = await d2.compile(source, { layout: engine, seeds }); }
    catch (error) { message = error instanceof Error ? error.message : String(error); }
    assert.equal(message, item.error, item.input.name);
    if (!message && item.seeds !== null) {
      assert.deepEqual(result.graph.layout.report.candidates.map((candidate) => candidate.seed), item.seeds, item.input.name);
    }
  }
  const nested = readOracleFixture("tala-seed-nesting-source-v090");
  assert.equal(nested.cases.length, 5);
  for (const item of nested.cases) {
    const a = await d2.compile(item.source, { layout: "tala", seeds: [1] });
    const b = await d2.compile(item.source, { layout: "tala", seeds: [7] });
    assert.deepEqual(a.graph, b.graph, `${item.name}: complete graph`);
    assert.deepEqual(a.diagram, b.diagram, `${item.name}: complete diagram`);
  }
}
