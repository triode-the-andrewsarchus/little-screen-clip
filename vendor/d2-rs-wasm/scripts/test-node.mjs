import { checkRenderScalePresence } from "../tests/render-scale-presence.js";
import { checkSingleBoardAnimation } from "../tests/render-single-animation.js";
import { checkMultiBoardAnimation } from "../tests/render-multi-animation.js";
// SPDX-License-Identifier: MPL-2.0

import runtime from "../src/node.cjs";
import { deepStrictEqual } from "node:assert";
import { checkCompiledEdgeIdentity } from "../tests/compiled-edge-identity.js";
import { checkSketchPrecedence } from "./check-sketch-precedence.mjs";
import { checkSeedConfig } from "./check-seed-config.mjs";
import { checkCancellationBoundary } from "./check-cancellation-boundary.mjs";
import { checkDeadlineBoundary } from "../tests/layout-deadline.js";
import { checkPublicOptionBoundaries } from "../tests/public-option-boundaries.js";
import { checkRawLabelSeedFailures } from "../tests/raw-label-seed-failures.js";
import { checkLegacyStringArguments } from "../tests/legacy-string-arguments.js";
import { checkRequestUnicode, checkPublicRequestUnicode } from "../tests/request-unicode.js";
import { checkElkRequest } from "../tests/elk-request.js";
import { checkUrlCodec } from "../tests/url-codec.js";
import { readOracleFixture } from "./oracle-fixture.mjs";

const { D2: BrowserFacadeD2 } = await import("../pkg/web/d2_api.js");

const source = "a -> b";
const options = JSON.stringify({ seeds: [1] });

const required = [
  "parseD2Source",
  "compileD2Source",
  "compileD2TargetJson",
  "layoutD2Source",
  "renderD2Source",
  "renderMarkdownHtml",
  "WasmCancellationToken",
  "encode",
  "decode",
  "version",
  "jsVersion",
  "getParentID",
  "getCompletions",
  "getBoardAtPosition",
  "getRefRanges",
  "applySourceEditsJson",
  "createRootFieldJson",
  "setRootFieldJson",
  "deleteRootFieldJson",
  "renameRootFieldJson",
  "reconnectEdgeJson",
  "moveObjectJson",
  "getObjOrder",
  "getELKGraph",
];
for (const name of required) {
  if (!(name in runtime)) throw new Error(`missing Node export ${name}`);
}
if (!("D2" in runtime)) throw new Error("missing high-level D2 export");

const browserEventFacade = new BrowserFacadeD2(
  async () => runtime,
  async () => {
    const worker = {
      onmessage: undefined,
      postMessage({ id }) {
        setImmediate(() => worker.onmessage?.(
          new MessageEvent("message", { data: { id, type: "result", data: "browser-event-ok" } }),
        ));
      },
    };
    setImmediate(() => worker.onmessage?.(
      new MessageEvent("message", { data: { type: "ready" } }),
    ));
    return worker;
  },
);
await browserEventFacade.ready;
if (await browserEventFacade.version() !== "browser-event-ok") {
  throw new Error("D2 did not unwrap browser MessageEvent responses");
}
await browserEventFacade.dispose();

let failingWorker;
const workerFailureFacade = new BrowserFacadeD2(
  async () => runtime,
  async () => {
    const worker = {
      onmessage: undefined,
      onerror: undefined,
      postMessage() {},
    };
    failingWorker = worker;
    setImmediate(() => worker.onmessage?.({ type: "ready" }));
    return worker;
  },
);
await workerFailureFacade.ready;
const workerFailureRequests = [
  workerFailureFacade.version(),
  workerFailureFacade.jsVersion(),
];
await new Promise((resolve) => setImmediate(resolve));
failingWorker.onerror(new Error("synthetic worker failure"));
const workerFailureResults = await Promise.allSettled(workerFailureRequests);
if (workerFailureResults.some((result) =>
  result.status !== "rejected" || !String(result.reason).includes("synthetic worker failure"))) {
  throw new Error("worker failure did not reject every pending request");
}
if (workerFailureFacade.pendingRequests.size !== 0) {
  throw new Error("worker failure left rejected requests pending");
}
await workerFailureFacade.dispose();

let disposedWorker;
const pendingDisposeFacade = new BrowserFacadeD2(
  async () => runtime,
  async () => {
    const worker = {
      onmessage: undefined,
      onerror: undefined,
      terminateCalls: 0,
      postMessage() {},
      terminate() { this.terminateCalls += 1; },
    };
    disposedWorker = worker;
    setImmediate(() => worker.onmessage?.({ type: "ready" }));
    return worker;
  },
);
await pendingDisposeFacade.ready;
const pendingDisposeRequest = pendingDisposeFacade.version();
await new Promise((resolve) => setImmediate(resolve));
await pendingDisposeFacade.dispose();
const pendingDisposeResult = await Promise.allSettled([pendingDisposeRequest]);
if (pendingDisposeResult[0]?.status !== "rejected" ||
    !String(pendingDisposeResult[0].reason).includes("disposed")) {
  throw new Error("disposing D2 did not reject its pending request");
}
if (pendingDisposeFacade.pendingRequests.size !== 0 || disposedWorker.terminateCalls !== 1) {
  throw new Error("disposing D2 did not clear pending requests and terminate the worker once");
}
await pendingDisposeFacade.dispose();
if (disposedWorker.terminateCalls !== 1) {
  throw new Error("repeated D2 disposal terminated the worker more than once");
}

