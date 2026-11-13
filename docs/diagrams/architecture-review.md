# Diagram integration architecture review

Compared against `before-parallel-diagram-families` with diff-aware fast
measurements. This integration checkpoint includes retained upstream Python,
Java, JavaScript and Rust source, including uncompiled reference/editor code.
It is not an application-only size comparison.

| Dimension | Baseline | Checkpoint | Delta |
| --- | ---: | ---: | ---: |
| Duplication | 7.22% | 7.39% | +0.17 percentage points |
| Average CCN | 3.56 | 3.43 | -0.13 |
| Maximum CCN | 274 | 274 | 0 |
| Functions | 25,796 | 35,202 | +9,406 |
| Clone groups | 3,132 | 4,240 | +1,108 |
| Measured cycles | 16 | 134 | +118 |
| Source lines | 655,537 | 802,930 | +147,393 |

The tool reports **REGRESSION**. Most growth is the separately committed
canonical engine/source inventories, not a new application framework. Imported
source stays intact for provenance, maintenance and companion redistribution;
it is not rewritten or moved to game the measurements.

## Seven review questions

1. **Unnecessary complexity:** the application hotspots are direct format
   dispatch (`render_inline`, CCN35), drawing operation dispatch (`apply`,17),
   toolbar choices (16) and gesture completion (16). Their increases name the
   new formats/actions/gesture. They do not create another parser, writer or
   lifecycle framework. Small mixed responsibilities were separated instead:
   Vega cloning is now at most13, Structurizr deployment10, DBML extension14,
   and the owned WireViz paths at most15 after physical-list/presentation reuse.
2. **Duplication:** reported application fragments include unchanged portions of
   touched viewers/tests, identical required license texts, declarative registry
   rows, policy-bearing SVG adapter calls and small shared Task API sequences.
   Actual reused mechanisms include camera rectangle projection, deferred Fit,
   a single Python resource boundary, a single Java invocation boundary and
   immutable companion publication. The seven-line pin/cable pairing expression
   serves parser normalization versus transaction admission; it is not a second
   parser or electrical graph implementation. Test fixture repetitions remain
   explicit. No generic abstraction was added solely to lower clone counts.
3. **Dependencies:** no new Rust application dependency cycle was found. The
   non-vendor Symbolator singleton reported as a cycle conflates the local
   `render(request)` callback with NuCanvas `canvas.render(False)`. Those are
   different functions. The remaining new cycle reports are in retained upstream
   inventories; the report does not prove new runtime import cycles.
4. **Proportionality:** each family reuses the existing bounded diagram worker,
   SVG composer, rasterizer, row cache, Markdown presentation and text session.
   Native editing adds preserving operations to the existing model/session;
   canonical text measurement reuses the renderer's actual shaping/fonts.
5. **New hotspots:** the four application coordinators above are reviewed
   necessary domain dispatch. No accidental high-complexity algorithm was left
   behind the newly introduced UI controls. The DBML table grammar's extra
   partial/record arms are likewise typed grammar mapping, not another parser.
6. **Existing debt:** operation dispatch grew, while the real stale-Fit policy
   was centralized in the existing Camera and shared across three canvases.
   Endpoint/movement/bounds mechanisms were reused rather than duplicated.
   No unrelated application hotspot was shuffled or deleted.
7. **Reuse opportunities:** reuse searches and source inspection selected the
   existing Task/CommandRequest, Preview/Camera, FileSource/undo/save,
   Excalidraw document records/shaper, WireViz Plan/YAML spans and Graphviz field
   ports. Native text/resize and wiring retain one accepted-edit and file-writer
   owner. Difficult source operations remain explicit refusals.

**Verdict:** accept the measured necessary engine/domain growth. Keep pristine
imports separate from adaptations, retain the stated compatibility limits, and
validate source preservation and real native gestures before completion. The
review found and fixed rejected text-draft loss plus stale-Fit handling; those
have separate commits and regression checks.

## Validation evidence

- Serialized workspace tests: 3,168 passed, 36 ignored, 0 failed across 47 test/doc targets.
  The default parallel run hit existing system/video helper admission contention
  (Busy/QueueFull/deadline); no production admission limits were weakened.
- Workspace all-target Clippy passes with `-D warnings`; normal desktop builds
  are warning-free.
- All12 new diagram languages pass native light/dark strip, hover, source/error
  and Markdown checks (131/130 observations).
- Mermaid's actual one-shot Fit/edit/Undo/Save scenarios pass in both themes.
- Native text/resize scenarios pass in both themes (32 observations each),
  including rejected-Apply recovery and exact source restoration.
- Native wiring scenarios pass in both themes (16 observations each), retaining
  electrical/source and held-artwork proofs rather than only matching source words.


## Vector preview and fixed-control follow-up

Compared with `before-vector-preview-ui`, source lines grow by 578 and functions
by 18. Duplication decreases by 0.01 percentage points, average CCN is unchanged at
the displayed precision, maximum CCN remains 274, and structural health changes
by -0.03. No rendering dependency or application dependency cycle is introduced.

The initial new properties coordinator and retained preparation coordinator were
simplified: selection actions have their own UI subsection, and all retained
clip/mask/pattern definitions share one geometry-validation traversal. The final
comparison reports no new excessive-complexity function. Existing short
preview/camera/worker tails and text-test fixtures remain explicit; no new
parser, cache, font stack, write owner or generic editor framework is added.

The tool still labels the comparison REGRESSION. Its Python-cycle additions and
removals have identical members in a different order, in untouched files. Its
acceptance-script locator scores reflect new test callers, not application
coupling. The remaining clone flags touch existing editor lifecycle sequences
and unchanged test setup. Those flags do not justify a second abstraction layer
for this bounded UI/painting change. The necessary controls and native shadow
composition are accepted with source-preservation and real GL validation.

Current validation, including the isolated media-seek test qualification, is
recorded in [vector previews](vector-previews.md).
