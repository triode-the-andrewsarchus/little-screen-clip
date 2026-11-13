# Native chart previews

## Vega dataflow chart

```vega
{
  "$schema": "https://vega.github.io/schema/vega/v6.json",
  "description": "Canonical Vega: inline data, formula, band/linear scales and native SVG marks.",
  "background": "white",
  "width": 440,
  "height": 240,
  "padding": 12,
  "title": "Local preview throughput",
  "data": [{"name": "table", "values": [
    {"stage": "Parse", "ms": 8}, {"stage": "Layout", "ms": 23}, {"stage": "Raster", "ms": 15}
  ], "transform": [{"type": "formula", "expr": "datum.ms * 2", "as": "budget"}]}],
  "scales": [
    {"name": "x", "type": "band", "domain": {"data": "table", "field": "stage"}, "range": "width", "padding": 0.25},
    {"name": "y", "type": "linear", "domain": {"data": "table", "field": "budget"}, "range": "height", "nice": true, "zero": true}
  ],
  "axes": [{"orient": "bottom", "scale": "x"}, {"orient": "left", "scale": "y", "title": "Budget (ms)"}],
  "marks": [{"type": "rect", "from": {"data": "table"}, "encode": {"enter": {
    "x": {"scale": "x", "field": "stage"}, "width": {"scale": "x", "band": 1},
    "y": {"scale": "y", "field": "budget"}, "y2": {"scale": "y", "value": 0}, "fill": {"value": "#295f98"}
  }}}]
}

```

## Vega-Lite grouped bars

```vegalite
{
  "$schema": "https://vega.github.io/schema/vega-lite/v6.json",
  "description": "Canonical Vega-Lite: inline observations, aggregate encoding and color legend.",
  "title": "Latency by renderer",
  "width": 440,
  "height": 240,
  "data": {"values": [
    {"renderer": "SVG", "latency": 12, "phase": "First"},
    {"renderer": "SVG", "latency": 10, "phase": "First"},
    {"renderer": "Raster", "latency": 18, "phase": "First"},
    {"renderer": "SVG", "latency": 5, "phase": "Cached"},
    {"renderer": "Raster", "latency": 7, "phase": "Cached"}
  ]},
  "mark": {"type": "bar", "cornerRadiusTopLeft": 4, "cornerRadiusTopRight": 4},
  "encoding": {
    "x": {"field": "renderer", "type": "nominal", "title": "Renderer"},
    "xOffset": {"field": "phase"},
    "y": {"aggregate": "mean", "field": "latency", "type": "quantitative", "title": "Mean latency (ms)"},
    "color": {"field": "phase", "type": "nominal", "scale": {"range": ["#295f98", "#da9b42"]}}
  }
}

```

These are static previews. Use inline data; linked data and image URLs are not loaded.
