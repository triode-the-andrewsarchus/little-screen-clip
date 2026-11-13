// SPDX-License-Identifier: MPL-2.0

// Public D2.js-compatible facade shared by the browser ESM and Node entry
// points. The Rust/WASM module remains the algorithm boundary; this file only
// assembles the request/response contract and keeps lifecycle state.

function sourceRenderResponse(output, options) {
  const target = options?.target;
  if (output === "" && options?.animateInterval > 0 && typeof target === "string" &&
      (target === "*" || target.endsWith(".*"))) {
    // With no rendered boards Go returns a nil []byte, serialized as null.
    // The original worker passes that null to atob, then decodes its bytes.
    return new TextDecoder().decode(Uint8Array.from(globalThis.atob(null), c => c.charCodeAt(0)));
  }
  return output;
}

function runtimeMethod(runtime, camel, snake = camel) {
  const method = runtime[camel] || runtime[snake];
  if (typeof method !== "function") {
    throw new Error(`WASM runtime is missing ${camel}`);
  }
  return method.bind(runtime);
}

function base64Bytes(value) {
  if (value === undefined || value === null) return undefined;
  if (typeof value === "string") return value;
  const bytes = value instanceof Uint8Array ? value : new Uint8Array(value);
  let binary = "";
  const chunkSize = 0x8000;
  for (let offset = 0; offset < bytes.length; offset += chunkSize) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + chunkSize));
  }
  return globalThis.btoa(binary);
}

function sourceOptions(options, files, inputPath) {
  const canonicalOverrides = (value) => value && Object.fromEntries(
    Object.entries(value).filter(([, color]) => color !== null).map(([key, color]) => [key.toUpperCase(), color]),
  );
  const result = {
    utf16_positions: true,
    path: inputPath,
    files,
    engine: options.layout,
    seeds: options.seeds,
    max_concurrency: options.maxConcurrency,
    target: options.target,
    pad: options.pad,
    scale: options.scale,
    theme_id: options.themeID,
    dark_theme_id: options.darkThemeID,
    center: options.center,
    sketch: options.sketch,
    isometric: options.isometric,
    salt: options.salt,
    force_appendix: options.forceAppendix,
    no_xml_tag: options.noXMLTag,
    ascii: options.ascii,
    ascii_mode: options.asciiMode,
    animate_interval: options.animateInterval,
    theme_overrides: canonicalOverrides(options.themeOverrides),
    dark_theme_overrides: canonicalOverrides(options.darkThemeOverrides),
    asset_data: options.assetData,
    font_regular: base64Bytes(options.fontRegular),
    font_italic: base64Bytes(options.fontItalic),
    font_bold: base64Bytes(options.fontBold),
    font_semibold: base64Bytes(options.fontSemibold),
  };
  return Object.fromEntries(Object.entries(result).filter(([, value]) => value !== undefined && value !== null));
}

function mergeRenderOptions(options, config) {
  // d2lib.applyDefaults feeds the resolved sketch flag back to Compile's
  // render options even when neither the caller nor source config sets it.
  const effectiveSketch = options.sketch ?? config?.sketch ?? false;
  if (!config) return { ...options, sketch: effectiveSketch };
  const fromConfig = {};
  if (config.sketch !== null && config.sketch !== undefined) fromConfig.sketch = config.sketch;
  if (config.isometric !== null && config.isometric !== undefined) fromConfig.isometric = config.isometric;
  if (config.theme_id !== null && config.theme_id !== undefined) fromConfig.themeID = config.theme_id;
  if (config.dark_theme_id !== null && config.dark_theme_id !== undefined) {
    fromConfig.darkThemeID = config.dark_theme_id;
  }
  if (config.pad !== null && config.pad !== undefined) fromConfig.pad = config.pad;
  if (config.center !== null && config.center !== undefined) fromConfig.center = config.center;
  if (config.layout_engine) fromConfig.layout = config.layout_engine;
  if (config.theme_overrides && Object.keys(config.theme_overrides).length) {
    fromConfig.themeOverrides = Object.fromEntries(
      Object.entries(config.theme_overrides).map(([key, value]) => [key.toLowerCase(), value]),
    );
  }
  if (config.dark_theme_overrides && Object.keys(config.dark_theme_overrides).length) {
    fromConfig.darkThemeOverrides = Object.fromEntries(
      Object.entries(config.dark_theme_overrides).map(([key, value]) => [key.toLowerCase(), value]),
    );
  }
  return { ...fromConfig, ...options, sketch: effectiveSketch };
}

