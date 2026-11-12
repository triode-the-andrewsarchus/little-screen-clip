---
title: Native diagram previews
tags: [Pikchr, SvgBob, Mermaid]
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
