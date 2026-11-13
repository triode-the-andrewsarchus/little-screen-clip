// SPDX-License-Identifier: MPL-2.0

// Types for the browser and bundler entry point, src/index.js: an ES module
// with named exports and no default export. Node resolves the package to the
// CommonJS src/node.cjs instead, typed by src/node.d.cts.

export type InitInput = RequestInfo | URL | Response | WebAssembly.Module | BufferSource;

export interface Runtime {
  default(input?: InitInput | Promise<InitInput>): Promise<void>;
  layout_graph_json(graphJson: string, optionsJson: string): string;
  layout_graph_json_with_cancellation(graphJson: string, optionsJson: string, token: WasmCancellationToken): string;
  layout_d2_json(inputJson: string, optionsJson: string): string;
  layout_d2_json_with_cancellation(inputJson: string, optionsJson: string, token: WasmCancellationToken): string;
  route_edges_d2_json(inputJson: string, optionsJson: string): string;
  route_edges_d2_json_with_cancellation(inputJson: string, optionsJson: string, token: WasmCancellationToken): string;
  compile_d2_source(source: string, optionsJson: string): string;
  compile_d2_target_json(source: string, optionsJson: string): string;
  compile_d2_public_target_json(source: string, optionsJson: string): string;
  prepare_d2_public_compile_request_json(requestJson: string): string;
  parse_d2_source(source: string, optionsJson: string): string;
  format_d2_source(source: string, optionsJson: string): string;
  layout_d2_source(source: string, optionsJson: string): string;
  layout_d2_source_with_cancellation(source: string, optionsJson: string, token: WasmCancellationToken): string;
  render_d2_source(source: string, optionsJson: string): string;
  render_markdown_html(source: string): string;
  complete_d2_source(source: string, optionsJson: string): string;
  board_at_position_json(source: string, optionsJson: string): string;
  reference_ranges_json(source: string, key: string): string;
  reference_ranges_files_json(requestJson: string): string;
  apply_source_edits_json(requestJson: string): string;
  create_root_field_json(requestJson: string): string;
  set_root_field_json(requestJson: string): string;
  delete_root_field_json(requestJson: string): string;
  rename_root_field_json(requestJson: string): string;
  reconnect_edge_json(requestJson: string): string;
  move_object_json(requestJson: string): string;
  encode(script?: string | null): string;
  decode(encoded?: string | null): string;
  version(): string;
  js_version(): string;
  getParentID(id?: string | null): string;
  getCompletions(source?: string | null, line?: number | null, column?: number | null): string;
  getBoardAtPosition(source?: string | null, line?: number | null, column?: number | null): string;
  getRefRanges(filesJson?: string | null, path?: string | null, key?: string | null, boardPathJson?: string | null): string;
  getObjOrder(source?: string | null): string;
  getELKGraph(requestJson?: string | null): string;
}

export class WasmCancellationToken {
  constructor();
  cancel(): void;
  expire(): void;
  is_cancelled(): boolean;
}

export function init(input?: InitInput | Promise<InitInput>): Promise<Runtime>;
export const load: typeof init;
export function raw(): Promise<Runtime>;