function flattenNodes(node, output = []) {
  for (const child of node?.children || []) {
    output.push(child);
    flattenNodes(child, output);
  }
  return output;
}

function numberValue(value, fallback) {
  if (value === undefined || value === null || value === "") return fallback;
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : fallback;
}

function booleanValue(value, fallback = false) {
  if (typeof value === "boolean") return value;
  if (value === undefined || value === null || value === "") return fallback;
  return ["1", "true", "t", "yes"].includes(String(value).toLowerCase());
}

function targetPosition(value) {
  return typeof value === "string" ? value.toLowerCase() : "";
}

function astString(value) {
  if (typeof value === "string") return value;
  if (Array.isArray(value)) return value.map(astString).join("");
  if (!value || typeof value !== "object") return "";
  if (typeof value.string === "string") return value.string;
  if (typeof value.raw_string === "string") return value.raw_string;
  for (const key of [
    "unquoted_string",
    "double_quoted_string",
    "single_quoted_string",
    "block_string",
    "string",
    "value",
  ]) {
    if (value[key] !== undefined) return astString(value[key]);
  }
  return "";
}

function astPath(path) {
  return (path || []).map(astString).filter(Boolean).join(".");
}

function astReferenceIndex(ast) {
  const fields = new Map();
  const edges = new Map();
  const add = (table, key, value) => {
    const values = table.get(key) || [];
    values.push(value);
    table.set(key, values);
  };
  const visit = (map, prefix = []) => {
    for (const wrapper of map?.nodes || []) {
      const key = wrapper?.map_key;
      if (!key) continue;
      const fieldPath = key.key?.path ? [...prefix, astPath(key.key.path)] : null;
      const addPath = (path, edgeIndex) => {
        const parts = path?.path || [];
        for (let i = 0; i < parts.length; i++) {
          const id = [...prefix, ...parts.slice(0, i + 1).map(astString)].join(".");
          add(fields, id, {
            key: path,
            key_path_index: i,
            map_key_edge_index: edgeIndex,
          });
        }
      };
      if (fieldPath) addPath(key.key, -1);
      for (const [edgeIndex, edge] of (key.edges || []).entries()) {
        addPath(edge.src, edgeIndex);
        addPath(edge.dst, edgeIndex);
        const src = [...prefix, astPath(edge.src?.path)].join(".");
        const dst = [...prefix, astPath(edge.dst?.path)].join(".");
        add(edges, `${src}\u0000${dst}`, { map_key_edge_index: edgeIndex });
      }
      if (key.value?.map) visit(key.value.map, fieldPath || prefix);
    }
  };
  visit(ast);
  return { fields, edges };
}

