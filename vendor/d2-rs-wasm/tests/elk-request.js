// SPDX-License-Identifier: MPL-2.0

export function checkElkRequest(runtime, fixture) {
  if (fixture.cases.length !== 179) throw new Error("Incomplete Go ELK request inventory");
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
