// SPDX-License-Identifier: MPL-2.0

// The nodejs wasm-bindgen target initializes synchronously from its adjacent
// `.wasm` file. This CommonJS entrypoint is selected by package exports for
// Node while the browser entrypoint remains the lazy ESM wrapper in index.js.
const createRuntime = require("../pkg/node/d2_runtime.cjs");
const { recoverRuntime } = require("./recover-runtime.cjs");
const { D2: BaseD2 } = require("./d2-api.cjs");
const { Worker } = require("node:worker_threads");
const { join } = require("node:path");

const camel = {
  layoutGraphJson: "layout_graph_json",
  layoutGraphJsonWithCancellation: "layout_graph_json_with_cancellation",
  layoutD2Json: "layout_d2_json",
  layoutD2JsonWithCancellation: "layout_d2_json_with_cancellation",
  routeEdgesD2Json: "route_edges_d2_json",
  routeEdgesD2JsonWithCancellation: "route_edges_d2_json_with_cancellation",
  compileD2Source: "compile_d2_source",
  compileD2TargetJson: "compile_d2_target_json",
  parseD2Source: "parse_d2_source",
  formatD2Source: "format_d2_source",
  layoutD2Source: "layout_d2_source",
  layoutD2SourceWithCancellation: "layout_d2_source_with_cancellation",
  renderD2Source: "render_d2_source",
  renderMarkdownHtml: "render_markdown_html",
  completeD2Source: "complete_d2_source",
  boardAtPositionJson: "board_at_position_json",
  referenceRangesJson: "reference_ranges_json",
  referenceRangesFilesJson: "reference_ranges_files_json",
  applySourceEditsJson: "apply_source_edits_json",
  createRootFieldJson: "create_root_field_json",
  setRootFieldJson: "set_root_field_json",
  deleteRootFieldJson: "delete_root_field_json",
  renameRootFieldJson: "rename_root_field_json",
  reconnectEdgeJson: "reconnect_edge_json",
  moveObjectJson: "move_object_json",
  encode: "encode",
  decode: "decode",
  version: "version",
  jsVersion: "js_version",
  getParentID: "getParentID",
  getCompletions: "getCompletions",
  getBoardAtPosition: "getBoardAtPosition",
  getRefRanges: "getRefRanges",
  getObjOrder: "getObjOrder",
  getELKGraph: "getELKGraph",
};

let runtime;
let initialRuntime = require("../pkg/node/d2_rs_wasm.cjs");
function currentRuntime() {
  if (!runtime) {
    runtime = recoverRuntime(initialRuntime || createRuntime(), () => { runtime = undefined; });
    initialRuntime = undefined;
  }
  return runtime;
}
const exported = { load: async () => exported, raw: async () => exported };
for (const name of Object.keys(currentRuntime())) {
  if (name === "WasmCancellationToken") {
    Object.defineProperty(exported, name, { enumerable: true, get: () => currentRuntime()[name] });
  } else if (typeof runtime[name] === "function") {
    exported[name] = (...args) => currentRuntime()[name](...args);
  }
}
exported.D2 = class D2 extends BaseD2 {
  constructor() {
    super(
      () => exported,
      async () => new Worker(join(__dirname, "d2-worker.node.mjs"), { type: "module" }),
    );
  }
};
for (const [name, target] of Object.entries(camel)) exported[name] = exported[target];
module.exports = exported;
