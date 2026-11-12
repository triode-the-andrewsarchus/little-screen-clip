// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! WebAssembly and JavaScript bindings for d2-rs: a Rust port of upstream D2's
//! `d2js` package, covering compile, layout, render and source-editing calls.
//!
//! The API takes and returns JSON strings, which keeps it portable across
//! browsers, Node.js and bundlers and keeps 64-bit seeds exact. It needs no
//! filesystem. The `oracle-fixtures` feature compiles tests that read
//! regenerated Go-oracle artifacts.

use base64::Engine as _;
use d2_rs_core::{
    CancellationToken,
    ast::{Child, Shared},
};
use d2_rs_editor::{
    MoveOptions, PositionEncoding, SourceEdit, SourcePosition, apply_edits, board_at_position,
    completion_items, create_root_field, delete_root_field, move_object_with_options,
    reconnect_edge, reference_ranges, reference_ranges_with_files, rename_root_field,
    set_root_field,
};
use d2_rs_elk::{ElkEdge, ElkGraph, ElkLabel, ElkNode, ElkPort};
use d2_rs_export::render_ascii_with_options;
use d2_rs_language::{ParseOptions, compiler, parse, parse_map_key};
use d2_rs_layout::{EngineKind, LayoutDispatcher};
use d2_rs_lib::{
    D2Options, layout_json, layout_json_with_cancellation, route_edges_json,
    route_edges_json_with_cancellation,
};
use d2_rs_render::RenderOptions;
use d2_rs_tala::{Engine, Graph, LayoutOptions};
use d2_rs_text::markdown::html::render_markdown_html as render_d2_markdown_html;
use d2_rs_text::measure::{
    TrueTypeFont, bundled_fuzzy_bubbles_bold, bundled_fuzzy_bubbles_regular,
    bundled_source_code_pro_bold, bundled_source_code_pro_italic, bundled_source_code_pro_regular,
    bundled_source_code_pro_semibold, bundled_source_sans_pro_bold, bundled_source_sans_pro_italic,
    bundled_source_sans_pro_regular, bundled_source_sans_pro_semibold,
};
use flate2::{Compression, write::DeflateEncoder};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::Write;
use std::str::FromStr;
use std::sync::OnceLock;
use wasm_bindgen::prelude::*;

const GET_OBJ_ORDER_DEPRECATION: &str = "d2.getObjOrder is deprecated and will be removed after one compatibility release; use d2oracle.GetObjOrder in Go integrations.";
const GET_ELK_GRAPH_DEPRECATION: &str = "d2.getELKGraph is deprecated and will be removed after one compatibility release; use d2.compile with options.layout set to \"elk\".";

static GET_OBJ_ORDER_DEPRECATION_WARNED: OnceLock<()> = OnceLock::new();
static GET_ELK_GRAPH_DEPRECATION_WARNED: OnceLock<()> = OnceLock::new();

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(inline_js = r#"
export function warnDeprecatedWasmCall(message) {
  try {
    const consoleObject = globalThis.console;
    if (consoleObject && typeof consoleObject.warn === "function") {
      consoleObject.warn(message);
    }
  } catch (_) {
    // A consumer may replace console.warn with a throwing function. A
    // deprecation notice must never change the compatibility call's result.
  }
}
"#)]
extern "C" {
    #[wasm_bindgen(js_name = warnDeprecatedWasmCall)]
    fn warn_deprecated_wasm_call(message: &str);
}

#[cfg(not(target_arch = "wasm32"))]
fn warn_deprecated_wasm_call(_message: &str) {}

fn warn_deprecated_once(warned: &OnceLock<()>, message: &'static str) {
    if warned.set(()).is_ok() {
        warn_deprecated_wasm_call(message);
    }
}

/// Lays out a serialized [`d2_rs_tala::Graph`] and returns a serialized
/// [`d2_rs_tala::LayoutResult`].
///
/// `options_json` has the same shape as [`LayoutOptions`], for example
/// `{"seeds":[1]}`. Passing an empty string uses the default options.
///
/// # Errors
///
/// Returns a JavaScript exception string when either JSON document is invalid
/// or the layout engine cannot produce geometry.
#[wasm_bindgen]
pub fn layout_graph_json(graph_json: &str, options_json: &str) -> Result<String, JsValue> {
    layout_graph_json_impl(graph_json, options_json).map_err(js_error)
}

/// A shared cooperative cancellation handle for calls made from a worker or
/// another host task. Cancellation is observed at TALA seed, stage, routing,
/// and work-guard boundaries; a synchronous call on the same JavaScript
/// thread cannot be interrupted until control returns to the host.
#[wasm_bindgen]
pub struct WasmCancellationToken {
    inner: CancellationToken,
}

#[wasm_bindgen]
impl WasmCancellationToken {
    /// Creates a token in the non-cancelled state.
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self {
            inner: CancellationToken::new(),
        }
    }

    /// Requests cancellation for calls sharing this token.
    pub fn cancel(&self) {
        self.inner.cancel();
    }

    /// Records deadline expiration, preserving whichever context cause occurred first.
    pub fn expire(&self) {
        self.inner.expire();
    }

    /// Returns whether cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.inner.is_cancelled()
    }
}

impl Default for WasmCancellationToken {
    fn default() -> Self {
        Self::new()
    }
}

/// Lays out a serialized graph while observing a shared cancellation token.
#[wasm_bindgen]
pub fn layout_graph_json_with_cancellation(
    graph_json: &str,
    options_json: &str,
    token: &WasmCancellationToken,
) -> Result<String, JsValue> {
    layout_graph_json_with_cancellation_impl(graph_json, options_json, &token.inner)
        .map_err(js_error)
}

/// Lays out a D2 serialized-graph JSON document and returns the same document
/// with object boxes, edge routes, and label positions updated.
///
/// `options_json` has the shape `{"seeds":[1,2,3]}`. Passing an empty string
/// uses the default options.
///
/// # Errors
///
/// Returns a JavaScript exception string when either JSON document is invalid
/// or the D2 adapter or layout engine rejects the graph.
#[wasm_bindgen]
pub fn layout_d2_json(input_json: &str, options_json: &str) -> Result<String, JsValue> {
    layout_d2_json_impl(input_json, options_json).map_err(js_error)
}

/// Lays out a serialized D2 graph while observing a shared cancellation token.
#[wasm_bindgen]
pub fn layout_d2_json_with_cancellation(
    input_json: &str,
    options_json: &str,
    token: &WasmCancellationToken,
) -> Result<String, JsValue> {
    layout_d2_json_with_cancellation_impl(input_json, options_json, &token.inner).map_err(js_error)
}

/// Routes the requested edges in a serialized D2 graph using the same
/// `g`/`gEdges` envelope used by the serialized graph adapter.
#[wasm_bindgen]
pub fn route_edges_d2_json(input_json: &str, options_json: &str) -> Result<String, JsValue> {
    route_edges_d2_json_impl(input_json, options_json).map_err(js_error)
}

/// Routes a serialized D2 graph while observing a shared cancellation token.
#[wasm_bindgen]
pub fn route_edges_d2_json_with_cancellation(
    input_json: &str,
    options_json: &str,
    token: &WasmCancellationToken,
) -> Result<String, JsValue> {
    route_edges_d2_json_with_cancellation_impl(input_json, options_json, &token.inner)
        .map_err(js_error)
}

mod public_compile_request;
mod public_render_options;
mod source_compile_error;
mod source_graph_metadata;
mod target_render;
use source_compile_error::SourceCompileError;

/// Compiles D2 source into the ordered intermediate-representation summary
/// used by the native compiler boundary.
///
/// The result is JSON with the same shape as `d2-rs compile`. Imports are
/// accepted from the optional in-memory `files` map; a browser binding never
/// reaches into the native filesystem.
#[wasm_bindgen]
pub fn compile_d2_source(source: &str, options_json: &str) -> Result<String, JsValue> {
    compile_d2_source_impl(source, options_json).map_err(js_error)
}

/// Compiles and lays out source while retaining the measured graph and
/// first-party style metadata needed by the high-level D2 target facade.
///
/// This is an additive binding: `layout_d2_source` keeps returning the small
/// layout result used by existing callers, while this function joins each
/// stable node/edge index with its source identity, shape, label, and style.
/// It keeps target projection in the Rust source boundary instead of asking a
/// JavaScript caller to reconstruct compiler semantics from the IR summary.
#[wasm_bindgen]
pub fn compile_d2_target_json(source: &str, options_json: &str) -> Result<String, JsValue> {
    compile_d2_target_json_impl(source, options_json).map_err(js_error)
}

/// Public D2.compile transport: compiler diagnostics are an ordered JSON array
/// in Error.message, as in d2wasm.Compile. Native target helpers retain plain text.
#[wasm_bindgen]
pub fn compile_d2_public_target_json(source: &str, options_json: &str) -> Result<String, JsValue> {
    compile_d2_target_json_with_policy(source, options_json, true)
        .map_err(|error| js_error(error.public_message()))
}

/// Decode the original CompileRequest JSON before source/option processing.
#[wasm_bindgen]
pub fn prepare_d2_public_compile_request_json(request_json: &str) -> Result<String, JsValue> {
    public_compile_request::prepare(request_json).map_err(js_error)
}

/// Parses D2 source and returns the serialized AST map.
///
/// `options_json` accepts `{"utf16_positions":true}` (or the `utf16` alias).
/// The parser retains D2's UTF-16 source-position mode while all parsing is
/// performed in memory, so this function is safe to call from a browser.
#[wasm_bindgen]
pub fn parse_d2_source(source: &str, options_json: &str) -> Result<String, JsValue> {
    parse_d2_source_impl(source, options_json).map_err(js_error)
}

/// Formats valid D2 source using the native formatter and returns UTF-8 text.
///
/// Formatting is intentionally a parse-then-format operation, matching the
/// native command boundary. Parse diagnostics are returned as a JavaScript
/// exception instead of being mixed into the formatted output.
#[wasm_bindgen]
pub fn format_d2_source(source: &str, options_json: &str) -> Result<String, JsValue> {
    format_d2_source_impl(source, options_json).map_err(js_error)
}

/// Parses, compiles, and lays out D2 source entirely in Rust/WASM.
///
/// `options_json` accepts the [`D2Options`] fields, including ordered `seeds`,
/// an optional `engine` (`tala`, `dagre`, or `elk`), plus the
/// `target` board path (for example `layers.architecture`), plus the
/// `utf16_positions` source option. `render_d2_source` additionally accepts
/// `pad`, `scale`, `theme` ID, `center`, and `sketch` options. Imports are
/// accepted when the caller supplies `files` and an optional virtual `path`;
/// browser bindings never read the host filesystem.
#[wasm_bindgen]
pub fn layout_d2_source(source: &str, options_json: &str) -> Result<String, JsValue> {
    layout_d2_source_impl(source, options_json).map_err(js_error)
}

/// Parses, compiles, and lays out source while observing a shared
/// cancellation token.
#[wasm_bindgen]
pub fn layout_d2_source_with_cancellation(
    source: &str,
    options_json: &str,
    token: &WasmCancellationToken,
) -> Result<String, JsValue> {
    layout_d2_source_with_cancellation_impl(source, options_json, &token.inner).map_err(js_error)
}

/// Renders D2 source to a semantic SVG document entirely in Rust/WASM.
///
/// This uses the same native source graph and SVG target as the `d2-rs`
/// executable. Asset loading and the full browser worker API are separate
/// layers; this call is synchronous and has no filesystem dependency.
#[wasm_bindgen]
pub fn render_d2_source(source: &str, options_json: &str) -> Result<String, JsValue> {
    render_d2_source_impl(source, options_json).map_err(js_error)
}

/// Render a serialized D2 target without source compilation or layout.
/// Options use D2.js render-option names; compile options are not inherited.
/// SVG uses the shared d2svg port; ASCII retains its native target artist.
#[wasm_bindgen]
pub fn render_d2_target_json(diagram_json: &str, options_json: &str) -> Result<String, JsValue> {
    render_d2_target_json_native(diagram_json, options_json).map_err(js_error)
}

/// The target-render boundary with native Rust errors, for hosts without a
/// JavaScript runtime. Uses the same request decoder and renderer as WASM.
pub fn render_d2_target_json_native(
    diagram_json: &str,
    options_json: &str,
) -> Result<String, String> {
    target_render::render(diagram_json, options_json)
}

/// Render an already-decoded request's diagram and options with the original
/// WASM data/error envelope. Successful SVG bytes use standard base64; errors
/// retain d2wasm.Render's message and status code. No JavaScript host is needed.
#[wasm_bindgen]
pub fn render_d2_target_response_json(diagram_json: &str, options_json: &str) -> String {
    target_render::response(diagram_json, options_json)
}

/// Parses a Markdown label and returns its deterministic HTML fragment.
///
/// The function has no filesystem or browser dependency and is useful to
/// callers that need D2's styled Markdown target independently of layout.
#[wasm_bindgen]
pub fn render_markdown_html(source: &str) -> String {
    render_d2_markdown_html(source.as_bytes())
}

/// Returns JSON completion items for a D2 source position.
///
/// `options_json` accepts `{"line":0,"column":12}` and an optional
/// `utf16_positions` (or `utf16`) flag. The helper is intentionally tolerant
/// of malformed source, as editor clients request completions while typing.
#[wasm_bindgen]
pub fn complete_d2_source(source: &str, options_json: &str) -> Result<String, JsValue> {
    complete_d2_source_impl(source, options_json).map_err(js_error)
}

/// Returns the board path containing a D2 source position as a JSON array.
///
/// `options_json` accepts `{"line":0,"column":0}` and the optional
/// `utf16_positions` (or `utf16`) flag. A malformed document returns the
/// parser diagnostic and partial board path in the exception text.
#[wasm_bindgen]
pub fn board_at_position_json(source: &str, options_json: &str) -> Result<String, JsValue> {
    board_at_position_json_impl(source, options_json).map_err(js_error)
}

/// Returns source and import ranges for a resolved field key as JSON.
#[wasm_bindgen]
pub fn reference_ranges_json(source: &str, key: &str) -> Result<String, JsValue> {
    reference_ranges_json_impl(source, key).map_err(js_error)
}

/// Returns source and import ranges from a virtual filesystem.
///
/// `request_json` has the shape `{"path":"index.d2","files":{"index.d2":"..."},
/// "board_path":["layer"],"key":"node"}`. This keeps browser callers
/// independent of native filesystem APIs while matching D2's board-aware
/// reference helper.
#[wasm_bindgen]
pub fn reference_ranges_files_json(request_json: &str) -> Result<String, JsValue> {
    reference_ranges_files_json_impl(request_json).map_err(js_error)
}

/// Applies non-overlapping source edits against one immutable document
/// snapshot and returns `{"source":"..."}`.
///
/// `request_json` has the shape
/// `{"source":"...","edits":[{"start":{"line":0,"column":0},
/// "end":{"line":0,"column":1},"replacement":"..."}],
/// "encoding":"utf8"}`. `encoding` may be `utf8` (the native D2
/// convention) or `utf16` (the browser/LSP convention). The ranges are
/// interpreted against the original source and are applied from the end of
/// the document toward the beginning.
#[wasm_bindgen]
pub fn apply_source_edits_json(request_json: &str) -> Result<String, JsValue> {
    apply_source_edits_json_impl(request_json).map_err(js_error)
}

/// Creates one direct field in the root map and returns the formatted source.
///
/// `request_json` has the shape `{"source":"...","key":"node"}`. The
/// operation is intentionally limited to root fields; nested boards, imports,
/// edges, and compiler-aware graph edits remain separate translation work.
#[wasm_bindgen]
pub fn create_root_field_json(request_json: &str) -> Result<String, JsValue> {
    create_root_field_json_impl(request_json).map_err(js_error)
}

/// Sets or creates one direct root field. The value is D2 syntax in the
/// request's optional `value` field.
#[wasm_bindgen]
pub fn set_root_field_json(request_json: &str) -> Result<String, JsValue> {
    set_root_field_json_impl(request_json).map_err(js_error)
}

/// Deletes one direct root field. Missing fields are a deterministic no-op.
#[wasm_bindgen]
pub fn delete_root_field_json(request_json: &str) -> Result<String, JsValue> {
    delete_root_field_json_impl(request_json).map_err(js_error)
}

/// Renames one direct root field, applying the editor's numeric suffix rule.
#[wasm_bindgen]
pub fn rename_root_field_json(request_json: &str) -> Result<String, JsValue> {
    rename_root_field_json_impl(request_json).map_err(js_error)
}

/// Reconnects one written single-edge key. The request accepts optional
/// `new_src`/`newSrc` and `new_dst`/`newDst` endpoint strings.
#[wasm_bindgen]
pub fn reconnect_edge_json(request_json: &str) -> Result<String, JsValue> {
    reconnect_edge_json_impl(request_json).map_err(js_error)
}

/// Moves an object through native compiler references, or changes an indexed
/// edge's arrows following D2's Move contract. Optional path, virtual files
/// and board names provide the compilation context; the result is recompiled.
#[wasm_bindgen]
pub fn move_object_json(request_json: &str) -> Result<String, JsValue> {
    move_object_json_impl(request_json).map_err(js_error)
}

/// Encodes a D2 script using the URL-safe compressed representation used by
/// D2's v0.9.0 raw WASM API. The result is a JSON object so callers can use
/// the same `{result: ...}` response shape as the original binding.
#[wasm_bindgen]
pub fn encode(script: Option<String>) -> String {
    let Some(script) = script else {
        return legacy_error("missing script argument", 400);
    };
    match encode_impl(&script) {
        Ok(encoded) => encoded,
        Err(error) => legacy_error(error, 500),
    }
}

/// Decodes a URL-safe compressed D2 script from the original raw WASM shape.
#[wasm_bindgen]
pub fn decode(encoded: Option<String>) -> String {
    let Some(encoded) = encoded else {
        return legacy_error("missing script argument", 400);
    };
    match decode_impl(&encoded) {
        Ok(decoded) => decoded,
        Err(error) => legacy_error(error, 500),
    }
}

/// Returns the pinned D2 compatibility version represented by this package.
#[wasm_bindgen]
pub fn version() -> String {
    "0.9.0".to_owned()
}

/// Returns the JavaScript binding build marker. Release automation can replace
/// this value when publishing a generated package, matching D2's `HEAD`
/// default for local builds.
#[wasm_bindgen]
pub fn js_version() -> String {
    "HEAD".to_owned()
}

/// Compatibility export for the raw v0.9.0 JavaScript API.  The old Go
/// binding returned a JSON envelope even for scalar helpers; keeping that
/// envelope here lets existing callers migrate to the Rust implementation
/// without changing their worker protocol.
#[wasm_bindgen(js_name = getParentID)]
pub fn get_parent_id(id: Option<String>) -> String {
    let Some(id) = id else {
        return legacy_error("missing id argument", 400);
    };
    match parse_map_key(id.as_bytes()) {
        Ok(map_key)
            if map_key
                .edges
                .as_ref()
                .is_some_and(|edges| !edges.is_empty()) =>
        {
            legacy_ok("")
        }
        Ok(map_key) => {
            let Some(path) = map_key.key.and_then(|path| path.read().string_ida()) else {
                return legacy_ok("");
            };
            let parent = if path.len() <= 1 {
                "root".to_owned()
            } else {
                path[..path.len() - 1].join(".")
            };
            legacy_ok(parent)
        }
        Err(error) => legacy_error(error.to_string(), 400),
    }
}

/// Compatibility export for `d2.getCompletions(text, line, column)`.
#[wasm_bindgen(js_name = getCompletions)]
pub fn get_completions(source: Option<String>, line: Option<u32>, column: Option<u32>) -> String {
    let (Some(source), Some(line), Some(column)) = (source, line, column) else {
        return legacy_error("missing required arguments", 400);
    };
    let items = completion_items(
        &source,
        line as usize,
        column as usize,
        PositionEncoding::Utf16,
    )
    .into_iter()
    .map(|item| {
        serde_json::json!({
            "label": item.label,
            "kind": item.kind as u8,
            "detail": item.detail,
            "insertText": item.insert_text,
        })
    })
    .collect::<Vec<_>>();
    legacy_ok(serde_json::json!({"items": items}))
}

/// Compatibility export for `d2.getBoardAtPosition(text, line, column)`.
#[wasm_bindgen(js_name = getBoardAtPosition)]
pub fn get_board_at_position(
    source: Option<String>,
    line: Option<u32>,
    column: Option<u32>,
) -> String {
    let (Some(source), Some(line), Some(column)) = (source, line, column) else {
        return legacy_error("missing required arguments", 400);
    };
    let result = board_at_position(
        &source,
        SourcePosition {
            line: line as usize,
            column: column as usize,
        },
        PositionEncoding::Utf16,
    );
    match result {
        Ok(path) => legacy_ok(serde_json::json!({"boardPath": path, "err": ""})),
        Err(error) => legacy_ok(serde_json::json!({
            "boardPath": error.path,
            "err": error.message,
        })),
    }
}

/// Compatibility export for the raw v0.9.0 reference-range helper.
#[wasm_bindgen(js_name = getRefRanges)]
pub fn get_ref_ranges(
    files_json: Option<String>,
    path: Option<String>,
    key: Option<String>,
    board_path_json: Option<String>,
) -> String {
    let (Some(files_json), Some(path), Some(key), Some(board_path_json)) =
        (files_json, path, key, board_path_json)
    else {
        return legacy_error("missing required arguments", 400);
    };
    let files = match serde_json::from_str::<HashMap<String, String>>(&files_json) {
        Ok(files) => files,
        Err(_) => return legacy_error("invalid fs argument", 400),
    };
    let board_path = match serde_json::from_str::<Vec<String>>(&board_path_json) {
        Ok(path) => path,
        Err(_) => return legacy_error("invalid boardPath argument", 400),
    };
    match reference_ranges_with_files(&path, &files, &board_path, &key) {
        Ok(ranges) => legacy_ok(serde_json::json!({
            "ranges": ranges.references,
            "importRanges": ranges.imports,
        })),
        Err(error) => legacy_error(error.to_string(), 500),
    }
}

