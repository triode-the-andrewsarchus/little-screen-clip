// SPDX-License-Identifier: MPL-2.0
// Independently captured original-Go multi-board frame/bounds/hash contracts.
export async function checkMultiBoardAnimation(createD2, cases) {
  if (cases.length !== 32) throw new Error("incomplete source multi-board fixture");
  const d2 = createD2();
  try {
    for (const c of cases) {
      const { diagram } = await d2.compile(c.input, c.compile_options);
      const svg = await d2.render(diagram, c.options);
      if (c.attributes === null) {
        if (svg !== c.value) throw new Error(`nil-render SDK decoding differs: ${c.name}`);
        continue;
      }
      const openings = svg.match(/<svg\b[^>]*>/g) || [];
      for (const [opening, expected] of [[openings[0], c.attributes], [openings[1], c.inner_attributes]]) {
        if (!opening) throw new Error(`missing SVG wrapper: ${c.name}`);
        for (const [name, value] of Object.entries(expected)) {
          const actual = opening.match(new RegExp(`${name}="([^"]*)"`))?.[1] ?? null;
          if (actual !== value) throw new Error(`${c.name} ${name}: ${actual} != ${value}`);
        }
      }
      if ((svg.match(/animation: d2Transition-/g) || []).length !== c.transitions) {
        throw new Error(`incorrect frame inventory: ${c.name}`);
      }
      for (let frame = 0; frame < c.transitions; frame++) {
        if (!svg.includes(`animation: d2Transition-${c.hash}-${frame} ${c.options.animateInterval * c.transitions}ms infinite`)) {
          throw new Error(`incorrect frame hash/index/duration: ${c.name} ${frame}`);
        }
      }
      if (typeof DOMParser !== "undefined" &&
          new DOMParser().parseFromString(svg, "image/svg+xml").querySelector("parsererror")) {
        throw new Error(`invalid animated XML: ${c.name}`);
      }
    }
  } finally { await d2.dispose(); }
}
