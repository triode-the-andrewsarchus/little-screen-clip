# DBML compatibility and native rendering

The adapter uses the existing native `dbml-rs` 1.0.1 Pest parser, its AST and
semantic relationship validation, then the existing Graphviz field-port renderer.
Source is parsed in full. No database, SQL, shell, browser or resource resolver
runs. Authored expressions are literal display text.

## Candidates checked

| Candidate | Actual role and result |
| --- | --- |
| [dbml-rs](https://github.com/apskhem/dbml-rs) | Native Rust grammar, source spans, table/enum/index/reference AST and semantic checks. The imported 1.0.1 lacked partials, records and optional operators. Extending this existing owner avoids a parallel parser. |
| [dbml-language-server](https://github.com/kamal-hamza/dbml-language-server) | Rust/Chumsky LSP with server/runtime dependencies; its documented syntax support does not supply the missing modern schema semantics. |
| [dbml-mermaid](https://github.com/notpeter/dbml-mermaid) | Rust exporter for tables and relationships. It is not a complete DBML schema parser and is unsuitable for preserving unsupported blocks. |
| [@dbml/parse](https://github.com/holistics/dbml/tree/master/packages/dbml-parse) | Canonical current TypeScript compiler. Version 12.0.0 was inspected and probed in a hostless Node VM with an in-memory single-file layout, with `Intl` absent to match QuickJS. It parses records and optional operators, but date records require Luxon's `Intl.DateTimeFormat`; interpreted tables retain partial injections rather than directly returning expanded table fields. |
| [@dbml/core](https://dbml.dbdiagram.io/js-module/core/) | Canonical model/normalization/export implementation, around 37 MB unpacked for the inspected release. It offers broader compatibility, but adds another compiler/model stack and dependency/runtime audit to this already native family. Its modern `dbmlv2` parser is distinct from the deprecated `dbml` parser. |

The canonical [current syntax](https://dbml.dbdiagram.io/docs/) and primary
[partial/record tests](https://github.com/holistics/dbml/blob/master/packages/dbml-parse/__tests__/examples/interpreter/record/table_partial.test.ts)
were checked for injection precedence and field order. No canonical implementation
was imported or represented as a native Rust parser. The canonical graphical
editor is [dbdiagram](https://dbdiagram.io/); this application provides source/live
preview editing, not dbdiagram's graphical relationship editing or layout storage.

## Scope of the local extension

`TablePartial` definitions, `~name` injection positions, standalone/inline `records`
and endpoint `?` markers are represented in the existing grammar and typed AST.
The parsed source and authored spans remain intact. Checked parsing retains an
`authored_blocks` snapshot alongside the resolved table definitions; unchecked
parsing still exposes the original blocks directly.

Partial fields appear at the winning definition's position: a local column wins
regardless of where its injection appears; otherwise the last injected partial
wins. Local settings and same-column indexes override inherited definitions.
Missing/duplicate partial names, repeated injections and nested injections are
explicit errors. Nested partial composition is not part of the supported grammar.
Expanded fields use the existing table/field/relationship budgets.

Records preserve raw single-quoted strings and escaped apostrophes, signed and
exponent numbers, case-insensitive boolean literals, empty cells, enum constants,
quoted date/time text and backtick expressions. Inside a table the column list
may be implicit; outside it is required. Implicit order follows the resolved
partial expansion. Duplicate record blocks/columns, missing targets/columns,
incorrect row arity, required nulls, invalid numeric/boolean forms and unknown
members of declared enums are errors. `[example]` is retained and displayed.
All rows are displayed as sample data below the table, with a 256-row/4096-cell
per-table limit. Nothing generates SQL or executes expressions.

The local parser preserves dates and custom SQL values as source text; it does
not perform canonical Luxon date coercion or full database-specific type/range
validation. A visible records warning states this. Quoted empty strings are null
for non-string values; ordinary quoted empty text stays text. Numbers are bounded
by the existing native scalar representations. These limits are not claims of
complete DBML conformance.

Explicit `?` markers on either end of all four relationship operators preserve
the authored endpoint and add an open circle to its one/many glyph. Composite
references retain individual field ports. Unmarked operators retain their maximum
cardinality symbols; nullable-column inference is not separately added to their
minimum cardinality. Presentation settings, groups, project/free-standing notes,
relationship names/actions retain the existing visible limitations.

Current DBML also has features outside this extension, including module imports,
table-level checks, diagram views and index sort-order syntax. Unsupported syntax
fails whole-source parsing instead of disappearing from the preview. There is no
file/network import fallback. Column-level metadata remains in the authored AST;
its presence is not a promise of SQL constraint execution or complete styling.

## Validation

The centrally serialized checks passed:

- `cargo test -p dbml-rs -j4 --lib`: 6/6, including five modern grammar/semantic
  regressions and the prior string-default analyzer regression.
- `cargo test -p little-clip-render -j4 --lib diagram::dbml::tests`: 8/8.
- Strict library Clippy passed for both packages (`-- -D warnings`).

Parser checks cover precedence/field order and authored-block retention, CRLF,
comments and literal whitespace, quoted/escaped/empty/exponent values, explicit
record targets and declared enum members, every optional operator/end pairing,
invalid partials/records and rejected import syntax. Renderer checks use actual
retained edge geometry and raster pixels, with the existing schema/reference
checks retained.

The authored `modern-schema.dbml` and its Markdown counterpart exercise both
partial field precedence and record forms. Actual native output was saved to
`/tmp/dbml-modern-review.svg` and `/tmp/dbml-modern-review.png` and inspected:
winning field positions/types and both sample datasets are legible; the optional
circle appears at the user endpoint, and the edge stays outside field text.
Temporary artifact-capture instrumentation was removed from tests afterward.
Native GUI acceptance/rebuild remains with the primary integration pass.

A scoped architecture measurement of the parser extension and adapter reports
maximum CCN 14, no functions above 15 and no clone groups. Valknut did not provide
cycle metrics for single-file targets; no cycle result is inferred.
