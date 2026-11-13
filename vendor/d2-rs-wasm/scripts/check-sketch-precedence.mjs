import assert from "node:assert/strict";
import { readOracleFixture } from "./oracle-fixture.mjs";

export async function checkSketchPrecedence(d2) {
  const fixture = readOracleFixture("sketch-precedence-source-v090");
  assert.equal(fixture.cases.length, 36);
  for (const item of fixture.cases) {
    const { engine, ...options } = item.options;
    const result = await d2.compile(item.source, { layout: engine, ...options });
    const nodes = result.graph.objects.map((object) => ({
      id: object.id,
      width: object.box.Width,
      height: object.box.Height,
      label_width: object.attributes.labelDimensions.width,
      label_height: object.attributes.labelDimensions.height,
    }));
    assert.deepEqual(nodes, item.nodes, `${item.name}: original node measurements`);
    assert.equal(result.renderOptions.sketch, item.effective_sketch, item.name);
  }
}
