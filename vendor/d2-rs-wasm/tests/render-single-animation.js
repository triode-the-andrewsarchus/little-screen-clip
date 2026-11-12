// SPDX-License-Identifier: MPL-2.0
// Single-board wrapping captured independently from pinned Go D2 v0.9.0.
export async function checkSingleBoardAnimation(createD2, cases) {
  if (cases.length !== 20) throw new Error("incomplete source animation fixture");
  const d2 = createD2();
  try {
    for (const c of cases) {
      const { diagram } = await d2.compile(c.input, c.compile_options);
      const svg = await d2.render(diagram, c.options);
      const openings = svg.match(/<svg\b[^>]*>/g) || [];
      for (const [opening, expected] of [[openings[0], c.attributes], [openings[1], c.inner_attributes]]) {
        if (!opening) throw new Error(`missing animated SVG wrapper: ${c.name}`);
        for (const [name, value] of Object.entries(expected)) {
          const actual = opening.match(new RegExp(`${name}="([^"]*)"`))?.[1] ?? null;
          if (actual !== value) throw new Error(`${c.name} ${name}: ${actual} != ${value}`);
        }
      }
      if ((svg.match(/animation: d2Transition-/g) || []).length !== c.transitions ||
          !svg.includes(`d2Transition-${c.hash}-0`)) {
        throw new Error(`source animation hash/frame inventory differs: ${c.name}`);
      }
      // Retain the full nested SVG label content when wrapping the board.
      const ordinary = await d2.render(diagram, { ...c.options, animateInterval: 0 });
      // d2animate.Wrap adds shared sketch pattern definitions. Compare the
      // painted content; its extra definition paths are not lost board paths.
      // Pinned Go sketch control: 11 total animated paths, 8 ordinary paths.
      const painted = value => value.replace(/<defs\b[^>]*>[\s\S]*?<\/defs>/g, "");
      for (const tag of ["svg", "text", "foreignObject", "path", "rect"]) {
        const count = value => (painted(value).match(new RegExp(`<${tag}\\b`, "g")) || []).length;
        if (count(svg) !== count(ordinary)) throw new Error(`incomplete ${tag} inventory: ${c.name}`);
      }
      if (typeof DOMParser !== "undefined" &&
          new DOMParser().parseFromString(svg, "image/svg+xml").querySelector("parsererror")) {
        throw new Error(`invalid animated SVG XML: ${c.name}`);
      }
    }
  } finally { await d2.dispose(); }
}