export function layoutGraphJson(graphJson: string, optionsJson?: string): Promise<string>;
export function layoutGraphJsonWithCancellation(graphJson: string, optionsJson: string, token: WasmCancellationToken): Promise<string>;
export function layoutD2Json(inputJson: string, optionsJson?: string): Promise<string>;
export function layoutD2JsonWithCancellation(inputJson: string, optionsJson: string, token: WasmCancellationToken): Promise<string>;
export function routeEdgesD2Json(inputJson: string, optionsJson?: string): Promise<string>;
export function routeEdgesD2JsonWithCancellation(inputJson: string, optionsJson: string, token: WasmCancellationToken): Promise<string>;
export function compileD2Source(source: string, optionsJson?: string): Promise<string>;
export function compileD2TargetJson(source: string, optionsJson?: string): Promise<string>;
export function parseD2Source(source: string, optionsJson?: string): Promise<string>;
export function formatD2Source(source: string, optionsJson?: string): Promise<string>;
export function layoutD2Source(source: string, optionsJson?: string): Promise<string>;
export function layoutD2SourceWithCancellation(source: string, optionsJson: string, token: WasmCancellationToken): Promise<string>;
export function renderD2Source(source: string, optionsJson?: string): Promise<string>;
export function renderMarkdownHtml(source: string): Promise<string>;
export function completeD2Source(source: string, optionsJson?: string): Promise<string>;
export function boardAtPositionJson(source: string, optionsJson?: string): Promise<string>;
export function referenceRangesJson(source: string, key: string): Promise<string>;
export function referenceRangesFilesJson(requestJson: string): Promise<string>;
export function applySourceEditsJson(requestJson: string): Promise<string>;
export function createRootFieldJson(requestJson: string): Promise<string>;
export function setRootFieldJson(requestJson: string): Promise<string>;
export function deleteRootFieldJson(requestJson: string): Promise<string>;
export function renameRootFieldJson(requestJson: string): Promise<string>;
export function reconnectEdgeJson(requestJson: string): Promise<string>;
export function moveObjectJson(requestJson: string): Promise<string>;
export function encode(script?: string | null): Promise<string>;
export function decode(encoded?: string | null): Promise<string>;
export function version(): Promise<string>;
export function jsVersion(): Promise<string>;
export function getParentID(id?: string | null): Promise<string>;
export function getCompletions(source?: string | null, line?: number | null, column?: number | null): Promise<string>;
export function getBoardAtPosition(source?: string | null, line?: number | null, column?: number | null): Promise<string>;
export function getRefRanges(filesJson?: string | null, path?: string | null, key?: string | null, boardPathJson?: string | null): Promise<string>;
export function getObjOrder(source?: string | null): Promise<string>;
export function getELKGraph(requestJson?: string | null): Promise<string>;

export const layout_graph_json: typeof layoutGraphJson;
export const layout_graph_json_with_cancellation: typeof layoutGraphJsonWithCancellation;
export const layout_d2_json: typeof layoutD2Json;
export const layout_d2_json_with_cancellation: typeof layoutD2JsonWithCancellation;
export const route_edges_d2_json: typeof routeEdgesD2Json;
export const route_edges_d2_json_with_cancellation: typeof routeEdgesD2JsonWithCancellation;
export const compile_d2_source: typeof compileD2Source;
export const compile_d2_target_json: typeof compileD2TargetJson;
export const parse_d2_source: typeof parseD2Source;
export const format_d2_source: typeof formatD2Source;
export const layout_d2_source: typeof layoutD2Source;
export const layout_d2_source_with_cancellation: typeof layoutD2SourceWithCancellation;
export const render_d2_source: typeof renderD2Source;
export const render_markdown_html: typeof renderMarkdownHtml;
export const complete_d2_source: typeof completeD2Source;
export const board_at_position_json: typeof boardAtPositionJson;
export const reference_ranges_json: typeof referenceRangesJson;
export const reference_ranges_files_json: typeof referenceRangesFilesJson;
export const apply_source_edits_json: typeof applySourceEditsJson;
export const create_root_field_json: typeof createRootFieldJson;
export const set_root_field_json: typeof setRootFieldJson;
export const delete_root_field_json: typeof deleteRootFieldJson;
export const rename_root_field_json: typeof renameRootFieldJson;
export const reconnect_edge_json: typeof reconnectEdgeJson;
export const move_object_json: typeof moveObjectJson;
export const js_version: typeof jsVersion;

export interface RenderOptions {
  sketch?: boolean | null;
  isometric?: boolean | null;
  themeID?: number | null;
  darkThemeID?: number | null;
  center?: boolean | null;
  pad?: number | null;
  scale?: number | null;
  forceAppendix?: boolean | null;
  target?: string | null;
  animateInterval?: number | null;
  salt?: string | null;
  noXMLTag?: boolean | null;
  ascii?: boolean | null;
  asciiMode?: string | null;
  themeOverrides?: Record<string, string | null> | null;
  darkThemeOverrides?: Record<string, string | null> | null;
  assetData?: Record<string, string> | null;
}