const d2 = new runtime.D2();
await d2.ready;
if (!d2.worker) throw new Error("D2 did not create its Node worker");
await checkSketchPrecedence(d2);
// Seed/concurrency controls are native helper extensions. The original public
// CompileOptions JSON ignores these keys; test the native contract explicitly.
await checkSeedConfig({
  async compile(source, options) {
    const target = JSON.parse(runtime.compileD2TargetJson(source, JSON.stringify({
      engine: options.layout, seeds: options.seeds, utf16_positions: true,
    })));
    return { graph: target, diagram: target.diagram };
  },
});
const standardCompileWarnings = [];
const originalStandardCompileWarn = console.warn;
try {
  console.warn = (message) => standardCompileWarnings.push(message);
  await d2.compile(source);
} finally {
  console.warn = originalStandardCompileWarn;
}
if (standardCompileWarnings.length !== 0) {
  throw new Error("standard D2.compile emitted a warning for an internal compatibility helper");
}
const compiled = await d2.compile(source, { layout: "tala" });
if (!compiled.diagram || compiled.diagram.shapes.length !== 2) {
  throw new Error("D2.compile did not return the source diagram projection");
}
if (compiled.fs.index !== "a -> b\n") {
  throw new Error("D2.compile did not return the formatted input filesystem");
}
if (!compiled.graph || !Array.isArray(compiled.graph.nodes) ||
    !Array.isArray(compiled.graph.edges) || !Array.isArray(compiled.graph.objects) ||
    !compiled.graph.root || !compiled.graph.layout ||
    compiled.graph.nodes.length !== 2 || compiled.graph.edges.length !== 1) {
  throw new Error("D2.compile did not return the compiled graph metadata");
}
if (compiled.graph.nodes[0]?.id !== "a" || compiled.graph.edges[0]?.source !== 0 ||
    compiled.graph.edges[0]?.target !== 1 || compiled.graph.objects[0]?.id !== "a" ||
    compiled.graph.edges[0]?.attributes?.label?.value !== "" ||
    !Array.isArray(compiled.graph.ast?.nodes) ||
    compiled.graph.objects[0]?.references?.[0]?.key?.path?.[0]?.unquoted_string === undefined ||
    compiled.graph.edges[0]?.references?.[0]?.map_key_edge_index !== 0) {
  throw new Error("D2.compile graph metadata lost stable node or edge identities");
}
await checkCompiledEdgeIdentity(d2, readOracleFixture("compiled-edge-identity-v090"));
const boardCompiled = await d2.compile(
  "root\nlayers: { one: { child }; two: { other } }\nscenarios: { alt: { variant } }\nsteps: { first: { step } }\n",
  { layout: "tala", seeds: [1] },
);
if (boardCompiled.diagram.layers?.map((board) => board.name).join(",") !== "one,two" ||
    boardCompiled.diagram.scenarios?.[0]?.name !== "alt" ||
    boardCompiled.diagram.steps?.[0]?.name !== "first" ||
    boardCompiled.graph.layers?.map((board) => board.name).join(",") !== "one,two") {
  throw new Error("D2.compile did not preserve nested board ordering");
}
const folderOnlyBoards = await d2.compile(
  "label: Architecture\nlayers: { group: { layers: { one: { child } } } }\n",
  { layout: "tala", seeds: [1] },
);
if (folderOnlyBoards.diagram.isFolderOnly !== true ||
    folderOnlyBoards.graph.isFolderOnly !== true ||
    folderOnlyBoards.diagram.layers?.[0]?.isFolderOnly !== true ||
    folderOnlyBoards.graph.layers?.[0]?.isFolderOnly !== true ||
    folderOnlyBoards.diagram.layers?.[0]?.layers?.[0]?.isFolderOnly !== false ||
    folderOnlyBoards.graph.layers?.[0]?.layers?.[0]?.isFolderOnly !== false) {
  throw new Error("D2.compile did not preserve folder-only board metadata");
}
const childSvg = await d2.render(boardCompiled.diagram.layers[0], { noXMLTag: true });
if (!childSvg.startsWith("<svg") || !childSvg.includes("child")) {
  throw new Error("D2.render did not retain the selected board target");
}
const nullableChildSvg = await d2.render(boardCompiled.diagram.layers[0], boardCompiled.renderOptions);
if (!nullableChildSvg.includes(">child<") || nullableChildSvg.includes(">root<")) {
  throw new Error("nullable returned target lost the selected child Diagram");
}
const nestedBoardCompiled = await d2.compile(
  "root\nlayers: { one: { child; layers: { inner: { grand } } } }\n",
  { layout: "tala", seeds: [1] },
);
const nullableGrandSvg = await d2.render(nestedBoardCompiled.diagram.layers[0].layers[0], nestedBoardCompiled.renderOptions);
if (!nullableGrandSvg.includes(">grand<") || nullableGrandSvg.includes(">root<")) {
  throw new Error("nullable returned target lost the selected grandchild Diagram");
}
const nestedChildAnimation = await d2.render(nestedBoardCompiled.diagram.layers[0], {
  target: "*",
  animateInterval: 1000,
});
if (!nestedChildAnimation.includes("grand") ||
    (nestedChildAnimation.match(/animation: d2Transition-/g) || []).length < 2) {
  throw new Error("D2.render did not scope nested-board targets to the child diagram");
}
const animatedBoards = await d2.compile(
  "x -> y\nlayers: { numbers: { 1 -> 2 } }\n",
  { target: "*", animateInterval: 1000 },
);
const animatedSvg = await d2.render(animatedBoards.diagram, animatedBoards.renderOptions);
if (!animatedSvg.startsWith("<?xml version=\"1.0\"" ) ||
    !animatedSvg.includes("d2Transition-") ||
    (animatedSvg.match(/animation: d2Transition-/g) || []).length < 2) {
  throw new Error("D2.render did not animate the root and nested boards");
}
const sketchHashBoards = await d2.compile(
  "vars: { d2-config: { theme-id: 4; sketch: true } }\nx -> y\nlayers: { n: { a -> b } }\n",
  { layout: "tala", target: "*", animateInterval: 100, seeds: [1] },
);
const sketchHashSvg = await d2.render(sketchHashBoards.diagram, sketchHashBoards.renderOptions);
if (!sketchHashSvg.includes("d2Transition-d2-3109697860")) {
  throw new Error("D2.render sketch target hash diverged from v0.9.0");
}
const unanimatedBoards = await d2.compile(
  "x -> y\nlayers: { numbers: { 1 -> 2 } }\n",
  { target: "*" },
);
let unanimatedRejected = false;
const unanimatedDefault = await d2.render(unanimatedBoards.diagram);
if (!unanimatedDefault.startsWith("<?xml") || unanimatedDefault.includes("animation: d2Transition-")) {
  throw new Error("D2.render inherited Compile's multiboard target instead of selecting one board by default");
}
try {
  await d2.render(unanimatedBoards.diagram, { target: "*" });
} catch (error) {
  unanimatedRejected = String(error).includes("only supported for animated SVGs");
}
if (!unanimatedRejected) throw new Error("D2.render accepted unanimated multi-board output");
const compiledSvg = await d2.render(compiled.diagram, { noXMLTag: true });
if (!compiledSvg.startsWith("<svg")) throw new Error("D2.render did not return SVG");
const appendixCompiled = await d2.compile("x: {tooltip: x appendix}", { forceAppendix: true });
const appendixSvg = await d2.render(appendixCompiled.diagram, { forceAppendix: true });
if (!appendixSvg.includes('class="appendix"')) {
  throw new Error("D2.render did not include the requested appendix");
}
const ascii = await d2.render(compiled.diagram, { ascii: true });
if (ascii.includes("<svg") || !ascii.includes("a") || !ascii.includes("b")) {
  throw new Error("D2.render did not return the ASCII target");
}
const standardAscii = await d2.render(compiled.diagram, { ascii: true, asciiMode: "standard" });
if (/[┌┐└┘│─]/u.test(standardAscii)) {
  throw new Error("D2.render standard ASCII mode retained Unicode box glyphs");
}
const styled = await d2.compile(
  "a: {shape: circle; style.fill: red; tooltip: hi; link: https://example.com}\n",
);
if (styled.diagram.shapes[0]?.type !== "oval" || styled.diagram.shapes[0]?.fill !== "red" ||
    styled.diagram.shapes[0]?.tooltip !== "hi") {
  throw new Error("D2.compile omitted target shape/style metadata");
}
const configured = await d2.compile(
  "vars: { d2-config: { theme-id: 4; pad: 10; center: true; layout-engine: elk } }\nx -> y\n",
);
if (configured.renderOptions.themeID !== 4 || configured.renderOptions.pad !== 10 ||
    configured.renderOptions.center !== true || configured.diagram.config.layoutEngine !== "elk" ||
    Object.hasOwn(configured.renderOptions, "layout")) {
  throw new Error("D2.compile omitted d2-config render options");
}
const overridden = await d2.compile(
  "vars: { d2-config: { theme-overrides: { B1: \"#000000\" } } }\na -> b\n",
);
const overriddenSvg = await d2.render(overridden.diagram, overridden.renderOptions);
if (overridden.renderOptions.themeOverrides?.b1 !== "#000000" ||
    !overriddenSvg.includes("#000000")) {
  throw new Error("D2 facade did not preserve theme overrides");
}
let customFontRejected = false;
try {
  await d2.compile("a\n", { fontRegular: new Uint8Array([1, 2, 3]) });
} catch (error) {
  customFontRejected = error instanceof Error && error.message === "invalid JSON input";
}
if (!customFontRejected) throw new Error("D2 facade did not validate custom font data");
const importedCompiled = await d2.compile({
  fs: { index: "a: @import\n", "import.d2": "x: {shape: circle}\n" },
});
if (!importedCompiled.diagram.shapes.some((shape) => shape.id === "a.x")) {
  throw new Error("D2.compile omitted a virtual import shape");
}
const classRowTargets = readOracleFixture("class-row-target-source-v090");
if (classRowTargets.cases.length !== 44) throw new Error("incomplete original class target fixture");
for (const row of classRowTargets.cases) {
  const actual = await d2.compile(row.source, { layout: "dagre" });
  deepStrictEqual(actual.diagram.shapes.map(({id, fields, methods}) => ({id, fields, methods})), row.shapes);
}
const targetRowCollections = readOracleFixture("target-row-collections-source-v090");
if (targetRowCollections.cases.length !== 15) throw new Error("incomplete original target collection fixture");
for (const row of targetRowCollections.cases) {
  const actual = await d2.compile(row.source, { layout: "dagre" });
  deepStrictEqual(actual.diagram.shapes.map(({id, fields, methods, columns}) => ({
    id, fields, methods,
    columnCount: columns === null ? null : columns.length,
  })), row.shapes);
}
const publicCompilerDiagnostics = readOracleFixture("public-compiler-diagnostics-source-v090");
const directDiagnosticFacade = new BrowserFacadeD2(() => runtime);
try {
  for (const row of publicCompilerDiagnostics.cases) {
    const inputPath = row.inputPath || "index.d2";
    const input = { fs: { ...row.files, [inputPath]: row.source }, inputPath };
    for (const facade of [d2, directDiagnosticFacade]) {
      let caught;
      try { await facade.compile(input, { layout: "dagre", ...row.options }); }
      catch (error) { caught = error; }
      if (!(caught instanceof Error)) throw new Error(`${row.name}: public compile must reject with Error`);
      deepStrictEqual({ name: caught.name, message: caught.message, code: caught.code ?? null }, row.error);
      deepStrictEqual((await facade.compile("a -> b\n")).diagram.shapes.length, 2);
    }
    let helperError;
    try {
      runtime.compileD2TargetJson(row.source, JSON.stringify({
        path: inputPath, files: input.fs, utf16_positions: true,
        engine: row.options?.layout || "dagre",
      }));
    } catch (error) { helperError = error.message ?? String(error); }
    deepStrictEqual(helperError, row.diagnostics.map((error) => error.errmsg).join("\n"));
  }
} finally { await directDiagnosticFacade.dispose(); }
function originalPublicCompilerError(row) {
  const original = publicCompilerDiagnostics.cases.find(
    (entry) => entry.source === row.source && entry.inputPath === row.path
      && (entry.options?.layout || "dagre") === (row.layout || "dagre"),
  );
  if (!original) throw new Error(`missing original public diagnostic fixture: ${row.name || row.path}`);
  return original.error.message;
}
const sqlColumnTargets = readOracleFixture("sql-column-target-source-v090");
if (sqlColumnTargets.cases.length !== 30) throw new Error("incomplete original SQL column fixture");
for (const row of sqlColumnTargets.cases) {
  const input = { fs: { [row.path]: row.source }, inputPath: row.path };
  if (row.error) {
    let error;
    try { await d2.compile(input, { layout: "dagre" }); }
    catch (caught) { error = caught.message ?? String(caught); }
    deepStrictEqual(error, originalPublicCompilerError(row));
  } else {
    const actual = await d2.compile(input, { layout: "dagre" });
    deepStrictEqual(actual.diagram.shapes, row.shapes);
  }
}
const targetShapeArrays = readOracleFixture("target-shape-arrays-source-v090");
if (targetShapeArrays.cases.length !== 151) throw new Error("incomplete original target shape array fixture");
for (const row of targetShapeArrays.cases) {
  const input = { fs: { ...row.files, [row.path]: row.source }, inputPath: row.path };
  if (row.error) {
    let error;
    try { await d2.compile(input, { layout: row.layout }); }
    catch (caught) { error = caught.message ?? String(caught); }
    deepStrictEqual(error, originalPublicCompilerError(row), row.name);
  } else {
    const actual = await d2.compile(input, { layout: row.layout });
    deepStrictEqual(actual.diagram, row.diagram, row.name);
  }
}
const iconC4Diagrams = readOracleFixture("target-icons-c4-source-v090");
if (iconC4Diagrams.cases.length !== 27) throw new Error("incomplete source icon/C4 matrix");
for (const row of iconC4Diagrams.cases) {
  const actual = await d2.compile({fs:{[row.path]:row.source},inputPath:row.path},{layout:row.layout});
  deepStrictEqual(actual.diagram,row.diagram,row.name);
}
const graphRootCases = readOracleFixture("public-graph-roots-source-v090").cases;
if (graphRootCases.length !== 87) throw new Error("incomplete original Graph root cohort");
let checkedGraphBoards = 0, checkedReferences = 0;
for (const row of graphRootCases) {
  const actual = await d2.compile({ fs: { "index.d2": row.source }, inputPath: "index.d2" }, { layout: row.engine });
  for (const board of row.boards) {
    let graph = actual.graph, diagram = actual.diagram;
    const parts = board.path.split(".").filter(Boolean);
    for (let i = 0; i < parts.length; i += 2) {
      graph = graph[parts[i]].find(child => child.name === parts[i + 1]);
      diagram = diagram[parts[i]].find(child => child.name === parts[i + 1]);
    }
    deepStrictEqual(graph.ast, board.ast, `${row.name}:${board.path}.ast`);
    deepStrictEqual(graph.root, board.root, `${row.name}:${board.path}.root`);
    deepStrictEqual(diagram.root, board.target_root, `${row.name}:${board.path}.target_root`);
    checkedGraphBoards++;
  }
  if (row.references) {
    for (const kind of ["objects", "edges"]) {
      deepStrictEqual(actual.graph[kind].map(object => object.references), row.references[kind], `${row.name}.${kind}.references`);
    }
    checkedReferences++;
  }
}
if (checkedGraphBoards !== 99 || checkedReferences !== 6) throw new Error("incomplete Graph/reference inventory");
const renderOptionCases = readOracleFixture("public-render-options-source-v090").cases;
if (renderOptionCases.length !== 147) throw new Error("incomplete source renderOptions cohort");
for (const row of renderOptionCases) {
  const actual = await d2.compile({fs: row.request.fs, inputPath: row.request.inputPath}, row.request.options);
  deepStrictEqual(actual.renderOptions, row.renderOptions, `${row.name}.renderOptions`);
  deepStrictEqual(actual.diagram, row.diagram, `${row.name}.diagram`);
}
const legendDiagrams = readOracleFixture("legend-target-source-v090");
if (legendDiagrams.cases.length !== 24) throw new Error("incomplete original legend matrix");
for (const row of legendDiagrams.cases) {
  let actual, error;
  try {actual=await d2.compile({fs:{[row.path]:row.source},inputPath:row.path},{layout:row.layout});}
  catch(e){error=e.message??String(e);}
  if(row.error) deepStrictEqual(error,originalPublicCompilerError(row),row.name);
  else {deepStrictEqual(error,undefined,row.name);deepStrictEqual(actual.diagram,row.diagram,row.name);}
}
const targetDiagrams = readOracleFixture("target-diagram-expanded-source-v090");
if (targetDiagrams.cases.length !== 26) throw new Error("incomplete source diagram expansion");
for (const row of targetDiagrams.cases) {
  if (row.panic) throw new Error(`source panic remains unqualified: ${row.name}`);
  let actual, error;
  try {actual = await d2.compile({fs:{[row.path]:row.source},inputPath:row.path},{layout:row.layout});}
  catch(e){error=e.message??String(e);}
  if(row.error) deepStrictEqual(error,originalPublicCompilerError(row),row.name);
  else {deepStrictEqual(error,undefined,row.name);deepStrictEqual(actual.diagram,row.diagram,row.name);}
}
await d2.dispose();
let disposed = false;
try {
  await d2.version();
} catch (error) {
  disposed = String(error).includes("disposed");
}
if (!disposed) throw new Error("D2.dispose did not reject later requests");

