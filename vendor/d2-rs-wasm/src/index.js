// SPDX-License-Identifier: MPL-2.0

// The generated wasm-bindgen module is intentionally kept out of Git. Build
// it with `pnpm build`; this thin layer gives browser and Node callers one
// lazy-loading entry point while keeping all algorithms in Rust.
import d2Api from "../pkg/web/d2_api.js";
import { createRuntime } from "../pkg/web/d2_runtime.js";
import { recoverRuntime } from "../pkg/web/recover_runtime.js";
import * as initialRuntime from "../pkg/web/d2_rs_wasm.js";

let runtimePromise;
let initialInput;
let initialized = false;

/** Load and initialize the generated wasm-bindgen module once. */
export function init(input) {
  if (!runtimePromise) {
    if (input !== undefined) initialInput = input;
    runtimePromise = Promise.resolve(initialInput).then(async (module) => {
      const runtime = initialized ? createRuntime() : initialRuntime;
      if (module && Object.getPrototypeOf(module) === Object.prototype) module = await module.module_or_path;
      // Fetch bodies are consumable; retain an unread copy for a replacement.
      if ((typeof Response === "function" && module instanceof Response) ||
          (typeof Request === "function" && module instanceof Request)) module = module.clone();
      await runtime.default({ module_or_path: module });
      initialized = true;
      return recoverRuntime(runtime, () => { runtimePromise = undefined; });
    });
  }
  return runtimePromise;
}

/** Alias used by callers that prefer an explicit load verb. */
export const load = init;

/** High-level D2.js-compatible compile/render lifecycle facade. */
export class D2 extends d2Api.D2 {
  constructor() {
    super(
      () => init(),
      async () => new Worker(new URL("./d2-worker.browser.js", import.meta.url), { type: "module" }),
    );
  }
}

const forward = (name, hasOptionalOptions = false) => async (...args) => {
  if (hasOptionalOptions && args.length < 2) args.push("");
  return (await init())[name](...args);
};

export const layoutGraphJson = forward("layout_graph_json", true);
export const layoutGraphJsonWithCancellation = forward("layout_graph_json_with_cancellation");
export const layoutD2Json = forward("layout_d2_json", true);
export const layoutD2JsonWithCancellation = forward("layout_d2_json_with_cancellation");
export const routeEdgesD2Json = forward("route_edges_d2_json", true);
export const routeEdgesD2JsonWithCancellation = forward("route_edges_d2_json_with_cancellation");
export const compileD2Source = forward("compile_d2_source", true);
export const compileD2TargetJson = forward("compile_d2_target_json", true);
export const parseD2Source = forward("parse_d2_source", true);
export const formatD2Source = forward("format_d2_source", true);
export const layoutD2Source = forward("layout_d2_source", true);
export const layoutD2SourceWithCancellation = forward("layout_d2_source_with_cancellation");
export const renderD2Source = forward("render_d2_source", true);
export const renderMarkdownHtml = forward("render_markdown_html");
export const completeD2Source = forward("complete_d2_source", true);
export const boardAtPositionJson = forward("board_at_position_json", true);
export const referenceRangesJson = forward("reference_ranges_json");
export const referenceRangesFilesJson = forward("reference_ranges_files_json");
export const applySourceEditsJson = forward("apply_source_edits_json");
export const createRootFieldJson = forward("create_root_field_json");
export const setRootFieldJson = forward("set_root_field_json");
export const deleteRootFieldJson = forward("delete_root_field_json");
export const renameRootFieldJson = forward("rename_root_field_json");
export const reconnectEdgeJson = forward("reconnect_edge_json");
export const moveObjectJson = forward("move_object_json");
export const encode = forward("encode");
export const decode = forward("decode");
export const version = forward("version");
export const jsVersion = forward("js_version");
export const getParentID = forward("getParentID");
export const getCompletions = forward("getCompletions");
export const getBoardAtPosition = forward("getBoardAtPosition");
export const getRefRanges = forward("getRefRanges");
export const getObjOrder = forward("getObjOrder");
export const getELKGraph = forward("getELKGraph");

// Snake-case aliases preserve the names emitted by wasm-bindgen for clients
// that want the raw Rust-facing vocabulary without importing `./raw`.
export const layout_graph_json = layoutGraphJson;
export const layout_graph_json_with_cancellation = layoutGraphJsonWithCancellation;
export const layout_d2_json = layoutD2Json;
export const layout_d2_json_with_cancellation = layoutD2JsonWithCancellation;
export const route_edges_d2_json = routeEdgesD2Json;
export const route_edges_d2_json_with_cancellation = routeEdgesD2JsonWithCancellation;
export const compile_d2_source = compileD2Source;
export const compile_d2_target_json = compileD2TargetJson;
export const parse_d2_source = parseD2Source;
export const format_d2_source = formatD2Source;
export const layout_d2_source = layoutD2Source;
export const layout_d2_source_with_cancellation = layoutD2SourceWithCancellation;
export const render_d2_source = renderD2Source;
export const render_markdown_html = renderMarkdownHtml;
export const complete_d2_source = completeD2Source;
export const board_at_position_json = boardAtPositionJson;
export const reference_ranges_json = referenceRangesJson;
export const reference_ranges_files_json = referenceRangesFilesJson;
export const apply_source_edits_json = applySourceEditsJson;
export const create_root_field_json = createRootFieldJson;
export const set_root_field_json = setRootFieldJson;
export const delete_root_field_json = deleteRootFieldJson;
export const rename_root_field_json = renameRootFieldJson;
export const reconnect_edge_json = reconnectEdgeJson;
export const move_object_json = moveObjectJson;
export const js_version = jsVersion;

/** Return the generated module, including the WasmCancellationToken class. */
export async function raw() {
  return init();
}