// Project the indexed Rust target into the serializable portion of the
// v0.9.0 d2graph.Graph shape. Go intentionally omits pointer-only fields such
// as Object.Graph/Object.Parent and Edge.Src/Edge.Dst from JSON; keep endpoint
// indexes and the Rust target metadata alongside the public object records so
// both API families remain usable without introducing cycles.
function makeCompiledGraph(target, layout, inputPath) {
  const metadataNodes = target?.nodes || [];
  const boxes = layout?.boxes || {};
  const labelStates = layout?.node_labels || {};
  const byIndex = new Map(metadataNodes.map((node) => [node.index, node]));
  const references = astReferenceIndex(target?.ast);
  const objectFor = (metadata) => {
    const node = metadata.node || {};
    const style = metadata.style || {};
    const state = labelStates[String(metadata.index)] || {};
    const box = boxes[String(metadata.index)] || {};
    const size = box.size || node.size || {};
    const origin = box.origin || { x: 0, y: 0 };
    const labelSize = state.size || node.label_size || { width: 0, height: 0 };
    return {
      id: metadata.id,
      id_val: metadata.id,
      box: {
        TopLeft: { x: numberValue(origin.x, 0), y: numberValue(origin.y, 0) },
        Width: numberValue(size.width, 0),
        Height: numberValue(size.height, 0),
      },
      labelPosition: state.position || null,
      iconPosition: state.icon_position || node.icon_position || null,
      attributes: {
        label: { value: metadata.label || "" },
        labelDimensions: {
          width: numberValue(labelSize.width, 0),
          height: numberValue(labelSize.height, 0),
        },
        style,
        iconStyle: {},
        icon: style.icon || null,
        tooltip: style.tooltip ? { value: style.tooltip } : null,
        link: style.link ? { value: style.link } : null,
        shape: { value: node.shape || "rectangle" },
        direction: { value: node.direction || "" },
        constraint: null,
      },
      zIndex: 0,
      parent: metadata.parent === null || metadata.parent === undefined
        ? null
        : byIndex.get(metadata.parent)?.id || null,
      references: references.fields.get(metadata.id) || [],
      target: metadata,
    };
  };
  const objects = metadataNodes.map(objectFor);
  const edges = (target?.edges || []).map((edge, index) => {
    const sourceObject = byIndex.get(edge.source);
    const targetObject = byIndex.get(edge.target);
    const label = edge.label || {};
    const arrows = edge.arrows || {};
    const arrowheads = edge.arrowheads || {};
    const style = edge.style || {};
    const pair = `${sourceObject?.id || ""}\u0000${targetObject?.id || ""}`;
    return {
      ...edge,
      index: edge.source_index ?? edge.index ?? index,
      source: edge.source,
      target: edge.target,
      src: sourceObject?.id || "",
      dst: targetObject?.id || "",
      src_arrow: Boolean(arrows.source),
      dst_arrow: Boolean(arrows.target),
      route: layout?.routes?.[String(index)] || [],
      attributes: {
        label: { value: label.text || "" },
        labelDimensions: label.size || { width: 0, height: 0 },
        style,
        iconStyle: {},
        shape: { value: "" },
        direction: { value: "" },
        constraint: null,
      },
      references: references.edges.get(pair) || [],
      zIndex: 0,
    };
  });
  const graph = target?.source_graph || {
    name: target?.source_graph_name ?? inputPath,
    isFolderOnly: metadataNodes.length === 0,
    ast: target?.ast,
    root: {
      id: "",
      id_val: "",
      box: { TopLeft: { x: 0, y: 0 }, Width: 0, Height: 0 },
      attributes: {
        label: { value: "" },
        labelDimensions: { width: 0, height: 0 },
        style: {},
        iconStyle: {},
        shape: { value: "" },
        direction: { value: target?.direction || "" },
        constraint: null,
      },
      zIndex: 0,
    },
    objects: objects.length ? objects : null,
    edges: edges.length ? edges : null,
  };
  const sourceFields = new Set(["id", "id_val", "box", "labelPosition", "iconPosition", "contentAspectRatio", "class", "sql_table", "references", "attributes", "zIndex", "index", "srcTableColumnIndex", "dstTableColumnIndex", "labelPercentage", "isCurve", "route", "src_arrow", "dst_arrow", "srcArrowhead", "dstArrowhead"]);
  const attachNativeFields = (projected, native) => {
    for (const [key,value] of Object.entries(native || {})) {
      if (!(key in projected) && !sourceFields.has(key)) Object.defineProperty(projected,key,{value,enumerable:false});
    }
    return projected;
  };
  if (target?.source_graph) {
    for (const [index,object] of (graph.objects || []).entries()) attachNativeFields(object,objects[target.source_graph_object_indices?.[index] ?? index]);
    for (const [index,edge] of (graph.edges || []).entries()) attachNativeFields(edge,edges[index]);
  }
  Object.defineProperty(graph,"native_edges",{value:edges,enumerable:false});
  if (target?.source_graph_root) graph.root = target.source_graph_root;
  if (target?.source_graph_theme) graph.theme = target.source_graph_theme;
  if (target?.source_graph_data && Object.keys(target.source_graph_data).length) {
    graph.data = target.source_graph_data;
  }
  // Keep native compatibility metadata readable without serializing it as
  // fictitious Go Graph fields. Raw Rust helpers remain the explicit API.
  for (const [key,value] of Object.entries(target || {})) {
    if (!(key in graph)) Object.defineProperty(graph,key,{value,enumerable:false});
  }
  return graph;
}