const ast = runtime.parseD2Source(source, "");
if (!ast.includes('"edges"')) throw new Error("parse output omitted edges");

const edited = JSON.parse(runtime.applySourceEditsJson(JSON.stringify({
  source: "a: 😀\nb -> c\n",
  encoding: "utf16",
  edits: [
    { start: { line: 0, column: 3 }, end: { line: 0, column: 5 }, replacement: "X" },
    { start: { line: 1, column: 0 }, end: { line: 1, column: 1 }, replacement: "d" },
  ],
})));
if (edited.source !== "a: X\nd -> c\n") {
  throw new Error("applySourceEditsJson did not apply UTF-16 ranges against one source snapshot");
}
let editRejected = false;
try {
  runtime.applySourceEditsJson(JSON.stringify({
    source: "abc",
    edits: [{ start: { line: 0, column: 0 }, end: { line: 0, column: 2 }, replacement: "x" },
      { start: { line: 0, column: 1 }, end: { line: 0, column: 3 }, replacement: "y" }],
  }));
} catch (error) {
  editRejected = String(error).includes("overlap");
}
if (!editRejected) throw new Error("applySourceEditsJson accepted overlapping edits");
const created = JSON.parse(runtime.createRootFieldJson(JSON.stringify({
  source: "a\n",
  key: "b",
})));
if (created.key !== "b" || created.source !== "a\nb\n") {
  throw new Error("createRootFieldJson did not return a validated root edit");
}
const set = JSON.parse(runtime.setRootFieldJson(JSON.stringify({
  source: "a: one\n",
  key: "a",
  value: "two",
})));
if (set.source !== "a: two\n") throw new Error("setRootFieldJson did not update a root field");
const renamed = JSON.parse(runtime.renameRootFieldJson(JSON.stringify({
  source: set.source,
  old_key: "a",
  new_key: "b",
})));
if (renamed.source !== "b: two\n") throw new Error("renameRootFieldJson did not preserve the value");
const deleted = JSON.parse(runtime.deleteRootFieldJson(JSON.stringify({
  source: renamed.source,
  key: "b",
})));
if (deleted.source !== "" || deleted.deleted !== true) {
  throw new Error("deleteRootFieldJson did not remove the root field");
}
const reconnected = JSON.parse(runtime.reconnectEdgeJson(JSON.stringify({
  source: "a -> b: label\nc\nd\n",
  edgeKey: "(a -> b)[0]",
  newSrc: "c",
  newDst: "d",
})));
if (reconnected.source !== "c -> d: label\nc\nd\n" || reconnected.changed !== true) {
  throw new Error("reconnectEdgeJson did not update the written edge");
}
const moved = JSON.parse(runtime.moveObjectJson(JSON.stringify({
  source: "a\nc\n",
  old_key: "c",
  new_key: "a.c",
  include_descendants: true,
})));
if (moved.key !== "a.c" || moved.source !== "a: {\n  c\n}\n") {
  throw new Error("moveObjectJson did not open an existing parent object as a container");
}
const flatMove = JSON.parse(runtime.moveObjectJson(JSON.stringify({
  source: "a.b: \"yo\"\n",
  old_key: "a.b",
  new_key: "b",
  include_descendants: true,
})));
if (flatMove.source !== "a\nb: \"yo\"\n") {
  throw new Error("moveObjectJson did not retain the old flat-key parent");
}