/// Compatibility export for the deprecated `d2.getObjOrder` helper.
///
/// The source graph's insertion order is the same breadth-first hierarchy
/// order consumed by the Rust layout boundary.  This is deliberately a
/// diagnostic compatibility helper; it does not expose the external plugin
/// protocol.
#[wasm_bindgen(js_name = getObjOrder)]
pub fn get_obj_order(source: Option<String>) -> String {
    warn_deprecated_once(&GET_OBJ_ORDER_DEPRECATION_WARNED, GET_OBJ_ORDER_DEPRECATION);
    let Some(source) = source else {
        return legacy_error("missing dsl argument", 400);
    };
    let parsed = parse_source_map(&source, "", true);
    let map = match parsed {
        Ok(map) => map,
        Err(error) => return legacy_error(error, 400),
    };
    let (compiler, root) = match compiler::compile_ir_with_options(
        Shared::new(map),
        compiler::CompileOptions {
            import_opener: None,
            utf16_positions: true,
        },
    ) {
        Ok(value) => value,
        Err(error) => {
            let code = if matches!(error, compiler::CompileError::NilBoardBase) {
                500
            } else {
                400
            };
            return legacy_error(error.to_string(), code);
        }
    };
    let graph = match compiler::lower_graph(&compiler, root) {
        Ok(graph) => graph,
        Err(error) => return legacy_error(error.to_string(), 500),
    };
    let mut queue = graph
        .nodes()
        .filter_map(|(id, node)| node.parent.is_none().then_some(id))
        .collect::<VecDeque<_>>();
    let mut order = Vec::with_capacity(graph.nodes().len());
    while let Some(id) = queue.pop_front() {
        let Some(node) = graph.node(id) else {
            continue;
        };
        order.push(node.external_id.clone());
        for (child_id, child) in graph.nodes() {
            if child.parent == Some(id) {
                queue.push_back(child_id);
            }
        }
    }
    // d2oracle.GetObjOrder keeps its nil slice until the first append.
    legacy_ok(serde_json::json!({"order": (!order.is_empty()).then_some(order)}))
}

/// Compatibility export for the deprecated raw `d2.getELKGraph` helper.
///
/// This keeps the v0.9.0 pre-layout graph shape available to callers that use
/// the old editor bridge. It is built from the same measured Rust graph as
/// the in-process layout path; the external plugin protocol is not involved.
#[wasm_bindgen(js_name = getELKGraph)]
pub fn get_elk_graph(request_json: Option<String>) -> String {
    warn_deprecated_once(&GET_ELK_GRAPH_DEPRECATION_WARNED, GET_ELK_GRAPH_DEPRECATION);
    let Some(request_json) = request_json else {
        return legacy_error("missing JSON argument", 400);
    };
    let (files, input_path) = match public_compile_request::elk_input(&request_json) {
        Ok(input) => input,
        Err(error) => return legacy_error(error, 400),
    };
    let files: HashMap<_, _> = files.into_iter().collect();
    let source = &files[&input_path];
    let path = source_path(Some(&input_path), true);
    let map = match parse_source_map(source, &path, true) {
        Ok(map) => map,
        Err(error) => return legacy_error(error, 400),
    };
    let (compiler, root) = match compiler::compile_ir_with_options(
        Shared::new(map),
        compiler::CompileOptions {
            import_opener: source_import_opener(&files),
            utf16_positions: true,
        },
    ) {
        Ok(value) => value,
        Err(error) => {
            let code = if matches!(error, compiler::CompileError::NilBoardBase) {
                500
            } else {
                400
            };
            return legacy_error(error.to_string(), code);
        }
    };
    let measurer = match WasmFontGraphMeasurer::bundled() {
        Ok(measurer) => measurer,
        Err(error) => return legacy_error(error, 500),
    };
    let (graph, labels, styles) =
        match compiler::lower_graph_with_measurer_and_labels(&compiler, root, &measurer) {
            Ok(value) => value,
            Err(error) => return legacy_error(error.to_string(), 500),
        };
    legacy_ok(legacy_elk_graph(&graph, &labels, &styles))
}

fn legacy_elk_graph(
    graph: &Graph,
    labels: &BTreeMap<d2_rs_core::NodeId, String>,
    styles: &BTreeMap<d2_rs_core::NodeId, d2_rs_language::compiler::SourceNodeStyle>,
) -> ElkGraph {
    // ConvertGraph applies the same graph-wide preparation as Layout before
    // it serializes the pre-layout ELK document.  Keep those effective sizes
    // local to this compatibility response: the source graph remains the
    // caller's measured graph and is not mutated by the WASM boundary.
    let effective_sizes = legacy_elk_effective_sizes(graph);
    let direction = match graph.direction {
        d2_rs_core::Direction::Right => "RIGHT",
        d2_rs_core::Direction::Left => "LEFT",
        d2_rs_core::Direction::Up => "UP",
        d2_rs_core::Direction::Down => "DOWN",
    };
    let layout_options =
        legacy_elk_root_options(direction, legacy_elk_self_loop_spacing(graph, None));
    let children: Vec<ElkNode> = graph
        .nodes()
        .filter(|(_, node)| node.parent.is_none())
        .map(|(id, _)| legacy_elk_node(graph, labels, styles, &effective_sizes, id))
        .collect();
    let mut edge_counts = BTreeMap::<(Option<String>, String, String), usize>::new();
    let edges = graph
        .edges()
        .filter_map(|(id, edge)| {
            let source = graph.node(edge.source)?.external_id.clone();
            let target = graph.node(edge.target)?.external_id.clone();
            let parent = graph
                .node(edge.source)
                .and_then(|node| node.parent)
                .filter(|parent| {
                    graph.node(edge.target).and_then(|node| node.parent) == Some(*parent)
                })
                .and_then(|parent| graph.node(parent).map(|node| node.external_id.clone()));
            let source_local = parent
                .as_deref()
                .and_then(|prefix| source.strip_prefix(&format!("{prefix}.")))
                .unwrap_or(&source)
                .to_owned();
            let target_local = parent
                .as_deref()
                .and_then(|prefix| target.strip_prefix(&format!("{prefix}.")))
                .unwrap_or(&target)
                .to_owned();
            let pair = (parent.clone(), source_local.clone(), target_local.clone());
            let ordinal = edge_counts.entry(pair).or_default();
            let local_id = format!("({source_local} -> {target_local})[{ordinal}]");
            let edge_id = parent
                .map(|prefix| format!("{prefix}.{local_id}"))
                .unwrap_or(local_id);
            *ordinal += 1;
            let labels = graph
                .edge_label(id)
                .filter(|label| !label.text.is_empty())
                .map(|label| {
                    vec![ElkLabel {
                        id: None,
                        text: label.text.clone(),
                        x: 0.0,
                        y: 0.0,
                        width: label.size.width,
                        height: label.size.height,
                        layout_options: Some(serde_json::json!({
                            "elk.edgeLabels.inline": true,
                        })),
                    }]
                })
                .unwrap_or_default();
            let columns = graph.edge_table_columns(id);
            let source_node = graph.node(edge.source)?;
            let target_node = graph.node(edge.target)?;
            let source = legacy_elk_edge_endpoint(
                styles,
                edge.source,
                source_node,
                columns.source,
                id.0 as usize,
                true,
            );
            let target = legacy_elk_edge_endpoint(
                styles,
                edge.target,
                target_node,
                columns.target,
                id.0 as usize,
                false,
            );
            Some(ElkEdge {
                junction_points: Vec::new(),
                id: edge_id,
                sources: vec![source],
                targets: vec![target],
                sections: Vec::new(),
                container: String::new(),
                labels,
                layout_options: None,
            })
        })
        .collect();
    let mut result = ElkGraph {
        children_present: false,
        edges_present: false,
        x: None,
        y: None,
        width: None,
        height: None,
        id: String::new(),
        layout_options,
        children,
        edges,
    };
    // `legacy_elk_node` owns the recursive node tree, so dynamic ports need a
    // second pass over the edges to be appended to their corresponding node.
    for (edge_id, edge) in graph.edges() {
        let columns = graph.edge_table_columns(edge_id);
        if columns.source.is_none()
            && graph
                .node(edge.source)
                .is_some_and(|node| node.shape == d2_rs_core::ShapeKind::SqlTable)
        {
            let port = legacy_elk_dynamic_port(graph, edge.source, edge_id.0 as usize, true);
            legacy_elk_append_port(&mut result.children, graph, edge.source, port);
        }
        if columns.target.is_none()
            && graph
                .node(edge.target)
                .is_some_and(|node| node.shape == d2_rs_core::ShapeKind::SqlTable)
        {
            let port = legacy_elk_dynamic_port(graph, edge.target, edge_id.0 as usize, false);
            legacy_elk_append_port(&mut result.children, graph, edge.target, port);
        }
    }
    legacy_elk_distribute_dynamic_ports(&mut result.children);
    result
}

/// Reproduce d2elklayout.ConvertGraph's pre-serialization size preparation.
///
/// The Go adapter reserves 40px per incident port on the flow cross-axis for
/// nodes with at least two incoming or outgoing edges.  It also adds the
/// measured label height plus label padding when an icon shares the node.
/// Explicit dimensions remain fixed for the degree rule, matching D2's
/// `HeightAttr`/`WidthAttr` checks.
fn legacy_elk_effective_sizes(graph: &Graph) -> BTreeMap<d2_rs_core::NodeId, d2_rs_core::Size> {
    let mut degrees = BTreeMap::<d2_rs_core::NodeId, (usize, usize)>::new();
    for (_, edge) in graph.edges() {
        degrees.entry(edge.source).or_default().1 += 1;
        degrees.entry(edge.target).or_default().0 += 1;
    }

    graph
        .nodes()
        .map(|(id, node)| {
            let (incoming, outgoing) = degrees.get(&id).copied().unwrap_or_default();
            let mut size = node.size;
            if incoming >= 2 || outgoing >= 2 {
                let port_extent = incoming.max(outgoing) as f64 * 40.0;
                match graph.direction {
                    d2_rs_core::Direction::Right | d2_rs_core::Direction::Left => {
                        if !node.fixed_height {
                            size.height = size.height.max(port_extent);
                        }
                    }
                    d2_rs_core::Direction::Down | d2_rs_core::Direction::Up => {
                        if !node.fixed_width {
                            size.width = size.width.max(port_extent);
                        }
                    }
                }
            }
            if node.has_icon
                && !matches!(
                    node.shape,
                    d2_rs_core::ShapeKind::Text
                        | d2_rs_core::ShapeKind::Class
                        | d2_rs_core::ShapeKind::SqlTable
                        | d2_rs_core::ShapeKind::Code
                )
                && let Some(label_size) = node
                    .label_size
                    .filter(|size| size.width > 0.0 || size.height > 0.0)
            {
                size.height += label_size.height + 5.0;
            }
            if let Some(external) = node.external_label.filter(|label| label.reserve_space) {
                // d2graph.SpacingOpt uses label.PADDING (5px) on the side
                // occupied by an outside label before serializing ELK.
                match external.side {
                    d2_rs_core::ExternalSide::Top | d2_rs_core::ExternalSide::Bottom => {
                        size.height += external.size.height + 5.0;
                    }
                    d2_rs_core::ExternalSide::Left | d2_rs_core::ExternalSide::Right => {
                        size.width += external.size.width + 5.0;
                    }
                }
            }
            (id, size)
        })
        .collect()
}

fn legacy_elk_has_label(node: &d2_rs_core::Node) -> bool {
    !matches!(
        node.shape,
        d2_rs_core::ShapeKind::Text
            | d2_rs_core::ShapeKind::Class
            | d2_rs_core::ShapeKind::SqlTable
            | d2_rs_core::ShapeKind::Code
    ) && node
        .label_size
        .is_some_and(|size| size.width > 0.0 || size.height > 0.0)
}

fn legacy_elk_label_position(
    node: &d2_rs_core::Node,
    has_label: bool,
    has_icon: bool,
    has_children: bool,
) -> d2_rs_core::LabelPosition {
    if !has_label {
        return d2_rs_core::LabelPosition::Unset;
    }
    if node.label_position != d2_rs_core::LabelPosition::Unset {
        return node.label_position;
    }
    if has_children {
        d2_rs_core::LabelPosition::InsideTopCenter
    } else if node
        .external_label
        .is_some_and(|label| label.side == d2_rs_core::ExternalSide::Bottom)
    {
        d2_rs_core::LabelPosition::OutsideBottomCenter
    } else if has_icon {
        d2_rs_core::LabelPosition::InsideTopCenter
    } else {
        d2_rs_core::LabelPosition::InsideMiddleCenter
    }
}

fn legacy_elk_icon_position(
    node: &d2_rs_core::Node,
    has_children: bool,
) -> d2_rs_core::LabelPosition {
    if let Some(position) = node.icon_position
        && position != d2_rs_core::LabelPosition::Unset
    {
        return position;
    }
    if has_children {
        d2_rs_core::LabelPosition::InsideTopLeft
    } else if matches!(
        node.shape,
        d2_rs_core::ShapeKind::SqlTable
            | d2_rs_core::ShapeKind::Class
            | d2_rs_core::ShapeKind::Text
            | d2_rs_core::ShapeKind::Code
    ) {
        d2_rs_core::LabelPosition::OutsideTopLeft
    } else {
        d2_rs_core::LabelPosition::InsideMiddleCenter
    }
}

fn legacy_elk_container_padding(
    graph: &Graph,
    labels: &BTreeMap<d2_rs_core::NodeId, String>,
    node_id: d2_rs_core::NodeId,
) -> String {
    let Some(node) = graph.node(node_id) else {
        return "[top=50,left=50,bottom=50,right=50]".to_owned();
    };
    let has_children = graph
        .nodes()
        .any(|(_, child)| child.parent == Some(node_id));
    if !has_children {
        return "[top=50,left=50,bottom=50,right=50]".to_owned();
    }
    let has_label =
        labels.get(&node_id).is_some_and(|label| !label.is_empty()) && legacy_elk_has_label(node);
    let has_icon = node.has_icon && node.shape != d2_rs_core::ShapeKind::Image;
    let label_position = legacy_elk_label_position(node, has_label, has_icon, true);
    let icon_position = legacy_elk_icon_position(node, true);
    let label_size = node.label_size.unwrap_or_default();
    let label_height = label_size.height + 10.0;
    let label_width = label_size.width + 10.0;
    let icon_size = 64.0 + 10.0;
    let mut top: f64 = 50.0;
    let mut left: f64 = 50.0;
    let mut bottom: f64 = 50.0;
    let mut right: f64 = 50.0;
    match label_position {
        d2_rs_core::LabelPosition::InsideTopLeft
        | d2_rs_core::LabelPosition::InsideTopCenter
        | d2_rs_core::LabelPosition::InsideTopRight => top = top.max(label_height),
        d2_rs_core::LabelPosition::InsideBottomLeft
        | d2_rs_core::LabelPosition::InsideBottomCenter
        | d2_rs_core::LabelPosition::InsideBottomRight => bottom = bottom.max(label_height),
        d2_rs_core::LabelPosition::InsideMiddleLeft => left = left.max(label_width),
        d2_rs_core::LabelPosition::InsideMiddleRight => right = right.max(label_width),
        _ => {}
    }
    if has_icon {
        match icon_position {
            d2_rs_core::LabelPosition::InsideTopLeft
            | d2_rs_core::LabelPosition::InsideTopCenter
            | d2_rs_core::LabelPosition::InsideTopRight => top = top.max(icon_size),
            d2_rs_core::LabelPosition::InsideBottomLeft
            | d2_rs_core::LabelPosition::InsideBottomCenter
            | d2_rs_core::LabelPosition::InsideBottomRight => bottom = bottom.max(icon_size),
            d2_rs_core::LabelPosition::InsideMiddleLeft => left = left.max(icon_size),
            d2_rs_core::LabelPosition::InsideMiddleRight => right = right.max(icon_size),
            _ => {}
        }
    }
    format!(
        "[top={},left={},bottom={},right={}]",
        top.ceil() as i64,
        left.ceil() as i64,
        bottom.ceil() as i64,
        right.ceil() as i64
    )
}

fn legacy_elk_node(
    graph: &Graph,
    labels: &BTreeMap<d2_rs_core::NodeId, String>,
    styles: &BTreeMap<d2_rs_core::NodeId, d2_rs_language::compiler::SourceNodeStyle>,
    effective_sizes: &BTreeMap<d2_rs_core::NodeId, d2_rs_core::Size>,
    id: d2_rs_core::NodeId,
) -> ElkNode {
    let node = graph.node(id).expect("ELK node ID exists");
    let effective_size = effective_sizes.get(&id).copied().unwrap_or(node.size);
    let label_records = labels
        .get(&id)
        .filter(|_| node.shape != d2_rs_core::ShapeKind::SqlTable)
        .filter(|label| !label.is_empty())
        .map(|text| {
            let size = node.label_size.unwrap_or(node.size);
            vec![ElkLabel {
                id: None,
                text: text.clone(),
                x: 0.0,
                y: 0.0,
                width: size.width,
                height: size.height,
                layout_options: None,
            }]
        })
        .unwrap_or_default();
    let children: Vec<ElkNode> = graph
        .nodes()
        .filter(|(_, child)| child.parent == Some(id))
        .map(|(child_id, _)| legacy_elk_node(graph, labels, styles, effective_sizes, child_id))
        .collect();
    let table_columns = styles
        .get(&id)
        .map(|style| style.table_columns.as_slice())
        .unwrap_or(&[]);
    let table_column_count = graph.table_column_count(id).unwrap_or(0);
    let table_column_count = table_columns.len().max(table_column_count);
    let ports = if node.shape == d2_rs_core::ShapeKind::SqlTable {
        let col_height = effective_size.height / (table_column_count as f64 + 1.0);
        let (source_side, target_side) = match graph.direction {
            d2_rs_core::Direction::Left => ("WEST", "EAST"),
            _ => ("EAST", "WEST"),
        };
        let mut ports = Vec::with_capacity(table_column_count * 2);
        for index in 0..table_column_count {
            let column = table_columns.get(index).map(String::as_str).unwrap_or("");
            let y = (index as f64 + 1.0) * col_height + col_height / 2.0;
            ports.push(legacy_elk_column_port(
                &format!("{}.{}.src", node.external_id, column),
                y,
                source_side,
            ));
            ports.push(legacy_elk_column_port(
                &format!("{}.{}.dst", node.external_id, column),
                y,
                target_side,
            ));
        }
        ports
    } else {
        Vec::new()
    };
    let layout_options = Some(if children.is_empty() {
        let mut options = serde_json::json!({
            "elk.direction": "",
            "elk.layered.edgeRouting.selfLoopDistribution": "EQUALLY",
            "elk.spacing.nodeSelfLoop": 0,
        });
        if node.shape == d2_rs_core::ShapeKind::SqlTable {
            options["elk.portConstraints"] = serde_json::json!("FIXED_POS");
        }
        options
    } else {
        let (minimum_width, minimum_height) = match graph.direction {
            d2_rs_core::Direction::Right | d2_rs_core::Direction::Left => {
                (effective_size.width.ceil(), effective_size.height.ceil())
            }
            d2_rs_core::Direction::Down | d2_rs_core::Direction::Up => {
                (effective_size.height.ceil(), effective_size.width.ceil())
            }
        };
        let self_loop_spacing = legacy_elk_self_loop_spacing(graph, Some(id));
        let padding = legacy_elk_container_padding(graph, labels, id);
        serde_json::json!({
            "elk.spacing.edgeNode": 40,
            "elk.layered.nodePlacement.bk.fixedAlignment": "BALANCED",
            "elk.layered.thoroughness": 8,
            "spacing.edgeEdgeBetweenLayers": 50,
            "elk.direction": "",
            "elk.hierarchyHandling": "INCLUDE_CHILDREN",
            "elk.layered.considerModelOrder.strategy": "NONE",
            "elk.layered.cycleBreaking.strategy": "GREEDY",
            "elk.nodeSize.constraints": "MINIMUM_SIZE",
            "elk.contentAlignment": "H_CENTER V_CENTER",
            "spacing.nodeNodeBetweenLayers": 70,
            "spacing.edgeNodeBetweenLayers": 40,
            "elk.spacing.nodeSelfLoop": self_loop_spacing,
            "elk.nodeSize.minimum": format!("({minimum_width}, {minimum_height})"),
            "elk.padding": padding,
        })
    });
    ElkNode {
        edges: Vec::new(),
        id: node.external_id.clone(),
        x: 0.0,
        y: 0.0,
        width: effective_size.width,
        height: effective_size.height,
        children,
        ports,
        labels: label_records,
        layout_options,
    }
}

fn legacy_elk_column_port(id: &str, y: f64, side: &str) -> ElkPort {
    ElkPort {
        id: id.to_owned(),
        x: 0.0,
        y,
        width: 0.0,
        height: 0.0,
        layout_options: serde_json::json!({
            "elk.direction": "",
            "elk.port.side": side,
            "elk.spacing.nodeSelfLoop": 0,
        }),
    }
}

