// SPDX-License-Identifier: MPL-2.0
import {createRequire} from "node:module";
import {readFileSync} from "node:fs";
import {checkTargetAscii} from "../tests/target-ascii.js";
import { readOracleFixture } from "./oracle-fixture.mjs";
const require=createRequire(import.meta.url),runtime=require("../src/node.cjs"),{D2}=require("../src/d2-api.cjs");
const fixture=readOracleFixture("raw-target-render-v090");
const clients=[new D2(()=>runtime),new runtime.D2()];
try {for(const client of clients)await checkTargetAscii(client,fixture);console.log("Target ASCII: 32 original-Go deserialized/mutated direct/worker outcomes");}
finally {await Promise.all(clients.map(client=>client.dispose()));}