const ir = runtime.compileD2Source(source, "");
if (!ir.includes('"root"')) throw new Error("compile output omitted root");
const imported = runtime.compileD2Source(
  "a: @import\n",
  JSON.stringify({ path: "index", files: { "import.d2": "x: {shape: circle}\n" } }),
);
if (!imported.includes('"a"') || !imported.includes('"x"')) {
  throw new Error("virtual import compilation omitted imported fields");
}

const layout = runtime.layoutD2Source(source, options);
const layoutValue = JSON.parse(layout);
if (!layout.includes('"boxes"') || !layout.includes('"routes"')) {
  throw new Error("layout output omitted boxes or routes");
}
if (layoutValue.boxes?.["0"]?.size?.width !== 53 || layoutValue.node_labels?.["0"]?.size?.width !== 8) {
  throw new Error("WASM layout did not use the bundled D2 font metrics");
}

const svg = runtime.renderD2Source(source, options);
if (!svg.startsWith('<?xml version="1.0" encoding="utf-8"?><svg') || !svg.includes("viewBox=")) {
  throw new Error("render output was not SVG");
}

const html = runtime.renderMarkdownHtml("**ok**");
if (!html.includes("<strong>ok</strong>")) {
  throw new Error("Markdown HTML output omitted strong text");
}

