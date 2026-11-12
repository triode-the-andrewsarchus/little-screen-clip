---
title: Native diagram previews
tags: [Pikchr, SvgBob, Mermaid, Lini, Graphviz, D2, BPMN]
---

# From source to preview

These diagrams also have standalone source files beside this document.

```pikchr
box "Source"; arrow; box "Native renderer"; arrow; box "Preview"
```

```svgbob
+----------+      +-----------------+      +-----------+
| Source   +----->| Native renderer +----->| Preview   |
+----------+      +-----------------+      +-----------+
```

```mermaid
flowchart LR
    Source[Source] --> Renderer[Native renderer]
    Renderer --> Preview[Preview]
```

```lini
{ direction: row; gap: 44; padding: 16 }
|box#source| "Source" { fill: --sky-wash; stroke: --sky-ink; color: --sky-ink }
|box#renderer| "Native renderer" { fill: --teal-wash; stroke: --teal-ink; color: --teal-ink }
|box#preview| "Preview" { fill: --purple-wash; stroke: --purple-ink; color: --purple-ink }
source -> renderer
renderer -> preview
```

```graphviz
digraph Preview {
    rankdir=LR;
    node [shape=box, style="rounded,filled", fontname="Helvetica", color="#00576a", fillcolor="#d1f4fc"];
    source [label="Source"];
    renderer [label="Native renderer", color="#006a61", fillcolor="#cdf7f0"];
    preview [label="Preview", color="#573790", fillcolor="#efe7ff"];
    source -> renderer -> preview;
}
```

[Native equation examples](math.md) exercise inline/display math, matrices,
emphasis, lists, quotes, literal dollars and source diagnostics.

```d2
direction: right
vars: { d2-config: { pad: 24 } }
source: Source { style: { fill: "#d1f4fc"; stroke: "#00576a" } }
compiler: Compile + layout { style: { fill: "#cdf7f0"; stroke: "#006a61" } }
scene: Native scene { style: { fill: "#efe7ff"; stroke: "#573790" } }
preview: Preview { style: { fill: "#fff1d7"; stroke: "#935f0b" } }
source -> compiler -> scene -> preview
```

## A process without authored layout

```bpmn
<definitions xmlns="http://www.omg.org/spec/BPMN/20100524/MODEL" id="review_definition" targetNamespace="urn:example">
 <process id="review"><startEvent id="start"/><userTask id="task" name="Review order"/><endEvent id="end"/>
  <sequenceFlow id="first" sourceRef="start" targetRef="task"/><sequenceFlow id="last" sourceRef="task" targetRef="end"/>
 </process>
</definitions>
```

`order.bpmn` also demonstrates authored coordinates and namespace-qualified colors.

## Clipboard sources

Copy a complete raw diagram from one of these code blocks to preview it on the
little screen, then open the captured text to edit its source with a live preview.
Distinctive Mermaid/DOT/BPMN headers, quoted Pikchr shapes, Lini declarations,
D2 direction headers with edges and closed SvgBob boxes are recognized. Ambiguous
edge-only D2 stays text; use a `.d2` file or Markdown fence to select that syntax.
Incomplete or failed inferred renders stay readable text on the little screen.
Use Copy source to publish edits, or Save As to create a new file in an existing
directory. Original clipboard bytes are retained until explicitly edited.

## Digital timing

```wavedrom
{signal:[{name:'clock',wave:'p.......'},{name:'data',wave:'x3..4..x',data:['Address','Payload']}]}
```

`spi.json5` demonstrates grouped lanes, event arrows and a heading. JSON/JSON5
files need a distinctive WaveJSON structure; ordinary JSON stays text. Explicit
`.wavejson` and `.wavedrom` files select the renderer even while source is invalid.
Register (`reg`) and logic (`assign`) figures also render.

Timing files also have a direct signal-cell painter. Choose a state and click or
drag a lane; use Undo/Redo or Ctrl+Z/Ctrl+Shift+Z while the canvas is focused.
The source and rendered timing stay linked, including comments and unrelated
properties. Gaps/subcycles, adding lanes and editing bus labels/arcs still use
the highlighted source editor.
