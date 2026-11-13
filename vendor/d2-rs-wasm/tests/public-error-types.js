// SPDX-License-Identifier: MPL-2.0
export async function checkPublicErrorTypes(client, fixture) {
  if (fixture.cases.length !== 17) throw new Error("Incomplete source error-type fixture");
  for (const c of fixture.cases) {
    let error = null, success = false;
    try {
      if (c.operation === "render") await client.render(structuredClone(c.diagram), c.options);
      else await client.compile(c.source, c.options);
      success = true;
    } catch (e) {
      error = {type:typeof e,is_error:e instanceof Error,name:e?.name??null,message:e?.message??String(e),code:e?.code??null};
    }
    if (success !== c.expected.success || JSON.stringify(error) !== JSON.stringify(c.expected.error)) {
      throw new Error(`${c.name}: public error type differs: ${JSON.stringify(error)}`);
    }
    const followup = await client.render({}, {noXMLTag:true});
    if (!followup.startsWith("<svg")) throw new Error(`${c.name}: unhealthy followup`);
  }
  return fixture.cases.length;
}