const encodedResponse = JSON.parse(runtime.encode(source));
if (typeof encodedResponse.result !== "string" || !encodedResponse.result) {
  throw new Error("encode did not return a compressed result");
}
const decodedResponse = JSON.parse(runtime.decode(encodedResponse.result));
if (decodedResponse.result !== source) throw new Error("decode did not round-trip source");
for (const [name, invoke] of [
  ["getParentID", () => runtime.getParentID()],
  ["getCompletions", () => runtime.getCompletions()],
  ["getBoardAtPosition", () => runtime.getBoardAtPosition()],
  ["getRefRanges", () => runtime.getRefRanges()],
  ["encode", () => runtime.encode()],
  ["decode", () => runtime.decode()],
]) {
  const response = JSON.parse(invoke());
  if (response.error?.code !== 400) {
    throw new Error(`${name} did not return a v0.9.0 missing-argument error envelope`);
  }
}
if (JSON.parse(runtime.getParentID()).error?.message !== "missing id argument" ||
    JSON.parse(runtime.getCompletions()).error?.message !== "missing required arguments" ||
    JSON.parse(runtime.getBoardAtPosition()).error?.message !== "missing required arguments" ||
    JSON.parse(runtime.getRefRanges()).error?.message !== "missing required arguments" ||
    JSON.parse(runtime.encode()).error?.message !== "missing script argument" ||
    JSON.parse(runtime.decode()).error?.message !== "missing script argument") {
  throw new Error("legacy helper missing-argument messages diverged from v0.9.0");
}
if (runtime.version() !== "0.9.0" || runtime.jsVersion() !== "HEAD") {
  throw new Error("version exports did not report pinned compatibility versions");
}

