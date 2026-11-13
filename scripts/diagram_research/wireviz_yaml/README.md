# WireViz YAML editing probe

This isolated executable compares the published `oneyaml` 1.2.0 and
`yaml-edit` 0.3.6 APIs against small harness-shaped documents. It is research
tooling, not a new application workspace member or a format renderer. Neither
dependency is added to the shipped application by this probe.

Run from the repository root, while other Cargo builds and cleanup are idle:

```bash
CARGO_TARGET_DIR=/tmp/wireviz-yaml-probe-target cargo run --locked --manifest-path scripts/diagram_research/wireviz_yaml/Cargo.toml -j4
```

The standalone workspace and target avoid rebuilding multimedia dependencies.
The executable performs no file mutations, network calls or resource loading;
Cargo may fetch the pinned packages. Assertions cover byte-preserving literal
edits, quoted identities, flow/block lists, comments, BOM/CRLF, escaped JSON
pointers, alias/merge write refusal, semantic duplicate rejection, malformed
input, excessive depth, expansion limits and failed patch rollback.

It also records three integration hazards:

- `yaml-edit` accepts a duplicate mapping that `oneyaml` refuses semantically.
- YAML 1.1 compatibility distinguishes quoted `"01"` from numeric `01`, and
  treats unquoted `NO`/`ON` as booleans. A WireViz reader must choose its scalar
  rules deliberately; swapping parsers silently changes pin identities.
- `oneyaml` correctly refuses a write *through* an alias or merge, but allows
  editing the anchor definition. That changes every alias user's meaning.
  Graphical edits still need an application-level semantic-delta guard.

Custom tags remain tagged values, rather than ordinary mappings; a future
WireViz adapter must reject unsupported tags explicitly. This executable
tests YAML behavior only. It does not prove WireViz connection compatibility,
Graphviz rendering, safety admission, editor undo or guarded file saving.
