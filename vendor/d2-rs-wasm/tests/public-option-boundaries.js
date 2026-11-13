// SPDX-License-Identifier: MPL-2.0
import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { deepStrictEqual, strictEqual } from 'node:assert';
import { readOracleFixture } from '../scripts/oracle-fixture.mjs';

export async function checkPublicOptionBoundaries(runtime, DirectFacade) {
  const fixture = readOracleFixture('public-option-boundaries-source-v090');
  strictEqual(fixture.requests.length, 355);
  const require = createRequire(import.meta.url);
  const filename = require.resolve('../pkg/node/d2_rs_wasm.cjs');
  delete require.cache[filename];
  const fresh = require(filename);
  const facades = [new runtime.D2(), new DirectFacade(() => fresh)];
  let faults = 0, probeErrors = 0;
  try {
    for (const facade of facades) {
      await facade.ready;
      for (const rows of fixture.repeats) {
        for (let i = 0; i < fixture.requests.length; i++) {
          const request = fixture.requests[i], expected = rows[i];
          let result = null, error = null;
          try { result = (await facade.compile(JSON.parse(request.request_json))).diagram; }
          catch (e) {
            if (!(e instanceof Error)) throw new Error(`${request.name}: non-Error public rejection`);
            error = { name: e.name, message: e.message, code: e.code ?? null };
          }
          if (expected.error?.message.startsWith('panic recovered:')) {
            deepStrictEqual([error?.name, error?.code, error?.message.split('\n')[0]], [expected.error.name, expected.error.code, expected.error.message.split('\n')[0]], request.name);
            strictEqual(result, null);
            faults++;
          } else {
            deepStrictEqual({result,error}, {result:expected.result,error:expected.error}, request.name);
          }
          let healthy = null, healthyError = null;
          try { healthy = (await facade.compile('a -> b\n', {layout:'dagre'})).diagram; }
          catch (e) { healthyError = e.message ?? String(e); }
          deepStrictEqual({healthy,healthyError}, {healthy:expected.healthy,healthyError:expected.healthyError}, `${request.name}: complete subsequent request`);
          probeErrors += healthyError !== null;
        }
      }
    }
  } finally { for (const facade of facades) await facade.dispose(); }
  strictEqual(faults, 96); // 48 per facade; full Go stacks remain unmatched.
  strictEqual(probeErrors, 192); // 96 per facade, required source behavior.
}
