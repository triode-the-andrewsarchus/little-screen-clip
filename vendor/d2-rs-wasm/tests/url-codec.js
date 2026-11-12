// SPDX-License-Identifier: MPL-2.0

export async function checkUrlCodec(api, fixture) {
  if (fixture.cases.length !== 987) throw new Error("Incomplete source codec inventory");
  for (const row of fixture.cases) {
    let result, error;
    try { result = await api.decode(row.encoded); }
    catch (thrown) { error = thrown instanceof Error ? thrown.message : String(thrown); }
    if (row.expected.error) {
      if (error !== row.expected.error.message) throw new Error(`${row.name}: ${JSON.stringify({ error, expected: row.expected.error.message, result })}`);
    } else if (error !== undefined || result !== row.expected.result) {
      throw new Error(`${row.name}: decoded content differs from Go`);
    }
    if (await api.decode("AwA=") !== "") throw new Error(`${row.name}: unhealthy followup`);
  }
  // PLAN permits different DEFLATE bytes. Verify the decoded script instead.
  for (const source of ["", "a -> b\n", "é😀\n", "\u2028\u2029", "repeat\n".repeat(10000)]) {
    if (await api.decode(await api.encode(source)) !== source) throw new Error("Codec roundtrip failed");
  }
  return fixture.cases.length;
}
