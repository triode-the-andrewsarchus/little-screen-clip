// SPDX-License-Identifier: MPL-2.0

export function checkRequestUnicode(runtime, fixture) {
  if (fixture.cases.length !== 220) throw new Error("Incomplete Go Unicode request inventory");
  const ordered = value => Array.isArray(value) ? value.map(ordered)
    : value && typeof value === "object"
      ? Object.fromEntries(Object.keys(value).sort().map(key => [key, ordered(value[key])]))
      : value;
  for (const row of fixture.cases) {
    const actual = JSON.parse(runtime.getELKGraph(row.request));
    if (JSON.stringify(ordered(actual)) !== JSON.stringify(ordered(row.expected))) {
      throw new Error(`${row.name}: ${JSON.stringify({ actual, expected: row.expected })}`);
    }
    const healthy = JSON.parse(runtime.getParentID("a.b"));
    if (healthy.data !== "a" || healthy.error) throw new Error(`${row.name}: runtime recovery failed`);
  }
  return fixture.cases.length;
}

export async function checkPublicRequestUnicode(api, fixture) {
  if (fixture.public_cases.length !== 60) throw new Error("Incomplete public Unicode request inventory");
  const ordered = value => Array.isArray(value) ? value.map(ordered)
    : value && typeof value === "object"
      ? Object.fromEntries(Object.keys(value).sort().map(key => [key, ordered(value[key])]))
      : value;
  for (const row of fixture.public_cases) {
    let result = null, error = null, resolved = false;
    try { result = await api.compile(JSON.parse(row.request)); resolved = true; }
    catch (e) { error = { name: e?.name ?? null, message: e?.message ?? String(e), code: e?.code ?? null }; }
    const expectedError = row.error && { name: row.error.name, message: row.error.message, code: row.error.code };
    if (JSON.stringify(ordered({ result, error, resolved })) !== JSON.stringify(ordered({ result: row.result, error: expectedError, resolved: row.resolved }))) {
      throw new Error(`${row.name}: public Unicode request differs from original Go`);
    }
    if (await api.decode("AwA=") !== "") throw new Error(`${row.name}: unhealthy runtime`);
  }
  return fixture.public_cases.length;
}
