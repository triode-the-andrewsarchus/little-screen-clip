# Native WireViz model and YAML parser

The harness model under `src/diagram/wireviz` is independently implemented from
the public [WireViz syntax documentation](https://github.com/wireviz/WireViz/blob/master/docs/syntax.md).
No WireViz Python implementation or graphical editor code is included or
translated. WireViz's GPL-3.0 application is not a runtime dependency.

YAML parsing uses the unmodified crates.io packages `oneyaml` 1.2.0,
`oneyaml-core` 1.2.0 and `oneyaml-syntax` 0.2.0 from
[recursive-ai-oss/oneyaml-rust](https://github.com/recursive-ai-oss/oneyaml-rust).
They are available under MIT OR Apache-2.0. This distribution uses their MIT
license, reproduced verbatim in `oneyaml-MIT.txt`; all three packages carry the
same copyright and MIT license text. No parser source is vendored or modified.

The separate research executable under `scripts/diagram_research/wireviz_yaml`
also evaluates `yaml-edit`; that comparison dependency is not linked into the
application.
