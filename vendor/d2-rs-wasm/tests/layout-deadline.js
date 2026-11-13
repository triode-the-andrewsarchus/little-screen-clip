// SPDX-License-Identifier: MPL-2.0
// Shared by Node and real browser runners; preserve full returned JSON strings.
export function checkDeadlineBoundary(runtime, fixture) {
  if (fixture.cases.length !== 340) throw new Error("Incomplete deadline inventory");
  const captures = [], active = new Map();
  for (const { input, error: expected } of fixture.cases) {
    const token = new runtime.WasmCancellationToken();
    switch (input.reason) {
      case "active": break;
      case "deadline": token.expire(); break;
      case "cancel": token.cancel(); break;
      case "cancel-then-deadline": token.cancel(); token.expire(); break;
      case "deadline-then-cancel": token.expire(); token.cancel(); break;
      default: throw new Error(`Unknown deadline cause ${input.reason}`);
    }
    let error = "", result_raw = null;
    try {
      const layout = runtime.layoutD2SourceWithCancellation || runtime.layout_d2_source_with_cancellation;
      result_raw = layout(input.source, JSON.stringify(input), token);
    } catch (thrown) { error = thrown instanceof Error ? thrown.message : String(thrown); }
    finally { token.free(); }
    if (error !== expected) throw new Error(`${input.name}: ${error} != ${expected}`);
    const key = JSON.stringify([input.source, input.path, input.engine, input.seeds]);
    if (input.reason === "active" && !error) active.set(key, result_raw);
    if (input.reason !== "active" && !error && active.has(key) && result_raw !== active.get(key)) {
      throw new Error(`${input.name}: complete successful layout changed under cancellation`);
    }
    captures.push({ name: input.name, error, result_raw });
  }
  return captures;
}