fn legacy_elk_edge_endpoint(
    styles: &BTreeMap<d2_rs_core::NodeId, d2_rs_language::compiler::SourceNodeStyle>,
    node_id: d2_rs_core::NodeId,
    node: &d2_rs_core::Node,
    column: Option<usize>,
    edge_index: usize,
    source: bool,
) -> String {
    if node.shape != d2_rs_core::ShapeKind::SqlTable {
        return node.external_id.clone();
    }
    if let Some(column) = column {
        let column_name = styles
            .get(&node_id)
            .and_then(|style| style.table_columns.get(column))
            .map(String::as_str)
            .unwrap_or("");
        return format!(
            "{}.{}.{}",
            node.external_id,
            column_name,
            if source { "src" } else { "dst" }
        );
    }
    format!(
        "{}.__root__.{}.{}",
        node.external_id,
        if source { "src" } else { "dst" },
        edge_index
    )
}

fn legacy_elk_dynamic_port(
    graph: &Graph,
    node_id: d2_rs_core::NodeId,
    edge_index: usize,
    source: bool,
) -> ElkPort {
    let node = graph.node(node_id).expect("ELK node ID exists");
    let side = if source {
        match graph.direction {
            d2_rs_core::Direction::Up => "NORTH",
            _ => "SOUTH",
        }
    } else {
        match graph.direction {
            d2_rs_core::Direction::Up => "SOUTH",
            _ => "NORTH",
        }
    };
    ElkPort {
        id: format!(
            "{}.__root__.{}.{}",
            node.external_id,
            if source { "src" } else { "dst" },
            edge_index
        ),
        x: 0.0,
        y: 0.0,
        width: 0.0,
        height: 0.0,
        layout_options: serde_json::json!({
            "elk.direction": "",
            "elk.port.side": side,
            "elk.spacing.nodeSelfLoop": 0,
        }),
    }
}

fn legacy_elk_append_port(
    nodes: &mut [ElkNode],
    graph: &Graph,
    node_id: d2_rs_core::NodeId,
    port: ElkPort,
) {
    let Some(node) = graph.node(node_id) else {
        return;
    };
    fn append_recursive(nodes: &mut [ElkNode], external_id: &str, port: &mut Option<ElkPort>) {
        for candidate in nodes {
            if candidate.id == external_id {
                if let Some(port) = port.take() {
                    candidate.ports.push(port);
                }
                return;
            }
            append_recursive(&mut candidate.children, external_id, port);
            if port.is_none() {
                return;
            }
        }
    }
    append_recursive(nodes, &node.external_id, &mut Some(port));
}

/// Match the Go converter's post-pass for table edges that attach to the
/// table as a whole. ELK spreads ports with the same side across the node's
/// width in edge order; fixed column ports keep their x coordinate at zero.
fn legacy_elk_distribute_dynamic_ports(nodes: &mut [ElkNode]) {
    for node in nodes {
        let mut side_counts = BTreeMap::<String, usize>::new();
        for port in &node.ports {
            if port.id.contains(".__root__.") {
                let side = port.layout_options["elk.port.side"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                *side_counts.entry(side).or_default() += 1;
            }
        }
        let mut side_indices = BTreeMap::<String, usize>::new();
        for port in &mut node.ports {
            if !port.id.contains(".__root__.") {
                continue;
            }
            let side = port.layout_options["elk.port.side"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            let index = side_indices.entry(side.clone()).or_default();
            let count = side_counts.get(&side).copied().unwrap_or(0);
            port.x = node.width * (*index as f64 + 1.0) / (count as f64 + 1.0);
            *index += 1;
        }
        legacy_elk_distribute_dynamic_ports(&mut node.children);
    }
}

fn legacy_elk_root_options(direction: &str, self_loop_spacing: f64) -> serde_json::Value {
    serde_json::json!({
        "elk.spacing.edgeNode": 40,
        "elk.layered.nodePlacement.bk.fixedAlignment": "BALANCED",
        "elk.layered.thoroughness": 8,
        "spacing.edgeEdgeBetweenLayers": 50,
        "elk.direction": direction,
        "elk.hierarchyHandling": "INCLUDE_CHILDREN",
        "elk.layered.considerModelOrder.strategy": "NODES_AND_EDGES",
        "elk.layered.cycleBreaking.strategy": "GREEDY_MODEL_ORDER",
        "elk.nodeSize.constraints": "MINIMUM_SIZE",
        "elk.contentAlignment": "H_CENTER V_CENTER",
        "elk.algorithm": "layered",
        "spacing.nodeNodeBetweenLayers": 70,
        "spacing.edgeNodeBetweenLayers": 40,
        "elk.spacing.nodeSelfLoop": self_loop_spacing,
    })
}

/// Match d2elklayout's graph-wide self-loop label adjustment. ELK's default
/// spacing is 50; a large label raises it to half the relevant label extent
/// plus five pixels. The relevant extent is width for vertical flow and
/// height for horizontal flow.
fn legacy_elk_self_loop_spacing(graph: &Graph, owner: Option<d2_rs_core::NodeId>) -> f64 {
    let vertical = matches!(
        graph.direction,
        d2_rs_core::Direction::Down | d2_rs_core::Direction::Up
    );
    let maximum = graph
        .edges()
        .filter_map(|(id, edge)| {
            (edge.source == edge.target
                && graph.node(edge.source).and_then(|node| node.parent) == owner)
                .then(|| graph.edge_label(id))
                .flatten()
        })
        .map(|label| {
            if vertical {
                label.size.width
            } else {
                label.size.height
            }
        })
        .fold(0.0_f64, f64::max);
    50.0_f64.max(maximum / 2.0 + 5.0)
}

fn legacy_ok<T: serde::Serialize>(data: T) -> String {
    serde_json::to_string(&serde_json::json!({"data": data}))
        .expect("legacy WASM response is serializable")
}

fn legacy_error(message: impl Into<String>, code: u16) -> String {
    let message = message.into();
    let code = if message.starts_with("panic recovered: ") {
        500
    } else {
        code
    };
    serde_json::to_string(&serde_json::json!({
        "error": {"message": message, "code": code}
    }))
    .expect("legacy WASM error response is serializable")
}

#[derive(Default, serde::Deserialize)]
struct SourceOptions {
    /// Virtual source path used for relative import resolution.
    #[serde(default)]
    path: Option<String>,
    /// Optional in-memory import files. Native filesystem access is never
    /// attempted by the WASM boundary.
    #[serde(default)]
    files: HashMap<String, String>,
    #[serde(default, alias = "utf16")]
    utf16_positions: bool,
}

#[derive(Default, serde::Deserialize)]
struct SourceLayoutOptions {
    #[serde(default)]
    isometric: Option<bool>,
    /// Normalized public response options; never passed to source layout/render.
    #[serde(default)]
    public_render_options: Option<serde_json::Map<String, serde_json::Value>>,
    /// Virtual source path used for relative import resolution.
    #[serde(default)]
    path: Option<String>,
    /// Optional in-memory import files. Native filesystem access is never
    /// attempted by the WASM boundary.
    #[serde(default)]
    files: HashMap<String, String>,
    #[serde(default)]
    seeds: Option<Vec<i64>>,
    #[serde(default)]
    max_concurrency: usize,
    /// Built-in engine selected for this source-layout call. D2 v0.9.0
    /// applies Dagre after source config when the caller leaves this unset.
    #[serde(default, alias = "layout")]
    engine: Option<String>,
    /// Optional alternating board wrapper/name path. `*` and an omitted value
    /// select the root board; a `.*` suffix follows D2's include-children
    /// spelling while selecting the named board itself for this single-board
    /// API.
    #[serde(default)]
    target: Option<String>,
    /// Optional output-canvas padding for source rendering.
    #[serde(default)]
    pad: Option<f64>,
    /// Optional output scale for source rendering.
    #[serde(default)]
    scale: Option<f64>,
    /// Optional salt used to isolate SVG IDs when embedding multiple copies.
    #[serde(default)]
    salt: Option<String>,
    /// Optional light theme ID from the pinned catalog for source rendering.
    #[serde(default, alias = "theme")]
    theme_id: Option<i64>,
    /// Optional dark theme ID for the prefers-color-scheme media rule.
    #[serde(default, alias = "dark_theme")]
    dark_theme_id: Option<i64>,
    /// Center the rendered SVG in its containing viewport. `None` preserves
    /// the source-config/default distinction so an explicit false wins.
    #[serde(default)]
    center: Option<bool>,
    /// Apply the deterministic seeded sketch overlay to rectangular shapes.
    #[serde(default)]
    sketch: Option<bool>,
    /// Optional light-theme color overrides keyed by D2 color code.
    #[serde(default)]
    theme_overrides: Option<BTreeMap<String, String>>,
    /// Optional dark-theme color overrides keyed by D2 color code.
    #[serde(default)]
    dark_theme_overrides: Option<BTreeMap<String, String>>,
    /// Optional icon/image bytes keyed by the source URL or path. Values are
    /// standard base64 strings so the JSON boundary remains text-only.
    #[serde(default)]
    asset_data: BTreeMap<String, String>,
    /// Render the source result as D2's text canvas instead of SVG.
    #[serde(default)]
    ascii: bool,
    /// ASCII charset mode (`extended` by default or `standard`).
    #[serde(default)]
    ascii_mode: Option<String>,
    /// Milliseconds between animated multi-board SVG frames.
    #[serde(default)]
    animate_interval: Option<u64>,
    /// Add the static export appendix for links and tooltips.
    #[serde(default)]
    force_appendix: bool,
    /// Omit the XML declaration on individual SVGs. Animation's Wrap always
    /// writes its own declaration, following the original API.
    #[serde(default)]
    no_xml_tag: bool,
    /// Optional standard-base64 custom font faces supplied by a JS caller.
    #[serde(default)]
    font_regular: Option<String>,
    #[serde(default)]
    font_italic: Option<String>,
    #[serde(default)]
    font_bold: Option<String>,
    #[serde(default)]
    font_semibold: Option<String>,
    #[serde(default, alias = "utf16")]
    utf16_positions: bool,
}

#[derive(Default, serde::Deserialize)]
struct CompletionOptions {
    #[serde(default)]
    line: usize,
    #[serde(default)]
    column: usize,
    #[serde(default, alias = "utf16")]
    utf16_positions: bool,
}

/// The browser bindings use the same embedded D2 faces as the native CLI.
/// Keeping this adapter in the WASM crate removes the old character-count
/// fallback, which made source layout depend on whether callers used Node or
/// the native executable.
struct WasmFontGraphMeasurer {
    regular: TrueTypeFont,
    bold: TrueTypeFont,
    italic: TrueTypeFont,
    semibold: TrueTypeFont,
    mono: TrueTypeFont,
    mono_bold: TrueTypeFont,
    mono_semibold: TrueTypeFont,
    mono_italic: TrueTypeFont,
}

impl WasmFontGraphMeasurer {
    fn bundled() -> Result<Self, String> {
        Self::bundled_with_sketch(false)
    }

    fn bundled_with_sketch(hand_drawn: bool) -> Result<Self, String> {
        Ok(Self {
            regular: if hand_drawn {
                bundled_fuzzy_bubbles_regular()
                    .map_err(|error| format!("could not parse Fuzzy Bubbles: {error}"))?
            } else {
                bundled_source_sans_pro_regular()
                    .map_err(|error| format!("could not parse Source Sans Pro: {error}"))?
            },
            bold: if hand_drawn {
                bundled_fuzzy_bubbles_bold()
                    .map_err(|error| format!("could not parse Fuzzy Bubbles Bold: {error}"))?
            } else {
                bundled_source_sans_pro_bold()
                    .map_err(|error| format!("could not parse Source Sans Pro Bold: {error}"))?
            },
            italic: if hand_drawn {
                bundled_fuzzy_bubbles_regular().map_err(|error| {
                    format!("could not parse Fuzzy Bubbles italic fallback: {error}")
                })?
            } else {
                bundled_source_sans_pro_italic()
                    .map_err(|error| format!("could not parse Source Sans Pro Italic: {error}"))?
            },
            semibold: if hand_drawn {
                bundled_fuzzy_bubbles_bold().map_err(|error| {
                    format!("could not parse Fuzzy Bubbles semibold fallback: {error}")
                })?
            } else {
                bundled_source_sans_pro_semibold()
                    .map_err(|error| format!("could not parse Source Sans Pro Semibold: {error}"))?
            },
            mono: bundled_source_code_pro_regular()
                .map_err(|error| format!("could not parse Source Code Pro: {error}"))?,
            mono_bold: bundled_source_code_pro_bold()
                .map_err(|error| format!("could not parse Source Code Pro Bold: {error}"))?,
            mono_semibold: bundled_source_code_pro_semibold()
                .map_err(|error| format!("could not parse Source Code Pro Semibold: {error}"))?,
            mono_italic: bundled_source_code_pro_italic()
                .map_err(|error| format!("could not parse Source Code Pro Italic: {error}"))?,
        })
    }

    fn from_options_with_sketch(
        options: &SourceLayoutOptions,
        hand_drawn: bool,
    ) -> Result<Self, String> {
        let has_custom = options.font_regular.is_some()
            || options.font_italic.is_some()
            || options.font_bold.is_some()
            || options.font_semibold.is_some();
        // AddFontFamily fills absent custom faces from SourceSansPro. The
        // default HandDrawn family applies only when no family was supplied.
        let mut fonts = Self::bundled_with_sketch(hand_drawn && !has_custom)?;
        let parse_custom =
            |name: &str, encoded: Option<&String>| -> Result<Option<TrueTypeFont>, String> {
                let Some(encoded) = encoded else {
                    return Ok(None);
                };
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(encoded.as_bytes())
                    .map_err(|error| format!("invalid base64 {name} font: {error}"))?;
                TrueTypeFont::parse(&bytes)
                    .map(Some)
                    .map_err(|error| format!("could not parse {name} font: {error}"))
            };
        if let Some(font) = parse_custom("regular", options.font_regular.as_ref())? {
            fonts.regular = font;
        }
        if let Some(font) = parse_custom("italic", options.font_italic.as_ref())? {
            fonts.italic = font;
        }
        if let Some(font) = parse_custom("bold", options.font_bold.as_ref())? {
            fonts.bold = font;
        }
        if let Some(font) = parse_custom("semibold", options.font_semibold.as_ref())? {
            fonts.semibold = font;
        }
        Ok(fonts)
    }

    fn styled(&self, bold: bool, italic: bool) -> &TrueTypeFont {
        if bold {
            &self.bold
        } else if italic {
            &self.italic
        } else {
            &self.regular
        }
    }

    fn mono_styled(&self, bold: bool, italic: bool) -> &TrueTypeFont {
        if bold {
            &self.mono_bold
        } else if italic {
            &self.mono_italic
        } else {
            &self.mono
        }
    }

    fn raw(
        font: &TrueTypeFont,
        fallback: &TrueTypeFont,
        label: &str,
        font_size: u32,
    ) -> d2_rs_core::Size {
        let metrics = font.measure_with_fallback(label, f64::from(font_size), fallback);
        d2_rs_core::Size {
            width: metrics.width.ceil(),
            height: metrics.height.ceil(),
        }
    }

    fn padded(raw: d2_rs_core::Size) -> d2_rs_core::Size {
        d2_rs_core::Size {
            width: (raw.width + 45.0).max(40.0),
            height: (raw.height + 45.0).max(24.0),
        }
    }

    /// The legacy D2 ruler measures the text nodes on either side of an
    /// unfetched image independently. Measuring the projected text as one
    /// string loses the side-bearing space that remains in that box model.
    fn markdown_dimensions(
        &self,
        source: &str,
        font_size: u32,
        bold: bool,
        italic: bool,
        mono_default: bool,
    ) -> d2_rs_core::Size {
        d2_rs_text::markdown_measure::MarkdownMeasurer {
            font: &self.regular,
            bold_font: &self.bold,
            italic_font: &self.italic,
            semibold_font: &self.semibold,
            mono_font: &self.mono,
            mono_bold_font: &self.mono_bold,
            mono_semibold_font: &self.mono_semibold,
            mono_italic_font: &self.mono_italic,
        }
        .markdown_dimensions(source, font_size, bold, italic, mono_default)
    }
}

impl compiler::GraphMeasurer for WasmFontGraphMeasurer {
    fn validate_markdown_label(&self, label: &str) -> Result<(), String> {
        d2_rs_text::markdown_measure::validate_markdown_label(label)
    }

    fn supports_shape_fitting(&self) -> bool {
        true
    }

    fn measure(&self, label: &str) -> d2_rs_core::Size {
        self.measure_at_size(label, 16)
    }

    fn measure_label(&self, label: &str) -> d2_rs_core::Size {
        self.measure_label_at_size(label, 16)
    }

    fn measure_at_size(&self, label: &str, font_size: u32) -> d2_rs_core::Size {
        self.measure_styled_at_size(label, font_size, false, false)
    }

    fn measure_styled_at_size(
        &self,
        label: &str,
        font_size: u32,
        bold: bool,
        italic: bool,
    ) -> d2_rs_core::Size {
        Self::padded(self.measure_styled_label_at_size(label, font_size, bold, italic))
    }

    fn measure_label_at_size(&self, label: &str, font_size: u32) -> d2_rs_core::Size {
        self.measure_styled_label_at_size(label, font_size, false, false)
    }

    fn measure_styled_label_at_size(
        &self,
        label: &str,
        font_size: u32,
        bold: bool,
        italic: bool,
    ) -> d2_rs_core::Size {
        Self::raw(self.styled(bold, italic), &self.mono, label, font_size)
    }

    fn measure_code_label_at_size(&self, label: &str, font_size: u32) -> d2_rs_core::Size {
        let metrics = self
            .mono
            .measure_d2_code_label(label, f64::from(font_size), &self.mono);
        d2_rs_core::Size {
            width: metrics.width.ceil(),
            height: metrics.height,
        }
    }

    fn measure_mono_label_at_size(&self, label: &str, font_size: u32) -> d2_rs_core::Size {
        Self::raw(&self.mono, &self.mono, label, font_size)
    }

    fn measure_mono_styled_label_at_size(
        &self,
        label: &str,
        font_size: u32,
        bold: bool,
        italic: bool,
    ) -> d2_rs_core::Size {
        Self::raw(self.mono_styled(bold, italic), &self.mono, label, font_size)
    }

    fn measure_markdown_label_at_size(&self, label: &str, font_size: u32) -> d2_rs_core::Size {
        self.markdown_dimensions(label, font_size, false, false, false)
    }
    fn measure_markdown_at_size(&self, label: &str, font_size: u32) -> d2_rs_core::Size {
        Self::padded(self.measure_markdown_label_at_size(label, font_size))
    }
    fn measure_styled_markdown_label_at_size(
        &self,
        label: &str,
        font_size: u32,
        bold: bool,
        italic: bool,
    ) -> d2_rs_core::Size {
        self.markdown_dimensions(label, font_size, bold, italic, false)
    }
    fn measure_styled_markdown_at_size(
        &self,
        label: &str,
        font_size: u32,
        bold: bool,
        italic: bool,
    ) -> d2_rs_core::Size {
        Self::padded(self.markdown_dimensions(label, font_size, bold, italic, false))
    }
    fn measure_mono_styled_markdown_label_at_size(
        &self,
        label: &str,
        font_size: u32,
        bold: bool,
        italic: bool,
    ) -> d2_rs_core::Size {
        self.markdown_dimensions(label, font_size, bold, italic, true)
    }
    fn measure_mono_styled_markdown_at_size(
        &self,
        label: &str,
        font_size: u32,
        bold: bool,
        italic: bool,
    ) -> d2_rs_core::Size {
        Self::padded(self.markdown_dimensions(label, font_size, bold, italic, true))
    }

    fn measure_latex_at_size(&self, label: &str, font_size: u32) -> d2_rs_core::Size {
        let Ok(metrics) = d2_rs_math::measure(label) else {
            return self.measure_at_size(label, font_size);
        };
        // Original Object.GetLabelSize uses intrinsic TeX dimensions at every font size.
        d2_rs_core::Size {
            width: (metrics.width + 45.0).ceil().max(40.0),
            height: (metrics.height + 45.0).ceil().max(24.0),
        }
    }

    fn measure_latex_label_at_size(&self, label: &str, font_size: u32) -> d2_rs_core::Size {
        let Ok(metrics) = d2_rs_math::measure(label) else {
            return self.measure_label_at_size(label, font_size);
        };
        // Original Object.GetLabelSize uses intrinsic TeX dimensions at every font size.
        d2_rs_core::Size {
            width: (metrics.width).ceil().max(1.0),
            height: (metrics.height).ceil().max(1.0),
        }
    }
}

fn layout_graph_json_impl(graph_json: &str, options_json: &str) -> Result<String, String> {
    let graph: Graph = serde_json::from_str(graph_json)
        .map_err(|error| format!(concat!("invalid Wef", "tan graph JSON: {}"), error))?;
    let options = parse_options::<LayoutOptions>(options_json, LayoutOptions::default)?;
    let result = Engine
        .layout(&graph, &options)
        .map_err(|error| format!("layout failed: {error}"))?;
    serde_json::to_string(&result)
        .map_err(|error| format!("could not encode layout result: {error}"))
}

fn layout_graph_json_with_cancellation_impl(
    graph_json: &str,
    options_json: &str,
    token: &CancellationToken,
) -> Result<String, String> {
    let graph: Graph = serde_json::from_str(graph_json)
        .map_err(|error| format!(concat!("invalid Wef", "tan graph JSON: {}"), error))?;
    let options = parse_options::<LayoutOptions>(options_json, LayoutOptions::default)?;
    let result = Engine
        .layout_with_cancellation(&graph, &options, token)
        .map_err(|error| format!("layout failed: {error}"))?;
    serde_json::to_string(&result)
        .map_err(|error| format!("could not encode layout result: {error}"))
}

fn layout_d2_json_impl(input_json: &str, options_json: &str) -> Result<String, String> {
    let options = parse_options::<D2Options>(options_json, D2Options::default)?;
    let output = layout_json(input_json.as_bytes(), &options)
        .map_err(|error| format!("D2 layout failed: {error}"))?;
    String::from_utf8(output).map_err(|error| format!("layout returned invalid UTF-8: {error}"))
}

fn layout_d2_json_with_cancellation_impl(
    input_json: &str,
    options_json: &str,
    token: &CancellationToken,
) -> Result<String, String> {
    let options = parse_options::<D2Options>(options_json, D2Options::default)?;
    let output = layout_json_with_cancellation(input_json.as_bytes(), &options, token)
        .map_err(|error| format!("D2 layout failed: {error}"))?;
    String::from_utf8(output).map_err(|error| format!("layout returned invalid UTF-8: {error}"))
}

fn route_edges_d2_json_impl(input_json: &str, options_json: &str) -> Result<String, String> {
    let options = parse_options::<D2Options>(options_json, D2Options::default)?;
    let output = route_edges_json(input_json.as_bytes(), &options)
        .map_err(|error| format!("D2 edge routing failed: {error}"))?;
    String::from_utf8(output).map_err(|error| format!("routing returned invalid UTF-8: {error}"))
}

fn route_edges_d2_json_with_cancellation_impl(
    input_json: &str,
    options_json: &str,
    token: &CancellationToken,
) -> Result<String, String> {
    let options = parse_options::<D2Options>(options_json, D2Options::default)?;
    let output = route_edges_json_with_cancellation(input_json.as_bytes(), &options, Some(token))
        .map_err(|error| format!("D2 edge routing failed: {error}"))?;
    String::from_utf8(output).map_err(|error| format!("routing returned invalid UTF-8: {error}"))
}

fn compile_d2_source_impl(source: &str, options_json: &str) -> Result<String, String> {
    let options = parse_options::<SourceOptions>(options_json, SourceOptions::default)?;
    let path = source_path(options.path.as_deref(), !options.files.is_empty());
    let map = parse_source_map(source, &path, options.utf16_positions)?;
    let (compiler, root) = compiler::compile_ir_with_options(
        Shared::new(map),
        compiler::CompileOptions {
            import_opener: source_import_opener(&options.files),
            utf16_positions: options.utf16_positions,
        },
    )
    .map_err(|error| error.to_string())?;
    serde_json::to_string(&compiler::summarize_ir(&compiler, root))
        .map_err(|error| format!("could not encode IR summary: {error}"))
}

fn compile_d2_target_json_impl(source: &str, options_json: &str) -> Result<String, String> {
    compile_d2_target_json_typed(source, options_json).map_err(|error| error.to_string())
}

fn compile_d2_target_json_typed(
    source: &str,
    options_json: &str,
) -> Result<String, SourceCompileError> {
    compile_d2_target_json_with_policy(source, options_json, false)
}

fn compile_d2_target_json_with_policy(
    source: &str,
    options_json: &str,
    public: bool,
) -> Result<String, SourceCompileError> {
    let root_layout = source_layout_with_policy(source, options_json, None, public)?;
    let options = parse_options::<SourceLayoutOptions>(options_json, SourceLayoutOptions::default)?;
    let ast = source_ast_raw(source, &options, options.target.as_deref(), true)?;
    // Board names, labels and the board tree only read the compiled IR, and
    // every board target compiles the same source, files and positions: share
    // one compile instead of recompiling the document for each query.
    let board_context = compile_source_context(source, &options)?;
    let root = target_metadata(root_layout, Some(ast), &options, &board_context)?;

    // d2wasm.Compile returns the complete board tree. A selected target is a
    // single-board request, so only enumerate children for the root request;
    // each child is laid out with the same options and explicit board target.
    let board_paths: Vec<String> = if options
        .target
        .as_deref()
        .is_none_or(|target| target.is_empty() || target == "*")
    {
        compiler::board_targets(&board_context.0, board_context.1)
            .into_iter()
            .map(|(path, _)| path)
            .collect()
    } else {
        Vec::new()
    };
    let mut boards = Vec::with_capacity(board_paths.len());
    if !board_paths.is_empty() {
        let mut child_options: serde_json::Value = if options_json.trim().is_empty() {
            serde_json::json!({})
        } else {
            serde_json::from_str(options_json)
                .map_err(|error| format!("invalid source options JSON: {error}"))?
        };
        for path in board_paths {
            child_options
                .as_object_mut()
                .ok_or_else(|| "source options must be a JSON object".to_owned())?
                .insert("target".to_owned(), serde_json::json!(path));
            let child_options_json = serde_json::to_string(&child_options)
                .map_err(|error| format!("could not encode board options: {error}"))?;
            let child_layout =
                source_layout_with_policy(source, &child_options_json, None, public)?;
            let child_ast = source_ast_raw(source, &options, Some(&path), false)?;
            let key = d2_rs_language::parse_key(path.as_bytes()).map_err(|e| e.to_string())?;
            let path_parts = key
                .string_ida()
                .ok_or_else(|| "board path has no components".to_owned())?;
            let path_boxes: Vec<_> = key
                .path
                .as_ref()
                .expect("parsed board path")
                .iter()
                .map(|slot| {
                    slot.read()
                        .as_ref()
                        .expect("board component")
                        .read()
                        .clone()
                })
                .collect();
            let parent_path = String::from_utf8(d2_rs_language::format::format_key_path(
                &path_boxes[..path_boxes.len() - 2],
            ))
            .map_err(|e| e.to_string())?;
            let child_source_options = parse_options::<SourceLayoutOptions>(
                &child_options_json,
                SourceLayoutOptions::default,
            )?;
            let graph = target_metadata(
                child_layout,
                Some(child_ast),
                &child_source_options,
                &board_context,
            )?;
            boards.push(TargetBoard {
                path_parts,
                parent_path,
                path,
                graph,
            });
        }
    }
    serde_json::to_string(&TargetResponse { root, boards }).map_err(|error| {
        SourceCompileError::Other(format!("could not encode target metadata: {error}"))
    })
}

fn compile_source_context(
    source: &str,
    options: &SourceLayoutOptions,
) -> Result<(compiler::Compiler, d2_rs_core::ir::NodeId), String> {
    let path = source_path(options.path.as_deref(), !options.files.is_empty());
    let map = parse_source_map(source, &path, options.utf16_positions)?;
    compiler::compile_ir_with_options(
        Shared::new(map),
        compiler::CompileOptions {
            import_opener: source_import_opener(&options.files),
            utf16_positions: options.utf16_positions,
        },
    )
    .map_err(|error| error.to_string())
}

fn source_ast_raw(
    source: &str,
    options: &SourceLayoutOptions,
    target: Option<&str>,
    format_selected: bool,
) -> Result<Box<serde_json::value::RawValue>, String> {
    // The AST is encoded where it is produced: later IR projections may share
    // its nodes, and the response embeds the same bytes twice (`ast` and
    // `source_graph.ast`) without building or cloning a JSON value tree.
    // d2compiler.Compile retains the parsed document AST. Only child boards
    // use d2ir.Map.AST(), which reconstructs their compiled semantic contents.
    let is_root = target.is_none_or(|target| target.is_empty() || target == "*");
    let path = source_path(options.path.as_deref(), !options.files.is_empty());
    let ast = Shared::new(parse_source_map(source, &path, options.utf16_positions)?);
    let (mut compiler, root) = compiler::compile_ir_with_options(
        ast.clone(),
        compiler::CompileOptions {
            import_opener: source_import_opener(&options.files),
            utf16_positions: options.utf16_positions,
        },
    )
    .map_err(|e| e.to_string())?;
    let map = match target {
        Some(target) if !target.is_empty() && target != "*" => {
            let target = target.strip_suffix(".*").unwrap_or(target);
            compiler::resolve_board_target(&compiler, root, target)
                .map_err(|error| error.to_string())?
        }
        _ => root,
    };
    // compileBoard copies the IR before constructing child Graph.ASTs.
    // Field names stay shared with the parsed AST; scalar values are cloned.
    let map = if matches!(compiler.arena.node(map), d2_rs_core::ir::Node::Field(_))
        && compiler.arena.map(map).is_none()
    {
        // compileBoardsField gives a mapless board an empty graph AST.
        compiler.arena.new_root()
    } else if format_selected {
        map
    } else {
        let parent = compiler.arena.parent(map);
        compiler.arena.copy(map, parent)
    };
    if is_root || !format_selected {
        // compileConfig formats array scalars through their actual AST nodes.
        // The formatter's source-spelling caches are visible on Graph.AST.
        compiler::extract_config_checked(&compiler, root).map_err(|e| {
            e.iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        })?;
        // d2wasm.Compile formats g.AST for the returned FS before encoding
        // the graph; formatting also populates quoted-string spelling caches.
        d2_rs_language::format::format(&Child::Map(ast.clone()));
    }
    if is_root {
        return serde_json::value::to_raw_value(&*ast.read())
            .map_err(|error| format!("could not encode compiled AST: {error}"));
    }
    let ast = compiler
        .arena
        .ast(map)
        .ok_or_else(|| "compiled graph has no AST projection".to_owned())?;
    // d2wasm.Compile formats only g.AST for the returned FS. Enumerated
    // child graphs retain their unformatted d2ir.Map.AST() projections.
    // Field.Copy shares names with the root AST, whereas Scalar.Copy clones
    // values: formatting the parsed root above preserves that distinction.
    if format_selected {
        d2_rs_language::format::format(&ast);
    }
    serde_json::value::to_raw_value(&ast)
        .map_err(|error| format!("could not encode compiled AST: {error}"))
}

fn source_board_targets(
    source: &str,
    options: &SourceLayoutOptions,
) -> Result<Vec<String>, String> {
    let (compiler, root) = compile_source_context(source, options)?;
    Ok(compiler::board_targets(&compiler, root)
        .into_iter()
        .map(|(path, _)| path)
        .collect())
}

/// Returns the synthetic target-root label used by d2exporter for a selected
/// board. Child Diagram values use their board name when no explicit label is
/// present, while the document root has an empty synthetic label.
fn source_board_hash_metadata(
    source: &str,
    options: &SourceLayoutOptions,
) -> Result<(Option<String>, bool), String> {
    let (_, label, is_root) = source_board_metadata(source, options)?;
    Ok((label, is_root))
}

fn source_board_metadata(
    source: &str,
    options: &SourceLayoutOptions,
) -> Result<(String, Option<String>, bool), String> {
    let (compiler, root) = compile_source_context(source, options)?;
    source_board_metadata_in(&compiler, root, options)
}

fn source_board_metadata_in(
    compiler: &compiler::Compiler,
    root: d2_rs_core::ir::NodeId,
    options: &SourceLayoutOptions,
) -> Result<(String, Option<String>, bool), String> {
    let Some(target) = options
        .target
        .as_deref()
        .filter(|target| !target.is_empty() && *target != "*")
    else {
        return Ok((String::new(), compiler::board_label(compiler, root), true));
    };
    let target = target.strip_suffix(".*").unwrap_or(target);
    let board =
        compiler::resolve_board_target(compiler, root, target).map_err(|e| e.to_string())?;
    let field = match compiler.arena.node(board) {
        d2_rs_core::ir::Node::Field(_) => Some(board),
        _ => compiler.arena.parent_field(board),
    };
    let name = field
        .and_then(|field| match compiler.arena.node(field) {
            d2_rs_core::ir::Node::Field(field) => Some(field.name.scalar_string().to_owned()),
            _ => None,
        })
        .unwrap_or_default();
    let label = compiler::board_label(compiler, board).unwrap_or_else(|| name.clone());
    Ok((name, Some(label), false))
}

/// One board of the public compile target, owned so that it serializes in a
/// single pass. Its key order is the insertion order of the former
/// `serde_json::json!` projection (serde_json `preserve_order`), and every
/// borrowed field serializes to the bytes its `serde_json::Value` did.
struct TargetMetadata {
    graph: Graph,
    layout: d2_rs_tala::LayoutResult,
    labels: BTreeMap<d2_rs_core::NodeId, String>,
    styles: BTreeMap<d2_rs_core::NodeId, compiler::SourceNodeStyle>,
    config: Option<compiler::DiagramConfig>,
    /// `target_shape` of each node, by node index; `diagram.shapes` lists the
    /// same records in `object_order`.
    node_shapes: Vec<serde_json::Value>,
    object_order: Vec<d2_rs_core::NodeId>,
    board_name: String,
    font_family: &'static str,
    target_root: serde_json::Value,
    target_connections: serde_json::Value,
    legend: Option<serde_json::Value>,
    diagram_config: Option<serde_json::Value>,
    source_graph_root: serde_json::Value,
    source_graph_theme: Option<serde_json::Value>,
    source_graph_data: BTreeMap<String, serde_json::Value>,
    source_graph: source_graph_metadata::GraphParts,
    source_render_options: Option<serde_json::Value>,
    ast: Option<Box<serde_json::value::RawValue>>,
}

#[derive(serde::Serialize)]
struct TargetBoard {
    path_parts: Vec<String>,
    parent_path: String,
    path: String,
    graph: TargetMetadata,
}

/// The root board's metadata followed by its child boards.
struct TargetResponse {
    root: TargetMetadata,
    boards: Vec<TargetBoard>,
}

fn target_metadata(
    (graph, layout, labels, styles, config): SourceLayout,
    ast: Option<Box<serde_json::value::RawValue>>,
    options: &SourceLayoutOptions,
    (board_compiler, board_root): &(compiler::Compiler, d2_rs_core::ir::NodeId),
) -> Result<TargetMetadata, String> {
    let node_shapes = (0..graph.nodes.len())
        .map(|index| {
            let id = d2_rs_core::NodeId(index as u32);
            d2_rs_render::target_shape_metadata(&graph, &layout, &labels, &styles, id)
                .map_err(|error| format!("could not encode target shape: {error}"))?
                .ok_or_else(|| "missing target shape".to_owned())
        })
        .collect::<Result<Vec<_>, String>>()?;
    let (board_name, root_label, is_root) =
        source_board_metadata_in(board_compiler, *board_root, options)?;
    let source_config = config.clone().unwrap_or_default();
    let render_options = RenderOptions {
        target_root_label: root_label,
        theme_id: options.theme_id.or(source_config.theme_id),
        dark_theme_id: options.dark_theme_id.or(source_config.dark_theme_id),
        ..Default::default()
    };
    let sketch = options.sketch.or(source_config.sketch).unwrap_or(false);
    let target_root =
        d2_rs_render::target_root_metadata(&graph, &render_options).map_err(|e| e.to_string())?;
    let target_connections = d2_rs_render::target_connections_metadata(&graph, &layout, &styles)
        .map_err(|e| e.to_string())?;
    let primary_mono = render_options
        .theme_id
        .and_then(d2_rs_core::theme::palette)
        .is_some_and(|p| p.special_rules.mono);
    // target_shapes_metadata is the target_shape of each ID in this order;
    // every node has one (checked above), so share those records.
    let object_order = d2_rs_render::source_graph_object_order(&graph);
    let custom_family = options.font_regular.is_some()
        || options.font_italic.is_some()
        || options.font_bold.is_some()
        || options.font_semibold.is_some();
    let font_family = if primary_mono {
        "SourceCodePro"
    } else if custom_family {
        "custom"
    } else if sketch {
        "HandDrawn"
    } else {
        "SourceSansPro"
    };
    let legend = match graph
        .legend
        .as_ref()
        .and_then(|legend| legend.source_graph.as_deref())
    {
        Some(legend) => d2_rs_render::target_legend_metadata(legend, render_options.theme_id)
            .map_err(|e| e.to_string())?,
        None => None,
    };
    let diagram_config = if is_root {
        let mut value =
            d2_rs_render::target_config_metadata(config.as_ref(), &render_options, sketch)
                .map_err(|e| e.to_string())?;
        // d2lib/d2.go:109–135: resolved false is omitted from the hashed
        // config, while the public RenderOptions always carries a bool.
        if options
            .isometric
            .or(source_config.isometric)
            .unwrap_or(false)
        {
            value["isometric"] = true.into();
        }
        Some(value)
    } else {
        None
    };
    let theme_id = render_options.theme_id.unwrap_or(0);
    let source_graph_root = source_graph_metadata::root(&graph, &layout);
    let source_graph_theme = source_graph_metadata::theme(theme_id);
    let source_graph_data = source_config.graph_data();
    let mut source_graph = source_graph_metadata::graph(&graph, &layout, &labels, is_root)?;
    if let Some(ast) = &ast
        && ast.get().contains("\"single_quoted_string\"")
    {
        let formatted_ast: serde_json::Value = serde_json::from_str(ast.get())
            .map_err(|error| format!("could not decode compiled AST: {error}"))?;
        source_graph_metadata::apply_formatted_reference_spellings(
            &mut source_graph,
            &formatted_ast,
        );
    }
    let source_render_options = options
        .public_render_options
        .as_ref()
        .map(|public_options| public_render_options::project(public_options, config.as_ref()));
    Ok(TargetMetadata {
        graph,
        layout,
        labels,
        styles,
        config,
        node_shapes,
        object_order,
        board_name,
        font_family,
        target_root,
        target_connections,
        legend,
        diagram_config,
        source_graph_root,
        source_graph_theme,
        source_graph_data,
        source_graph,
        source_render_options,
        ast,
    })
}

impl TargetMetadata {
    fn serialize_entries<M: serde::ser::SerializeMap>(&self, map: &mut M) -> Result<(), M::Error> {
        let graph = &self.graph;
        map.serialize_entry("nodes", &TargetNodes(self))?;
        map.serialize_entry("edges", &TargetEdges(graph))?;
        map.serialize_entry(
            "source_graph_object_indices",
            &SerializeIter(|| self.object_order.iter().map(|id| id.0)),
        )?;
        map.serialize_entry("source_graph_root", &self.source_graph_root)?;
        map.serialize_entry("source_graph_name", &self.board_name)?;
        map.serialize_entry("source_graph_theme", &self.source_graph_theme)?;
        map.serialize_entry("source_graph_data", &self.source_graph_data)?;
        map.serialize_entry("layout", &self.layout)?;
        map.serialize_entry("config", &self.config)?;
        map.serialize_entry("direction", &graph.direction)?;
        map.serialize_entry("root_sequence_diagram", &graph.root_sequence_diagram)?;
        map.serialize_entry("diagram", &TargetDiagramMetadata(self))?;
        map.serialize_entry("source_graph", &TargetSourceGraph(self))?;
        if let Some(value) = &self.source_render_options {
            map.serialize_entry("source_render_options", value)?;
        }
        if let Some(ast) = &self.ast {
            map.serialize_entry("ast", ast)?;
        }
        Ok(())
    }
}

impl serde::Serialize for TargetMetadata {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap as _;
        let mut map = serializer.serialize_map(None)?;
        self.serialize_entries(&mut map)?;
        map.end()
    }
}

impl serde::Serialize for TargetResponse {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap as _;
        let mut map = serializer.serialize_map(None)?;
        self.root.serialize_entries(&mut map)?;
        map.serialize_entry("boards", &self.boards)?;
        map.end()
    }
}