function boardPathParts(path) {
  return String(path || "").split(".").filter(Boolean);
}

function boardName(path) {
  const parts = boardPathParts(path);
  return parts[parts.length - 1] || "root";
}

function boardParentPath(path) {
  const parts = boardPathParts(path);
  return parts.slice(0, -2).join(".");
}

function diagramBoardPath(options) {
  const target = options?.target;
  if (!target || target === "*") return "";
  return String(target).replace(/\.\*$/, "");
}

function makeCompiledGraphTree(target, inputPath) {
  const root = makeCompiledGraph(target, target?.layout || {}, inputPath);
  const graphs = new Map([["", root]]);
  for (const entry of target?.boards || []) {
    const path = entry?.path || "";
    const parts = entry.path_parts ?? boardPathParts(path);
    if (parts.length < 2 || !entry?.graph) continue;
    const wrapper = parts[parts.length - 2];
    const parent = graphs.get(entry.parent_path ?? boardParentPath(path));
    if (!parent) continue;
    const child = makeCompiledGraph(entry.graph, entry.graph.layout || {}, parts[parts.length - 1]);
    parent[wrapper] ??= [];
    parent[wrapper].push(child);
    graphs.set(path, child);
  }
  return root;
}

function makeTargetDiagramTree(target, source, files, inputPath, options) {
  const root = makeDiagram(
    target,
    target?.layout || {},
    source,
    files,
    inputPath,
    options,
    "",
  );
  const diagrams = new Map([["", root]]);
  for (const entry of target?.boards || []) {
    const path = entry?.path || "";
    const parts = entry.path_parts ?? boardPathParts(path);
    if (parts.length < 2 || !entry?.graph) continue;
    const wrapper = parts[parts.length - 2];
    const parent = diagrams.get(entry.parent_path ?? boardParentPath(path));
    if (!parent) continue;
    parent[wrapper] ??= [];
    const childOptions = { ...options, target: path };
    const child = makeDiagram(
      entry.graph,
      entry.graph.layout || {},
      source,
      files,
      inputPath,
      childOptions,
      path,
    );
    parent[wrapper].push(child);
    diagrams.set(path, child);
  }
  return root;
}

function makeTargetDiagram(target, layout, source, files, inputPath, options) {
  if (!target.diagram) throw new Error("native target diagram missing");
  return target.diagram;
}

