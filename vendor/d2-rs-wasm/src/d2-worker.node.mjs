// SPDX-License-Identifier: MPL-2.0

import { createRequire } from "node:module";
import { parentPort } from "node:worker_threads";

const require = createRequire(import.meta.url);
const runtime = require("./node.cjs");

function method(name, alternate) {
  const value = runtime[name] || (alternate && runtime[alternate]);
  if (typeof value !== "function") throw new Error(`WASM runtime is missing ${name}`);
  return value.bind(runtime);
}

function base64Bytes(value) {
  if (value === undefined || value === null) return undefined;
  if (typeof value === "string") return value;
  const bytes = value instanceof Uint8Array ? value : new Uint8Array(value);
  let binary = "";
  for (let offset = 0; offset < bytes.length; offset += 0x8000) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + 0x8000));
  }
  return globalThis.btoa(binary);
}

function responseValue(raw, name) {
  let response = raw;
  if (typeof raw === "string") {
    try {
      response = JSON.parse(raw);
    } catch {
      return raw;
    }
  }
  if (response?.error) throw new Error(response.error.message || String(response.error));
  return response?.data ?? response;
}

function compile(data) {
  if (typeof runtime.prepare_d2_public_compile_request_json === "function") {
    data = JSON.parse(runtime.prepare_d2_public_compile_request_json(JSON.stringify(data)));
  }
  const fs = { ...(data.fs || {}) };
  const inputPath = data.inputPath ?? "index";
  const source = fs[inputPath];
  if (typeof source !== "string") throw new Error(`missing '${inputPath}' file in input fs`);
  const options = { ...(data.options || {}) };
  const canonicalOverrides = (value) => value && Object.fromEntries(
    Object.entries(value).filter(([, color]) => color !== null).map(([key, color]) => [key.toUpperCase(), color]),
  );
  const rustOptions = {
    utf16_positions: true,
    path: inputPath,
    files: fs,
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
  const optionsJson = JSON.stringify(data.sourceOptions || Object.fromEntries(
    Object.entries(rustOptions).filter(([, value]) => value !== undefined && value !== null),
  ));
  const target = JSON.parse(method("compile_d2_public_target_json", "compile_d2_target_json")(source, optionsJson));
  fs[inputPath] = method("format_d2_source", "formatD2Source")(source, optionsJson);
  return { fs, inputPath, source, options, layout: target.layout, target };
}

function render(data) {
  return method("render_d2_target_json")(
    JSON.stringify(data.diagram) ?? "null", JSON.stringify(data.options ?? {}),
  );
}

function scalar(name, alternate) {
  return responseValue(method(name, alternate)());
}

parentPort.on("message", (message) => {
  const { id, type, data } = message || {};
  try {
    let result;
    switch (type) {
      case "compile": result = compile(data); break;
      case "render": result = render(data); break;
      case "renderTarget": result = method("render_d2_target_json")(data.diagram, data.options); break;
      case "encode": {
        const encoded = responseValue(method("encode")(data));
        result = encoded?.result ?? encoded;
        break;
      }
      case "decode": {
        const decoded = responseValue(method("decode")(data));
        result = decoded?.result ?? decoded;
        break;
      }
      case "version": result = scalar("version"); break;
      case "jsVersion": result = scalar("js_version", "jsVersion"); break;
      default: throw new Error(`unknown D2 request ${type}`);
    }
    parentPort.postMessage({ id, type: "result", data: result });
  } catch (error) {
    parentPort.postMessage({ id, type: "error", error: error?.message || String(error) });
  }
});

parentPort.postMessage({ type: "ready" });