/// Serializes the sequence produced by a re-runnable iterator factory.
struct SerializeIter<F>(F);

impl<F, I> serde::Serialize for SerializeIter<F>
where
    F: Fn() -> I,
    I: IntoIterator,
    I::Item: serde::Serialize,
{
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq((self.0)())
    }
}

#[derive(serde::Serialize)]
struct TargetNodeEntry<'a> {
    target_shape: &'a serde_json::Value,
    index: usize,
    id: &'a str,
    parent: Option<u32>,
    label: Option<&'a String>,
    node: &'a d2_rs_core::Node,
    style: &'a compiler::SourceNodeStyle,
}

struct TargetNodes<'a>(&'a TargetMetadata);

impl serde::Serialize for TargetNodes<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let metadata = self.0;
        let default_style = compiler::SourceNodeStyle::default();
        serializer.collect_seq(
            metadata
                .graph
                .nodes
                .iter()
                .enumerate()
                .map(|(index, node)| {
                    let id = d2_rs_core::NodeId(index as u32);
                    TargetNodeEntry {
                        target_shape: &metadata.node_shapes[index],
                        index,
                        id: &node.external_id,
                        parent: node.parent.map(|parent| parent.0),
                        label: metadata.labels.get(&id),
                        node,
                        style: metadata.styles.get(&id).unwrap_or(&default_style),
                    }
                }),
        )
    }
}

#[derive(serde::Serialize)]
struct TargetEdgeEntry<'a> {
    index: usize,
    source_index: Option<usize>,
    abs_id: Option<String>,
    source: u32,
    target: u32,
    arrows: d2_rs_core::EdgeArrows,
    arrowheads: std::borrow::Cow<'a, d2_rs_core::EdgeArrowheads>,
    label: Option<&'a d2_rs_core::EdgeLabel>,
    style: std::borrow::Cow<'a, d2_rs_core::EdgeStyle>,
    link: Option<&'a String>,
    tooltip: Option<&'a String>,
    icon: Option<&'a String>,
    icon_position: Option<&'a d2_rs_core::LabelPosition>,
    table_columns: d2_rs_core::EdgeTableColumns,
    is_curve: bool,
}

struct TargetEdges<'a>(&'a Graph);

impl serde::Serialize for TargetEdges<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use std::borrow::Cow;
        let graph = self.0;
        fn present<T>(values: &[Option<T>], index: usize) -> Option<&T> {
            values.get(index).and_then(Option::as_ref)
        }
        serializer.collect_seq(graph.edges.iter().enumerate().map(|(index, edge)| {
            let id = d2_rs_core::EdgeId(index as u32);
            TargetEdgeEntry {
                index,
                source_index: graph.edge_index(id),
                abs_id: d2_rs_core::edge_abs_id(graph, id),
                source: edge.source.0,
                target: edge.target.0,
                arrows: graph.edge_arrows.get(index).copied().unwrap_or_default(),
                arrowheads: graph
                    .edge_arrowheads
                    .get(index)
                    .map_or_else(|| Cow::Owned(Default::default()), Cow::Borrowed),
                label: present(&graph.edge_labels, index),
                style: graph
                    .edge_styles
                    .get(index)
                    .map_or_else(|| Cow::Owned(Default::default()), Cow::Borrowed),
                link: present(&graph.edge_links, index),
                tooltip: present(&graph.edge_tooltips, index),
                icon: present(&graph.edge_icons, index),
                icon_position: present(&graph.edge_icon_positions, index),
                table_columns: graph
                    .edge_table_columns
                    .get(index)
                    .copied()
                    .unwrap_or_default(),
                is_curve: graph.edge_is_curves.get(index).copied().unwrap_or(false),
            }
        }))
    }
}

