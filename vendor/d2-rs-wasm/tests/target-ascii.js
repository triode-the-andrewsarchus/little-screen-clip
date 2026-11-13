// SPDX-License-Identifier: MPL-2.0
// These targets and expected bytes come from original Go, not native output.
export async function checkTargetAscii(client, fixture) {
  const cases = fixture.cases.filter(c => c.options?.ascii === true);
  if (cases.length !== 8) throw new Error("Incomplete ASCII client fixture");
  async function checkRender(diagram, c, phase) {
    let actual, failure;
    try { actual = await client.render(diagram, c.options); }
    catch (error) { failure = error?.message ?? String(error); }
    // Pinned d2wasm.Render validates the target before dispatching ASCII.
    // The oracle includes an invalid cubic route; assert its error as well
    // as the successful outputs, through both client and metadata paths.
    if (c.error !== undefined) {
      if (failure !== c.error) throw new Error(`${c.name}: ${phase}: expected ${c.error}, got ${failure}`);
    } else {
      if (failure !== undefined) throw new Error(`${c.name}: ${phase}: ${failure}`);
      if (actual !== c.output) throw new Error(`${c.name}: ${phase} target ASCII differs`);
    }
  }
  for (const c of cases) {
    await checkRender(structuredClone(c.diagram), c, "deserialized");
  }
  const compiled = await client.compile("a");
  for (const c of cases) {
    // Keep the real compile metadata while replacing the visible target. A
    // source replay would paint the original "a" instead of these records.
    compiled.diagram.shapes = structuredClone(c.diagram.shapes || []);
    compiled.diagram.connections = structuredClone(c.diagram.connections || []);
    await checkRender(compiled.diagram, c, "mutated");
  }
  return cases.length * 2;
}