function makeDiagram(graph, layout, source, files, inputPath, options, boardPath = "") {
  if (graph?.nodes) {
    const diagram = makeTargetDiagram(graph, layout, source, files, inputPath, options);
    return diagram;
  }
  const nodes = flattenNodes(graph);
  const boxes = layout.boxes || {};
  const labels = layout.node_labels || {};
  const shapes = nodes.map((node, index) => {
    const box = boxes[String(index)] || { origin: { x: 0, y: 0 }, size: { width: 0, height: 0 } };
    const label = node.labels?.[0]?.text || node.id || "";
    return {
      id: node.id,
      type: "rectangle",
      pos: { x: box.origin.x, y: box.origin.y },
      width: box.size.width,
      height: box.size.height,
      label,
      fontSize: labels[String(index)]?.font_size || node.labels?.[0]?.font_size || 16,
      opacity: 1,
      strokeDash: 0,
      strokeWidth: 2,
      borderRadius: 0,
      fill: "#ffffff",
      stroke: "#000000",
      animated: false,
      shadow: false,
      "3d": false,
      multiple: false,
      "double-border": false,
      tooltip: "",
      link: "",
      blend: false,
      zIndex: 0,
      level: 0,
    };
  });
  const connections = (graph.edges || []).map((edge, index) => ({
    id: edge.id,
    src: edge.sources?.[0] || "",
    dst: edge.targets?.[0] || "",
    label: edge.labels?.[0]?.text || "",
    route: layout.routes?.[String(index)] || [],
    srcArrow: "none",
    dstArrow: "arrow",
    opacity: 1,
    strokeDash: 0,
    strokeWidth: 2,
    stroke: "#000000",
    labelPosition: "",
    labelPercentage: 0.5,
    link: "",
    animated: false,
    tooltip: "",
    zIndex: 0,
  }));
  const diagram = {
    name: inputPath,
    isFolderOnly: shapes.length === 0,
    shapes,
    connections,
    root: {
      id: "",
      type: "rectangle",
      pos: { x: 0, y: 0 },
      width: 0,
      height: 0,
      label: "",
    },
  };
  return diagram;
}

class D2 {
  constructor(runtimeLoader, workerFactory = null) {
    this._runtimeLoader = runtimeLoader;
    this._workerFactory = workerFactory;
    this._workerMode = Boolean(workerFactory);
    this.worker = undefined;
    this.nextRequestId = 0;
    this.pendingRequests = new Map();
    this.disposed = false;
    this.disposePromise = null;
    this.ready = workerFactory ? this._initWorker() : Promise.resolve();
  }

  async _initWorker() {
    this.worker = await this._workerFactory();
    let isReady = false;
    const workerError = (error) => {
      const normalized = error instanceof Error
        ? error
        : new Error(error?.message || "D2 worker encountered an error");
      if (!isReady) this._readyReject?.(normalized);
      this._rejectPending(normalized);
    };
    const onMessage = (message) => {
      // Browser workers deliver a MessageEvent, while Node's worker_threads
      // callback receives the posted object directly. A result object itself
      // also has a `data` field, so checking only `message.data` would unwrap
      // the response and lose its request id.
      const data = typeof MessageEvent !== "undefined" && message instanceof MessageEvent
        ? message.data
        : message;
      if (data?.type === "ready") {
        isReady = true;
        this._readyResolve?.();
        return;
      }
      if (data?.type !== "result" && data?.type !== "error") return;
      const request = this.pendingRequests.get(data.id);
      if (!request) return;
      this.pendingRequests.delete(data.id);
      if (data.type === "result") request.resolve(request.type === "render"
        ? sourceRenderResponse(data.data, request.options) : data.data);
      else request.reject(new Error(data.error || "D2 worker request failed"));
    };
    const ready = new Promise((resolve, reject) => {
      this._readyResolve = resolve;
      this._readyReject = reject;
    });
    if (typeof this.worker.on === "function") {
      this.worker.on("message", onMessage);
      this.worker.on("error", workerError);
    } else {
      this.worker.onmessage = onMessage;
      this.worker.onerror = workerError;
    }
    if (typeof this.worker.postMessage !== "function") {
      throw new Error("D2 worker does not implement postMessage");
    }
    // Worker modules initialize the generated Rust/WASM runtime before they
    // announce `ready`; the message is retained as an explicit handshake so
    // pending requests cannot race module startup.
    return ready;
  }

