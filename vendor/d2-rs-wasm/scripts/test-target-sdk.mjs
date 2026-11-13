// SPDX-License-Identifier: MPL-2.0
import {createRequire} from "node:module";
import {readFileSync} from "node:fs";
import {checkTargetSdk} from "../tests/target-sdk.js";
import { readOracleFixture } from "./oracle-fixture.mjs";
const require=createRequire(import.meta.url),runtime=require("../src/node.cjs"),{D2}=require("../src/d2-api.cjs");
const fixture=readOracleFixture("target-sdk-source-v090");
const clients=[new D2(()=>runtime),new runtime.D2()];
try {for(const client of clients)await checkTargetSdk(client,fixture);console.log("Target SDK: 472 original-Go SVG/animation/error clone/mutation/inheritance outcomes across direct and worker clients");}
finally {await Promise.all(clients.map(client=>client.dispose()));}