struct TargetDiagramMetadata<'a>(&'a TargetMetadata);

impl serde::Serialize for TargetDiagramMetadata<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap as _;
        let metadata = self.0;
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("name", &metadata.board_name)?;
        map.serialize_entry("isFolderOnly", &metadata.graph.nodes.is_empty())?;
        map.serialize_entry("fontFamily", metadata.font_family)?;
        map.serialize_entry("monoFontFamily", "SourceCodePro")?;
        map.serialize_entry("root", &metadata.target_root)?;
        map.serialize_entry("connections", &metadata.target_connections)?;
        map.serialize_entry(
            "shapes",
            &SerializeIter(|| {
                metadata
                    .object_order
                    .iter()
                    .map(|id| &metadata.node_shapes[id.0 as usize])
            }),
        )?;
        if let Some(legend) = &metadata.legend {
            map.serialize_entry("legend", legend)?;
        }
        if let Some(config) = &metadata.diagram_config {
            map.serialize_entry("config", config)?;
        }
        map.end()
    }
}

struct TargetSourceGraph<'a>(&'a TargetMetadata);

impl serde::Serialize for TargetSourceGraph<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap as _;
        let metadata = self.0;
        let parts = &metadata.source_graph;
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("name", &metadata.board_name)?;
        map.serialize_entry("isFolderOnly", &metadata.graph.nodes.is_empty())?;
        map.serialize_entry("ast", &metadata.ast)?;
        map.serialize_entry("root", &metadata.source_graph_root)?;
        map.serialize_entry("objects", &parts.objects)?;
        map.serialize_entry("edges", &parts.edges)?;
        if let Some(legend) = &parts.legend {
            map.serialize_entry("legend", legend)?;
        }
        if let Some(theme) = &metadata.source_graph_theme {
            map.serialize_entry("theme", theme)?;
        }
        if !metadata.source_graph_data.is_empty() {
            map.serialize_entry("data", &metadata.source_graph_data)?;
        }
        map.end()
    }
}

fn parse_source_map(
    source: &str,
    path: &str,
    utf16_positions: bool,
) -> Result<d2_rs_core::ast::Map, String> {
    parse_source_map_typed(source, path, utf16_positions).map_err(|error| error.to_string())
}

fn parse_source_map_typed(
    source: &str,
    path: &str,
    utf16_positions: bool,
) -> Result<d2_rs_core::ast::Map, SourceCompileError> {
    let parsed = parse(
        path.to_owned(),
        source.as_bytes(),
        Some(ParseOptions {
            utf16_positions,
            errors: None,
        }),
    );
    if let Some(error) = parsed.error {
        return Err(error.into());
    }
    Ok(parsed.map)
}

fn source_path(path: Option<&str>, has_files: bool) -> String {
    path.map_or_else(
        || {
            if has_files {
                "index".to_owned()
            } else {
                "-".to_owned()
            }
        },
        ToOwned::to_owned,
    )
}

struct SourceMemoryDirectory;
impl d2_rs_language::stream::ByteReader for SourceMemoryDirectory {
    fn read_bytes(&mut self, _: &mut [u8]) -> (usize, Option<d2_rs_language::stream::ReadError>) {
        (
            0,
            Some(d2_rs_language::stream::ReadError::Other(
                "cannot read a directory".into(),
            )),
        )
    }
}

fn clean_source_memory_path(path: &str) -> String {
    if path.is_empty() {
        return ".".into();
    }
    d2_rs_core::ast::Import {
        path: d2_rs_core::ast::KeyPath::from_strings([path]).path,
        ..Default::default()
    }
    .path_with_pre()
}

fn source_import_opener(files: &HashMap<String, String>) -> Option<compiler::ImportOpener> {
    // d2wasm uses lib/memfs: cleaned names, immediate directory entries and
    // bare fs.ErrNotExist. An empty virtual FS still supports failed opens.
    let mut entries = HashMap::new();
    for (path, source) in files {
        let path = clean_source_memory_path(path);
        let dir = path
            .rfind('/')
            .map_or(".", |index| if index == 0 { "/" } else { &path[..index] })
            .to_owned();
        entries.insert(dir, None);
        entries.insert(path, Some(source.clone()));
    }
    Some(Box::new(move |path| {
        match entries.get(&clean_source_memory_path(path)) {
            Some(Some(source)) => Ok(Box::new(std::io::Cursor::new(source.as_bytes().to_vec()))
                as Box<dyn d2_rs_language::stream::ByteReader>),
            Some(None) => {
                Ok(Box::new(SourceMemoryDirectory) as Box<dyn d2_rs_language::stream::ByteReader>)
            }
            None => Err("file does not exist".into()),
        }
    }))
}

fn parse_d2_source_impl(source: &str, options_json: &str) -> Result<String, String> {
    let options = parse_options::<SourceOptions>(options_json, SourceOptions::default)?;
    let path = source_path(options.path.as_deref(), !options.files.is_empty());
    let map = parse_source_map(source, &path, options.utf16_positions)?;
    serde_json::to_string(&map).map_err(|error| format!("could not encode AST: {error}"))
}

fn format_d2_source_impl(source: &str, options_json: &str) -> Result<String, String> {
    let options = parse_options::<SourceOptions>(options_json, SourceOptions::default)?;
    let path = source_path(options.path.as_deref(), !options.files.is_empty());
    let map = parse_source_map(source, &path, options.utf16_positions)?;
    String::from_utf8(d2_rs_language::format::format(&Child::Map(Shared::new(
        map,
    ))))
    .map_err(|error| format!("formatter returned invalid UTF-8: {error}"))
}

fn layout_d2_source_impl(source: &str, options_json: &str) -> Result<String, String> {
    let (_, result, _, _, _) = source_layout_impl(source, options_json)?;
    serde_json::to_string(&result)
        .map_err(|error| format!("could not encode source layout result: {error}"))
}

fn layout_d2_source_with_cancellation_impl(
    source: &str,
    options_json: &str,
    token: &CancellationToken,
) -> Result<String, String> {
    let (_, result, _, _, _) =
        source_layout_impl_with_cancellation(source, options_json, Some(token))?;
    serde_json::to_string(&result)
        .map_err(|error| format!("could not encode source layout result: {error}"))
}

type SourceLayout = (
    Graph,
    d2_rs_tala::LayoutResult,
    BTreeMap<d2_rs_core::NodeId, String>,
    BTreeMap<d2_rs_core::NodeId, compiler::SourceNodeStyle>,
    Option<compiler::DiagramConfig>,
);

fn source_layout_impl(source: &str, options_json: &str) -> Result<SourceLayout, String> {
    source_layout_impl_with_cancellation(source, options_json, None)
}

fn source_layout_impl_with_cancellation(
    source: &str,
    options_json: &str,
    cancellation: Option<&CancellationToken>,
) -> Result<SourceLayout, String> {
    source_layout_typed(source, options_json, cancellation).map_err(|error| error.to_string())
}

fn source_layout_typed(
    source: &str,
    options_json: &str,
    cancellation: Option<&CancellationToken>,
) -> Result<SourceLayout, SourceCompileError> {
    source_layout_with_policy(source, options_json, cancellation, false)
}

fn source_layout_with_policy(
    source: &str,
    options_json: &str,
    cancellation: Option<&CancellationToken>,
    public: bool,
) -> Result<SourceLayout, SourceCompileError> {
    let mut options =
        parse_options::<SourceLayoutOptions>(options_json, SourceLayoutOptions::default)?;
    // d2wasm's ASCII compile request substitutes ELK before applyConfigs;
    // the resulting explicit layout pointer also wins over source config.
    if options.ascii
        && options
            .engine
            .as_deref()
            .is_none_or(|engine| engine == "dagre")
    {
        options.engine = Some("elk".to_owned());
    }
    let path = source_path(options.path.as_deref(), !options.files.is_empty());
    let map = parse_source_map_typed(source, &path, options.utf16_positions)?;
    let (compiler, root) = compiler::compile_ir_with_options(
        Shared::new(map),
        compiler::CompileOptions {
            import_opener: source_import_opener(&options.files),
            utf16_positions: options.utf16_positions,
        },
    )
    .map_err(|error| error.to_string())?;
    if !compiler.errors.is_empty() {
        return Err(SourceCompileError::Diagnostics(Some(
            compiler.errors.clone(),
        )));
    }
    let config = compiler::extract_config_checked(&compiler, root)
        .map_err(|errors| SourceCompileError::Diagnostics(Some(errors)))?;
    let graph_root = match options.target.as_deref() {
        None | Some("") | Some("*") => root,
        Some(target) => {
            let target = target.strip_suffix(".*").unwrap_or(target);
            compiler::resolve_board_target(&compiler, root, target)
                .map_err(|error| error.to_string())?
        }
    };
    if graph_root != root {
        // d2lib compiles every board before selecting the requested target.
        let used = compiler.variable_expansion_usage().1;
        compiler::board_is_renderable(&compiler, root).map_err(SourceCompileError::from)?;
        compiler.restore_variable_expansion_used(used);
    }
    let effective_theme_id = options
        .theme_id
        .or_else(|| config.as_ref().and_then(|config| config.theme_id));
    let caps_lock = effective_theme_id
        .and_then(d2_rs_core::theme::palette)
        .is_some_and(|palette| palette.special_rules.caps_lock);
    let hand_drawn = options
        .sketch
        .or_else(|| config.as_ref().and_then(|config| config.sketch))
        .unwrap_or(false);
    let theme_mono = effective_theme_id
        .and_then(d2_rs_core::theme::palette)
        .is_some_and(|p| p.special_rules.mono);
    let measurer = WasmFontGraphMeasurer::from_options_with_sketch(&options, hand_drawn)?;
    let (mut graph, labels, styles) = compiler::lower_graph_with_measurer_and_labels_theme_options(
        &compiler, graph_root, &measurer, caps_lock, theme_mono,
    )
    .map_err(SourceCompileError::from)?;
    // d2lib.compileInput rejects the resolved modes after compilation and
    // before invoking any layout engine (d2.go:117–119).
    if hand_drawn
        && options
            .isometric
            .or_else(|| config.as_ref().and_then(|config| config.isometric))
            .unwrap_or(false)
    {
        return Err("sketch cannot be combined with isometric rendering"
            .to_owned()
            .into());
    }
    graph.target_theme_id = effective_theme_id;
    // Match d2lib.applyConfigs/applyDefaults: an explicit API option wins,
    // then root vars.d2-config.layout-engine, then the v0.9.0 Dagre default.
    let config_engine = config
        .as_ref()
        .and_then(|config| config.layout_engine.as_deref());
    let configured_engine = options.engine.as_deref().or(config_engine);
    let engine = if public {
        // d2lib.compileBoard only calls the d2wasm resolver for nonempty
        // Graph.Objects, after graph validation and SetDimensions.
        if graph.nodes.is_empty() {
            EngineKind::Dagre
        } else {
            match configured_engine.unwrap_or("dagre") {
                "dagre" => EngineKind::Dagre,
                "elk" => EngineKind::Elk,
                "tala" => EngineKind::Tala,
                engine => return Err(format!("layout option '{engine}' not recognized").into()),
            }
        }
    } else {
        configured_engine
            .map(EngineKind::from_str)
            .transpose()
            .map_err(|error| error.to_string())?
            .unwrap_or(EngineKind::Dagre)
    };
    // Preserve invalid data until a core TALA invocation, including nested
    // scopes. Specialized layouts and other engines never consume it.
    if engine == EngineKind::Tala {
        graph.tala_seed_data = config
            .as_ref()
            .and_then(|config| config.tala_seeds().transpose());
    }
    let seeds = options.seeds.unwrap_or_else(|| D2Options::default().seeds);
    let result = LayoutDispatcher
        .layout_compiled_with_cancellation(
            &graph,
            engine,
            &LayoutOptions {
                seeds,
                max_concurrency: options.max_concurrency,
            },
            cancellation,
        )
        .map_err(|error| error.to_string())?;
    Ok((graph, result, labels, styles, config))
}

fn render_d2_source_impl(source: &str, options_json: &str) -> Result<String, String> {
    let options = parse_options::<SourceLayoutOptions>(options_json, SourceLayoutOptions::default)?;
    let multi_target = options
        .target
        .as_deref()
        .is_some_and(|target| target == "*" || target.ends_with(".*"));
    if !multi_target {
        return render_single_board_svg_impl(source, options_json);
    }

    let target = options.target.as_deref().unwrap_or("*");
    let Some(interval_ms) = options.animate_interval.filter(|interval| *interval > 0) else {
        return Err(format!(
            "target '{target}' only supported for animated SVGs"
        ));
    };
    if options.ascii {
        return Err(if target == "*" {
            "ASCII rendering does not support multi-board animation".to_owned()
        } else {
            "ASCII rendering does not support multi-board targets".to_owned()
        });
    }

    // d2animate.renderBoards emits the selected board first, followed by
    // descendants in source order. The root is the selected board for `*`;
    // a `foo.*` target selects foo and only its descendants.
    let base_target = if target == "*" {
        None
    } else {
        Some(target.strip_suffix(".*").unwrap_or(target))
    };
    let mut board_targets = Vec::new();
    board_targets.push(base_target.map(ToOwned::to_owned));
    let base_parts = base_target
        .map(|target| {
            d2_rs_language::parse_key(target.as_bytes())
                .map_err(|e| compiler::BoardTargetError::InvalidKey(e.to_string()).to_string())?
                .string_ida()
                .ok_or_else(|| "board target has no components".to_owned())
        })
        .transpose()?;
    for path in source_board_targets(source, &options)? {
        let include = match &base_parts {
            None => true,
            Some(base) => d2_rs_language::parse_key(path.as_bytes())
                .ok()
                .and_then(|key| key.string_ida())
                .is_some_and(|parts| parts.len() > base.len() && parts.starts_with(base)),
        };
        if include {
            board_targets.push(Some(path));
        }
    }

    let mut child_options: serde_json::Value = if options_json.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(options_json)
            .map_err(|error| format!("invalid source options JSON: {error}"))?
    };
    let child_options = child_options
        .as_object_mut()
        .ok_or_else(|| "source options must be a JSON object".to_owned())?;
    let mut frames = Vec::with_capacity(board_targets.len());
    let mut root_render_options = None;
    for board_target in board_targets {
        match &board_target {
            Some(path) => {
                child_options.insert("target".to_owned(), serde_json::Value::String(path.clone()));
            }
            None => {
                child_options.remove("target");
            }
        }
        let board_options = serde_json::to_string(child_options)
            .map_err(|error| format!("could not encode board render options: {error}"))?;
        let board = prepare_source_board(source, &board_options)?;
        root_render_options.get_or_insert((board.render_options, board.sketch));
        frames.push((
            board_target.unwrap_or_default(),
            board.target_json,
            !board.is_folder_only,
        ));
    }
    let tree = d2_rs_render::target_diagram_tree_json(base_target.unwrap_or(""), &frames)?;
    let diagram = d2_rs_render::target::TargetDiagram::decode_json(&tree)
        .map_err(|error| error.to_string())?
        .ok_or("missing target diagram")?;
    let (render_options, sketch) = root_render_options.ok_or("missing root board")?;
    let output = diagram.render_d2svg(
        &render_options,
        sketch,
        options.no_xml_tag,
        interval_ms,
        true,
    )?;
    String::from_utf8(output).map_err(|error| error.to_string())
}

struct PreparedBoard {
    ascii: Option<String>,
    target_json: Vec<u8>,
    render_options: RenderOptions,
    sketch: bool,
    is_folder_only: bool,
}

fn render_single_board_svg_impl(source: &str, options_json: &str) -> Result<String, String> {
    let options = parse_options::<SourceLayoutOptions>(options_json, SourceLayoutOptions::default)?;
    let board = prepare_source_board(source, options_json)?;
    if let Some(ascii) = board.ascii {
        return Ok(ascii);
    }
    let diagram = d2_rs_render::target::TargetDiagram::decode_json(&board.target_json)
        .map_err(|error| error.to_string())?
        .ok_or("missing target diagram")?;
    let output = diagram.render_d2svg(
        &board.render_options,
        board.sketch,
        options.no_xml_tag,
        options.animate_interval.unwrap_or(0),
        false,
    )?;
    String::from_utf8(output).map_err(|error| error.to_string())
}