  _rejectPending(error) {
    for (const request of this.pendingRequests.values()) request.reject(error);
    this.pendingRequests.clear();
  }

  _assertLive() {
    if (this.disposed) throw new Error("D2 instance has been disposed");
  }

  async _runtime() {
    this._assertLive();
    return this._runtimeLoader();
  }

  async sendMessage(type, data) {
    this._assertLive();
    if (this._workerMode) {
      await this.ready;
      this._assertLive();
      const id = this.nextRequestId++;
      const operation = new Promise((resolve, reject) => {
        this.pendingRequests.set(id, { resolve, reject, type, options: data?.options });
        try {
          this.worker.postMessage({ id, type, data });
        } catch (error) {
          this.pendingRequests.delete(id);
          reject(error);
        }
      });
      return operation;
    }
    const id = this.nextRequestId++;
    let rejectPending;
    const operation = new Promise((resolve, reject) => {
      rejectPending = reject;
      this.pendingRequests.set(id, { resolve, reject });
      Promise.resolve()
        .then(async () => {
          const runtime = await this._runtime();
          switch (type) {
            case "compile":
              return this._compile(runtime, data);
            case "render":
              return this._render(runtime, data);
            case "renderTarget":
              return runtimeMethod(runtime, "render_d2_target_json")(data.diagram, data.options);
            case "encode":
            case "decode":
              {
                const response = JSON.parse(await runtimeMethod(runtime, type)(data));
                if (response?.error) throw new Error(response.error.message || String(response.error));
                return (response?.data ?? response).result;
              }
            case "version":
              return await runtimeMethod(runtime, "version")();
            case "jsVersion":
              return await runtimeMethod(runtime, "js_version", "jsVersion")();
            default:
              throw new Error(`unknown D2 request ${type}`);
          }
        })
        // The Go SDK's worker unwraps WASM errors into JavaScript Error
        // objects. wasm-bindgen throws string JsValues at the raw boundary;
        // preserve the same public rejection contract in the direct client.
        .then(resolve, error => reject(error instanceof Error ? error : new Error(String(error))))
        .finally(() => this.pendingRequests.delete(id));
    });
    // Retain the reject function for disposal even after the promise is
    // created. This mirrors the worker facade's pending-request semantics.
    this.pendingRequests.get(id).reject = rejectPending;
    return operation;
  }

