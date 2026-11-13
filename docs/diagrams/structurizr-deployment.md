# Structurizr deployment rendering

The deployment SVG backend now renders the resolved view membership rather than
walking and drawing every deployment node in the environment. It uses the
existing borrowed `Model::deployment_elements` inventory, shared style resolver,
GridLayout/Sugiyama layout and SVG painter. No extra renderer, persistent cache,
resource resolver or host application dependency was added.

## Selection and placement identity

Only explicitly resolved `ViewProperties.elements` and requested relationships
with both selected endpoints are painted. Empty and excluded selections remain
empty. A relationship cannot cause an omitted placement to reappear. Placement
IDs stay distinct from referenced container/software-system IDs, so repeated
instances have separate SVG groups and actual relationship endpoints.

Node and infrastructure labels retain their own names, descriptions, technologies
and tags. Instances use their referenced static label/technology, inherit static
tags, retain placement tags and override the description when supplied. Instance
metadata includes the actual instance number. Wrong static-reference kinds,
foreign environment members, duplicate requested placements, invalid software
system scopes, the scoped system's own system-instance representation and mixed
system/child-container abstraction produce errors. Explicitly resolved external
container selections retain existing parser behavior; the renderer does not add
a second selection grammar or silently prune them.

## Actual containment and link visibility

The selected hierarchy follows real deployment parents. If an intermediate host
is absent from a programmatically constructed view, its displayed descendants
use the nearest selected real ancestor; no hidden host is reintroduced. Selected
hosts size themselves around their displayed children with a header and insets.
Each sibling level reuses Sugiyama, with relationship endpoints projected to the
corresponding sibling subtrees. The existing rank placement uses default rank
dimensions, so each sibling run supplies the largest actual sibling dimensions
to prevent variable-size nested hosts overlapping between ranks.

Explicit host coordinates move that host's subtree. A child's own absolute
coordinates apply afterward. Containers can then grow or shift only as needed
to enclose those authored child positions. User-specified overlapping sibling
positions remain an authored layout conflict rather than being silently
rearranged.

Host boundaries paint outermost first, before any relationships or leaf
foregrounds. Their opaque backgrounds therefore cannot conceal their internal
links. Containing hosts are omitted from orthogonal pathfinding obstacles;
ordinary deployed instances and infrastructure remain obstacles. Self
relationships use an exterior orthogonal loop, so the leaf foreground cannot
hide a line drawn entirely through its own fill. Empty zero-sized renderer
configurations now receive a valid minimum viewport, preserving empty deployment
views through downstream SVG validation.

## Architecture check

The scoped source measurement found a mixed selection/layout preparation function
at CCN16. Separating concrete-placement validation from containment/layout
preparation leaves a maximum CCN10 and no high-complexity functions or clone
groups in the new module. This is a local measurement; single-file cycle
analysis is unavailable, and the primary agent owns the combined comparison.

## Validation and inspected artifact

The complete existing backend suite passes with:

```sh
cargo test --manifest-path vendor/structurizr-rs/Cargo.toml --target-dir target -j4
```

This includes 94 renderer tests, with five new deployment regressions covering:

- Actual parent/child rectangle containment and disjoint sibling rectangles in
  all four layout directions; placement identities, metadata and style dimensions.
- Host/link/leaf painter ordering, selected link geometry inside a common host,
  positive visible edge segments and exterior self-loop geometry.
- Explicit absolute positions with subtree movement and real containing bounds.
- Exact exclusion, omitted intermediate hosts, empty viewports and unchanged
  relationship filtering.
- Environment, scope, invalid instance base and abstraction failures, including
  separately rendered software-system placements.

A separate trusted probe parsed a real DSL workspace using `structurizr-dsl`,
then called the changed deployment renderer. It contains an outer region, two
sibling hosts, infrastructure, repeated API placements, a database placement,
replicated SQL links, an infrastructure link and an explicit self relationship.
The SVG was rasterized with installed `rsvg-convert` and visually inspected:
containment is visible, hosts remain behind links, and all selected relationship
types remain visible. Review artifacts:

- `/tmp/structurizr-deployment-review.structurizr`
- `/tmp/structurizr-deployment-review.svg`
- `/tmp/structurizr-deployment-review.png`

The shared host adapter now adds deployment views to its existing titled-page
chain. The explicit deployment rejection is removed; the image-view rejection,
sixteen-view/cumulative SVG budgets, shared font/image composer and source/error
workflow remain in place. The nested inspected workspace is retained as
`docs/examples/diagrams/deployment.structurizr` and the standalone Markdown review
`docs/examples/diagrams/deployment.md`.

Two added host regressions count actual outlined colored placement/link shapes,
check rendered blue PNG pixels, narrow the selection to one API placement, keep
an empty selection empty and reject invalid members/scopes/environments. Image
view rejection has an explicit regression. The targeted host run is pending the
serialized Cargo window; native GUI/Markdown acceptance remains with the primary
integration pass. This change does not claim image-view support, full styling
parity, graphical editing or new deployment DSL semantics.
