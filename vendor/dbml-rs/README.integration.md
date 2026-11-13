# DBML parser integration

Pristine registry source was imported separately from application changes:
`dbml-rs` 1.0.1, upstream revision
`fea4bb7a9d0de7b36c335041e59db05a95fe2256`, published archive checksum
`3ddeeba177b8047c8bdd42d265c653af5e635f73b2e442150fda4d829990079f`.
The original MIT and Apache-2.0 license texts are retained.

The local analyzer change checks an optional type-length argument with `first()`
instead of indexing an empty vector. Valid bare `varchar`, `char`, `varbit` and
`bit` with string defaults otherwise panic. Explicit character-length limits count
Unicode characters rather than UTF-8 bytes. A focused library regression covers
both optional lengths and single-character non-ASCII defaults, while retaining
errors for values exceeding explicit limits. No grammar or parser code is copied
into application modules.

The application uses the native parser in its existing disposable diagram worker;
DBML expressions are text, never executed. Newer language constructs unsupported
by this release must report parse errors rather than being stripped from input.

Private upstream analysis/optional-utility modules retain their unused helper
APIs under scoped `dead_code` allowances. The published dependency hides these
through Cargo dependency lint caps; the local path patch otherwise emits four
pre-existing warnings. Application and adapter warnings remain enabled.

The local grammar/AST extension adds `TablePartial` and ordered `~name`
injections, inline/standalone CSV `records` with `[example]`, and explicit `?`
relationship endpoints. It uses the existing Pest grammar and native analyzer;
there is no source rewriting or second application parser. Checked parsing keeps
`authored_blocks` before deriving effective tables, so inherited definitions do
not erase local/partial source identities. Original input and spans are retained.
Records preserve raw scalar/expression spellings; no SQL is executed. Row shape,
column/partial identities and numeric/boolean/declared-enum forms are checked;
date normalization and custom database type coercion are intentionally unclaimed
and visibly disclosed. See `docs/diagrams/dbml-research.md` for scope and limits.
