// SPDX-License-Identifier: MPL-2.0

export function checkLegacyStringArguments(runtime, fixture) {
  const values = {
    undefined: undefined, null: null, true: true, false: false,
    zero: 0, one: 1, negative: -1, fraction: 0.5, small: 1e-7,
    large: 1e21, "negative-zero": -0, nan: NaN, infinity: Infinity,
    string: "a.b", empty: "", object: {}, array: [], symbol: Symbol("x"),
    function: () => {},
  };
  if (fixture.cases.length !== 60) throw new Error("Legacy argument inventory is incomplete");
  const ordered = value => Array.isArray(value) ? value.map(ordered)
    : value && typeof value === "object"
      ? Object.fromEntries(Object.keys(value).sort().map(key => [key, ordered(value[key])]))
      : value;
  for (const row of fixture.cases) {
    const args = row.tag === "omitted" ? [] : [values[row.tag]];
    const actual = JSON.parse(runtime[row.method](...args));
    if (JSON.stringify(ordered(actual)) !== JSON.stringify(ordered(row.expected))) {
      throw new Error(`${row.name}: ${JSON.stringify({ actual, expected: row.expected })}`);
    }
    const healthy = JSON.parse(runtime.getParentID("a.b"));
    if (healthy.data !== "a" || healthy.error) throw new Error(`${row.name}: runtime recovery failed`);
  }
  return fixture.cases.length;
}
