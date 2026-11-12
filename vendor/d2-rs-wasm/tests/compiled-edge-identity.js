// SPDX-License-Identifier: MPL-2.0
import { sameJson } from "./exact-json.js";
export async function checkCompiledEdgeIdentity(d2, fixture) {
  if (fixture.cases.length !== 40) throw new Error('Incomplete compiled edge identity inventory');
  const rows = [];
  for (const row of fixture.cases) {
    const result = await d2.compile(row.input.source, { layout: 'dagre' });
    const edges = (result.graph.native_edges ?? result.graph.edges ?? []).map(e => ({
      abs_id:e.abs_id, src:e.src, dst:e.dst, src_arrow:e.src_arrow, dst_arrow:e.dst_arrow, index:e.index,
    }));
    const ids = result.diagram.connections.map(e=>e.id);
    const expectedIds = row.connections.map(e=>e.id);
    const stable = value => JSON.stringify(value, ['abs_id','src','dst','src_arrow','dst_arrow','index']);
    if (stable(edges) !== stable(row.edges) || JSON.stringify(ids) !== JSON.stringify(expectedIds)) {
      throw new Error(`${row.input.name}: edge identities differ: ${JSON.stringify({edges,ids,expected:row.edges})}`);
    }
    if (!sameJson(result.diagram.connections, row.connections)) {
      throw new Error(`${row.input.name}: complete target connections differ from original Go`);
    }
    // Source indices repeat across distinct endpoints. Routes still use the
    // internal array identity. The target separately applies Go export precision;
    // compare it against the complete original-Go target connection record.
    for (const [index, edge] of (result.graph.native_edges ?? result.graph.edges ?? []).entries()) {
      const route = result.graph.layout.routes[String(index)] || [];
      if (JSON.stringify(edge.route) !== JSON.stringify(route)) {
        throw new Error(`${row.input.name}: connection ${index} lost its route`);
      }
    }
    rows.push({ name:row.input.name, edges, diagram_ids:ids });
  }
  return rows;
}
