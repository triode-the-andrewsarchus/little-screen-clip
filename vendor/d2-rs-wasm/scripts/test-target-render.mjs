// SPDX-License-Identifier: MPL-2.0
import { createRequire } from "node:module";
import { readFileSync } from "node:fs";
import { checkRawTargetRender } from "../tests/raw-target-render.js";
import { readOracleFixture } from "./oracle-fixture.mjs";
const runtime = createRequire(import.meta.url)("../pkg/node/d2_rs_wasm.cjs");
const fixture = readOracleFixture("raw-target-render-v090");
console.log(`Native target render contracts: ${checkRawTargetRender(runtime, fixture)} original Go controls`);
