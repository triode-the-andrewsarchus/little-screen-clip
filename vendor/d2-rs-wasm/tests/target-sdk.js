// SPDX-License-Identifier: MPL-2.0
// Pinned original-Go target contracts; no expected output is generated natively.
function attributes(svg, inner = false) {
  const tags = [...svg.matchAll(/<svg\b([^>]*)>/g)];
  const tag = tags[inner ? 1 : 0]?.[1];
  if (tag === undefined) return null;
  return Object.fromEntries(["width", "height", "viewBox", "preserveAspectRatio"].map(name =>
    [name, new RegExp(`(?:^|\\s)${name}="([^"]*)"`).exec(tag)?.[1] ?? null]));
}
export function checkTargetSvgOutcome(svg, row) {
  const check = (ok, message) => { if (!ok) throw new Error(`${row.name}: ${message}`); };
  if (row.sdk_output !== undefined) { check(svg === row.sdk_output, "nil-board SDK response differs"); return; }
  check(svg.startsWith("<?xml") === row.xml, "XML declaration differs");
  check(JSON.stringify(attributes(svg)) === JSON.stringify(row.attributes), "SVG attributes differ");
  check(JSON.stringify(attributes(svg, true)) === JSON.stringify(row.inner_attributes), "inner SVG attributes differ");
  const frames = [...svg.matchAll(/animation: d2Transition-(d2-\d+)-(\d+) (\d+)ms infinite/g)].map(m => ({ hash:m[1], index:Number(m[2]), total_ms:Number(m[3]) }));
  check(JSON.stringify(frames) === JSON.stringify(row.frames), "animation hash/order/timing differs");
  if (row.hash !== undefined) check(svg.includes(row.hash), "target hash differs");
  if (row.root_omits_fill) {
    const background = svg.match(/<rect\b[^>]*>/)?.[0];
    check(background !== undefined && !/\sfill=/.test(background), "empty root inherited a theme fill");
  }
}
export async function checkTargetSdk(client, fixture) {
  if (fixture.cases.length !== 117) throw new Error("Incomplete target SDK fixture");
  const compiled = await client.compile("sentinel", {layout:"elk",sketch:true,themeID:200,scale:2,pad:123});
  for (const metadata of [false, true]) for (const row of fixture.cases) {
    let diagram = structuredClone(row.diagram);
    if (metadata && diagram !== null) {
      for (const key of Object.keys(compiled.diagram)) delete compiled.diagram[key];
      Object.assign(compiled.diagram, diagram); diagram = compiled.diagram;
    }
    let output, error;
    try { output = await client.render(diagram, row.options); }
    catch (e) { error = e?.message ?? String(e); }
    if (row.error !== undefined) {
      if (error !== row.error) throw new Error(`${row.name}: expected ${row.error}, got ${error}`);
    } else {
      if (error !== undefined) throw new Error(`${row.name}: ${error}`);
      checkTargetSvgOutcome(output, row);
    }
  }
  const inherited = await client.compile(fixture.inheritance.input, fixture.inheritance.compile_options);
  for (const row of fixture.inheritance.cases) {
    let output, error;
    try { output = await client.render(inherited.diagram, row.options); }
    catch (e) { error = e?.message ?? String(e); }
    if (row.error !== undefined) {
      if (error !== row.error) throw new Error(`${row.name}: expected ${row.error}, got ${error}`);
    } else {
      if (error !== undefined) throw new Error(`${row.name}: ${error}`);
      checkTargetSvgOutcome(output, row);
    }
  }
  return fixture.cases.length * 2 + fixture.inheritance.cases.length;
}
