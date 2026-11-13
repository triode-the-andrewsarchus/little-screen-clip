# Erd compatibility grammar

The native Erd parser and Graphviz adapter follow the original bracket/entity
language documented by Andrew Gallant's Erd project, released under the
Unlicense. Its original grammar, semantic model, presentation reference and
fixtures are retained without modifications in `vendor/erd-reference`.

Original source, revision d18075d205583458b477cdcb7fe8bb41ec092cf9:
https://github.com/BurntSushi/erd/tree/d18075d205583458b477cdcb7fe8bb41ec092cf9

The native Rust implementation adds bounded parsing and exact authored token
spans. It does not execute Haskell, read Erd configuration files, call an
installed Graphviz executable or import the unrelated rusterd DSL. The original
reference and fixture import was committed separately from application code.
The full original Unlicense follows in this combined notice.