const parent = JSON.parse(runtime.getParentID("a.b"));
if (parent.data !== "a") throw new Error("legacy getParentID did not return the parent");
const completions = JSON.parse(runtime.getCompletions("node: {shape: }", 0, 14));
if (!completions.data?.items?.some((item) => item.label === "rectangle")) {
  throw new Error("legacy getCompletions omitted shape values");
}
const unicodeCompletionSource = '"😀".node: {shape: }';
const unicodeCompletions = JSON.parse(runtime.getCompletions(
  unicodeCompletionSource,
  0,
  unicodeCompletionSource.length - 1,
));
const unicodeCompletionExpected = readOracleFixture("render-regressions-master").rows
  .find((row) => row.kind === "completion" && row.name === "unicode-completion");
if (!unicodeCompletionExpected) throw new Error("missing unicode completion oracle row");
deepStrictEqual(unicodeCompletions, unicodeCompletionExpected.got);
const board = JSON.parse(runtime.getBoardAtPosition("layers: { one: { service: api } }", 0, 20));
if (JSON.stringify(board.data?.boardPath) !== JSON.stringify(["layers", "one"])) {
  throw new Error("legacy getBoardAtPosition returned the wrong board");
}
const outsideBoard = JSON.parse(runtime.getBoardAtPosition("service: api\nlayers: { one: { x } }", 0, 0));
if (outsideBoard.data?.boardPath !== null) {
  throw new Error("legacy getBoardAtPosition must preserve a nil board path as null");
}
const refs = JSON.parse(runtime.getRefRanges(
  JSON.stringify({ index: "a: hello\nb: a\n" }),
  "index",
  "a",
  "[]",
));
if (!Array.isArray(refs.data?.ranges)) throw new Error("legacy getRefRanges omitted ranges");
if (JSON.parse(runtime.getRefRanges("not json", "index", "a", "[]")).error?.message !==
    "invalid fs argument" ||
    JSON.parse(runtime.getRefRanges("{}", "index", "a", "not json")).error?.message !==
      "invalid boardPath argument") {
  throw new Error("legacy getRefRanges did not preserve v0.9.0 JSON argument errors");
}
const missingRefs = JSON.parse(runtime.getRefRanges(
  JSON.stringify({ index: "a: hello\n" }),
  "index",
  "missing",
  "[]",
));
if (missingRefs.data?.ranges !== null || missingRefs.data?.importRanges !== null) {
  throw new Error("legacy getRefRanges must preserve nil slices as null");
}
const objOrderWarnings = [];
const originalObjOrderWarn = console.warn;
let order;
try {
  console.warn = (message) => objOrderWarnings.push(message);
  order = JSON.parse(runtime.getObjOrder("a -> b\n"));
  const missingOrderArgument = JSON.parse(runtime.getObjOrder());
  if (missingOrderArgument.error?.message !== "missing dsl argument" ||
      missingOrderArgument.error?.code !== 400) {
    throw new Error("legacy getObjOrder did not preserve its missing-argument error");
  }
} finally {
  console.warn = originalObjOrderWarn;
}
if (JSON.stringify(order.data?.order) !== JSON.stringify(["a", "b"])) {
  throw new Error("legacy getObjOrder returned an unexpected order");
}
if (objOrderWarnings.length !== 1 || objOrderWarnings[0] !==
    "d2.getObjOrder is deprecated and will be removed after one compatibility release; use d2oracle.GetObjOrder in Go integrations.") {
  throw new Error("legacy getObjOrder did not warn exactly once with the v0.9.0 notice");
}

