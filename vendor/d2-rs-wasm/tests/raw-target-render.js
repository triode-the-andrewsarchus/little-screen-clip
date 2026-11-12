// SPDX-License-Identifier: MPL-2.0
// Shared client contract for the native target entry point. No compile call.
export function checkRawTargetRender(runtime, fixture) {
  if (typeof runtime.render_d2_target_json !== "function") throw new Error("Missing target renderer");
  for (const c of fixture.cases) {
    let output, failure;
    try { output = runtime.render_d2_target_json(JSON.stringify(c.diagram), JSON.stringify(c.options)); }
    catch (error) { failure = error?.message ?? String(error); }
    if (c.error !== undefined) {
      if (failure !== c.error) throw new Error(`${c.name}: expected ${c.error}, got ${failure}`);
    } else {
      if (failure !== undefined) throw new Error(`${c.name}: ${failure}`);
      if (c.output !== undefined) {
        if (output !== c.output) throw new Error(`${c.name}: ASCII bytes differ`);
        continue;
      }
      if (!output.includes(c.hash)) throw new Error(`${c.name}: target hash differs`);
      if (!output.includes(`viewBox="${c.viewBox}"`)) throw new Error(`${c.name}: canvas differs`);
      if (output.startsWith("<?xml") !== c.xml) throw new Error(`${c.name}: XML declaration differs`);
    }
  }
  return fixture.cases.length;
}
