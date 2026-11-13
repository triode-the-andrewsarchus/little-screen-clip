import { checkRenderScalePresence } from "./render-scale-presence.js";
import { checkSingleBoardAnimation } from "./render-single-animation.js";
import { checkMultiBoardAnimation } from "./render-multi-animation.js";
import { checkTargetAscii } from "./target-ascii.js";
import { checkTargetSdk } from "./target-sdk.js";
import { checkPublicErrorTypes } from "./public-error-types.js";
// SPDX-License-Identifier: MPL-2.0
import * as api from "../src/index.js";
import { sameJson } from "./exact-json.js";
import { checkCompiledEdgeIdentity } from "./compiled-edge-identity.js";
import { checkDeadlineBoundary } from "./layout-deadline.js";
import { checkLegacyStringArguments } from "./legacy-string-arguments.js";
import { checkRequestUnicode, checkPublicRequestUnicode } from "./request-unicode.js";
import { checkElkRequest } from "./elk-request.js";
import { checkUrlCodec } from "./url-codec.js";
import { D2 as FallbackD2 } from "../pkg/web/d2_api.js";
import { createRuntime } from "../pkg/web/d2_runtime.js";
import { recoverRuntime } from "../pkg/web/recover_runtime.js";
import { checkRuntimeRecovery } from "./runtime-recovery.js";

function check(condition, message) {
  if (!condition) throw new Error(message);
}
function equal(actual, expected, message) {
  check(sameJson(actual, expected), `${message}: ${JSON.stringify({ actual, expected })}`);
}
const fixture = async (name) => {
  const response = await fetch(`/fixture/${name}`);
  check(response.ok, `fixture ${name}: ${response.status}`);
  return response.json();
};
const message = (error) => error instanceof Error ? error.message : String(error);

// Preserve the returned JSON string. Parsing/stringifying would erase signed
// zero and round source int64 fields before the independent exact comparison.
export async function captureRaw(inputs) {
  const runtime = await api.raw();
  return inputs.map((input) => {
    if (input.operation === "move") {
      let error = "", result_raw = null;
      try { result_raw = runtime.move_object_json(input.request_json); }
      catch (thrown) { error = message(thrown); }
      return { name: input.name, error, result_raw };
    }
    if (input.operation !== undefined && input.operation !== "route") throw new Error(`Unknown raw operation: ${input.operation}`);
    const token = new runtime.WasmCancellationToken();
    if (input.cancel) token.cancel();
    let error = "", result_raw = null;
    try {
      const options = JSON.stringify({ seeds: input.seeds, max_concurrency: 1 });
      result_raw = input.operation === "route"
        ? runtime.route_edges_d2_json_with_cancellation(input.request_json, options, token)
        : runtime.layout_d2_json_with_cancellation(input.raw_graph, options, token);
    } catch (thrown) { error = message(thrown); }
    finally { token.free(); }
    return { name: input.name, error, result_raw };
  });
}

