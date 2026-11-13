// SPDX-License-Identifier: MPL-2.0
// Node ESM consumes the CommonJS module's default export.
import d2rs from "@d2-rs/wasm";
import raw from "@d2-rs/wasm/raw";

const d2: d2rs.D2 = new d2rs.D2();
const compiled: Promise<d2rs.CompileResponse> = d2.compile("a -> b");
const version: string = d2rs.version();
const jsVersion: string = d2rs.jsVersion();
const parsed: string = d2rs.parseD2Source("a", "{}");
const loaded: Promise<typeof d2rs> = d2rs.raw();
const rawVersion: string = raw.version();
// @ts-expect-error D2's lifecycle facade remains asynchronous in Node.
const syncCompiled: d2rs.CompileResponse = d2.compile("a -> b");
void [compiled, version, jsVersion, parsed, loaded, rawVersion, syncCompiled];