let elkWarningAttempts = 0;
const originalElkWarn = console.warn;
let elk;
try {
  console.warn = () => {
    elkWarningAttempts += 1;
    throw new Error("consumer console.warn failed");
  };
  elk = JSON.parse(runtime.getELKGraph(JSON.stringify({ fs: { index: source } })));
  const missingElkArgument = JSON.parse(runtime.getELKGraph());
  if (missingElkArgument.error?.message !== "missing JSON argument" ||
      missingElkArgument.error?.code !== 400) {
    throw new Error("legacy getELKGraph did not preserve its missing-argument error");
  }
} finally {
  console.warn = originalElkWarn;
}
if (elkWarningAttempts !== 1) {
  throw new Error("legacy getELKGraph did not attempt its warning exactly once");
}
if (!Array.isArray(elk.data?.children) || elk.data.children.length !== 2) {
  throw new Error("legacy getELKGraph omitted root children");
}
const nestedElk = JSON.parse(runtime.getELKGraph(JSON.stringify({
  fs: { index: "container: {\n  a -> b\n}\n" },
})));
if (nestedElk.data?.edges?.[0]?.id !== "container.(a -> b)[0]") {
  throw new Error("legacy getELKGraph did not preserve nested edge identity");
}
const tableElk = JSON.parse(runtime.getELKGraph(JSON.stringify({
  fs: { index: `users: {\n  shape: sql_table\n  id: int\n  name: string\n}\norders: {\n  shape: sql_table\n  user_id: int\n}\norders.user_id -> users.id\n` },
})));
if (tableElk.data?.children?.find((node) => node.id === "users")?.ports?.length !== 4) {
  throw new Error("legacy getELKGraph omitted SQL-table column ports");
}
if (tableElk.data?.edges?.[0]?.sources?.[0] !== "orders.user_id.src" ||
    tableElk.data?.edges?.[0]?.targets?.[0] !== "users.id.dst") {
  throw new Error("legacy getELKGraph did not route SQL-table edges through ports");
}