export let observation;
export async function runContracts() {
  await checkRuntimeRecovery(async () => {
    const runtime = createRuntime();
    await runtime.default();
    return runtime;
  }, recoverRuntime);
  const checks = [], captures = { cancellation: [], deferred: [], seeds: [], sketch: [] };
  checks.push("WASM trap recovery and cancellation-token isolation");
  observation = { checks, captures };
  const runtime = await api.raw();
  check(await api.init() === runtime && await api.load() === runtime, "lazy runtime identity");
  checks.push("lazy runtime identity");
  const errorTypes = await fixture("public-error-types-source-v090");
  const errorClients = [new api.D2(), new FallbackD2(() => runtime)];
  try { for (const client of errorClients) await checkPublicErrorTypes(client, errorTypes); }
  finally { await Promise.all(errorClients.map(client => client.dispose())); }
  checks.push("34 original-Go public error object/message/name/code and healthy-followup contracts");
  const targetSdkFixture = await fixture("target-sdk-source-v090");
  const targetClients = [new api.D2(), new FallbackD2(() => runtime)];
  try { for (const client of targetClients) await checkTargetSdk(client, targetSdkFixture); }
  finally { await Promise.all(targetClients.map(client => client.dispose())); }
  checks.push("472 original-Go supplied SVG/animation/error clone/mutation/inheritance outcomes through worker and direct runtime");
  const scalePresenceFixture = await fixture("svg-scale-render-source-v090");
  await checkRenderScalePresence(() => new api.D2(), scalePresenceFixture);
  await checkRenderScalePresence(() => new FallbackD2(() => runtime), scalePresenceFixture);
  const singleAnimationFixture = await fixture("animation-single-board-source-v090");
  await checkSingleBoardAnimation(() => new api.D2(), singleAnimationFixture);
  await checkSingleBoardAnimation(() => new FallbackD2(() => runtime), singleAnimationFixture);
  checks.push("20 original-Go single-board animation contracts through worker and direct runtime");
  const multiAnimationFixture = await fixture("animation-multi-board-source-v090");
  await checkMultiBoardAnimation(() => new api.D2(), multiAnimationFixture);
  await checkMultiBoardAnimation(() => new FallbackD2(() => runtime), multiAnimationFixture);
  checks.push("32 original-Go multi-board animation contracts through worker and direct runtime");
  checks.push("29 original-Go scale presence/compile-render contracts through worker and direct runtime");
  const legacyArguments = await fixture("legacy-string-arguments-source-v090");
  checkLegacyStringArguments(runtime, legacyArguments);
  checks.push("60 original-Go legacy string argument and healthy-followup contracts");
  checkElkRequest(runtime, await fixture("elk-request-source-v090"));
  checks.push("179 original-Go ELK request and healthy-followup contracts");
  const requestUnicode = await fixture("request-unicode-source-v090");
  checkRequestUnicode(runtime, requestUnicode);
  checks.push("220 original-Go JSON Unicode request contracts");
  const requestUnicodeWorker = new api.D2();
  await requestUnicodeWorker.ready;
  try { await checkPublicRequestUnicode(requestUnicodeWorker, requestUnicode); }
  finally { await requestUnicodeWorker.dispose(); }
  checks.push("60 original-Go public Unicode requests through a real worker");
  const requestUnicodeFallback = new FallbackD2(() => runtime);
  try { await checkPublicRequestUnicode(requestUnicodeFallback, requestUnicode); }
  finally { await requestUnicodeFallback.dispose(); }
  checks.push("60 original-Go public Unicode requests through fallback");
  const codecFixture = await fixture("url-codec-source-v090");
  const codecWorker = new api.D2();
  await codecWorker.ready;
  try { await checkUrlCodec(codecWorker, codecFixture); }
  finally { await codecWorker.dispose(); }
  checks.push("987 original-Go codec contracts through a real worker");
  const codecFallback = new FallbackD2(() => runtime);
  try { await checkUrlCodec(codecFallback, codecFixture); }
  finally { await codecFallback.dispose(); }
  checks.push("987 original-Go codec contracts through the no-worker fallback");
  const layoutContext = await fixture("layout-context-admission-master");
  const cancellation = { cases: layoutContext.cancellation };
  check(cancellation.cases.length === 304, "complete cancellation inventory");
  let previous;
  for (const row of cancellation.cases) {
    const token = new runtime.WasmCancellationToken();
    if (row.input.cancel) token.cancel();
    let error = "", result_raw = null;
    try { result_raw = runtime.layout_d2_source_with_cancellation(row.input.source, JSON.stringify(row.input), token); }
    catch (thrown) { error = message(thrown); }
    finally { token.free(); }
    check(error === row.error, `${row.input.name}: cancellation error ${error}`);
    if (row.input.cancel && !error && previous?.error === "") {
      check(result_raw === previous.result_raw, `${row.input.name}: complete cancellation pair`);
    }
    captures.cancellation.push({ name: row.input.name, error, result_raw });
    previous = { error, result_raw };
  }
  checks.push("304 source cancellation cases and complete successful pairs");
  captures.deadline = checkDeadlineBoundary(runtime, { cases: layoutContext.deadlines });
  checks.push("340 source deadline and first-cause contracts with complete successful pairs");

  const d2 = new api.D2();
  try {
    await d2.ready;
    const renderOptions = (await fixture("public-render-options-source-v090")).cases;
    check(renderOptions.length === 147, "complete public renderOptions inventory");
    captures.renderOptions = [];
    captures.nullableRenderRoundtrips = [];
    for (const row of renderOptions) {
      const result = await d2.compile({ fs: row.request.fs, inputPath: row.request.inputPath }, row.request.options);
      equal(result.renderOptions, row.renderOptions, `${row.name}: complete renderOptions`);
      equal(result.diagram, row.diagram, `${row.name}: complete Diagram`);
      captures.renderOptions.push({ name: row.name, renderOptions: result.renderOptions, diagram: result.diagram });
      if (["response-defaults", "response-config", "response-config-input-overrides"].includes(row.name.split("/").at(-1))) {
        const svg = await d2.render(result.diagram, result.renderOptions);
        check(svg.includes("<svg") && !new DOMParser().parseFromString(svg, "image/svg+xml").querySelector("parsererror"), `${row.name}: nullable renderOptions roundtrip`);
        captures.nullableRenderRoundtrips.push({ name: row.name, svg });
      }
    }
    check(captures.renderOptions.length === 147 && captures.nullableRenderRoundtrips.length === 9, "complete nullable render inventory");
    checks.push("147 complete source renderOptions/Diagrams and nine nullable worker render roundtrips");
    check(d2.worker instanceof Worker, "real browser module worker");
    checks.push("real browser module worker");
    captures.edge_identity = await checkCompiledEdgeIdentity(d2, await fixture("compiled-edge-identity-v090"));
    checks.push("40 source compiled edge indices, IDs and complete route associations");
    for (const [name, expectedCount, collection] of [
      ["tala-deferred-seeds-source-v090", 100, "deferred"],
      ["tala-seed-config-source-v090", 72, "seeds"],
    ]) {
      const rows = (await fixture(name)).cases;
      check(rows.length === expectedCount, `${name}: complete fixture inventory`);
      for (const row of rows) {
        // These source fixtures describe the native helper extension. Go's
        // public CompileOptions does not accept seeds; Node uses this boundary too.
        let error = "", result, result_raw = null;
        try {
          result_raw = runtime.compile_d2_target_json(row.input.source, JSON.stringify({
            engine: row.input.engine || "tala", seeds: row.input.seeds, utf16_positions: true,
          }));
          result = JSON.parse(result_raw);
        }
        catch (thrown) { error = message(thrown); }
        check(error === row.error, `${row.input.name}: native helper error ${error}`);
        const seeds = result?.layout?.report.candidates.map((candidate) => candidate.seed);
        if (!error && row.seeds !== undefined && row.seeds !== null) equal(seeds, row.seeds, `${row.input.name}: candidate seeds`);
        captures[collection].push({ name: row.input.name, error, seeds, result_raw });
      }
      checks.push(`${expectedCount} ${collection} source cases through native raw helper; public seeds remain ignored`);
    }
    const sketch = (await fixture("sketch-precedence-source-v090")).cases;
    check(sketch.length === 36, "complete sketch inventory");
    for (const row of sketch) {
      const { engine, ...options } = row.options;
      const result = await d2.compile(row.source, { layout: engine, ...options });
      const nodes = result.graph.objects.map((object) => ({ id: object.id, width: object.box.Width,
        height: object.box.Height, label_width: object.attributes.labelDimensions.width,
        label_height: object.attributes.labelDimensions.height }));
      equal(nodes, row.nodes, `${row.name}: source measurements`);
      check(result.renderOptions.sketch === row.effective_sketch, `${row.name}: effective sketch`);
      captures.sketch.push({ name: row.name, nodes, sketch: result.renderOptions.sketch });
    }
    checks.push("36 source sketch/measurement cases through real worker");
    for (const engine of ["tala", "dagre", "elk"]) {
      const source = "a -> b\n";
      const result = await d2.compile(source, { layout: engine, seeds: [1] });
      check(result.diagram.shapes.length === 2 && result.graph.edges.length === 1, `${engine}: graph inventory`);
      check(result.fs.index === source, `${engine}: formatted filesystem`);
      const svg = await d2.render(result.diagram, { noXMLTag: true });
      check(svg.startsWith("<svg") && !new DOMParser().parseFromString(svg, "image/svg+xml").querySelector("parsererror"), `${engine}: complete parseable SVG`);
      const ascii = await d2.render(result.diagram, { ascii: true, asciiMode: "standard" });
      check(!ascii.includes("<svg") && ascii.includes("a") && ascii.includes("b") && !/[┌┐└┘│─]/u.test(ascii), `${engine}: ASCII`);
      checks.push(`${engine}: compile/render SVG/ASCII`);
    }
    const imported = await d2.compile({ fs: { index: "a: @import\n", "import.d2": "x: {shape: circle}\n" } });
    check(imported.diagram.shapes.some((shape) => shape.id === "a.x"), "virtual import");
    checks.push("virtual import");
    const boards = await d2.compile("root\nlayers: { one: { child; layers: { inner: { grand } } }; two: { other } }\n", { layout: "tala", seeds: [1] });
    equal(boards.diagram.layers.map((board) => board.name), ["one", "two"], "board order");
    const child = await d2.render(boards.diagram.layers[0], { noXMLTag: true });
    check(child.startsWith("<svg") && child.includes("child"), "selected board render");
    captures.nullableChildScope = [];
    for (const [name, diagram, expected] of [
      ["child", boards.diagram.layers[0], "child"],
      ["grand", boards.diagram.layers[0].layers[0], "grand"],
    ]) {
      const svg = await d2.render(diagram, boards.renderOptions);
      check(svg.includes(`>${expected}<`) && !svg.includes(">root<"), `${name}: nullable target keeps Diagram scope`);
      check(!new DOMParser().parseFromString(svg, "image/svg+xml").querySelector("parsererror"), `${name}: complete scoped SVG`);
      captures.nullableChildScope.push({ name, svg });
    }
    const animation = await d2.render(boards.diagram.layers[0], { target: "*", animateInterval: 1000 });
    check(animation.includes("grand") && (animation.match(/animation: d2Transition-/g) || []).length >= 2, "nested animation inventory");
    checks.push("nested board order, nullable scope, selection and animation");
    const unicode = "a: \"😀 café 界\"\n";
    check(await d2.decode(await d2.encode(unicode)) === unicode, "UTF-8 codec roundtrip");
    checks.push("UTF-8 codec roundtrip");
    const versions = await Promise.all([d2.version(), d2.jsVersion(), d2.version()]);
    check(versions[0] === "0.9.0" && versions[0] === versions[2] && typeof versions[1] === "string", "concurrent response identities");
    checks.push("concurrent response identities");
    let fontError = "";
    try { await d2.compile("a\n", { fontRegular: new Uint8Array([1, 2, 3]) }); }
    catch (error) { fontError = message(error); }
    check(fontError === "invalid JSON input", "original typed-array JSON rejection");
    check((await d2.compile("recovered\n")).diagram.shapes.length === 1, "worker survives request error");
    checks.push("typed-array validation and worker error recovery");
    const pending = Promise.allSettled([d2.compile("a -> b\n", { layout: "tala" }), d2.version()]);
    await Promise.resolve();
    check(d2.pendingRequests.size === 2, "requests posted before disposal");
    await d2.dispose();
    const settled = await pending;
    check(settled.every((result) => result.status === "rejected" && String(result.reason).includes("disposed")), "dispose rejects pending requests");
    check(d2.pendingRequests.size === 0, "dispose clears pending requests");
    await d2.dispose();
    let disposed = "";
    try { await d2.version(); } catch (error) { disposed = message(error); }
    check(disposed.includes("disposed"), "dispose rejects later requests");
    checks.push("pending/later request rejection and idempotent disposal");
  } finally { await d2.dispose(); }
  const asciiFixture = await fixture("raw-target-render-v090");
  const asciiClients = [new api.D2(), new FallbackD2(() => api.raw())];
  try { for (const client of asciiClients) await checkTargetAscii(client, asciiFixture); }
  finally { await Promise.all(asciiClients.map(client => client.dispose())); }
  checks.push("32 original-Go ASCII deserialized/mutated target outcomes through worker and direct runtime");
  const edited = JSON.parse(await api.applySourceEditsJson(JSON.stringify({ source: "a: 😀\nb -> c\n", encoding: "utf16", edits: [
    { start: { line: 0, column: 3 }, end: { line: 0, column: 5 }, replacement: "X" },
    { start: { line: 1, column: 0 }, end: { line: 1, column: 1 }, replacement: "d" },
  ] })));
  check(edited.source === "a: X\nd -> c\n", "UTF-16 edits through browser wrapper");
  checks.push("UTF-16 edits through browser wrapper");
  return { checks, captures };
}
