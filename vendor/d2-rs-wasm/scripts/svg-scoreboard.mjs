#!/usr/bin/env node
// SPDX-License-Identifier: MPL-2.0
// d2svg-golden.py manifest adapter exercising the built Node WASM module.
import { createRequire } from "node:module";
import { readFileSync, writeFileSync } from "node:fs";

const runtime = createRequire(import.meta.url)("../pkg/node/d2_rs_wasm.cjs");
const rows = JSON.parse(readFileSync(process.argv[2], "utf8"));
for (const row of rows) {
  try {
    const input = readFileSync(row.input, "utf8");
    const diagram = JSON.parse(input);
    const config = diagram.config ?? {};
    const options = { pad: 0 };
    for (const name of ["themeID", "darkThemeID", "sketch", "center", "themeOverrides", "darkThemeOverrides"]) {
      if (config[name] != null) options[name] = config[name];
    }
    if (row.theme_id != null) options.themeID = row.theme_id;
    if (["layers", "scenarios", "steps"].some(key => diagram[key]?.length)) {
      options.target = "*";
      options.animateInterval = 1000;
    }
    writeFileSync(row.output, runtime.render_d2_target_json(input, JSON.stringify(options)));
  } catch (error) {
    writeFileSync(`${row.output}.err`, error?.message ?? String(error));
  }
}
