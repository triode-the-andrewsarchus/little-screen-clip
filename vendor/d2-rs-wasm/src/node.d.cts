// SPDX-License-Identifier: MPL-2.0

// Types for the Node entry point, src/node.cjs. That file is CommonJS and
// assigns one object to `module.exports`, so Node's ESM loader exposes it only
// as the default export:
//
//   import d2rs from "@d2-rs/wasm";      // ESM
//   const { D2 } = require("@d2-rs/wasm"); // CommonJS
//
// A named ESM import such as `import { D2 } from "@d2-rs/wasm"` fails in Node.
// Unlike the browser entry point, the lower-level functions are the
// synchronous wasm-bindgen bindings and return strings directly, the module
// initializes on `require`, there is no `init()`, and `load()` and `raw()`
// resolve to this same object.

import type * as Api from "./index.js" with { "resolution-mode": "import" };
import type * as Raw from "../pkg/node/d2_rs_wasm.cjs";

type Sync<F> = F extends (...args: infer A) => Promise<infer R> ? (...args: A) => R : never;
type FacadeExports = "init" | "load" | "raw" | "D2" | "WasmCancellationToken";
type Functions = { [K in Exclude<keyof typeof Api, FacadeExports>]: Sync<(typeof Api)[K]> };

interface NodeExports extends Omit<Api.Runtime, "default" | keyof Functions>, Functions {
  D2: typeof Api.D2;
  WasmCancellationToken: typeof Raw.WasmCancellationToken;
  load(): Promise<NodeExports>;
  raw(): Promise<NodeExports>;
}

declare const d2rs: NodeExports;

declare namespace d2rs {
  export type D2 = Api.D2;
  export type WasmCancellationToken = Raw.WasmCancellationToken;
  export type RenderOptions = Api.RenderOptions;
  export type CompileOptions = Api.CompileOptions;
  export type CompileRequest = Api.CompileRequest;
  export type CompileResponse = Api.CompileResponse;
  export type CompiledRenderOptions = Api.CompiledRenderOptions;
  export type CompiledGraph = Api.CompiledGraph;
  export type CompiledGraphNode = Api.CompiledGraphNode;
  export type CompiledGraphEdge = Api.CompiledGraphEdge;
  export type CompiledGraphObject = Api.CompiledGraphObject;
  export type CompiledGraphReference = Api.CompiledGraphReference;
  export type CompiledGraphLegend = Api.CompiledGraphLegend;
  export type Diagram = Api.Diagram;
  export type Legend = Api.Legend;
  export type Point = Api.Point;
  export type ShapeBase = Api.ShapeBase;
  export type Shape = Api.Shape;
  export type Connection = Api.Connection;
  export type TextLabel = Api.TextLabel;
  export type Text = Api.Text;
  export type Class = Api.Class;
  export type ClassField = Api.ClassField;
  export type ClassMethod = Api.ClassMethod;
  export type SQLTable = Api.SQLTable;
  export type SQLColumn = Api.SQLColumn;
}

export = d2rs;