fn prepare_source_board(source: &str, options_json: &str) -> Result<PreparedBoard, String> {
    let options = parse_options::<SourceLayoutOptions>(options_json, SourceLayoutOptions::default)?;
    let (graph, result, labels, styles, source_config) = source_layout_impl(source, options_json)?;
    let source_config = source_config.unwrap_or_default();
    if options.ascii {
        let ascii_options = RenderOptions {
            ascii_mode: options.ascii_mode.clone(),
            scale: options.scale,
            theme_id: options.theme_id.or(source_config.theme_id),
            ..Default::default()
        };
        let output = String::from_utf8(
            render_ascii_with_options(
                &graph,
                &result,
                &labels,
                &styles,
                &ascii_options,
                options.sketch.or(source_config.sketch).unwrap_or(false),
            )
            .map_err(|error| format!("ASCII rendering failed: {error}"))?,
        )
        .map_err(|error| format!("ASCII rendering returned invalid UTF-8: {error}"))?;
        return Ok(PreparedBoard {
            ascii: Some(output),
            target_json: Vec::new(),
            render_options: RenderOptions::default(),
            sketch: false,
            is_folder_only: graph.nodes.is_empty(),
        });
    }
    let pad = options
        .pad
        .or_else(|| source_config.pad.map(|value| value as f64))
        .unwrap_or(RenderOptions::default().pad);
    let scale = options.scale;
    let (target_root_label, include_config_in_hash) = source_board_hash_metadata(source, &options)?;
    let theme_id = options.theme_id.or(source_config.theme_id);
    let dark_theme_id = options.dark_theme_id.or(source_config.dark_theme_id);
    let center = options.center.or(source_config.center).unwrap_or(false);
    let sketch = options.sketch.or(source_config.sketch).unwrap_or(false);
    // Preserve the Go WASM API's pointer semantics: an omitted override map
    // inherits source config, while an explicitly supplied empty map clears it.
    let theme_overrides = options
        .theme_overrides
        .unwrap_or(source_config.theme_overrides);
    let dark_theme_overrides = options
        .dark_theme_overrides
        .unwrap_or(source_config.dark_theme_overrides);
    if !pad.is_finite() || pad < 0.0 {
        return Err("pad must be a finite non-negative number".into());
    }
    if scale.is_some_and(|value| !value.is_finite()) {
        return Err("scale must be finite".into());
    }
    if let Some(theme_id) = theme_id
        && d2_rs_core::theme::find(theme_id).is_none()
    {
        return Err(format!("unknown theme ID {theme_id}"));
    }
    if let Some(theme_id) = dark_theme_id
        && d2_rs_core::theme::find(theme_id).is_none()
    {
        return Err(format!("unknown dark theme ID {theme_id}"));
    }
    let asset_data = options
        .asset_data
        .into_iter()
        .map(|(key, value)| {
            base64::engine::general_purpose::STANDARD
                .decode(value.as_bytes())
                .map(|bytes| (key.clone(), bytes))
                .map_err(|error| format!("invalid base64 asset {key:?}: {error}"))
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    let render_options = RenderOptions {
        pad,
        scale,
        ascii_mode: None,
        salt: options.salt,
        theme_id,
        center,
        // `sketch` is passed separately so the existing render options
        // JSON and Rust struct remain compatible for ordinary callers.
        theme_overrides,
        dark_theme_id,
        dark_theme_overrides,
        asset_data,
        unavailable_images: Default::default(),
        force_appendix: options.force_appendix,
        // Transparent page canvases are selected by native paged exports;
        // the existing WASM render API keeps an opaque canvas by default.
        transparent_background: false,
        target_root_label,
        include_config_in_hash,
        d2svg_hash_input: None,
        custom_fonts: Default::default(),
        hand_drawn_font: false,
    };
    let target_json = d2_rs_render::target_diagram_json(
        &graph,
        &result,
        &labels,
        &styles,
        &render_options,
        sketch,
    )?;
    Ok(PreparedBoard {
        ascii: None,
        target_json,
        render_options,
        sketch,
        is_folder_only: graph.nodes.is_empty(),
    })
}

fn complete_d2_source_impl(source: &str, options_json: &str) -> Result<String, String> {
    let options = parse_options::<CompletionOptions>(options_json, CompletionOptions::default)?;
    let encoding = if options.utf16_positions {
        PositionEncoding::Utf16
    } else {
        PositionEncoding::Utf8
    };
    serde_json::to_string(&completion_items(
        source,
        options.line,
        options.column,
        encoding,
    ))
    .map_err(|error| format!("could not encode completion items: {error}"))
}

fn board_at_position_json_impl(source: &str, options_json: &str) -> Result<String, String> {
    let options = parse_options::<CompletionOptions>(options_json, CompletionOptions::default)?;
    let encoding = if options.utf16_positions {
        PositionEncoding::Utf16
    } else {
        PositionEncoding::Utf8
    };
    let path = board_at_position(
        source,
        SourcePosition {
            line: options.line,
            column: options.column,
        },
        encoding,
    )
    .map_err(|error| {
        let path = error
            .path
            .as_ref()
            .map(|path| format!("{path:?}"))
            .unwrap_or_else(|| "null".to_owned());
        format!("{} (partial board path: {path})", error)
    })?;
    serde_json::to_string(&path).map_err(|error| format!("could not encode board path: {error}"))
}

fn reference_ranges_json_impl(source: &str, key: &str) -> Result<String, String> {
    serde_json::to_string(&reference_ranges(source, key).map_err(|error| error.to_string())?)
        .map_err(|error| format!("could not encode reference ranges: {error}"))
}

#[derive(Default, serde::Deserialize)]
struct ReferenceFilesRequest {
    path: String,
    files: HashMap<String, String>,
    #[serde(default)]
    board_path: Vec<String>,
    key: String,
}

#[derive(serde::Deserialize)]
struct SourceEditsRequest {
    source: String,
    #[serde(default)]
    edits: Vec<SourceEdit>,
    #[serde(default)]
    encoding: Option<String>,
    #[serde(default, alias = "utf16_positions", alias = "utf16")]
    utf16_positions: bool,
}

fn apply_source_edits_json_impl(request_json: &str) -> Result<String, String> {
    let request: SourceEditsRequest = serde_json::from_str(request_json)
        .map_err(|error| format!("invalid source-edits request JSON: {error}"))?;
    let encoding = match request
        .encoding
        .as_deref()
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("utf8") | Some("utf-8") => PositionEncoding::Utf8,
        Some("utf16") | Some("utf-16") => PositionEncoding::Utf16,
        Some(other) => return Err(format!("unsupported source edit encoding '{other}'")),
        None if request.utf16_positions => PositionEncoding::Utf16,
        None => PositionEncoding::Utf8,
    };
    let source = apply_edits(&request.source, &request.edits, encoding)
        .map_err(|error| format!("could not apply source edits: {error}"))?;
    serde_json::to_string(&serde_json::json!({"source": source}))
        .map_err(|error| format!("could not encode edited source: {error}"))
}

#[derive(serde::Deserialize)]
struct CreateRootFieldRequest {
    source: String,
    key: String,
}

fn create_root_field_json_impl(request_json: &str) -> Result<String, String> {
    let request: CreateRootFieldRequest = serde_json::from_str(request_json)
        .map_err(|error| format!("invalid create-root-field request JSON: {error}"))?;
    let result = create_root_field(&request.source, &request.key)
        .map_err(|error| format!("could not create root field: {error}"))?;
    serde_json::to_string(&result)
        .map_err(|error| format!("could not encode created root field: {error}"))
}

#[derive(serde::Deserialize)]
struct SetRootFieldRequest {
    source: String,
    key: String,
    #[serde(default)]
    value: Option<String>,
}

fn set_root_field_json_impl(request_json: &str) -> Result<String, String> {
    let request: SetRootFieldRequest = serde_json::from_str(request_json)
        .map_err(|error| format!("invalid set-root-field request JSON: {error}"))?;
    serde_json::to_string(
        &set_root_field(&request.source, &request.key, request.value.as_deref())
            .map_err(|error| format!("could not set root field: {error}"))?,
    )
    .map_err(|error| format!("could not encode set root field: {error}"))
}

#[derive(serde::Deserialize)]
struct DeleteRootFieldRequest {
    source: String,
    key: String,
}

fn delete_root_field_json_impl(request_json: &str) -> Result<String, String> {
    let request: DeleteRootFieldRequest = serde_json::from_str(request_json)
        .map_err(|error| format!("invalid delete-root-field request JSON: {error}"))?;
    serde_json::to_string(
        &delete_root_field(&request.source, &request.key)
            .map_err(|error| format!("could not delete root field: {error}"))?,
    )
    .map_err(|error| format!("could not encode delete root field: {error}"))
}

#[derive(serde::Deserialize)]
struct RenameRootFieldRequest {
    source: String,
    #[serde(alias = "oldKey")]
    old_key: String,
    #[serde(alias = "newKey")]
    new_key: String,
}

fn rename_root_field_json_impl(request_json: &str) -> Result<String, String> {
    let request: RenameRootFieldRequest = serde_json::from_str(request_json)
        .map_err(|error| format!("invalid rename-root-field request JSON: {error}"))?;
    serde_json::to_string(
        &rename_root_field(&request.source, &request.old_key, &request.new_key)
            .map_err(|error| format!("could not rename root field: {error}"))?,
    )
    .map_err(|error| format!("could not encode rename root field: {error}"))
}

#[derive(serde::Deserialize)]
struct ReconnectEdgeRequest {
    source: String,
    #[serde(alias = "edgeKey")]
    edge_key: String,
    #[serde(default, alias = "newSrc")]
    new_src: Option<String>,
    #[serde(default, alias = "newDst")]
    new_dst: Option<String>,
}

fn reconnect_edge_json_impl(request_json: &str) -> Result<String, String> {
    let request: ReconnectEdgeRequest = serde_json::from_str(request_json)
        .map_err(|error| format!("invalid reconnect-edge request JSON: {error}"))?;
    serde_json::to_string(
        &reconnect_edge(
            &request.source,
            &request.edge_key,
            request.new_src.as_deref(),
            request.new_dst.as_deref(),
        )
        .map_err(|error| format!("could not reconnect edge: {error}"))?,
    )
    .map_err(|error| format!("could not encode reconnect edge: {error}"))
}

#[derive(serde::Deserialize)]
struct MoveObjectRequest {
    source: String,
    #[serde(alias = "oldKey")]
    old_key: String,
    #[serde(alias = "newKey")]
    new_key: String,
    #[serde(default, alias = "includeDescendants")]
    include_descendants: bool,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    files: std::collections::HashMap<String, String>,
    #[serde(default, alias = "boardPath")]
    board_path: Vec<String>,
}

fn move_object_json_impl(request_json: &str) -> Result<String, String> {
    let request: MoveObjectRequest = serde_json::from_str(request_json)
        .map_err(|error| format!("invalid move-object request JSON: {error}"))?;
    serde_json::to_string(
        &move_object_with_options(
            &request.source,
            &request.old_key,
            &request.new_key,
            request.include_descendants,
            &MoveOptions {
                path: request.path.unwrap_or_else(|| "-".into()),
                files: request.files,
                board_path: request.board_path,
            },
        )
        .map_err(|error| format!("could not move object: {error}"))?,
    )
    .map_err(|error| format!("could not encode moved object: {error}"))
}

fn reference_ranges_files_json_impl(request_json: &str) -> Result<String, String> {
    let request: ReferenceFilesRequest = serde_json::from_str(request_json)
        .map_err(|error| format!("invalid reference-ranges request JSON: {error}"))?;
    serde_json::to_string(
        &reference_ranges_with_files(
            &request.path,
            &request.files,
            &request.board_path,
            &request.key,
        )
        .map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("could not encode reference ranges: {error}"))
}

mod url_codec;

fn encode_impl(script: &str) -> Result<String, String> {
    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::best());
    encoder
        .write_all(script.as_bytes())
        .map_err(|error| format!("could not encode D2 script: {error}"))?;
    let compressed = encoder
        .finish()
        .map_err(|error| format!("could not finish D2 script encoding: {error}"))?;
    let result = base64::engine::general_purpose::URL_SAFE.encode(compressed);
    serde_json::to_string(&serde_json::json!({"result": result}))
        .map_err(|error| format!("could not encode URL result: {error}"))
}

fn decode_impl(encoded: &str) -> Result<String, String> {
    let script = url_codec::decode(encoded)?;
    serde_json::to_string(&serde_json::json!({"result": script}))
        .map_err(|error| format!("could not encode decoded URL result: {error}"))
}

fn parse_options<T>(input: &str, default: impl FnOnce() -> T) -> Result<T, String>
where
    T: serde::de::DeserializeOwned,
{
    if input.trim().is_empty() {
        Ok(default())
    } else {
        serde_json::from_str(input).map_err(|error| format!("invalid options JSON: {error}"))
    }
}

