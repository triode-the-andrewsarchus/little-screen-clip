// SPDX-License-Identifier: MPL-2.0
// Resolve the published Node conditions, rather than the browser declarations.
import d2rs = require("@d2-rs/wasm");
import raw = require("@d2-rs/wasm/raw");

const d2: d2rs.D2 = new d2rs.D2();
const compiled: Promise<d2rs.CompileResponse> = d2.compile("a -> b");
const version: string = d2rs.version();
const jsVersion: string = d2rs.jsVersion();
const parsed: string = d2rs.parseD2Source("a", "{}");
const loaded: Promise<typeof d2rs> = d2rs.load();
const rawVersion: string = raw.version();
const token = new d2rs.WasmCancellationToken();
token.cancel();
token.free();
// @ts-expect-error Node's low-level functions are synchronous.
const asyncVersion: Promise<string> = d2rs.version();
// @ts-expect-error Node initializes on require and has no init export.
d2rs.init();
void [compiled, version, jsVersion, parsed, loaded, rawVersion, asyncVersion];
