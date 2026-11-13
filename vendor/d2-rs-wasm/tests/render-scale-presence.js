// SPDX-License-Identifier: MPL-2.0
// Root sizing contracts independently captured from pinned original Go D2.
export async function checkRenderScalePresence(createD2, cases) {
  if (cases.length !== 29) throw new Error("incomplete source scale fixture");
  const d2 = createD2();
  const compiled = new Map();
  try {
    for (const c of cases) {
      if (!compiled.has(c.name)) compiled.set(c.name, await d2.compile(c.input, c.compile_options));
      const svg = await d2.render(compiled.get(c.name).diagram, c.options);
      const root = svg.match(/<svg\b[^>]*>/)?.[0];
      if (!root) throw new Error(`missing SVG root: ${c.name}`);
      const attributes = Object.fromEntries(Object.keys(c.attributes).map(name => [
        name, root.match(new RegExp(`${name}="([^"]*)"`))?.[1] ?? null,
      ]));
      if (JSON.stringify(attributes) !== JSON.stringify(c.attributes)) {
        throw new Error(`${c.name} ${JSON.stringify(c.options)}: ${JSON.stringify(attributes)} != ${JSON.stringify(c.attributes)}`);
      }
      if ((svg.match(/animation: d2Transition-/g) || []).length !== c.transitions) {
        throw new Error(`animation inventory differs: ${c.name}`);
      }
    }
  } finally { await d2.dispose(); }
}
