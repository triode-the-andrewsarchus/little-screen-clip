# Structurizr native libraries

Selected pristine sources from https://github.com/Helms-AI/structurizr-rs
at revision `59a5c4a93bfcb11e501d81a06cd4492c01f1f890`.

The upstream workspace declares `license = "MIT"` and authors
`["structurizr-rs contributors"]`; see the preserved `Cargo.toml.upstream`.
No root license-text file is present in that revision.

This selection contains core models, DSL parsing and native SVG rendering.
Web/server/MCP/scripting/GitHub/code-analysis applications are not imported.
The original workspace manifest is retained as `Cargo.toml.upstream`, rather
than resolving its unrelated applications. Local adaptations are subsequent
commits, separate from this pristine import.

## Local changes

- Focused Cargo workspace resolves only these three native libraries.
- Remove unused Pest dependencies and HTTP/theme/cache dependencies.
- Remove all filesystem reads/writes, include resolution and script/plugin
  execution. Model JSON serialization and inert documentation references remain.
- Reject execution-dependent/external workspace declarations instead of
  silently presenting a partially evaluated model.
- Add source/token/nesting budgets, complete-brace and trailing-input checks,
  using the original lexer rather than another DSL grammar.
- Keep native models, validation, navigation, layout and SVG drawing code.
- Reproduce standard MIT terms in `LICENSE-MIT` with upstream's declared
  contributor attribution; these terms were not a separate file upstream.

- Correct implied relationships to lift containment endpoints, enabled by
  default, rather than calculate transitive graph reachability. Preserve explicit
  pairs and relationship metadata; reject unsupported Java strategy names.

- Resolve static view membership by scope, preserve authored selection order and
  empty/disconnected selections, and report unsupported terms. Core owns static
  element enumeration/metadata. Native static and filtered renderers share one
  layout/paint path, consuming resolved elements and relationships directly.

- Bind dynamic interactions to model relationships with scope validation,
  technology selection and replies; preserve explicit/parallel ordering and
  shared annotations. Render model styles and self-call loops, reject corrupt
  bindings, and report overflowing numeric tokens. Add end-to-end regressions.

- Preserve custom views and canonical `element` declarations, optional metadata,
  tags/properties, ordered selection and annotations. Add custom core lookup and
  share the static renderer, removing the separate layout and unused traversal.
  Reject C4/custom mixtures and cover actual SVG membership/edge selection.

- Pass actual layout direction into the existing orthogonal router, select the
  correct sibling axis and keep simple sibling routes outside nodes. Consolidate
  private SVG forwarding methods; add end-to-end attachment and corridor tests.

- Give each deployed instance a unique placement identity, require correctly
  typed existing model references, and defer bare deployment construction until
  static declarations exist. Preserve instance descriptions, type/inline tags,
  properties and inert health checks, plus infrastructure technology/properties.
  Diagnose unsupported deployment groups instead of silently dropping them.

- Share a borrowed deployment containment/environment inventory in core. Resolve
  environment aliases, validate view scope/environment and preserve ordered
  branch/leaf selections, parent closure, empty views, annotations and exact
  selected relationships. Diagnose instance abstraction conflicts and cover
  wildcard branch/previous-selection behavior with regression tests.

- Replicate model relationships between same-environment placements with metadata,
  explicit-link precedence and a 16384-link expansion budget. Retain authored
  environment/nested/grouped relationships through shared checked builders;
  report unknown nested endpoints instead of silently dropping links.

- Retain filtered AST views during workspace construction using shared metadata
  conversion. Strengthen parser/SVG tests with explicit view counts and actual
  relationship-geometry counts so an empty collection cannot pass vacuously.
