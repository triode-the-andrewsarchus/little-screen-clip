// SPDX-License-Identifier: MPL-2.0
// Exact JSON-domain equality shared by Node and real browser source fixtures.
export function sameJson(actual, expected) {
  if (Object.is(actual, expected)) return true;
  if (actual === null || expected === null || typeof actual !== "object" || typeof expected !== "object") return false;
  if (Array.isArray(actual) !== Array.isArray(expected)) return false;
  if (Array.isArray(actual) && actual.length !== expected.length) return false;
  const keys = Object.keys(actual), other = Object.keys(expected);
  return keys.length === other.length && keys.every(key => Object.hasOwn(expected, key) && sameJson(actual[key], expected[key]));
}
