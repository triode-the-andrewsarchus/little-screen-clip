// SPDX-License-Identifier: MPL-2.0
// Regenerated Go-oracle fixtures for the Node/browser scripts (docs/FIXTURES.md).
// A fixture's logical path is its former tracked location; the artifact lives
// in $D2RS_FIXTURES_DIR (relative to the workspace root) or
// <workspace>/oracle-cache/fixtures. A missing artifact or stamp throws with
// the regeneration command; nothing is skipped.
import {existsSync, readFileSync} from "node:fs";
import {resolve} from "node:path";
import {fileURLToPath} from "node:url";

const workspace = resolve(fileURLToPath(new URL("../../..", import.meta.url)));

export function oracleFixturePath(name) {
  const logical = `crates/d2-rs-wasm/tests/fixtures/${name}.json`;
  const root = process.env.D2RS_FIXTURES_DIR ? resolve(workspace, process.env.D2RS_FIXTURES_DIR) : resolve(workspace, "oracle-cache/fixtures");
  const path = resolve(root, logical);
  if (!existsSync(path) || !existsSync(path + ".stamp.json")) {
    throw new Error(`oracle fixture \`${logical}\` is missing from ${root}; regenerate it with:\n    uv run --no-project python scripts/regen-fixtures.py --only ${logical}`);
  }
  return path;
}

export function readOracleFixture(name) {
  return JSON.parse(readFileSync(oracleFixturePath(name), "utf8"));
}
