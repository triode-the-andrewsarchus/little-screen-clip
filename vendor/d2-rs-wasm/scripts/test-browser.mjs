// SPDX-License-Identifier: MPL-2.0
import { chromium } from "playwright-core";
import { createServer } from "node:http";
import { readFile, writeFile, mkdir } from "node:fs/promises";
import { resolve, extname, sep } from "node:path";
import { fileURLToPath } from "node:url";
import assert from "node:assert/strict";
import { oracleFixturePath } from "./oracle-fixture.mjs";

const crate = resolve(fileURLToPath(new URL("..", import.meta.url)));
const app = resolve(process.env.D2_RS_BROWSER_PACKAGE || crate);
const output = resolve(process.env.D2_RS_BROWSER_OUTPUT || resolve(crate, "browser-test-output.json"));
await readFile(resolve(app, "pkg/web/d2_rs_wasm.js"));
await readFile(resolve(app, "pkg/web/d2_rs_wasm_bg.wasm"));
const channel = process.env.D2_RS_BROWSER_CHANNEL || "chrome";
const fixtureNames = new Set(["public-error-types-source-v090", "target-sdk-source-v090", "raw-target-render-v090", "request-unicode-source-v090", "elk-request-source-v090", "url-codec-source-v090", "legacy-string-arguments-source-v090", "layout-context-admission-master", "compiled-edge-identity-v090", "tala-deferred-seeds-source-v090", "tala-seed-config-source-v090", "sketch-precedence-source-v090", "public-render-options-source-v090", "svg-scale-render-source-v090", "animation-single-board-source-v090", "animation-multi-board-source-v090"]);
// Every served fixture is a regenerated oracle artifact (docs/FIXTURES.md), read from the fixture cache.
const mime = { ".js": "text/javascript", ".wasm": "application/wasm", ".json": "application/json" };
const report = { channel, complete: false, page_errors: [], requests_failed: [], workers: [], raw: [] };
const server = createServer(async (req, res) => {
  try {
    const pathname = new URL(req.url, "http://localhost").pathname;
    if (pathname === "/") { res.writeHead(200, { "Content-Type": "text/html" }); res.end("<!doctype html><title>D2 browser contracts</title><p>D2 browser contract runner</p>"); return; }
    let file;
    if (pathname.startsWith("/fixture/")) {
      const name = pathname.slice(9); assert(fixtureNames.has(name));
      file = oracleFixturePath(name);
    } else {
      assert(pathname.startsWith("/app/"));
      file = resolve(app, decodeURIComponent(pathname.slice(5))); assert(file.startsWith(app + sep));
    }
    const body = await readFile(file);
    res.writeHead(200, { "Content-Type": mime[extname(file)] || "application/octet-stream", "Cache-Control": "no-store" }); res.end(body);
  } catch (error) { res.writeHead(404, { "Content-Type": "text/plain" }); res.end(String(error)); }
});
await new Promise((done) => server.listen(0, "127.0.0.1", done));
const origin = `http://127.0.0.1:${server.address().port}`;
let browser, page;
const bounded = async (operation, milliseconds, label) => {
  let timer;
  try { return await Promise.race([operation, new Promise((_, reject) => { timer = setTimeout(() => reject(new Error(`${label} timed out after ${milliseconds} ms`)), milliseconds); })]); }
  finally { clearTimeout(timer); }
};
try {
  browser = await chromium.launch({ channel, headless: true, chromiumSandbox: true });
  report.browser_version = browser.version();
  const context = await browser.newContext();
  await context.route("**/*", (route) => route.request().url().startsWith(origin + "/") ? route.continue() : route.abort("blockedbyclient"));
  page = await context.newPage();
  page.on("pageerror", (error) => report.page_errors.push(String(error)));
  page.on("requestfailed", (request) => report.requests_failed.push({ url: request.url(), error: request.failure()?.errorText }));
  page.on("worker", (worker) => report.workers.push(worker.url()));
  await page.goto(origin);
  report.user_agent = await page.evaluate(() => navigator.userAgent);
  report.contracts = await bounded(page.evaluate(async () => (await import("/app/tests/browser-contracts.js")).runContracts()), 180_000, "browser contracts");
  if (process.env.D2_RS_BROWSER_MATRIX) {
    const inputs = JSON.parse(await readFile(process.env.D2_RS_BROWSER_MATRIX, "utf8"));
    for (let index = 0; index < inputs.length; index += 32) {
      const batch = inputs.slice(index, index + 32);
      const rows = await bounded(page.evaluate(async (values) => (await import("/app/tests/browser-contracts.js")).captureRaw(values), batch), 30_000, `raw cases ${index}..${index + batch.length}`);
      assert.equal(rows.length, batch.length); report.raw.push(...rows);
    }
    assert.equal(report.raw.length, inputs.length);
  }
  assert.equal(report.page_errors.length, 0); assert.equal(report.requests_failed.length, 0);
  assert(report.workers.length > 0 && report.workers.every((url) => url.endsWith("/src/d2-worker.browser.js")));
  report.complete = true;
  console.log(`${channel} ${report.browser_version}: ${report.contracts.checks.length} browser contracts, ${report.raw.length} raw calls`);
} catch (error) {
  report.failure = String(error.stack || error);
  if (page) {
    try { report.partial_contracts = await bounded(page.evaluate(async () => (await import("/app/tests/browser-contracts.js")).observation), 3000, "partial capture"); }
    catch (captureError) { report.partial_capture_error = String(captureError); }
  }
  throw error;
}
finally {
  await mkdir(resolve(output, ".."), { recursive: true }); await writeFile(output, JSON.stringify(report, null, 2) + "\n");
  if (browser) await browser.close();
  await new Promise((done) => server.close(done));
}