const token = new runtime.WasmCancellationToken();
if (token.is_cancelled()) throw new Error("new cancellation token is cancelled");
token.cancel();
if (!token.is_cancelled()) throw new Error("cancel() did not update token state");
let cancelled = false;
try {
  runtime.layoutD2SourceWithCancellation(source, JSON.stringify({ engine: "tala", seeds: [1] }), token);
} catch (error) {
  cancelled = String(error).toLowerCase().includes("cancel");
}
if (!cancelled) throw new Error("cancelled layout did not return a cancellation error");
token.free();
const liveToken = new runtime.WasmCancellationToken();
runtime.layoutD2SourceWithCancellation(source, options, liveToken);
liveToken.free();
checkCancellationBoundary(runtime);
checkDeadlineBoundary(runtime, { cases: readOracleFixture("layout-context-admission-master").deadlines });

let malformed = false;
try {
  runtime.parseD2Source("a: {", "");
} catch {
  malformed = true;
}
if (!malformed) throw new Error("malformed source did not reject");

await checkPublicOptionBoundaries(runtime, BrowserFacadeD2);

console.log("d2-rs-wasm Node bindings smoke test passed");

checkRawLabelSeedFailures(runtime);
checkLegacyStringArguments(runtime, readOracleFixture("legacy-string-arguments-source-v090"));
checkElkRequest(runtime, readOracleFixture("elk-request-source-v090"));
const requestUnicode = readOracleFixture("request-unicode-source-v090");
checkRequestUnicode(runtime, requestUnicode);
const requestUnicodeWorker = new runtime.D2();
await requestUnicodeWorker.ready;
try { await checkPublicRequestUnicode(requestUnicodeWorker, requestUnicode); }
finally { await requestUnicodeWorker.dispose(); }
const requestUnicodeFallback = new BrowserFacadeD2(() => runtime);
try { await checkPublicRequestUnicode(requestUnicodeFallback, requestUnicode); }
finally { await requestUnicodeFallback.dispose(); }
const codecFixture = readOracleFixture("url-codec-source-v090");
const codecWorker = new runtime.D2();
await codecWorker.ready;
try { await checkUrlCodec(codecWorker, codecFixture); }
finally { await codecWorker.dispose(); }
const codecFallback = new BrowserFacadeD2(() => runtime);
try { await checkUrlCodec(codecFallback, codecFixture); }
finally { await codecFallback.dispose(); }

const scalePresenceFixture = readOracleFixture("svg-scale-render-source-v090");
await checkRenderScalePresence(() => new runtime.D2(), scalePresenceFixture);
await checkRenderScalePresence(() => new BrowserFacadeD2(() => runtime), scalePresenceFixture);
const singleAnimationFixture = readOracleFixture("animation-single-board-source-v090");
await checkSingleBoardAnimation(() => new runtime.D2(), singleAnimationFixture);
await checkSingleBoardAnimation(() => new BrowserFacadeD2(() => runtime), singleAnimationFixture);
const multiAnimationFixture = readOracleFixture("animation-multi-board-source-v090");
await checkMultiBoardAnimation(() => new runtime.D2(), multiAnimationFixture);
await checkMultiBoardAnimation(() => new BrowserFacadeD2(() => runtime), multiAnimationFixture);

const { checkPublicErrorTypes } = await import("../tests/public-error-types.js");
const publicErrorFixture = readOracleFixture("public-error-types-source-v090");
const publicErrorClients = [new runtime.D2(),new BrowserFacadeD2(()=>runtime)];
try {for (const client of publicErrorClients) await checkPublicErrorTypes(client, publicErrorFixture);}
finally {await Promise.all(publicErrorClients.map(client=>client.dispose()));}