  async _compile(runtime, request) {
    if (typeof runtime.prepare_d2_public_compile_request_json === "function") {
      try {
        request = JSON.parse(runtime.prepare_d2_public_compile_request_json(JSON.stringify(request)));
      } catch (error) { throw error instanceof Error ? error : new Error(String(error)); }
    }
    const fs = { ...(request.fs || {}) };
    const inputPath = request.inputPath ?? "index";
    const source = fs[inputPath];
    if (typeof source !== "string") throw new Error(`missing '${inputPath}' file in input fs`);
    const options = { ...(request.options || {}) };
    const jsonOptions = JSON.stringify(request.sourceOptions || sourceOptions(options, fs, inputPath));
    let layout;
    let graph;
    const targetMethod = runtime.compile_d2_public_target_json
      || runtime.compile_d2_target_json || runtime.compileD2TargetJson;
    if (typeof targetMethod === "function") {
      let target;
      try {
        target = JSON.parse(await targetMethod.call(runtime, source, jsonOptions));
      } catch (error) {
        // wasm-bindgen's native helpers throw strings. The public facade,
        // like the original Node worker, rejects with an Error instance.
        throw error instanceof Error ? error : new Error(String(error));
      }
      const formatted = await runtimeMethod(runtime, "format_d2_source", "formatD2Source")(
        source,
        jsonOptions,
      );
      fs[inputPath] = formatted;
      layout = target.layout || {};
      graph = target;
      const effectiveOptions = { ...mergeRenderOptions(options, target.config), ...target.source_render_options };
      const diagram = makeTargetDiagramTree(target, source, fs, inputPath, effectiveOptions);
      return {
        fs,
        inputPath,
        diagram,
        // D2 v0.9.0 exposes the compiled graph alongside the target diagram.
        // Keep both the source-shaped object projection and the indexed Rust
        // metadata in the response so editor and incremental-rendering callers
        // do not have to reconstruct graph identities from the SVG diagram.
        graph: makeCompiledGraphTree(target, inputPath),
        renderOptions: target.source_render_options || effectiveOptions,
      };
    } else {
      layout = JSON.parse(await runtimeMethod(runtime, "layout_d2_source", "layoutD2Source")(
        source,
        jsonOptions,
      ));
      const formatted = await runtimeMethod(runtime, "format_d2_source", "formatD2Source")(
        source,
        jsonOptions,
      );
      fs[inputPath] = formatted;
      const elkResponse = JSON.parse(await runtimeMethod(runtime, "getELKGraph")(
        JSON.stringify({ fs, inputPath }),
      ));
      if (elkResponse.error) throw new Error(elkResponse.error.message);
      graph = elkResponse.data || {};
    }
    const diagram = makeDiagram(graph, layout, source, fs, inputPath, options);
    return {
      fs,
      inputPath,
      diagram,
      graph: makeCompiledGraph(graph, layout, inputPath),
      renderOptions: { ...options },
    };
  }

  _compileWorkerResponse(response) {
    const { fs, inputPath, options, source, layout, target, graph } = response;
    const targetGraph = target || graph;
    const effectiveLayout = layout || targetGraph?.layout || {};
    const effectiveOptions = { ...mergeRenderOptions(options, targetGraph?.config), ...targetGraph?.source_render_options };
    const diagram = makeTargetDiagramTree(
      targetGraph,
      source,
      fs,
      inputPath,
      effectiveOptions,
    );
    return {
      fs,
      inputPath,
      diagram,
      // Keep the worker response identical to the direct-runtime response:
      // `graph` is the compiled graph/target payload, while `layout` remains
      // an internal input to the diagram projection.
      graph: makeCompiledGraphTree(targetGraph, inputPath),
      renderOptions: targetGraph?.source_render_options || effectiveOptions,
    };
  }

  async _render(runtime, request) {
    const options = request.options ?? {};
    const output = await runtimeMethod(runtime, "render_d2_target_json")(
      JSON.stringify(request.diagram) ?? "null", JSON.stringify(options),
    );
    return sourceRenderResponse(output, options);
  }

  compile(input, options = {}) {
    const request = typeof input === "string"
      ? { fs: { index: input }, options: { ...options } }
      : {
          ...input,
          options: { ...options, ...(input?.options || {}) },
        };
    if (this._workerMode) {
      return this.sendMessage("compile", request).then((response) =>
        this._compileWorkerResponse(response)
      );
    }
    return this.sendMessage("compile", request);
  }

  async render(diagram, options = {}) {
    return this.sendMessage("render", { diagram, options });
  }

  encode(script) {
    return this.sendMessage("encode", script);
  }

  decode(encoded) {
    return this.sendMessage("decode", encoded);
  }

  version() {
    return this.sendMessage("version");
  }

  jsVersion() {
    return this.sendMessage("jsVersion");
  }

  async dispose() {
    if (this.disposePromise) return this.disposePromise;
    this.disposed = true;
    const error = new Error("D2 instance has been disposed");
    this._rejectPending(error);
    this.disposePromise = (async () => {
      try {
        await this.ready;
      } catch {
        // Initialization errors are already delivered through `ready`.
      }
      const worker = this.worker;
      this.worker = undefined;
      if (worker?.terminate) await worker.terminate();
    })();
    return this.disposePromise;
  }
}

module.exports = { D2 };