fn js_error(message: String) -> JsValue {
    JsValue::from_str(&message)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[cfg(feature = "oracle-fixtures")]
    fn source_layout_keeps_all_original_compiler_diagnostics() {
        let fixtures: serde_json::Value = serde_json::from_str(d2_rs_testdata::load_str(
            concat!(env!("CARGO_MANIFEST_DIR"), "/../d2-rs-language"),
            "tests/fixtures/public-compiler-memfs-diagnostics-v090.json",
        ))
        .unwrap();
        let mut compared = 0;
        for (request, source) in fixtures["requests"]
            .as_array()
            .unwrap()
            .iter()
            .zip(fixtures["source"].as_array().unwrap())
        {
            let decode = |hex: &str| {
                (0..hex.len())
                    .step_by(2)
                    .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
                    .collect::<Vec<_>>()
            };
            let input = String::from_utf8(decode(request["hex"].as_str().unwrap())).unwrap();
            let files = request["files_hex"]
                .as_object()
                .unwrap()
                .iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        String::from_utf8(decode(v.as_str().unwrap())).unwrap(),
                    )
                })
                .collect::<HashMap<_, _>>();
            let options =
                serde_json::json!({"path":request["path"],"utf16":request["utf16"],"files":files});
            let error = source_layout_impl(&input, &options.to_string()).unwrap_err();
            assert_eq!(error, source["error"].as_str().unwrap(), "{request}");
            compared += 1;
        }
        assert_eq!(compared, 28);
    }

    use d2_rs_tala::{Direction, Edge, Node, Size};

    fn serialized_graph() -> String {
        let mut graph = Graph::with_direction(Direction::Right);
        let a = graph.add_node(Node::new(
            "a",
            Size {
                width: 80.0,
                height: 40.0,
            },
        ));
        let b = graph.add_node(Node::new(
            "b",
            Size {
                width: 80.0,
                height: 40.0,
            },
        ));
        graph.add_edge(Edge {
            source: a,
            target: b,
        });
        serde_json::to_string(&graph).unwrap()
    }

    #[test]
    fn lays_out_core_graph_json() {
        let output = layout_graph_json_impl(&serialized_graph(), r#"{"seeds":[1]}"#).unwrap();
        let output: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(output["boxes"].as_object().unwrap().len(), 2);
        assert_eq!(output["routes"].as_object().unwrap().len(), 1);
    }

    #[test]
    fn cancelled_wasm_layouts_return_a_clean_error() {
        let token = WasmCancellationToken::new();
        token.cancel();
        let error = layout_graph_json_with_cancellation_impl(
            &serialized_graph(),
            r#"{"seeds":[1]}"#,
            &token.inner,
        )
        .unwrap_err();
        assert!(error.contains("canceled") || error.contains("cancelled"));
    }

    #[test]
    fn reports_invalid_options() {
        let error = layout_graph_json_impl(&serialized_graph(), "not JSON").unwrap_err();
        assert!(error.starts_with("invalid options JSON:"));
    }

    #[test]
    fn lays_out_d2_json() {
        let input = include_str!("../../d2-rs-lib/tests/fixtures/simple.json");
        let output = layout_d2_json_impl(input, r#"{"seeds":[1]}"#).unwrap();
        let output: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(output["objects"].as_array().unwrap().len(), 3);
        assert_eq!(output["edges"].as_array().unwrap().len(), 2);
        assert!(output["objects"][0]["box"].is_object());
        assert!(output["edges"][0]["route"].is_array());
    }

    #[test]
    fn compiles_d2_source_to_ordered_ir_json() {
        let output = compile_d2_source_impl("a -> b\n", "").unwrap();
        let output: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert!(output["errors"].as_array().unwrap().is_empty());
        assert!(
            output["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|node| { node["kind"] == "edge" && node["path"] == "root.\"(a -> b)[0]\"" })
        );
    }

    #[test]
    fn compiles_source_target_metadata_with_styles_and_layout() {
        let output = compile_d2_target_json_impl(
            "a: {shape: circle; style.fill: red; tooltip: hi}\n",
            r#"{"seeds":[1]}"#,
        )
        .unwrap();
        let output: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(output["nodes"][0]["id"], "a");
        assert_eq!(output["nodes"][0]["node"]["shape"], "circle");
        assert_eq!(output["nodes"][0]["style"]["fill"], "red");
        assert!(output["layout"]["boxes"]["0"].is_object());
        assert!(output["ast"]["nodes"].is_array());
    }

    #[test]
    fn compiles_root_board_metadata_in_source_order() {
        let output = compile_d2_target_json_impl(
            "root\nlayers: { one: { child }; two: { other } }\nscenarios: { alt: { variant } }\nsteps: { first: { step } }\n",
            r#"{"seeds":[1]}"#,
        )
        .unwrap();
        let output: serde_json::Value = serde_json::from_str(&output).unwrap();
        let paths = output["boards"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["path"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            paths,
            ["layers.one", "layers.two", "scenarios.alt", "steps.first"]
        );
        assert_eq!(output["boards"][0]["graph"]["nodes"][0]["id"], "child");
        assert!(output["boards"][0]["graph"]["ast"]["nodes"].is_array());
    }

    #[test]
    fn mapless_board_metadata_preserves_empty_graph_ast_and_label() {
        let output = compile_d2_target_json_impl(
            "root\nlayers: {empty; caption: Title; drawn: {child}}",
            r#"{"layout":"dagre"}"#,
        )
        .unwrap();
        let output: serde_json::Value = serde_json::from_str(&output).unwrap();
        let boards = output["boards"].as_array().unwrap();
        assert_eq!(boards.len(), 3);
        for (board, name, label) in [
            (&boards[0], "empty", "empty"),
            (&boards[1], "caption", "Title"),
        ] {
            assert_eq!(board["path"], format!("layers.{name}"));
            assert_eq!(board["graph"]["diagram"]["name"], name);
            assert_eq!(board["graph"]["diagram"]["root"]["label"], label);
            assert!(board["graph"]["nodes"].as_array().unwrap().is_empty());
            assert!(board["graph"]["ast"]["nodes"].is_null());
            assert!(board["graph"]["ast"]["range"].is_string());
        }
    }

    #[test]
    fn rejects_invalid_custom_font_data() {
        let error = compile_d2_target_json_impl("a\n", r#"{"font_regular":"AQID"}"#).unwrap_err();
        assert!(error.contains("regular font"));
    }

    #[test]
    fn source_compilation_accepts_utf16_alias() {
        let output = compile_d2_source_impl("a: café\n", r#"{"utf16":true}"#).unwrap();
        let output: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert!(output["errors"].as_array().unwrap().is_empty());
    }

    #[test]
    fn source_compilation_reports_parse_errors() {
        let error = compile_d2_source_impl("a: {\n", "").unwrap_err();
        assert!(!error.is_empty());
    }

    #[test]
    fn parses_source_to_ast_json() {
        let output = parse_d2_source_impl("a: hello\n", "").unwrap();
        let output: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(
            output["nodes"][0]["map_key"]["key"]["path"][0]["unquoted_string"]["value"][0]["string"],
            "a"
        );
    }

    #[test]
    fn formats_source_in_wasm_boundary() {
        let output = format_d2_source_impl("a:   hello\n", "").unwrap();
        assert_eq!(output, "a: hello\n");
    }

    #[test]
    fn applies_utf16_source_edits_from_json() {
        let output = apply_source_edits_json_impl(
            r#"{"source":"a: 😀\nb -> c\n","encoding":"utf16","edits":[{"start":{"line":0,"column":3},"end":{"line":0,"column":5},"replacement":"X"},{"start":{"line":1,"column":0},"end":{"line":1,"column":1},"replacement":"d"}]}"#,
        )
        .unwrap();
        let output: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(output["source"], "a: X\nd -> c\n");
    }

    #[test]
    fn source_edit_json_rejects_unknown_encoding() {
        let error = apply_source_edits_json_impl(r#"{"source":"a","encoding":"utf32","edits":[]}"#)
            .unwrap_err();
        assert!(error.contains("unsupported source edit encoding"));
    }

    #[test]
    fn creates_root_field_from_json() {
        let output = create_root_field_json_impl(r#"{"source":"a\n","key":"b"}"#).unwrap();
        let output: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(output["key"], "b");
        assert_eq!(output["source"], "a\nb\n");
    }

    #[test]
    fn edits_root_fields_from_json() {
        let set =
            set_root_field_json_impl(r#"{"source":"a: one\n","key":"a","value":"two"}"#).unwrap();
        let set: serde_json::Value = serde_json::from_str(&set).unwrap();
        assert_eq!(set["source"], "a: two\n");
        let rename = rename_root_field_json_impl(
            &serde_json::json!({"source": set["source"], "old_key": "a", "new_key": "b"})
                .to_string(),
        )
        .unwrap();
        let rename: serde_json::Value = serde_json::from_str(&rename).unwrap();
        assert_eq!(rename["source"], "b: two\n");
        let delete = delete_root_field_json_impl(
            &serde_json::json!({"source": rename["source"], "key": "b"}).to_string(),
        )
        .unwrap();
        let delete: serde_json::Value = serde_json::from_str(&delete).unwrap();
        assert_eq!(delete["source"], "");
        assert_eq!(delete["deleted"], true);
    }

    #[test]
    fn reconnects_edge_from_json() {
        let output = reconnect_edge_json_impl(
            r#"{"source":"a -> b: label\nc\nd\n","edgeKey":"(a -> b)[0]","newSrc":"c","newDst":"d"}"#,
        )
        .unwrap();
        let output: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(output["source"], "c -> d: label\nc\nd\n");
        assert_eq!(output["changed"], true);
    }

    #[test]
    fn moves_objects_from_json() {
        let output = move_object_json_impl(
            r#"{"source":"a: { b }\nx: {}\na.b -> x\n","old_key":"a.b","new_key":"x.b","include_descendants":true}"#,
        )
        .unwrap();
        let output: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(output["key"], "x.b");
        assert!(output["source"].as_str().unwrap().contains("x.b -> x"));

        let split = move_object_json_impl(
            r#"{"source":"a.b: {\n  shape: hexagon\n  child\n}\n","old_key":"a.b","new_key":"b","include_descendants":false}"#,
        )
        .unwrap();
        let split: serde_json::Value = serde_json::from_str(&split).unwrap();
        assert!(split["source"].as_str().unwrap().contains("a: {"));
        assert!(split["source"].as_str().unwrap().contains("shape: hexagon"));
        assert!(split["source"].as_str().unwrap().contains("child"));
    }

    #[cfg(feature = "oracle-fixtures")]
    #[test]
    fn moves_objects_with_board_and_virtual_file_context() {
        let fixture: serde_json::Value = serde_json::from_str(d2_rs_testdata::load_str(
            concat!(env!("CARGO_MANIFEST_DIR"), "/../d2-rs-editor"),
            "tests/fixtures/move-transplant-v090.json",
        ))
        .unwrap();
        let cases = fixture["cases"].as_array().unwrap();
        assert_eq!(cases.len(), 392);
        let mut checked = 0;
        for case in &cases[354..] {
            for camel_case in [false, true] {
                let mut request = case["input"].clone();
                if camel_case {
                    if let Some(board) = request.as_object_mut().unwrap().remove("board_path") {
                        request["boardPath"] = board;
                    }
                }
                let result = move_object_json_impl(&request.to_string());
                let expected_error = case["error"].as_str().unwrap();
                if expected_error.is_empty() {
                    let output: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
                    assert_eq!(output["source"], case["source"], "{}", request["name"]);
                } else {
                    assert_eq!(
                        result.unwrap_err(),
                        format!("could not move object: {expected_error}"),
                        "{}",
                        request["name"]
                    );
                }
                checked += 1;
            }
        }
        assert_eq!(checked, 76);
    }

    #[test]
    fn lays_out_source_without_filesystem_access() {
        let output = layout_d2_source_impl("a -> b\n", r#"{"seeds":[7]}"#).unwrap();
        let output: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(output["report"]["selected_seed"], 7);
        assert_eq!(output["boxes"].as_object().unwrap().len(), 2);
        assert_eq!(output["routes"].as_object().unwrap().len(), 1);
    }

    #[cfg(feature = "oracle-fixtures")]
    #[test]
    fn source_cancellation_matches_original_invocation_boundaries() {
        // Current master engine/router callbacks, after active compilation.
        // The unchanged v0.9.0 fixtures remain historical inputs/evidence.
        let fixture: serde_json::Value = serde_json::from_str(d2_rs_testdata::oracle_fixture!(
            "tests/fixtures/layout-context-admission-master.json"
        ))
        .unwrap();
        let cases = fixture["cancellation"].as_array().unwrap();
        assert_eq!(cases.len(), 304);
        let mut mismatches = Vec::new();
        for case in cases {
            let input = &case["input"];
            let token = CancellationToken::new();
            if input["cancel"].as_bool().unwrap() {
                token.cancel();
            }
            let actual = source_layout_impl_with_cancellation(
                input["source"].as_str().unwrap(),
                &input.to_string(),
                Some(&token),
            )
            .err()
            .unwrap_or_default();
            if actual != case["error"].as_str().unwrap() {
                mismatches.push(format!(
                    "{}: actual={actual:?}; source={}",
                    input["name"], case["error"]
                ));
            }
        }
        assert!(
            mismatches.is_empty(),
            "{} / 304 mismatches:\n{}",
            mismatches.len(),
            mismatches.join("\n")
        );
    }

    #[cfg(feature = "oracle-fixtures")]
    #[test]
    fn source_deadlines_match_original_invocation_boundaries() {
        let fixture: serde_json::Value = serde_json::from_str(d2_rs_testdata::oracle_fixture!(
            "tests/fixtures/layout-context-admission-master.json"
        ))
        .unwrap();
        let cases = fixture["deadlines"].as_array().unwrap();
        assert_eq!(cases.len(), 340);
        let mut mismatches = Vec::new();
        for case in cases {
            let input = &case["input"];
            let token = CancellationToken::new();
            match input["reason"].as_str().unwrap() {
                "active" => {}
                "deadline" => token.expire(),
                "cancel" => token.cancel(),
                "cancel-then-deadline" => {
                    token.cancel();
                    token.expire();
                }
                "deadline-then-cancel" => {
                    token.expire();
                    token.cancel();
                }
                reason => panic!("unknown cause {reason}"),
            }
            let actual = source_layout_impl_with_cancellation(
                input["source"].as_str().unwrap(),
                &input.to_string(),
                Some(&token),
            )
            .err()
            .unwrap_or_default();
            if actual != case["error"].as_str().unwrap() {
                mismatches.push(format!(
                    "{}: actual={actual:?}; source={}",
                    input["name"], case["error"]
                ));
            }
        }
        assert!(
            mismatches.is_empty(),
            "{} / 340 mismatches:\n{}",
            mismatches.len(),
            mismatches.join("\n")
        );
    }

    #[test]
    fn cancelled_source_layouts_stop_before_engine_work() {
        let token = WasmCancellationToken::new();
        token.cancel();
        let error = layout_d2_source_with_cancellation_impl(
            "a -> b\n",
            r#"{"engine":"tala"}"#,
            &token.inner,
        )
        .unwrap_err();
        assert_eq!(error, "context canceled");
        // Master checks the context before entering its Dagre solver.
        assert_eq!(
            layout_d2_source_with_cancellation_impl("a -> b\n", "", &token.inner).unwrap_err(),
            "failed to dagre layout: context canceled"
        );
    }

    #[cfg(feature = "oracle-fixtures")]
    #[test]
    fn source_sketch_precedence_matches_original_measurements_on_every_board() {
        // Original d2lib.applyConfigs preserves a non-nil caller Sketch value,
        // including false, before applyDefaults selects the measuring font.
        let fixture: serde_json::Value = serde_json::from_str(d2_rs_testdata::oracle_fixture!(
            "tests/fixtures/sketch-precedence-source-v090.json"
        ))
        .unwrap();
        let cases = fixture["cases"].as_array().unwrap();
        assert_eq!(cases.len(), 36);
        let mut failures = Vec::new();
        for case in cases {
            let (graph, _, _, _, _) = source_layout_impl(
                case["source"].as_str().unwrap(),
                &case["options"].to_string(),
            )
            .unwrap();
            let actual: Vec<_> = graph
                .nodes()
                .map(|(_, node)| {
                    let label = node.label_size.unwrap();
                    (
                        node.external_id.clone(),
                        [node.size.width, node.size.height, label.width, label.height]
                            .map(f64::to_bits),
                    )
                })
                .collect();
            let expected: Vec<_> = case["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .map(|node| {
                    (
                        node["id"].as_str().unwrap().to_owned(),
                        ["width", "height", "label_width", "label_height"]
                            .map(|key| node[key].as_f64().unwrap().to_bits()),
                    )
                })
                .collect();
            if actual != expected {
                failures.push(format!(
                    "{}: actual {actual:?}; expected {expected:?}",
                    case["name"]
                ));
            }
        }
        assert!(
            failures.is_empty(),
            "{} / 36 failed: {failures:#?}",
            failures.len()
        );
    }

    #[cfg(feature = "oracle-fixtures")]
    #[test]
    fn source_tala_seed_config_matches_original_engine_boundary() {
        let fixture: serde_json::Value = serde_json::from_str(d2_rs_testdata::oracle_fixture!(
            "tests/fixtures/tala-seed-config-source-v090.json"
        ))
        .unwrap();
        let cases = fixture["cases"].as_array().unwrap();
        assert_eq!(cases.len(), 72);
        let mut failures = Vec::new();
        for case in cases {
            let input = &case["input"];
            let result = source_layout_impl(input["source"].as_str().unwrap(), &input.to_string());
            let expected_error = case["error"].as_str().unwrap();
            match result {
                Err(error) if error == expected_error => {}
                Ok((_, layout, _, _, _)) if expected_error.is_empty() => {
                    if !case["seeds"].is_null() {
                        let actual = serde_json::json!(
                            layout
                                .report
                                .candidates
                                .iter()
                                .map(|candidate| candidate.seed)
                                .collect::<Vec<_>>()
                        );
                        if actual != case["seeds"] {
                            failures.push(format!(
                                "{}: seeds {actual}, want {}",
                                input["name"], case["seeds"]
                            ));
                        }
                    }
                }
                other => failures.push(format!(
                    "{}: result {other:?}, want error {expected_error:?}",
                    input["name"]
                )),
            }
        }
        assert!(
            failures.is_empty(),
            "{} / 72 failed: {failures:#?}",
            failures.len()
        );
    }

    #[cfg(feature = "oracle-fixtures")]
    #[test]
    fn source_tala_seed_config_overrides_options_through_nested_layouts_and_boards() {
        let fixture: serde_json::Value = serde_json::from_str(d2_rs_testdata::oracle_fixture!(
            "tests/fixtures/tala-seed-nesting-source-v090.json"
        ))
        .unwrap();
        let cases = fixture["cases"].as_array().unwrap();
        assert_eq!(cases.len(), 5);
        for case in cases {
            let source = case["source"].as_str().unwrap();
            let first: serde_json::Value = serde_json::from_str(
                &compile_d2_target_json_impl(source, r#"{"engine":"tala","seeds":[1]}"#).unwrap(),
            )
            .unwrap();
            let second: serde_json::Value = serde_json::from_str(
                &compile_d2_target_json_impl(source, r#"{"engine":"tala","seeds":[7]}"#).unwrap(),
            )
            .unwrap();
            assert_eq!(first, second, "{}", case["name"]);
        }
    }

    #[test]
    fn source_layout_selects_dagre_engine() {
        let output =
            layout_d2_source_impl("a -> b\n", r#"{"engine":"dagre","seeds":[9]}"#).unwrap();
        let output: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(output["report"]["selected_seed"], 9);
        assert_eq!(output["boxes"].as_object().unwrap().len(), 2);
        assert_eq!(output["routes"].as_object().unwrap().len(), 1);
        assert!(
            output["report"]["warnings"][0]
                .as_str()
                .unwrap()
                .contains("Dagre")
        );
    }

    #[test]
    fn source_layout_defaults_to_dagre_like_v090() {
        let implicit = layout_d2_source_impl("a -> b\n", r#"{"seeds":[9]}"#).unwrap();
        let explicit =
            layout_d2_source_impl("a -> b\n", r#"{"engine":"dagre","seeds":[9]}"#).unwrap();
        let implicit: serde_json::Value = serde_json::from_str(&implicit).unwrap();
        let explicit: serde_json::Value = serde_json::from_str(&explicit).unwrap();
        assert_eq!(implicit["boxes"], explicit["boxes"]);
        assert_eq!(implicit["routes"], explicit["routes"]);
        assert_eq!(implicit["report"], explicit["report"]);
    }

    #[test]
    fn source_layout_applies_root_layout_config_before_default() {
        let output = layout_d2_source_impl(
            "vars: {d2-config: {layout-engine: elk}}\na -> b\n",
            r#"{"seeds":[9]}"#,
        )
        .unwrap();
        let output: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert!(
            output["report"]["warnings"]
                .as_array()
                .is_some_and(|warnings| warnings.iter().any(|warning| {
                    warning
                        .as_str()
                        .is_some_and(|warning| warning.contains("ELK"))
                })),
            "source layout did not apply vars.d2-config.layout-engine: {output}"
        );
    }

    #[cfg(feature = "oracle-fixtures")]
    #[test]
    fn source_tala_seed_validation_follows_actual_core_invocations() {
        let fixture: serde_json::Value = serde_json::from_str(d2_rs_testdata::oracle_fixture!(
            "tests/fixtures/tala-deferred-seeds-source-v090.json"
        ))
        .unwrap();
        let cases = fixture["cases"].as_array().unwrap();
        assert_eq!(cases.len(), 100);
        for case in cases {
            let input = &case["input"];
            let result = source_layout_impl(input["source"].as_str().unwrap(), &input.to_string());
            assert_eq!(
                result.err().unwrap_or_default(),
                case["error"].as_str().unwrap(),
                "{}",
                input["name"]
            );
        }
    }

    #[test]
    fn source_layout_applies_root_tala_seeds_when_api_omits_them() {
        let output = layout_d2_source_impl(
            "vars: {d2-config: {data: {tala-seeds: [17]}}}\na -> b\n",
            r#"{"engine":"tala"}"#,
        )
        .unwrap();
        let output: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(output["report"]["selected_seed"], 17);
    }

    #[test]
    fn source_layout_rejects_invalid_theme_override_after_ir_compilation() {
        let error = layout_d2_source_impl(
            "vars: {d2-config: {theme-overrides: {B1: potato}}}\na\n",
            "",
        )
        .unwrap_err();
        assert!(
            error.contains(
                "expected \"B1\" to be a valid named color (\"orange\") or a hex code (\"#f0ff3a\")"
            ),
            "{error}"
        );
    }

    #[test]
    fn source_layout_rejects_ir_config_errors_before_dispatch() {
        let error =
            layout_d2_source_impl("vars: {d2-config: {sketch: nope}}\na\n", "").unwrap_err();
        assert!(
            error.contains("expected a boolean for \"sketch\", got \"nope\""),
            "{error}"
        );
    }

    #[test]
    fn source_layout_selects_a_nested_board() {
        let output = layout_d2_source_impl(
            "layers: { one: { service: api } }\n",
            r#"{"target":"layers.one","seeds":[7]}"#,
        )
        .unwrap();
        let output: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(output["boxes"].as_object().unwrap().len(), 1);
        assert!(
            output["node_labels"]["0"]["size"]["width"]
                .as_f64()
                .unwrap()
                > 0.0
        );
    }

    #[test]
    fn source_layout_reports_original_empty_virtual_fs_error() {
        let error = layout_d2_source_impl("x: @missing\n", r#"{"path":"input.d2"}"#).unwrap_err();
        assert_eq!(
            error,
            "input.d2:1:4: failed to import \"missing.d2\": file does not exist"
        );
    }

    #[test]
    fn source_layout_accepts_virtual_import_files() {
        let options =
            r#"{"path":"index","files":{"import.d2":"x: {shape: circle}\n"},"seeds":[7]}"#;
        let output = layout_d2_source_impl("a: @import\n", options).unwrap();
        let output: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert!(output["boxes"].as_object().unwrap().len() >= 2);
        assert!(output["node_labels"].as_object().unwrap().len() >= 2);
    }

    #[test]
    fn legacy_elk_graph_keeps_measured_children_and_edges() {
        let response = get_elk_graph(Some(r#"{"fs":{"index":"a -> b\n"}}"#.to_owned()));
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();
        assert_eq!(response["error"], serde_json::Value::Null);
        assert_eq!(response["data"]["children"].as_array().unwrap().len(), 2);
        assert_eq!(response["data"]["edges"].as_array().unwrap().len(), 1);
        assert_eq!(response["data"]["edges"][0]["sources"][0], "a");
    }

    #[test]
    fn legacy_elk_graph_matches_sql_table_ports() {
        let source = "users: {\n  shape: sql_table\n  id: int\n  name: string\n}\norders: {\n  shape: sql_table\n  user_id: int\n}\norders.user_id -> users.id\n";
        let response: serde_json::Value = serde_json::from_str(&get_elk_graph(Some(format!(
            r#"{{"fs":{{"index":{}}}}}"#,
            serde_json::to_string(source).unwrap()
        ))))
        .unwrap();
        let children = response["data"]["children"].as_array().unwrap();
        let users = children.iter().find(|node| node["id"] == "users").unwrap();
        assert_eq!(users["labels"], serde_json::Value::Null);
        assert_eq!(users["ports"].as_array().unwrap().len(), 4);
        assert_eq!(users["ports"][0]["id"], "users.id.src");
        assert_eq!(users["ports"][0]["y"], 54.0);
        assert_eq!(users["ports"][0]["layoutOptions"]["elk.port.side"], "EAST");
        let edge = &response["data"]["edges"][0];
        assert_eq!(edge["sources"][0], "orders.user_id.src");
        assert_eq!(edge["targets"][0], "users.id.dst");
    }

    #[test]
    fn legacy_elk_graph_distributes_whole_table_ports() {
        let source = "users: {\n  shape: sql_table\n  id: int\n}\norders: {\n  shape: sql_table\n  id: int\n}\nusers -> orders\nusers -> orders\n";
        let request = format!(
            r#"{{"fs":{{"index":{}}}}}"#,
            serde_json::to_string(source).unwrap()
        );
        let response: serde_json::Value =
            serde_json::from_str(&get_elk_graph(Some(request))).unwrap();
        let children = response["data"]["children"].as_array().unwrap();
        let users = children.iter().find(|node| node["id"] == "users").unwrap();
        let width = users["width"].as_f64().unwrap();
        assert_eq!(users["ports"][2]["id"], "users.__root__.src.0");
        assert_eq!(users["ports"][2]["x"].as_f64().unwrap(), width / 3.0);
        assert_eq!(users["ports"][3]["id"], "users.__root__.src.1");
        assert_eq!(users["ports"][3]["x"].as_f64().unwrap(), width * 2.0 / 3.0);
        assert_eq!(
            response["data"]["edges"][0]["sources"][0],
            "users.__root__.src.0"
        );
        assert_eq!(
            response["data"]["edges"][0]["targets"][0],
            "orders.__root__.dst.0"
        );
    }

    #[test]
    fn legacy_elk_graph_marks_edge_labels_inline_and_scales_self_loops() {
        let labeled = get_elk_graph(Some(
            r#"{"fs":{"index":"a -> b: hello world\n"}}"#.to_owned(),
        ));
        let labeled: serde_json::Value = serde_json::from_str(&labeled).unwrap();
        assert_eq!(
            labeled["data"]["edges"][0]["labels"][0]["layoutOptions"]["elk.edgeLabels.inline"],
            true
        );

        let looped = get_elk_graph(Some(
            r#"{"fs":{"index":"a -> a: this is a deliberately wide self loop label\n"}}"#
                .to_owned(),
        ));
        let looped: serde_json::Value = serde_json::from_str(&looped).unwrap();
        assert!(
            looped["data"]["layoutOptions"]["elk.spacing.nodeSelfLoop"]
                .as_f64()
                .unwrap()
                > 50.0
        );
    }

    #[test]
    fn legacy_elk_size_preparation_matches_degree_and_icon_rules() {
        let mut graph = Graph::default();
        let hub = graph.add_node(d2_rs_core::Node::new(
            "hub",
            d2_rs_core::Size {
                width: 20.0,
                height: 30.0,
            },
        ));
        graph.node_mut(hub).unwrap().label_size = Some(d2_rs_core::Size {
            width: 40.0,
            height: 12.0,
        });
        graph.node_mut(hub).unwrap().has_icon = true;
        graph.node_mut(hub).unwrap().external_label = Some(d2_rs_core::ExternalLabel {
            size: d2_rs_core::Size {
                width: 18.0,
                height: 9.0,
            },
            side: d2_rs_core::ExternalSide::Bottom,
            alignment: d2_rs_core::ExternalAlignment::Center,
            automatic: false,
            reserve_space: true,
        });
        for name in ["a", "b", "c"] {
            let target = graph.add_node(d2_rs_core::Node::new(
                name,
                d2_rs_core::Size {
                    width: 20.0,
                    height: 20.0,
                },
            ));
            graph.add_edge(d2_rs_core::Edge {
                source: hub,
                target,
            });
        }

        let effective = legacy_elk_effective_sizes(&graph);
        let size = effective[&hub];
        // Default DOWN flow reserves 40px per outgoing port on width, then
        // adds the measured label plus five pixels for the icon row.
        assert_eq!(size.width, 120.0);
        assert_eq!(size.height, 61.0);

        graph.direction = d2_rs_core::Direction::Right;
        let horizontal = legacy_elk_effective_sizes(&graph);
        assert_eq!(horizontal[&hub].width, 20.0);
        assert_eq!(horizontal[&hub].height, 151.0);
    }

    #[test]
    fn legacy_elk_container_padding_accounts_for_default_label_lane() {
        let response = get_elk_graph(Some(
            r#"{"fs":{"index":"container: {\n  label-position: inside_middle_left\n  child\n}\n"}}"#
                .to_owned(),
        ));
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();
        let container = &response["data"]["children"][0];
        let padding = container["layoutOptions"]["elk.padding"].as_str().unwrap();
        assert_ne!(padding, "[top=50,left=50,bottom=50,right=50]");
        assert!(padding.contains("top="));
    }

    #[test]
    fn renders_source_to_svg_in_wasm_boundary() {
        let output = render_d2_source_impl("a -> b\n", r#"{"seeds":[7]}"#).unwrap();
        assert!(output.starts_with("<?xml version=\"1.0\" encoding=\"utf-8\"?><svg "));
        assert!(output.contains("<path d=\"M "));
        assert!(output.contains("<rect"));
    }

    #[cfg(feature = "oracle-fixtures")]
    #[test]
    fn wasm_source_svg_is_byte_identical_to_pinned_go() {
        let svg = render_d2_source_impl("a -> b\n", r#"{"pad":0}"#).unwrap();
        assert_eq!(
            svg,
            d2_rs_testdata::load_str(
                concat!(env!("CARGO_MANIFEST_DIR"), "/../d2-rs-cli"),
                "tests/fixtures/sanity-basic-dagre-v090.svg"
            )
            .trim_end_matches('\n')
        );
        let no_xml = render_d2_source_impl("a -> b\n", r#"{"pad":0,"no_xml_tag":true}"#).unwrap();
        assert_eq!(
            no_xml,
            svg.strip_prefix("<?xml version=\"1.0\" encoding=\"utf-8\"?>")
                .unwrap()
        );
    }

    #[cfg(feature = "oracle-fixtures")]
    #[test]
    fn multi_board_animation_matches_original_go_folder_bounds_and_hashes() {
        let cases: Vec<serde_json::Value> = serde_json::from_str(d2_rs_testdata::oracle_fixture!(
            "tests/fixtures/animation-multi-board-source-v090.json"
        ))
        .unwrap();
        assert_eq!(cases.len(), 32);
        for case in cases {
            let name = case["name"].as_str().unwrap();
            let mut options = case["compile_options"].as_object().unwrap().clone();
            options.extend(case["options"].as_object().unwrap().clone());
            let interval = options.remove("animateInterval").unwrap();
            options.insert("animate_interval".into(), interval);
            let output = render_d2_source_impl(
                case["input"].as_str().unwrap(),
                &serde_json::to_string(&options).unwrap(),
            )
            .unwrap();
            if case["attributes"].is_null() {
                assert!(
                    output.is_empty(),
                    "{name}: folder-only tree must return empty output"
                );
                continue;
            }
            let doc = roxmltree::Document::parse(&output).unwrap();
            let root = doc.root_element();
            let inner = root
                .children()
                .find(|n| n.is_element() && n.tag_name().name() == "svg")
                .unwrap();
            for (node, key) in [(root, "attributes"), (inner, "inner_attributes")] {
                for (attribute, expected) in case[key].as_object().unwrap() {
                    assert_eq!(
                        node.attribute(attribute.as_str()),
                        expected.as_str(),
                        "{name} {key} {attribute}"
                    );
                }
            }
            assert_eq!(
                output.matches("animation: d2Transition-").count(),
                case["transitions"].as_u64().unwrap() as usize,
                "{name}"
            );
            assert!(
                output.contains(&format!(
                    "d2Transition-{}-0",
                    case["hash"].as_str().unwrap()
                )),
                "{name}: hash differs"
            );
        }
    }

    #[cfg(feature = "oracle-fixtures")]
    #[test]
    fn single_board_animation_matches_original_go_targets_and_bounds() {
        let cases: Vec<serde_json::Value> = serde_json::from_str(d2_rs_testdata::oracle_fixture!(
            "tests/fixtures/animation-single-board-source-v090.json"
        ))
        .unwrap();
        assert_eq!(cases.len(), 20);
        for case in cases {
            let name = case["name"].as_str().unwrap();
            let mut options = case["compile_options"].as_object().unwrap().clone();
            options.extend(case["options"].as_object().unwrap().clone());
            let interval = options.remove("animateInterval").unwrap();
            options.insert("animate_interval".into(), interval);
            let options_json = serde_json::to_string(&options).unwrap();
            let source = case["input"].as_str().unwrap();
            let output = render_d2_source_impl(source, &options_json).unwrap();
            let document = roxmltree::Document::parse(&output).unwrap();
            let root = document.root_element();
            let inner = root
                .children()
                .find(|n| n.is_element() && n.tag_name().name() == "svg")
                .unwrap();
            for (node, key) in [(root, "attributes"), (inner, "inner_attributes")] {
                for (attribute, expected) in case[key].as_object().unwrap() {
                    assert_eq!(
                        node.attribute(attribute.as_str()),
                        expected.as_str(),
                        "{name} {key} {attribute}"
                    );
                }
            }
            assert_eq!(
                output.matches("animation: d2Transition-").count(),
                case["transitions"].as_u64().unwrap() as usize,
                "{name}"
            );
            assert!(
                output.contains(&format!(
                    "d2Transition-{}-0",
                    case["hash"].as_str().unwrap()
                )),
                "{name}: animation hash differs"
            );
            // Nested Markdown/equation SVG and their labels must survive the
            // conversion of the board wrapper to an animation group.
            options.insert("animate_interval".into(), serde_json::json!(0));
            let ordinary =
                render_single_board_svg_impl(source, &serde_json::to_string(&options).unwrap())
                    .unwrap();
            let ordinary_doc = roxmltree::Document::parse(&ordinary).unwrap();
            for tag in ["svg", "text", "foreignObject", "path", "rect"] {
                let count = |doc: &roxmltree::Document<'_>| {
                    doc.descendants()
                        .filter(|n| {
                            n.is_element()
                                && n.tag_name().name() == tag
                                && !n.ancestors().any(|parent| parent.has_tag_name("defs"))
                        })
                        .count()
                };
                assert_eq!(
                    count(&document),
                    count(&ordinary_doc),
                    "{name}: incomplete {tag} inventory"
                );
            }
        }
    }

    #[test]
    fn wasm_source_render_matches_v090_multiboard_contract() {
        let source = "x -> y\nlayers: { n: { a -> b } }\n";
        let error = render_d2_source_impl(source, r#"{"target":"*"}"#).unwrap_err();
        assert!(error.contains("only supported for animated SVGs"));
        let output = render_d2_source_impl(
            source,
            r#"{"target":"*","animate_interval":1000,"seeds":[7]}"#,
        )
        .unwrap();
        assert!(output.starts_with("<?xml version=\"1.0\""));
        assert!(output.contains("d2Transition-"));
        // HashID is Diagram.Bytes over the root plus descendants. These are
        // the pinned Go 1.27/v0.9.0 values for this witness.
        assert!(output.contains("d2Transition-d2-912644653-0"));
        assert!(output.contains("d2-3603291771"));
        assert!(output.matches("animation: d2Transition-").count() >= 2);
        assert!(output.contains("n") || output.contains("a"));
        let tala_output = render_d2_source_impl(
            source,
            r#"{"target":"*","animate_interval":100,"engine":"tala","seeds":[1]}"#,
        )
        .unwrap();
        assert!(tala_output.contains("d2Transition-d2-3671043608-0"));
    }

    #[test]
    fn wasm_source_render_rejects_ascii_multiboard_output() {
        let source = "x -> y\nlayers: { numbers: { 1 -> 2 } }\n";
        let error = render_d2_source_impl(
            source,
            r#"{"target":"*","animate_interval":1000,"ascii":true}"#,
        )
        .unwrap_err();
        assert!(error.contains("ASCII rendering does not support multi-board animation"));
    }

    #[test]
    fn renders_source_to_ascii_in_wasm_boundary() {
        let extended = render_d2_source_impl("a -> b\n", r#"{"ascii":true}"#).unwrap();
        assert!(!extended.contains("<svg"));
        assert!(extended.contains('a') && extended.contains('b'));
        let standard =
            render_d2_source_impl("a -> b\n", r#"{"ascii":true,"ascii_mode":"standard"}"#).unwrap();
        assert!(
            !['┌', '┐', '└', '┘', '│', '─']
                .iter()
                .any(|character| standard.contains(*character))
        );
    }

    #[cfg(feature = "oracle-fixtures")]
    #[test]
    fn source_ascii_caller_matches_complete_original_go_outcomes() {
        let rows: Vec<serde_json::Value> = serde_json::from_str(d2_rs_testdata::load_str(
            concat!(env!("CARGO_MANIFEST_DIR"), "/../d2-rs-export"),
            "tests/fixtures/ascii-callers-source-v090.json",
        ))
        .unwrap();
        assert_eq!(rows.len(), 192);
        for row in rows {
            let options = serde_json::json!({"ascii":true,"engine":row["engine"],"ascii_mode":row["mode"],"scale":row["scale"]});
            let output =
                render_d2_source_impl(row["source"].as_str().unwrap(), &options.to_string())
                    .unwrap();
            assert_eq!(output, row["output"].as_str().unwrap(), "{}", row["name"]);
        }
    }

    #[cfg(feature = "oracle-fixtures")]
    #[test]
    fn wasm_source_render_applies_padding_and_scale_options() {
        let default_output = render_d2_source_impl("a\n", "").unwrap();
        let padded_output = render_d2_source_impl("a\n", r#"{"pad":0}"#).unwrap();
        let scaled_output = render_d2_source_impl("a\n", r#"{"scale":2}"#).unwrap();
        let view_box_width = |output: &str| {
            output
                .split("viewBox=\"")
                .nth(1)
                .and_then(|value| value.split('"').next())
                .and_then(|value| value.split_whitespace().nth(2))
                .and_then(|value| value.parse::<f64>().ok())
                .expect("numeric viewBox width")
        };
        assert_eq!(
            view_box_width(&default_output) - view_box_width(&padded_output),
            200.0
        );
        assert_eq!(
            view_box_width(&scaled_output),
            view_box_width(&default_output)
        );
        // Original d2svg.Render scales the outer width/height and retains
        // the coordinate-system viewBox. The source fixture binds both.
        assert_original_render_options("default", &default_output);
        assert_original_render_options("pad", &padded_output);
        assert_original_render_options("scale", &scaled_output);
    }

    #[cfg(feature = "oracle-fixtures")]
    #[test]
    fn wasm_source_render_preserves_explicit_scale_presence() {
        let cases: serde_json::Value = serde_json::from_str(d2_rs_testdata::oracle_fixture!(
            "tests/fixtures/svg-scale-presence-source-v090.json"
        ))
        .unwrap();
        for case in cases.as_array().unwrap() {
            let svg = render_d2_source_impl("a -> b\n", &case["options"].to_string()).unwrap();
            let root = svg
                .split("<svg ")
                .nth(1)
                .unwrap()
                .split('>')
                .next()
                .unwrap();
            for (name, expected) in case["svg_attributes"].as_object().unwrap() {
                let prefix = format!("{name}=\"");
                let actual = root.split(&prefix).nth(1).and_then(|s| s.split('"').next());
                assert_eq!(
                    actual,
                    expected.as_str(),
                    "options={} {name}",
                    case["options"]
                );
            }
        }
    }

    #[test]
    fn wasm_source_render_applies_theme_palette() {
        let output = render_d2_source_impl("a\n", r#"{"theme":1}"#).unwrap();
        assert!(output.contains("fill=\"#EEF1F8\""));
        assert!(output.contains("stroke=\"#0A0F25\""));
    }

    #[test]
    fn wasm_source_render_applies_center_option() {
        let output = render_d2_source_impl("a\n", r#"{"center":true}"#).unwrap();
        assert!(output.contains("preserveAspectRatio=\"xMidYMid meet\""));
    }

    #[cfg(feature = "oracle-fixtures")]
    #[test]
    fn wasm_source_render_applies_sketch_option() {
        let output = render_d2_source_impl("a: box\n", r#"{"sketch":true}"#).unwrap();
        assert_original_render_options("sketch", &output);
    }

    #[cfg(feature = "oracle-fixtures")]
    #[test]
    fn wasm_source_render_hydrates_root_render_config() {
        let output = render_d2_source_impl(
            "vars: {d2-config: {theme-id: 1; center: true; sketch: true; pad: 7}}\na: box\n",
            "",
        )
        .unwrap();
        assert!(output.contains("preserveAspectRatio=\"xMidYMid meet\""));
        assert_original_render_options("config", &output);
        assert!(output.contains("fill-B6"));
        assert!(output.contains("#EEF1F8"));
        assert!(output.contains("viewBox=\""));
    }

    #[cfg(feature = "oracle-fixtures")]
    fn assert_original_render_options(name: &str, output: &str) {
        let fixture: serde_json::Value = serde_json::from_str(d2_rs_testdata::oracle_fixture!(
            "tests/fixtures/render-options-source-v090.json"
        ))
        .unwrap();
        let case = fixture["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["name"] == name)
            .unwrap();
        let outer_svg = output
            .split("<svg ")
            .nth(1)
            .unwrap()
            .split('>')
            .next()
            .unwrap();
        for (key, value) in case["svg_attributes"].as_object().unwrap() {
            let attribute = format!("{key}=\"{}\"", value.as_str().unwrap());
            assert!(
                outer_svg.contains(&attribute),
                "{name}: missing {attribute}"
            );
        }
        // d2svg now preserves the pinned Go rough path bytes themselves.
        let document = roxmltree::Document::parse(output).unwrap();
        let expected_paths = case["shape_paths"].as_array().unwrap();
        for path in expected_paths {
            assert!(
                document.descendants().any(|node| {
                    node.has_tag_name("path") && node.attribute("d") == path.as_str()
                }),
                "{name}: missing original rough path {path}"
            );
        }
    }

    #[test]
    fn wasm_source_render_salts_fragment_ids() {
        // Keep this target-identity witness on TALA: the test asserts the
        // salted ID rewrite, while the public source default follows D2's
        // Dagre selection and has a different layout hash.
        let output =
            render_d2_source_impl("a -> b\n", r#"{"engine":"tala","salt":"worker-2"}"#).unwrap();
        assert!(output.contains("<g class=\"YQ==\">"));
        assert!(output.contains("<g class=\"Yg==\">"));
        assert!(output.contains("<g class=\"KGEgLSZndDsgYilbMF0=\">"));
        assert!(output.contains("id=\"mk-d2-3951373038-3488378134\""));
        assert!(output.contains("marker-end=\"url(#mk-d2-3951373038-3488378134)\""));
        assert!(output.contains("mask=\"url(#d2-3951373038)\""));
        assert!(output.contains("<mask id=\"d2-3951373038\""));
    }

    #[test]
    fn wasm_source_render_applies_theme_overrides() {
        let output = render_d2_source_impl(
            "a -> b\n",
            r##"{"theme_overrides":{"B1":"#123456","B6":"#abcdef","N1":"#010203"}}"##,
        )
        .unwrap();
        assert!(output.contains("stroke=\"#123456\""));
        assert!(output.contains("fill=\"#abcdef\""));
        assert!(output.contains("fill=\"#010203\""));
    }

    #[test]
    fn explicit_empty_theme_override_maps_clear_source_config() {
        let source = "vars: {d2-config: {theme-overrides: {B1: '#123456'}; dark-theme-overrides: {N1: '#abcdef'}}}\na -> b\n";
        let inherited = render_d2_source_impl(source, r#"{"dark_theme_id":200}"#).unwrap();
        assert!(inherited.contains("#123456"));
        assert!(inherited.contains("#abcdef"));

        let cleared = render_d2_source_impl(
            source,
            r#"{"dark_theme_id":200,"theme_overrides":{},"dark_theme_overrides":{}}"#,
        )
        .unwrap();
        assert!(!cleared.contains("#123456"));
        assert!(!cleared.contains("#abcdef"));
    }

    #[test]
    fn wasm_source_render_embeds_supplied_asset_data() {
        let encoded = base64::engine::general_purpose::STANDARD.encode(b"<svg></svg>");
        let options = format!(r##"{{"asset_data":{{"asset.svg":"{encoded}"}}}}"##);
        let output =
            render_d2_source_impl("a: {shape: image; icon: asset.svg}\n", &options).unwrap();
        assert!(output.contains("href=\"data:image/svg+xml;base64,PHN2Zz48L3N2Zz4=\""));
    }

    #[test]
    fn wasm_source_render_applies_dark_theme_palette_and_overrides() {
        let output = render_d2_source_impl(
            "a -> b\n",
            r##"{"dark_theme_id":200,"dark_theme_overrides":{"N1":"#ffffff"}}"##,
        )
        .unwrap();
        assert!(output.contains("prefers-color-scheme:dark"));
        assert!(output.contains("#CBA6f7"));
        assert!(output.contains("#ffffff"));
    }

    #[test]
    fn renders_markdown_html_in_wasm_boundary() {
        let output = render_markdown_html("**bold** [link](https://example.com)");
        assert!(output.contains("<strong>bold</strong>"));
        assert!(output.contains("href=\"https://example.com\""));
    }

    #[cfg(feature = "oracle-fixtures")]
    #[test]
    fn sequence_outside_actor_exported_geometry_matches_original_successful_targets() {
        let fixture: serde_json::Value = serde_json::from_str(d2_rs_testdata::oracle_fixture!(
            "tests/fixtures/sequence-outside-actor-source-v090.json"
        ))
        .unwrap();
        let cases = fixture["cases"].as_array().unwrap();
        assert_eq!(cases.len(), 18);
        for case in cases {
            let (graph, result, _, _, _) =
                source_layout_impl_with_cancellation(case["source"].as_str().unwrap(), "{}", None)
                    .unwrap();
            let expected = case["diagram"]["shapes"].as_array().unwrap();
            assert_eq!(graph.nodes().count(), expected.len());
            for shape in expected {
                let (id, _) = graph
                    .nodes()
                    .find(|(_, node)| node.external_id == shape["id"].as_str().unwrap())
                    .unwrap();
                let actual = result.boxes[&id];
                // Original d2exporter.toShape casts the layout box to int;
                // raw float geometry is a distinct, unqualified contract.
                for (name, value) in [("width", actual.size.width), ("height", actual.size.height)]
                {
                    assert_eq!(
                        value.trunc(),
                        shape[name].as_f64().unwrap(),
                        "{} {name}: {}",
                        shape["id"],
                        case["source"]
                    );
                }
                for (name, value) in [("x", actual.origin.x), ("y", actual.origin.y)] {
                    assert_eq!(
                        value.trunc(),
                        shape["pos"][name].as_f64().unwrap(),
                        "{} {name}: {}",
                        shape["id"],
                        case["source"]
                    );
                }
            }
        }
    }

    #[test]
    fn wasm_markdown_measurer_preserves_source_font_roles_and_line_layout() {
        let measurer = WasmFontGraphMeasurer::bundled().expect("bundled fonts");
        for (label, mono, width, height) in [
            ("# mono heading", true, 231.0, 51.0),
            ("[inner](https://inner.example)", false, 36.0, 24.0),
            ("Kubernetes Integrations", false, 162.0, 24.0),
            (
                "a := 5\nb := a + 7\nfmt.Printf(\"%d\", b)",
                false,
                225.0,
                24.0,
            ),
        ] {
            assert_eq!(
                measurer.markdown_dimensions(label, 16, false, false, mono),
                d2_rs_core::Size { width, height },
                "{label}"
            );
        }
    }

    #[test]
    fn wasm_markdown_measurer_matches_raw_inline_native_dimensions() {
        let measurer = WasmFontGraphMeasurer::bundled().expect("bundled fonts");
        let dimensions = compiler::GraphMeasurer::measure_markdown_label_at_size(
            &measurer,
            "<b>bold</b> <em>italic</em> <s>gone</s> <strike>old</strike><br />line",
            16,
        );
        assert_eq!(dimensions.width, 129.0);
        assert_eq!(dimensions.height, 48.0);
    }

    #[test]
    fn wasm_markdown_measurer_matches_unfetched_image_dimensions() {
        let measurer = WasmFontGraphMeasurer::bundled().expect("bundled fonts");
        let dimensions = compiler::GraphMeasurer::measure_markdown_label_at_size(
            &measurer,
            "before ![build passing](https://example.invalid/status.svg) after",
            16,
        );
        assert_eq!(dimensions.width, 83.0);
        assert_eq!(dimensions.height, 24.0);
    }

    #[test]
    fn returns_source_completions_in_wasm_boundary() {
        let output =
            complete_d2_source_impl("node: {shape: }", r#"{"line":0,"column":14}"#).unwrap();
        let output: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert!(
            output
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["label"] == "rectangle")
        );
    }

    #[cfg(feature = "oracle-fixtures")]
    #[test]
    fn legacy_completion_export_uses_utf16_columns() {
        let master: serde_json::Value = serde_json::from_str(d2_rs_testdata::oracle_fixture!(
            "tests/fixtures/render-regressions-master.json"
        ))
        .unwrap();
        let expected = |name: &str| {
            master["rows"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["kind"] == "completion" && row["name"] == name)
                .unwrap()["got"]
                .clone()
        };
        assert!(!completion_items("node: {shape: }", 0, 14, PositionEncoding::Utf16).is_empty());
        let output: serde_json::Value = serde_json::from_str(&get_completions(
            Some("node: {shape: }".to_owned()),
            Some(0),
            Some(14),
        ))
        .unwrap();
        assert_eq!(output, expected("ascii-completion"));

        let unicode_source = "\"😀\".node: {shape: }";
        let output: serde_json::Value = serde_json::from_str(&get_completions(
            Some(unicode_source.to_owned()),
            Some(0),
            Some(unicode_source.encode_utf16().count() as u32 - 1),
        ))
        .unwrap();
        assert_eq!(output, expected("unicode-completion"));
    }

    #[test]
    fn returns_board_path_in_wasm_boundary() {
        let output = board_at_position_json_impl(
            "layers: {\n  one: { service: api }\n}\n",
            r#"{"line":1,"column":15}"#,
        )
        .unwrap();
        assert_eq!(output, r#"["layers","one"]"#);
    }

    #[test]
    fn preserves_nil_board_path_in_both_wasm_boundaries() {
        let source = "x -> y\nlayers: { basic: { a -> b } }\n";
        let output = board_at_position_json_impl(source, r#"{"line":0,"column":0}"#).unwrap();
        assert_eq!(output, "null");

        let legacy: serde_json::Value = serde_json::from_str(&get_board_at_position(
            Some(source.to_owned()),
            Some(0),
            Some(0),
        ))
        .unwrap();
        assert!(legacy["data"]["boardPath"].is_null());
        assert_eq!(legacy["data"]["err"], "");
    }

    #[test]
    fn returns_reference_ranges_in_wasm_boundary() {
        let output = reference_ranges_json_impl("a: hello\nb: a\n", "a").unwrap();
        let output: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert!(!output["references"].as_array().unwrap().is_empty());
        assert!(output["imports"].is_null());
    }

    #[test]
    fn preserves_nil_reference_slices_in_both_wasm_boundaries() {
        let output = reference_ranges_json_impl("a: hello\n", "missing").unwrap();
        let output: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert!(output["references"].is_null());
        assert!(output["imports"].is_null());

        let legacy: serde_json::Value = serde_json::from_str(&get_ref_ranges(
            Some(r#"{"index.d2":"a: hello\n"}"#.to_owned()),
            Some("index.d2".to_owned()),
            Some("missing".to_owned()),
            Some("[]".to_owned()),
        ))
        .unwrap();
        assert!(legacy["data"]["ranges"].is_null());
        assert!(legacy["data"]["importRanges"].is_null());
    }

    #[test]
    fn returns_virtual_filesystem_reference_ranges_in_wasm_boundary() {
        let request = serde_json::json!({
            "path": "index.d2",
            "files": {
                "index.d2": "...@ok\nhi\nhey: @ok\n",
                "ok.d2": "what\nlala\nokay\n"
            },
            "key": "okay"
        });
        let output = reference_ranges_files_json_impl(&request.to_string()).unwrap();
        let output: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert!(
            output["references"][0]
                .as_str()
                .unwrap()
                .starts_with("ok.d2,")
        );
        assert_eq!(output["imports"].as_array().unwrap().len(), 1);
    }
}

// Reads regenerated oracle fixtures (docs/FIXTURES.md).
#[cfg(all(test, feature = "oracle-fixtures"))]
mod sequence_actor_label_policy_tests;

#[cfg(all(test, feature = "oracle-fixtures"))]
mod sequence_target_export_tests;

// Reads regenerated oracle fixtures (docs/FIXTURES.md).
#[cfg(all(test, feature = "oracle-fixtures"))]
mod sequence_span_export_tests;

#[cfg(all(test, feature = "oracle-fixtures"))]
mod sequence_fault_tests;

#[cfg(all(test, feature = "oracle-fixtures"))]
mod math_nil_child_tests;

// Reads regenerated oracle fixtures (docs/FIXTURES.md).
#[cfg(all(test, feature = "oracle-fixtures"))]
mod root_structured_tests;

#[cfg(test)]
mod public_diagnostic_tests;

// Reads regenerated oracle fixtures (docs/FIXTURES.md).
#[cfg(all(test, feature = "oracle-fixtures"))]
mod public_option_tests;

// Reads regenerated oracle fixtures (docs/FIXTURES.md).
#[cfg(all(test, feature = "oracle-fixtures"))]
mod public_graph_root_tests;

// Reads regenerated oracle fixtures (docs/FIXTURES.md).
#[cfg(all(test, feature = "oracle-fixtures"))]
mod public_graph_inheritance_tests;
// Reads regenerated oracle fixtures (docs/FIXTURES.md).
#[cfg(all(test, feature = "oracle-fixtures"))]
mod public_graph_object_tests;

#[cfg(test)]
mod board_fault_tests {
    use super::*;
    #[test]
    fn nil_board_base_returns_a_recoverable_error() {
        let source = "steps: { a: 1; b: { x } }";
        let error = compile_d2_target_json_impl(source, "{}").unwrap_err();
        assert_eq!(
            error.lines().next(),
            Some(
                "panic recovered: runtime error: invalid memory address or nil pointer dereference"
            )
        );
        let response: serde_json::Value =
            serde_json::from_str(&get_obj_order(Some(source.into()))).unwrap();
        assert_eq!(response["error"]["code"], 500);
        assert_eq!(response["error"]["message"], error);
        // parse.go:1262 faults before Compile can turn ParseError.Errors into
        // diagnostic JSON. wrapWASMCall transports the recovered fault as 500.
        let source = "a: x*${x}a*";
        let expected = "panic recovered: runtime error: slice bounds out of range [2:1]";
        let fault = compile_d2_target_json_typed(source, "{}").unwrap_err();
        assert!(matches!(fault, SourceCompileError::Other(_)));
        assert_eq!(fault.public_message(), expected);
        let response: serde_json::Value =
            serde_json::from_str(&get_obj_order(Some(source.into()))).unwrap();
        assert_eq!(response["error"]["code"], 500);
        assert_eq!(response["error"]["message"], expected);
        let options = serde_json::json!({
            "path": "index.d2", "files": {"broken.d2": source}
        });
        let fault = compile_d2_target_json_typed("...@broken", &options.to_string()).unwrap_err();
        assert!(matches!(fault, SourceCompileError::Other(_)));
        assert_eq!(fault.public_message(), expected);
        assert!(compile_d2_target_json_impl("a -> b", "{}").is_ok());
    }
}