export interface CompileOptions extends RenderOptions {
  layout?: "dagre" | "elk" | "tala";
  /** Base64 strings and byte arrays follow Go's JSON []byte decoding.
   * Uint8Array serializes as an object and is rejected by the original API. */
  fontRegular?: string | number[] | Uint8Array;
  fontItalic?: string | number[] | Uint8Array;
  fontBold?: string | number[] | Uint8Array;
  fontSemibold?: string | number[] | Uint8Array;
}

export interface CompileRequest {
  fs: Record<string, string>;
  inputPath?: string;
  options?: CompileOptions;
}

/** Indexed source/target metadata returned by the Rust compile boundary. */
export interface CompiledGraphNode {
  index: number;
  id: string;
  parent: number | null;
  label?: string | null;
  node: Record<string, unknown>;
  style: Record<string, unknown>;
}

export interface CompiledGraphReference {
  /** Object references carry a KeyPath; edge references omit it. */
  key?: Record<string, unknown>;
  key_path_index?: number;
  map_key_edge_index: number;
}

export interface CompiledGraphEdge {
  /** Equivalent-connection number in the source graph. */
  index: number;
  srcTableColumnIndex?: number;
  dstTableColumnIndex?: number;
  labelPosition?: string;
  labelPercentage?: number;
  isCurve: boolean;
  route?: Point[];
  src_arrow: boolean;
  dst_arrow: boolean;
  srcArrowhead?: Record<string, unknown>;
  dstArrowhead?: Record<string, unknown>;
  attributes: Record<string, unknown>;
  references?: CompiledGraphReference[];
  zIndex: number;
  /** Non-enumerable native diagnostics, present for authored edges. */
  source_index?: number;
  abs_id?: string;
  source?: number;
  target?: number;
  src?: string;
  dst?: string;
  arrows?: Record<string, unknown>;
  arrowheads?: Record<string, unknown>;
  label?: Record<string, unknown> | null;
  style?: Record<string, unknown>;
  link?: string | null;
  tooltip?: string | null;
  icon?: string | null;
  icon_position?: string | null;
  table_columns?: Record<string, number | null>;
  is_curve?: boolean;
}

export interface CompiledGraphObject {
  class?: Class;
  sql_table?: SQLTable;
  id: string;
  id_val: string;
  box?: {
    TopLeft: Point | null;
    Width: number;
    Height: number;
  };
  labelPosition?: string | null;
  iconPosition?: string | null;
  attributes: Record<string, unknown>;
  zIndex: number;
  parent?: string | null;
  references?: CompiledGraphReference[];
  target?: CompiledGraphNode;
}

/** Compiled legend records before layout, distinct from target Legend shapes. */
export interface CompiledGraphLegend {
  label?: string;
  objects?: CompiledGraphObject[];
  edges?: CompiledGraphEdge[];
}

export interface CompiledGraph {
  legend?: CompiledGraphLegend;
  data?: Record<string, unknown>;
  /** Authored native edge diagnostics, excluding generated sequence lifelines. */
  native_edges?: CompiledGraphEdge[];
  nodes: CompiledGraphNode[];
  edges: CompiledGraphEdge[] | null;
  objects: CompiledGraphObject[] | null;
  ast?: Record<string, unknown>;
  boards?: Array<{ path: string; graph: CompiledGraph }>;
  layout: Record<string, unknown>;
  config?: Record<string, unknown> | null;
  direction?: string;
  root_sequence_diagram?: boolean;
  name: string;
  isFolderOnly: boolean;
  root: CompiledGraphObject;
  layers?: CompiledGraph[];
  scenarios?: CompiledGraph[];
  steps?: CompiledGraph[];
}

/** Complete Go CompileResponse render options, after source configuration/defaults. */
export interface CompiledRenderOptions {
  pad: number;
  sketch: boolean;
  isometric: boolean;
  center: boolean;
  themeID: number;
  darkThemeID: number | null;
  themeOverrides: Record<string, string | null> | null;
  darkThemeOverrides: Record<string, string | null> | null;
  scale: number | null;
  forceAppendix: boolean | null;
  target: string | null;
  animateInterval: number | null;
  salt: string | null;
  noXMLTag: boolean | null;
  ascii: boolean | null;
  asciiMode: string | null;
}

