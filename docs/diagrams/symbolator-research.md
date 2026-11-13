# Symbolator: canonical HDL interface symbols

Research checked 2026-11-13. Symbolator means the HDL interface extractor and its
circuit-symbol renderer: VHDL entities/components, Verilog modules, generic or
parameter blocks, directed ports, buses and section metacomments. It does not mean
an unrelated SVG shape library or synthesis/netlist exporter.

## Candidates

| Candidate | Parser/render implementation | Decision |
| --- | --- | --- |
| [kevinpt/Symbolator](https://github.com/kevinpt/symbolator) + [HDLparse](https://github.com/kevinpt/hdlparse) | Original Python documentation extractor and NuCanvas/Cairo/SVG symbol renderer | Canonical semantics, but old Python compatibility and limited entity handling |
| [hdl/Symbolator](https://github.com/hdl/symbolator) + [pyHDLParser](https://github.com/hdl/pyHDLParser) | Python 3 fork; parser exposes real VHDL entity and component objects | Actual compatible implementation, but CLI still filters out entity objects |
| [zebreus/Symbolator](https://github.com/zebreus/symbolator) | Consolidated canonical renderer + HDLparse/NuCanvas; direct VHDL entity/component and Verilog support | Selected 1.2.2 commit; preserves section metacomments and canonical pin geometry |
| [HDLTree](https://github.com/rpatel3001/hdltree) | Lark-based VHDL parser feeding Symbolator; original Verilog parser retained | More complete VHDL grammar, but documented section-metacomment loss and larger runtime/dependency surface |
| [vhdl_lang / vhdl_syntax](https://github.com/VHDL-LS/rust_hdl) | Real native Rust AST/CST parsers with much broader VHDL support | Credible native alternative, but does not render Symbolator symbols; requires a new interface adapter and renderer |
| [sv-parser](https://github.com/dalance/sv-parser) | Native Rust IEEE SystemVerilog parser/preprocessor and CST | Credible native alternative; broader grammar, but a second parser dependency plus a new Symbolator renderer and guarded preprocessing are needed |
| [sverilogparse](https://docs.rs/sverilogparse/0.3.3/sverilogparse/) | Structural Verilog parser | A different narrower grammar; no VHDL or canonical symbol renderer |

The canonical companion is selected for the smallest faithful implementation of
this family. Python/Pillow already serve the BlockDiag family, so the same
bounded process, fixed embedded importer, bundled metric faces and resource-audit
policy are reused. The real canonical parser and SVG writer remain in use. No
Cairo/Pango installation, browser, server, simulator, synthesis tool, package
installation or HDL execution is needed. Pillow replaces only text measurement;
SVG outlining uses the same bundled font faces.

## Editor scope

The [canonical documentation](https://zebreus.github.io/symbolator/) describes a
source-to-symbol tool, not a source-preserving graphical circuit editor. No
reusable canonical graphical editor was identified. General circuit editors such
as [Logisim Evolution](https://github.com/logisim-evolution/logisim-evolution)
manage their own circuit model and would not preserve Symbolator HDL or
metacomment sections. The practical native editor is highlighted HDL source with
live symbol output and errors. A future port/property editor should edit HDL
through source locations rather than convert the interface to another circuit
format. Ordinary `.v`/`.vhdl` source files should remain ordinary source unless
opened explicitly as Symbolator; dedicated `.symbolator` files/fences are explicit.

## Import and adaptations

[`vendor/symbolator/PROVENANCE.json`](../../vendor/symbolator/PROVENANCE.json)
records the exact upstream commit and archive digest. Pristine MIT-licensed
sources were committed separately before adaptations. The complete adaptation
list is in [`ADAPTATIONS.md`](../../vendor/symbolator/ADAPTATIONS.md).

Three focused fixtures reproduce parser failures in the pristine fork:

- `module.symbolator.v`: the first ANSI port begins on the module-header line;
  removing the erroneous start-of-line anchor keeps the `clk` port.
- `entity.symbolator.vhdl`: a named `end entity sample_counter;` closure now emits
  the entity instead of silently dropping it.
- `component.symbolator.vhdl`: the optional component `is` keyword may be omitted.

Cairo imports are deferred to their original optional code paths. NuCanvas's real
SVG writer accepts an owned text stream. The adapter supplies bounded in-memory
font metrics, the canonical clock/bubble/arrow marker definitions and source-only
extraction. It never invokes CLI directory scans or type-cache I/O. File-based
entrypoints are explicitly disabled, and the common audit policy fails closed on
filesystem, directory, network and process access after trusted setup. Includes,
macros and HDL imports are rejected rather than resolved. VHDL `library`/`use`
clauses remain declarations and do not load external libraries.

## Compatibility and honest limits

This is interface documentation extraction, not HDL compilation or elaboration.
Every successful preview retains a warning stating that distinction. The canonical
parser supports documented Verilog-1995/2001 module declarations and the supported
VHDL entity/component interface subset. It preserves generic/parameter sections,
named metacomment sections, type labels, bus ranges, clock triangles, active-low
bubbles and bidirectional markers. It registers array types from the supplied
source only. Unsupported SystemVerilog types, language extensions and complex
expressions retain upstream extraction limits. Behavioral HDL, package resolution,
simulation and synthesis are not performed, and output is not a syntax-correctness
certificate.

The adapter detects unbalanced interface delimiters, incomplete declarations and
unterminated strings/comments, including a truncated final interface after a valid
first one. Unsupported/empty interfaces produce actionable errors; they do not
become an unrelated fallback picture. At most sixteen interfaces and 512 pins or
parameters per interface are admitted, with the existing source/time/memory/output
budgets. All interfaces receive separate SVG pages before the shared composer
stacks, outlines and rasterizes them. Authored section names are escaped labels,
not Pango/SVG elements.

## Validation evidence

Isolated probes against pristine commit `c872deda` reproduced all three lexer
regressions: no named VHDL entity, no component without `is`, and a lost first
same-line ANSI `clk` port. Adapted probes retained the expected five entity ports,
four component ports and six Verilog module ports. Visual inspection confirmed
section colors, generic blocks, type labels, clock triangles, reset bubbles,
bus strokes and bidirectional arrows.

Serialized Rust validation passed:

- `cargo test -p little-clip-render -j4 --lib diagram::python::tests`: 2 passed.
- `cargo test -p little-clip-render -j4 --lib diagram::blockdiag::tests`: 3 passed.
- `cargo test -p little-clip-render -j4 --lib diagram::symbolator::tests`: 4 passed.

These checks include actual SVG composition/outlining/rasterization, all focused
parser fixtures, multiple-module preservation, literal markup labels,
truncated-interface errors and denied file/directory/network/process/dlopen
operations. Shared adapter measurement has maximum cyclomatic complexity 13 and
no functions above 15; the only small duplicate is direct calls to the existing
SVG policy owner. Application/Markdown acceptance remains with the coordinating
agent's integration checks.
