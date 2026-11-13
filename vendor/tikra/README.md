<p align="center"><img src="https://raw.githubusercontent.com/matheusmota-dev-br/tikra/main/docs/banner.svg" alt="tikra: TikZ to SVG and PNG, no TeX install" width="100%"></p>

# tikra

[![coverage](https://img.shields.io/endpoint?url=https%3A%2F%2Fmatheusmota-dev-br.github.io%2Ftikra%2Fcoverage%2Fbadge.json)](https://matheusmota-dev-br.github.io/tikra/coverage/html/)

tikra converts TikZ and pgfplots figures to SVG and PNG, with no TeX installation, no fonts on the system and no external processes. It reimplements TikZ, pgf and pgfplots in Rust, and labels are set by [mathra](https://github.com/matheusmota-dev-br/mathra) in Latin Modern, TeX's own Computer Modern. It runs in JavaScript, Python, Rust and the terminal, and every one gives the same SVG, byte for byte. **[Try it in the browser](https://matheusmota-dev-br.github.io/tikra/)** · **[See the gallery](https://matheusmota-dev-br.github.io/tikra/gallery.html)** · **[Watch the 60-second demo](https://youtu.be/_TRXke0q16s)** · **[See how it grew](https://matheusmota-dev-br.github.io/tikra/changelog.html)** ([CHANGELOG.md](https://github.com/matheusmota-dev-br/tikra/blob/main/CHANGELOG.md)).

<p align="center"><a href="https://youtu.be/_TRXke0q16s"><img src="https://img.youtube.com/vi/_TRXke0q16s/maxresdefault.jpg" alt="tikra in 60 seconds: watch the demo on YouTube" width="720"></a></p>

## Install and use

| Where | Install | Registry |
|---|---|---|
| JavaScript and TypeScript: Node, bundlers, browsers | `npm install @mathlabs/tikra` | [npm](https://www.npmjs.com/package/@mathlabs/tikra) |
| Python 3.9+: wheels for Linux, macOS and Windows | `pip install tikra` | [PyPI](https://pypi.org/project/tikra/) |
| Terminal | `cargo install tikra-cli` | [crates.io](https://crates.io/crates/tikra-cli) |
| Rust | `cargo add tikra` | [crates.io](https://crates.io/crates/tikra) · [docs.rs](https://docs.rs/tikra) |

### JavaScript and TypeScript

Types included. In Node it works right away; in browsers and bundlers, await `init()` once. More in [`npm/README.md`](https://github.com/matheusmota-dev-br/tikra/blob/main/npm/README.md).

```js
import { init, tikzToSvg, render, png } from "@mathlabs/tikra";

await init();                                                    // browsers and bundlers; Node needs nothing
const svg = tikzToSvg(String.raw`\tikz \draw[->] (0,0) -- (2,1) node[right] {$v$};`);
const { warnings, alt, description } = render(code);             // what was ignored, and the figure in words
const bytes = png(svg, { dpi: 300 });                            // Uint8Array; { background: false } for transparent
```

### Python

```python
import tikra

svg = tikra.tikz_to_svg(r"\tikz \draw[->] (0,0) -- (2,1) node[right] {$v$};")
open("v.png", "wb").write(tikra.png(svg, dpi=300))     # background=False for transparent
tikra.render(code)["warnings"]                          # dict: svg, alt, description, warnings
```

Types included; more in [`crates/python/README.md`](https://github.com/matheusmota-dev-br/tikra/blob/main/crates/python/README.md).

### Terminal

```console
$ tikra figure.tex -o figure.svg
$ tikra figure.tex -o figure.png --dpi 300       # white background
$ tikra figure.tex -o figure.png --transparent
$ tikra figure.tex -o figure.gif                # the figure drawing itself; .apng too
$ tikra figure.tex -o figure.gcode               # strokes for a pen plotter; .hpgl too
$ tikra figure.tex --pen -o figure.svg           # a pen drawing the figure; .gif and .apng too
$ tikra figure.tex --slides -o figure           # Beamer's \\only<2->…: figure-1.svg, figure-2.svg…
$ tikra figure.tex --click -o figure.svg        # the same slides in one SVG that advances on a click
$ cat figure.tex | tikra --warnings              # SVG on stdout, warnings listed
$ tikra figure.tex --describe                    # the figure in words: alt text and description
$ tikra figure.tex -o figure.svg --locale pt-BR  # its words in Portuguese; the drawing is the same
```

### Rust

```rust
let out = tikra::convert(r"\begin{tikzpicture}\draw[->] (0,0) -- (2,1) node[right] {$v$};\end{tikzpicture}")?;
std::fs::write("v.svg", &out.svg)?;
std::fs::write("v.png", tikra::png(&out.svg, 300.0, true)?)?;
for w in &out.warnings { eprintln!("{w}"); }
println!("{}", out.description);                 // the figure in words; `out.alt` in one sentence
```

A package drawn on TikZ can be added from another crate as a library of [`tikra::extension`](https://docs.rs/tikra/latest/tikra/extension/): TeX definitions run once, native keys and node shapes written in Rust, passed to `tikra::convert_with_libraries`. TikZ's own circuit libraries are written this way.

The banner above is drawn by tikra itself from [`docs/banner.tex`](https://github.com/matheusmota-dev-br/tikra/blob/main/docs/banner.tex).

## Examples

Each image is an SVG drawn by tikra from the figure in [`corpus/`](https://github.com/matheusmota-dev-br/tikra/tree/main/corpus) it links to, shown on a white card; `tests/examples.rs` keeps them in sync with the code. The [gallery](https://matheusmota-dev-br.github.io/tikra/gallery.html) draws all 252 figures of the corpus, by topic, live in the browser.

<table>
<tr><td align="center"><a href="https://github.com/matheusmota-dev-br/tikra/blob/main/corpus/plots-3d/lorenz-attractor.tex"><img src="https://raw.githubusercontent.com/matheusmota-dev-br/tikra/main/docs/examples/lorenz-attractor.svg" alt="The Lorenz attractor, colored by height" width="380"></a><br><sub>The Lorenz attractor, colored by height</sub></td><td align="center"><a href="https://github.com/matheusmota-dev-br/tikra/blob/main/corpus/charts/nightingale-rose.tex"><img src="https://raw.githubusercontent.com/matheusmota-dev-br/tikra/main/docs/examples/nightingale-rose.svg" alt="Nightingale's rose of the causes of mortality" width="380"></a><br><sub>Nightingale's rose of the causes of mortality</sub></td></tr>
<tr><td align="center"><a href="https://github.com/matheusmota-dev-br/tikra/blob/main/corpus/plots-2d/normal-distribution.tex"><img src="https://raw.githubusercontent.com/matheusmota-dev-br/tikra/main/docs/examples/normal-distribution.svg" alt="The bell curve and the 68–95–99.7 rule" width="380"></a><br><sub>The bell curve and the 68–95–99.7 rule</sub></td><td align="center"><a href="https://github.com/matheusmota-dev-br/tikra/blob/main/corpus/diagrams/petersen-graph.tex"><img src="https://raw.githubusercontent.com/matheusmota-dev-br/tikra/main/docs/examples/petersen-graph.svg" alt="The Petersen graph" width="380"></a><br><sub>The Petersen graph</sub></td></tr>
<tr><td align="center"><a href="https://github.com/matheusmota-dev-br/tikra/blob/main/corpus/science/feynman-diagram.tex"><img src="https://raw.githubusercontent.com/matheusmota-dev-br/tikra/main/docs/examples/feynman-diagram.svg" alt="A Feynman diagram, with decorations" width="380"></a><br><sub>A Feynman diagram, with decorations</sub></td><td align="center"><a href="https://github.com/matheusmota-dev-br/tikra/blob/main/corpus/geometry/golden-spiral.tex"><img src="https://raw.githubusercontent.com/matheusmota-dev-br/tikra/main/docs/examples/golden-spiral.svg" alt="The golden spiral" width="380"></a><br><sub>The golden spiral</sub></td></tr>
<tr><td align="center"><a href="https://github.com/matheusmota-dev-br/tikra/blob/main/corpus/nodes-and-text/periodic-table-corner.tex"><img src="https://raw.githubusercontent.com/matheusmota-dev-br/tikra/main/docs/examples/periodic-table-corner.svg" alt="The periodic table, by family" width="380"></a><br><sub>The periodic table, by family</sub></td><td align="center"><a href="https://github.com/matheusmota-dev-br/tikra/blob/main/corpus/plots-3d/surface-3d.tex"><img src="https://raw.githubusercontent.com/matheusmota-dev-br/tikra/main/docs/examples/surface-3d.svg" alt="A pgfplots surface and its colorbar" width="380"></a><br><sub>A pgfplots surface and its colorbar</sub></td></tr>
<tr><td align="center"><a href="https://github.com/matheusmota-dev-br/tikra/blob/main/corpus/plots-3d/smooth-shaded-surface.tex"><img src="https://raw.githubusercontent.com/matheusmota-dev-br/tikra/main/docs/examples/smooth-shaded-surface.svg" alt="A surface shaded smoothly between its points" width="380"></a><br><sub>A surface shaded smoothly between its points</sub></td><td align="center"><a href="https://github.com/matheusmota-dev-br/tikra/blob/main/corpus/decorations/clock-snapshots.tex"><img src="https://raw.githubusercontent.com/matheusmota-dev-br/tikra/main/docs/examples/clock-snapshots.svg" alt="One animation, three snapshots" width="380"></a><br><sub>One animation, three snapshots</sub></td></tr>
</table>

## How close to TikZ

Every example of the TikZ manual and of the pgfplots manual is drawn by pdflatex and by tikra, and the two are compared figure by figure ([docs/conformance.md](https://github.com/matheusmota-dev-br/tikra/blob/main/docs/conformance.md), [how](https://github.com/matheusmota-dev-br/tikra/blob/main/tools/conformance/README.md)):

| | Figures | Drawn as TeX draws them |
|---|---:|---:|
| Every figure of the manuals | 2502 | 2464 (98.4%) |
| Our 100%: the ones a converter can reproduce | 2486 | 2485 (99.9%) |

"Our 100%" leaves out what no converter can reproduce (random numbers without a seed, image files, gnuplot, the day's date) and counts the figures that differ only by antialiasing (how Ghostscript and tikra put lines, letters and pattern tiles on the pixels) as drawn; the list is checked by eye ([unreproducible.json](https://github.com/matheusmota-dev-br/tikra/blob/main/tools/conformance/unreproducible.json)). tikra converts a figure in about half a millisecond, against about 0.7 s for pdflatex.

## Input

A full `standalone` document, a bare `tikzpicture` or `pgfpicture`, an inline `\tikz`, a lone `axis` environment, or loose TikZ statements. The preamble is read for `\definecolor`, `\colorlet`, `\tikzset`, `\pgfplotsset`, `\newcommand`, `\def` and `\DeclareMathOperator`; `\usetikzlibrary` lines are accepted, and every library is available. `\pagecolor` paints the page behind the figure.

## What it draws

TikZ and pgf as the manuals describe them, through a small TeX interpreter (macros, `\def`, `\newcommand`, conditionals, registers, `\foreach`, pgfmath and `\tikzmath`):

- **Paths and coordinates**: every path operation (`--`, `|-`, `to`, `.. controls ..`, `arc`, `circle`, `rectangle`, `grid`, `plot`, `svg`, `let`), `calc`, positioning, polar and 3D coordinates, intersections, `through`, transformations and `transform canvas`, nonlinear transformations, `spy`, views (`meet`, `slice`).
- **Nodes and shapes**: every shape library (geometric, symbols, arrows, multipart, callouts, misc), labels and pins, `quotes`, matrices and `matrix of nodes`, `fit`, pics and `\pgfdeclareshape`; text with `text width`, alignment, line breaking with hyphenation, `itemize`, simple `tabular`, saved boxes (`\savebox`, `\usebox`), font sizes and families.
- **Diagrams**: chains, trees, the `graphs` library, mind maps, automata, Petri nets, ER diagrams, circuits (`circuits.ee.IEC`, logic gates), calendars, folding, turtle and Lindenmayer systems.
- **Painting**: arrow tips (`arrows.meta`, `bending`), dashes, opacity, transparency groups and blend modes, shadings (axial, radial, ball, declared and functional), fadings, patterns (classic, `patterns.meta`, declared), shadows, backgrounds and layers, animations as a snapshot at a given time (`make snapshot of`, `\pgfsysanimsnapshot`), every decoration (`pathmorphing`, `pathreplacing`, `markings`, `shapes`, `text`, `fractals`, `footprints`) and the older `snakes` library.
- **pgfplots**: 2D and 3D axes, `\addplot` of expressions, coordinates and tables, marks, legends, colorbars and colormaps, bar charts and histograms, error bars, `fill between`, surfaces, meshes, patches and contours (`shader=interp` shaded smoothly), images placed on the axes (`\addplot graphics` with a PNG), polar, ternary, Smith charts, `dateplot`, group plots and statistics; and the `datavisualization` library.

The details, and what is left out, are in [`spec/subset.md`](https://github.com/matheusmota-dev-br/tikra/blob/main/spec/subset.md) and [docs/conformance.md](https://github.com/matheusmota-dev-br/tikra/blob/main/docs/conformance.md).

Labels: node text is typeset by mathra in Latin Modern Math (in `\fontfamily{ptm}`, Times: TeX Gyre Termes, at TeX's widths), with `$…$` and `\(…\)` for math, `\textbf`, `\textit`, `\texttt`, `\textsc` and `\scshape`, `\textcolor`, `\\` for line breaks and `\tiny`…`\Huge`. Math follows mathra's default locale (English).

## The figure in words

Each figure comes with a text that tells what it shows, for those who cannot see it: screen readers read it, and it can be the `alt` of an image. tikra writes it from what it drew: a chart's kind, title, axes and their ranges, and its data (each value, or the formula and where the curve is lowest and highest); a diagram's nodes and its connections, with their direction and labels; the text of the other figures. Math is read the way [mathra](https://github.com/matheusmota-dev-br/mathra) speaks it.

The words come in mathra's 16 languages, English by default: `en`, `pt-BR`, `es`, `fr`, `de`, `it`, `ru`, `zh`, `ja`, `ko`, `hi`, `ar`, `id`, `tr`, `nl` and `pl`. The figure is drawn the same in all of them; only what is said about it changes, with the language's plurals, decimal comma and quotes, and its math spoken in that language. Each language is a JSON file in [`crates/core/src/locales/`](https://github.com/matheusmota-dev-br/tikra/tree/main/crates/core/src/locales), and a phrase a language lacks is English. English and pt-BR are reviewed; the other languages still need a native speaker's review.

| | |
|---|---|
| `alt` | One sentence, the SVG's `<title>`: *Bar chart: Rain by city and decade*. |
| `description` | The whole of it, the SVG's `<desc>`: *Bar chart titled “Rain by city and decade”. Horizontal axis, with the categories Lisbon, Oslo, Quito, Nairobi. Vertical axis “rainy days per year”, from 0 to 100. 3 series. “1990s”: 4 bars — Lisbon: 62, Oslo: 88, …* |

`render(code, { locale: "pt-BR" })` returns both in JavaScript, `render(code, locale="pt-BR")` in Python, `convert_with(code, &Options::locale(Locale::PtBr))` in Rust (`Output::alt`, `Output::description`), and `tikra figure.tex --describe --locale pt-BR` prints them; `tikzToSvg` and `tikz_to_svg` take the locale too, for the SVG's words. Every SVG has ids of its own for them, so a page with many figures labels each by its own words.

**The description is not perfect.** It tells what tikra can read in the drawing, not what the figure means: charts and diagrams come out well; a free drawing (a spiral, a Venn diagram, a timeline drawn by hand) gets little more than its text and how many shapes it has, and a figure can be told in a way its author did not intend. It is a starting point to review, not a finished text: read it before you publish, and when it falls short, write the figure's own with `alt`.

The author knows best what a figure means. Their words, given as LaTeX's tagging gives them, with `alt` on the picture, are told instead, as written in whatever language they are (math spoken in the locale), and then there is no description of tikra's:

```latex
\begin{tikzpicture}[alt={The unit circle, with the angle $\theta$ and its sine and cosine}]
```

## Warnings, not errors

What tikra does not understand becomes a **warning**, and the figure is drawn with the rest: unknown commands, environments and options, TikZ escaped twice or for Markdown (undone), labels with broken LaTeX (recovered by mathra's tolerant mode). `--warnings` lists them; `Output::warnings` has them in the library.

Limits protect against hostile input: 100,000 statements per figure, 64 levels of nesting, 2,000 samples per plot, coordinates within ±100,000 cm, PNG up to 100 megapixels. Past a limit you get a clear error or a warning, never a hang.

## Benchmark

[`corpus/`](https://github.com/matheusmota-dev-br/tikra/tree/main/corpus) holds 252 hand-made figures, one folder per topic, that cover the features above. Reproduce with:

```console
$ cargo run --release -p tikra --example bench
```

One run on an Apple M5 (arm64), 15 runs per figure, median per figure:

| Figures | Without warnings | With warnings | Errors |
|---:|---:|---:|---:|
| 252 | 252 | 0 | 0 |

| Per figure | Mean | p95 |
|---|---:|---:|
| SVG | 1.97 ms | 7.00 ms |
| SVG + PNG at 300 dpi | 7.30 ms | 18.86 ms |

The corpus now holds large figures too (3D surfaces of a thousand cells, fractals, the Lorenz attractor): most figures take well under a millisecond.

`tests/corpus.rs` checks on every `cargo test` that all corpus figures convert without warnings.

## Development

```console
$ cargo test                                  # every test
$ scripts/coverage.sh                          # coverage: every line runs in the tests
$ cargo clippy --all-targets -- -D warnings
$ npm/build.sh && npm/test/run.sh             # the npm package, packed and installed as users get it
$ site/build.sh                               # the playground: then open site/index.html
$ scripts/release.sh --dry-run                # what a release would publish
$ tools/conformance/run.sh                    # how far from TikZ: every manual example, TeX next to tikra
```

What each test file covers and the inputs built to break the interpreter are in [docs/tests.md](https://github.com/matheusmota-dev-br/tikra/blob/main/docs/tests.md).

tikra depends on the published `mathra` crate. To work against a local checkout of mathra next to this one, add to `.cargo/config.toml` (ignored by git), and leave the `Cargo.lock` changes it causes out of commits:

```toml
[patch.crates-io]
mathra = { path = "../mathra/crates/core" }
```

**Releasing.** Each package has its own version: `Cargo.toml` (crates `tikra` and `tikra-cli`), `npm/package.json` and `crates/python/Cargo.toml`. Bump the ones to release in a pull request, and turn `## Unreleased` of [CHANGELOG.md](https://github.com/matheusmota-dev-br/tikra/blob/main/CHANGELOG.md) into `## <version> - <date>`, with what changed and the corpus figures that show it; merge it, then run `scripts/release.sh` on `main`: it tags every version not yet on its registry (`v*`, `npm-v*`, `py-v*`) and pushes the tags at once; each tag runs its workflow (`crates.yml`, `npm.yml`, `pypi.yml`), which publishes with trusted publishing, without stored tokens. A rerun of a half-done release skips what is already out. The script stops when the version has no section in the changelog, and makes the GitHub release from it.

The first release needs a few steps by hand, because crates.io and npm only let a package that already exists trust a workflow:

1. crates.io: `cargo login`, then `cargo publish -p tikra` and `cargo publish -p tikra-cli`. Then, for each crate, Settings → Trusted Publishing: repository `matheusmota-dev-br/tikra`, workflow `crates.yml`.
2. npm: `npm/build.sh && cd npm/pkg && npm publish --access public` (the `@mathlabs` scope). Then in the package settings, Trusted Publisher → GitHub Actions: repository `matheusmota-dev-br/tikra`, workflow `npm.yml`.
3. PyPI accepts a trusted publisher before the project exists: Publishing → add a pending publisher, project `tikra`, repository `matheusmota-dev-br/tikra`, workflow `pypi.yml`, environment `pypi`. The `py-v*` tag publishes it.
4. GitHub: Settings → Pages → Source: "GitHub Actions", for the playground (`pages.yml`), and an environment named `pypi`.

The checks run on your machine: `git config core.hooksPath .githooks` once, and every commit runs fmt, clippy and the Rust tests, every push the wasm, npm and Python tests (`scripts/check.sh` runs them all by hand). `.github/workflows/ci.yml` runs the same checks again on `main` after each merge.

## Credits

tikra reads the languages of [TikZ and pgf](https://github.com/pgf-tikz/pgf), by Till Tantau and the PGF/TikZ team, and of [pgfplots](https://github.com/pgf-tikz/pgfplots), by Christian Feuersänger, and its figures are checked against the ones TeX draws from them. TikZ, pgf and pgfplots remain the reference; tikra is an independent project, not affiliated with them.

## Licenses

Code: MIT or Apache-2.0. Labels are set in Latin Modern Math, the OpenType version of TeX's Computer Modern (GUST Font License), which comes from mathra with its STIX Two Math (SIL OFL 1.1), and labels in Times in TeX Gyre Termes (GUST Font License, [crates/core/src/fonts](https://github.com/matheusmota-dev-br/tikra/blob/main/crates/core/src/fonts/README.md)); the Python and npm packages carry the license texts.