export interface CompileResponse {
  fs: Record<string, string>;
  inputPath: string;
  diagram: Diagram;
  graph: CompiledGraph;
  renderOptions: CompiledRenderOptions;
}

export interface Diagram {
  fontFamily?: any; // d2fonts.FontFamily in the original SDK
  name: string;
  isFolderOnly: boolean;
  shapes: Shape[];
  connections: Connection[];
  root: Shape;
  config?: RenderOptions;
  layers?: Array<Diagram | undefined>;
  scenarios?: Array<Diagram | undefined>;
  steps?: Array<Diagram | undefined>;
  description?: string;
  legend?: Legend;
}

export interface Legend {
  shapes?: Shape[];
  connections?: Connection[];
}

export interface Point {
  x: number;
  y: number;
}

export interface ShapeBase {
  id: string;
  type: string;
  pos: Point;
  width: number;
  height: number;
  opacity: number;
  strokeDash: number;
  strokeWidth: number;
  borderRadius: number;
  fill: string;
  fillPattern?: string;
  stroke: string;
  animated: boolean;
  shadow: boolean;
  "3d": boolean;
  multiple: boolean;
  "double-border": boolean;
  tooltip: string;
  link: string;
  icon?: string;
  iconPosition: string;
  labelPosition?: string;
  zIndex: number;
  level: number;
  blend: boolean;
  label: string;
  fontSize: number;
  fontFamily: string;
  color: string;
  labelWidth: number;
  labelHeight: number;
  language?: string;
  italic?: boolean;
  bold?: boolean;
  underline?: boolean;
  contentAspectRatio?: number;
  primaryAccentColor?: string;
  secondaryAccentColor?: string;
  neutralAccentColor?: string;
}

export interface ClassField {
  name: string;
  type: string;
  visibility: string;
  underline: boolean;
}

export interface ClassMethod {
  name: string;
  return: string;
  visibility: string;
  underline: boolean;
}

export interface Class {
  fields: ClassField[] | null;
  methods: ClassMethod[] | null;
}

export interface SQLTable {
  columns: SQLColumn[] | null;
}

export interface Text {
  label: string;
  fontSize: number;
  fontFamily: string;
  language: string;
  color: string;
  italic: boolean;
  bold: boolean;
  underline: boolean;
  labelWidth: number;
  labelHeight: number;
  labelFill?: string;
}

export interface SQLColumn {
  name: Text;
  type: Text;
  constraint: string[] | null;
  reference: string;
}

export interface Shape extends ShapeBase {
  fields?: ClassField[];
  methods?: ClassMethod[];
  columns?: SQLColumn[];
}

export interface Connection {
  id: string;
  src: string;
  dst: string;
  label: string;
  labelWidth: number;
  labelHeight: number;
  route: Point[];
  srcArrow: string;
  dstArrow: string;
  opacity: number;
  strokeDash: number;
  strokeWidth: number;
  stroke: string;
  labelPosition: string;
  labelPercentage: number;
  link: string;
  icon?: string;
  iconPosition?: string;
  animated: boolean;
  tooltip: string;
  zIndex: number;
  isCurve: boolean;
  srcLabel?: TextLabel;
  dstLabel?: TextLabel;
  prettyLink?: string;
}

export interface TextLabel {
  label: string;
  fontSize: number;
  fontFamily: string;
  color: string;
  italic?: boolean;
  bold?: boolean;
  underline?: boolean;
  labelWidth: number;
  labelHeight: number;
}

export class D2 {
  constructor();
  compile(input: string | CompileRequest, options?: CompileOptions): Promise<CompileResponse>;
  render(diagram: Diagram, options?: RenderOptions): Promise<string>;
  encode(script: string): Promise<string>;
  decode(encoded: string): Promise<string>;
  version(): Promise<string>;
  jsVersion(): Promise<string>;
  dispose(): Promise<void>;
}
