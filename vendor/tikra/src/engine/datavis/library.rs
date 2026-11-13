//! The keys of data visualizations that hold values or stand for lists of other keys: the styles
//! of ticks, grids and labels, the about strategies, the style sheets, the legend placements and
//! the axis systems (`school book axes`, `scientific axes`, `scientific polar axes` and its
//! ranges). Their names and what they set are the interface of TikZ's data visualization libraries;
//! the keys with code are native ([`super::keys`]).
//!
//! The axis systems are built from tables: the same options for the x and the y axis, each with
//! the other one as its direction axis; for polar axes, one row per range.

/// Commands, the `/pgf/data` keys and the generic keys of `/tikz/data visualization`.
const BASE: &str = r"
\def\tikzdatavisualizationset{\pgfqkeys{/tikz/data visualization}}
\def\pgfdvdeclarestylesheet#1#2{ \pgfkeys{/pgf/data visualization/style sheets/#1/.cd,#2} }
\def\tikra@dv@also#1{\expandafter\pgfkeysalso\expandafter{#1}}
\def\tikra@dv@append#1#2{\expandafter\def\expandafter#1\expandafter{#1#2}}
\def\tikra@dv@at{}\def\tikra@dv@atoptions{}\def\tikra@dv@defaultticks{}\def\tikra@dv@ticks{}
\def\tikra@dv@axis{}\def\tikra@dv@visualizer{}\def\tikra@dv@legend{}\def\tikra@dv@handler{lineto}
\newif\iftikra@dv@low \newif\iftikra@dv@high \newif\iftikra@dv@textonly \newif\ifpgfdvfilterpassed
\pgfkeys{/pgf/data/.cd,
  format/.initial=table, read from file/.initial=, inline/.style={read from file=},
  separator/.initial={,}, headline/.initial=, store in group/.initial=, use group/.initial=,
  new group/.code={\expandafter\gdef\csname tikra@dv@group@#1\endcsname{}},
  vars/.initial=, funcs/.initial=, samples/.initial=25, evaluator/.initial=\pgfmathparse,
  set/.style={/data point/set={#1}}}
\pgfkeys{/pgf/every data/.style={/tikz/every data/.try,/tikz/data visualization/every data/.try},
  /data point/outlier/.initial=, /data point/outlier/.default=true}
\tikzset{visualizer color/.code=\colorlet{visualizer color}{#1}, visualizer color=black}
\tikzdatavisualizationset{styling/.style=, style/.style={styling/.append style={#1}},
  @node styling/.initial=, node style/.style={/tikz/data visualization/@node styling/.append={,#1}},
  text colored/.style={node style={text=visualizer color}},
  low/.initial=0, high/.initial=0, padded/.style={low=padded min,high=padded max},
  tick length/.style={high={#1},low={-#1}},
  common/.initial=, major/.initial=, minor/.initial=, subminor/.initial=, direction axis/.initial=,
  at/.code=\def\tikra@dv@at{#1}, also at/.code=\tikra@dv@append\tikra@dv@at{,#1},
  options at/.code=\tikra@dv@append\tikra@dv@atoptions{,{#1}},
  no tick text at/.style={options at={#1 as [no tick text]}},
  at default ticks/.code=\tikra@dv@also\tikra@dv@defaultticks,
  at ticks/.code={\tikra@dv@also\tikra@dv@defaultticks\tikra@dv@also\tikra@dv@ticks},
  tick text at low/.is if=tikra@dv@low, tick text at high/.is if=tikra@dv@high,
  no tick text/.code={\tikra@dv@lowfalse\tikra@dv@highfalse},
  stack/.style={tick text even padding={#1}}, stack/.default=1em,
  stack'/.style={tick text odd padding={#1}}, stack'/.default=1em,
  tick text padding/.style={tick text even padding={#1},tick text odd padding={#1}}, clean ticks/.style={node style=fill=white},
  every ticks/.style={node style={font=\footnotesize,inner sep=1pt,outer sep=.1666em,rounded corners=1.5pt}},
  every data set label/.style={every ticks,node style={font=\small}},
  every axis label/.style={node style={font=\small}},
  every label in legend/.style={node style={fill=none}},
  every grid/.style={low=min,high=max},
  axis layer/.style=on background layer, grid layer/.style=on background layer,
  ticks layer/.style=on background layer, ticks node layer/.style=,
  @step/.initial=, step/.style={@step=\def\tikra@dv@step{#1}}, about/.style={@step=\tikra@dv@about{#1}},
  none/.style={@step=}, few/.style={about=3}, some/.style={about=5}, many/.style={about=10},
  phase/.initial=0, minor steps between steps/.initial=, minor steps between steps/.default=9,
  tick typesetter/.code=\pgfmathprintnumber{#1}, tick prefix/.initial=, tick suffix/.initial=,
  tick unit/.style={tick suffix={$\,#1$}},
  about strategy/.initial=,
  xy axes/.style={x axis={#1},y axis={#1}}, uv axes/.style={u axis={#1},v axis={#1}},
  xyz axes/.style={x axis={#1},y axis={#1},z axis={#1}}, uvw axes/.style={u axis={#1},v axis={#1},w axis={#1}},
}
\tikzdatavisualizationset{axis options/.cd,
  padding/.style={padding max={#1},padding min={-#1}},
  length/.style={scaling=min at 0 and max at {#1}},
  power unit length/.style={scaling=1 at 0 and 10 at {#1}},
  linear steps/.style={tick placement strategy=linear},
  exponential steps/.style={tick placement strategy=exponential},
  logarithmic/.style={function=\pgfdvmathln{\pgfvalue}{\pgfvalue},@default scaling=1 at 0 and 10 at 1,exponential steps},
  degrees/.style={scaling=0 at 0 and 1 at 1},
  radians/.style={scaling=0 at 0 and 0.03141592653589793 at 1.8},
  percent/.style={scaling=0 at 0 and 1 at 3.6},
  full circle/.style={scaling=min at 0 and max at 360},
  half circle/.style={scaling=min at 0 and max at 180},
  quarter circle/.style={scaling=min at 0 and max at 90},
}
\tikzdatavisualizationset{visualizer options/.cd, style/.style={/tikz/data visualization/visualizers/\tikra@dv@visualizer/styling/.append style={,#1}},
  label in legend options/.style={/tikz/data visualization/visualizers/\tikra@dv@visualizer/label in legend options/.append style={,#1}},
  attribute 1/.style={/data point/\tikra@dv@visualizer/attribute 1={#1}},
  attribute 2/.style={/data point/\tikra@dv@visualizer/attribute 2={#1}},
  pin in data/.style={label in data={tikra pin=true,#1}},
}
\tikzdatavisualizationset{visualizer label options/.cd,
  text/.initial=, text'/.style={text={#1},node style=swap,tikra pin sign=-},
  node style/.code={\pgfkeysalso{/tikz/data visualization/node style={#1}}},
  text colored/.style={/tikz/data visualization/node style={text=visualizer color}},
  pin angle/.initial=, pin length/.initial=3ex, tikra pin/.initial=, tikra pin sign/.initial=,
}
\tikzdatavisualizationset{legend entry options/.cd,
  text/.initial=, legend/.initial=main legend, setup/.initial=,
  node style/.code={\pgfkeysalso{/tikz/data visualization/node style={#1}}},
  text colored/.style={/tikz/data visualization/node style={text=visualizer color}},
  visualizer in legend/.initial=, visualizer in legend styling/.initial=,
  visualizer in legend style/.style={/tikz/data visualization/legend entry options/visualizer in legend styling/.append={,#1}},
  text right/.style={node style={anchor=mid west,inner ysep=1pt,inner xsep=0pt,reset cm,xshift=.333em},visualizer in legend style={reset cm}},
  text left/.style={node style={anchor=mid east,inner ysep=1pt,inner xsep=0pt,reset cm,xshift=-.333em,align=right},visualizer in legend style={reset cm,xscale=-1}},
  text only/.style={node style={anchor=mid west,inner ysep=1pt,inner xsep=0pt,reset cm,text=visualizer color},/utils/exec=\tikra@dv@textonlytrue},
  label in legend line coordinates/.initial=, label in legend mark coordinates/.initial=, label in legend rectangle coordinates/.initial={(-1ex,-0.5ex),(-1ex,0.968ex),(0ex,0.968ex),(0ex,-0.5ex)},
  % what a visualizer shows in a legend unless told: a zig zag line, a circle, one mark
  default label in legend path/.style=zig zag label in legend line, default label in legend closed path/.style=circular label in legend line,
  default label in legend mark/.style=label in legend one mark,
}
";

/// Tick and grid styles of each kind: the tick's options, the grid's options.
const KINDS: &[(&str, &str, &str)] = &[
    ("major", "style={line cap=round},tick length=2pt", "style={solid, help lines,thin,black!25}"),
    ("minor", "style={solid, help lines,thin,line cap=round},tick length=1.4pt", "style={solid, help lines,black!25}"),
    ("subminor", "style={solid, help lines,line cap=round},tick length=0.8pt", "style={solid, help lines,black!10}"),
];

/// The about strategies: thresholds of a step's first digit, and the digit they round to.
const STRATEGIES: &[(&str, &str)] = &[
    ("standard", "1.5/1.0,2.3/2.0,4/2.5,7/5,11/10"),
    ("euro", "1.5/1.0,3/2.0,7/5,11/10"),
    ("decimal", "3/1.0,11/10"),
    ("half", "2/1.0,7/5,11/10"),
    ("quarter", "1.8/1.0,4/2.5,7/5,11/10"),
    ("int", "1.5/1.0,2.5/2.0,3.5/3.0,4.5/4.0,7/5,11/10"),
];

/// The style sheets: name, the style for values without one, and the numbered styles.
const SHEETS: &[(&str, &str, &[&str])] = &[
    ("vary thickness", "line width={0.3pt+(#1)*0.2pt}", &[]),
    (
        "vary dashing",
        "solid",
        &[
            "solid",
            "dash pattern=on 5\\pgflinewidth off 2\\pgflinewidth",
            "dash pattern=on 1.5\\pgflinewidth off 1.5\\pgflinewidth",
            "dash pattern=on 5\\pgflinewidth off 1.5\\pgflinewidth on 1.5\\pgflinewidth off 1.5\\pgflinewidth",
            "dash pattern=on 10\\pgflinewidth off 3\\pgflinewidth",
            "dash pattern=on 10\\pgflinewidth off 2\\pgflinewidth on 2\\pgflinewidth off 2\\pgflinewidth",
            "dash pattern=on 8\\pgflinewidth off 2\\pgflinewidth on 1.5\\pgflinewidth off 1.5\\pgflinewidth on 1.5\\pgflinewidth off 1.5\\pgflinewidth on 1.5\\pgflinewidth off 1.5\\pgflinewidth",
        ],
    ),
    ("strong colors", "visualizer color=black", &["black", "red!80!black", "blue!80!black", "green!60!black", "orange!80!black", "black!60"]),
    ("* mark", "mark=*,every mark/.append style=thin,mark size=1.4pt", &[]),
    ("dot mark", "mark=*,every mark/.append style=thin,mark size=0.6pt", &[]),
    ("o mark", "mark=o,every mark/.append style=thin,mark size=1.4pt", &[]),
];

/// The marks of `cross marks`, in order.
const CROSS_MARKS: &[&str] = &["x", "+", "Mercedes star", "Mercedes star flipped", "star"];

/// The color series style sheets: name, model, first color, step.
const SERIES: &[(&str, &str, &str, &str)] = &[
    ("vary hue", "hsb", ".4,0.9,0.8", ".213,0,0"),
    ("shades of blue", "hsb", ".65,1.4,1", "0,-.4,-.0"),
    ("shades of red", "hsb", "0,1.4,1", "0,-.4,-.0"),
    ("gray scale", "hsb", "0,0,-.34", "0,0,.34"),
];

/// Legend placements outside the data: name, `columns`/`rows`, the point, the anchor, more.
const OUTSIDE: &[(&str, &str, &str, &str, &str)] = &[
    ("east outside", "columns", "[xshift=.8em]data visualization bounding box.east", "west", ""),
    ("north east outside", "columns", "[xshift=.8em]data visualization bounding box.north east|-data bounding box.north", "north west", ""),
    ("south east outside", "columns", "[xshift=.8em]data visualization bounding box.south east|-data bounding box.south", "south west", ""),
    ("west outside", "columns", "[xshift=-.8em]data visualization bounding box.west", "east", "label style=text left,"),
    ("north west outside", "columns", "[xshift=-.8em]data visualization bounding box.north west|-data bounding box.north", "north east", "label style=text left,"),
    ("south west outside", "columns", "[xshift=-.8em]data visualization bounding box.south west|-data bounding box.south", "south east", "label style=text left,"),
    ("south outside", "rows", "[yshift=-.5em]data visualization bounding box.south", "north", ""),
    ("north outside", "rows", "[yshift=.5em]data visualization bounding box.north", "south", ""),
];

/// Legend placements inside the data's box.
const INSIDE: &[(&str, &str, &str, &str)] = &[
    ("north east inside", "columns", "[xshift=-.3em]data bounding box.north east", "north east"),
    ("south west inside", "columns", "[xshift=.3em]data bounding box.south west", "south west"),
    ("north west inside", "columns", "[xshift=.3em]data bounding box.north west", "north west"),
    ("south east inside", "columns", "[xshift=-.3em]data bounding box.south east", "south east"),
    ("west inside", "columns", "[xshift=.3em]data bounding box.west", "west"),
    ("east inside", "columns", "[xshift=.3em]data bounding box.east", "east"),
    ("north inside", "rows", "data bounding box.north", "north"),
    ("south inside", "rows", "data bounding box.south", "south"),
];

/// Legends placed at a data point: name and anchor.
const AT_VALUES: &[(&str, &str)] = &[
    ("at values", "center"),
    ("right of", "west"),
    ("left of", "east"),
    ("above of", "south"),
    ("below of", "north"),
    ("above right of", "south west"),
    ("above left of", "south east"),
    ("below right of", "north west"),
    ("below left of", "north east"),
];

/// Legend entry samples: name, the line's coordinates, the marks' coordinates.
const SAMPLES: &[(&str, &str, &str)] = &[
    ("label in legend one mark", "", "(0,0)"),
    ("label in legend three marks", "", "(-3ex,-.3ex),(-1.5ex,.3ex),(0,0)"),
    ("straight label in legend line", "(-2em,0),(-1.5em,0),(-.5em,0),(0,0)", "(-1.5em,0),(-.5em,0)"),
    ("zig zag label in legend line", "(-2em,0),(-1.5em,.3ex),(-.5em,-.3ex),(0,0)", "(-1.5em,.3ex),(-.5em,-.3ex)"),
];

/// How a visualizer joins its points: the option, the handler, the legend sample.
const HANDLERS: &[(&str, &str, &str)] = &[
    ("no lines", "discard", "default label in legend mark"),
    ("smooth line", "curveto", "default label in legend path"),
    ("smooth cycle", "closedcurve", "default label in legend closed path"),
    ("straight line", "lineto", "default label in legend path"),
    ("straight cycle", "polygon", "default label in legend closed path"),
    ("polygon", "polygon", "default label in legend closed path"),
    ("gap line", "lineto", "default label in legend path"),
    ("gap cycle", "polygon", "gap circular label in legend line"),
];

/// Points around an ellipse, shifted, as legend sample coordinates.
fn ellipse_points(angles: &[i32], shift: &str, radii: &str) -> String {
    angles.iter().map(|a| format!("([{shift}]{a}:{radii})")).collect::<Vec<_>>().join(",")
}

fn other(axis: &str) -> &'static str {
    if axis == "x" {
        "y"
    } else {
        "x"
    }
}

/// `school book axes`: axes through the origin with arrows, one unit a centimeter.
fn school_book() -> String {
    let unit = r"\pgfkeysvalueof{/tikz/data visualization/school book axes/unit}";
    let mut axes = String::from("xy Cartesian");
    let mut labels = String::new();
    for a in ["x", "y"] {
        let o = other(a);
        let zero = if a == "x" { "{style={draw=none},no tick text}" } else { "{high=0,low=0,style={draw=none},node style={anchor=north east}}" };
        axes.push_str(&format!(
            ",{a} axis={{include value=0,scaling=0 at 0 and {unit} at 1cm,visualize axis={{{o} axis={{goto=0}},style=->,padded}},\
             visualize ticks={{common={{{o} axis={{goto=0}},direction axis={o} axis}},major={{tick text at low,options at=0 as [{zero}]}}}},\
             visualize grid={{common={{direction axis={o} axis,padded}}}},ticks={{step={unit}}},padding=0.75em}}"
        ));
        labels.push_str(&format!("{a} axis={{visualize label={{{a} axis={{goto=padded max,anchor at max}},{o} axis={{goto=0}}}}}},"));
    }
    format!(
        "\\tikzdatavisualizationset{{new axis system={{school book axes}}{{{axes}}}{{standard labels}}{{@make labels}},\
         school book axes/unit/.initial=1,\
         school book axes/standard labels/.style={{/tikz/data visualization/@make labels/.style={{{labels}}}}}}}\n"
    )
}

/// `scientific axes`: a box of a given width and height around the data, with ticks outside or
/// inside, or `clean`; the labels below and left, upright, or at the ends.
fn scientific() -> String {
    let p = "/tikz/data visualization/scientific axes";
    let mut out = format!(
        "\\tikzdatavisualizationset{{new axis system={{scientific axes}}{{xy Cartesian,every axis/.append style={{style={{draw=black!50}}}},\
         every ticks/.append style={{style={{draw=black!50}}}},x axis={{length=\\pgfkeysvalueof{{{p}/width}},ticks}},\
         y axis={{length=\\pgfkeysvalueof{{{p}/height}},ticks}}}}{{standard labels,outer ticks}}{{@make axes,@make labels}},\
         scientific axes/width/.initial=5cm,scientific axes/height/.initial=0.618*\\pgfkeysvalueof{{{p}/width}}}}\n"
    );
    // ticks outside or inside: the side that stays at the axis line
    for (name, at_min, at_max) in [("outer ticks", "high", "low"), ("inner ticks", "low", "high")] {
        let mut body = String::new();
        for a in ["x", "y"] {
            let o = other(a);
            body.push_str(&format!(
                "{a} axis={{visualize ticks={{common={{{o} axis={{goto=padded min}},direction axis={o} axis,{at_min}=0pt}},major={{tick text at low}}}},\
                 visualize ticks={{common={{{o} axis={{goto=padded max}},direction axis={o} axis,{at_max}=0pt}}}},\
                 visualize axis={{{o} axis={{goto=padded min}},padded}},visualize axis={{{o} axis={{goto=padded max}},padded}},\
                 visualize grid={{common={{direction axis={o} axis,padded}}}}}},"
            ));
        }
        out.push_str(&format!("\\tikzdatavisualizationset{{scientific axes/{name}/.style={{/tikz/data visualization/@make axes/.style={{{body}}}}}}}\n"));
    }
    let mut clean = String::new();
    for a in ["x", "y"] {
        let o = other(a);
        let frame = "style={black!25, line cap=rect}";
        clean.push_str(&format!(
            "{a} axis={{visualize ticks={{common={{{o} axis={{goto=padded min}},direction axis={o} axis,high=0pt}},major={{tick text at low}}}},\
             visualize axis={{{o} axis={{goto=padded min}}}},visualize axis={{{o} axis={{goto=min}},{frame}}},visualize axis={{{o} axis={{goto=max}},{frame}}},\
             visualize grid={{common={{direction axis={o} axis}}}},padding=.5em}},"
        ));
    }
    out.push_str(&format!("\\tikzdatavisualizationset{{scientific axes/clean/.style={{/tikz/data visualization/@make axes/.style={{{clean}}}}}}}\n"));
    let bb = "data visualization bounding box";
    for (name, x, y) in [
        (
            "standard labels",
            format!("x axis={{goto pos=.5}},node style={{at={{(0,0 |- {bb}.south)}},below}}"),
            format!("y axis={{goto pos=.5}},node style={{at={{(0,0 -| {bb}.west)}},rotate=90,above}}"),
        ),
        (
            "upright labels",
            format!("x axis={{goto pos=.5}},node style={{at={{(0,0 |- {bb}.south)}},below}}"),
            format!("y axis={{goto pos=.5}},node style={{at={{(0,0 -| {bb}.west)}},left}}"),
        ),
        (
            "end labels",
            format!("y axis={{goto=padded min}},node style={{at={{(0,0 -| {bb}.east)}},right}}"),
            format!("x axis={{goto=padded min}},node style={{at={{(0,0 |- {bb}.north)}},above}}"),
        ),
    ] {
        out.push_str(&format!(
            "\\tikzdatavisualizationset{{scientific axes/{name}/.style={{/tikz/data visualization/@make labels/.style={{x axis={{visualize label={{{x}}}}},y axis={{visualize label={{{y}}}}}}}}}}}\n"
        ));
    }
    out
}

/// A range of `scientific polar axes`: its name, the angle axis' options, the units (degrees or
/// radians), the anchor of the 0 tick's text, the angles (and the side of their text) of the
/// radius axes, and those of the clean variant.
struct Range {
    name: &'static str,
    angle: &'static str,
    unit: &'static str,
    zero: &'static str,
    inner: &'static str,
    clean: &'static str,
}

const PI: &str = "3.141592653589793";
const HALF_PI: &str = "1.570796326794897";

/// The clean variant's radius axis at `at`: its texts on `side`, its ticks from the axis toward
/// `end`, moved by the angle axis' padding at the range's `end` (`shift` is `xshift`, `yshift`,
/// `-xshift`…), and more options for its ticks.
fn entry(which: &str, at: &str, side: &str, end: &str, shift: &str, more: &str) -> String {
    let shift = match shift.strip_prefix('-') {
        _ if shift.is_empty() => String::new(),
        Some(k) => format!("{k}=-\\pgfkeysvalueof{{/tikz/data visualization/angle axis/padding {which}}}"),
        None => format!("{shift}=\\pgfkeysvalueof{{/tikz/data visualization/angle axis/padding {which}}}"),
    };
    format!("{{{at}}}{{{side}}}{{{end}}}{{{shift}}}{{{more}}}")
}

fn ranges() -> Vec<Range> {
    let nt = "options at=0 as [{style={draw=none}, no tick text}]";
    let leak = |s: String| -> &'static str { Box::leak(s.into_boxed_str()) };
    vec![
        Range {
            name: "0 to pi half",
            angle: leak(format!("min value=0, max value={HALF_PI}")),
            unit: "@rad",
            zero: "north east",
            inner: leak(format!("{{0}}{{low}},{{{HALF_PI}}}{{high}}")),
            clean: leak(format!("{},{}", entry("min", "0", "low", "high", "yshift", ""), entry("max", HALF_PI, "high", "low", "-xshift", ""))),
        },
        Range {
            name: "0 to 90",
            angle: "min value=0, max value=90",
            unit: "@degrees",
            zero: "north east",
            inner: "{0}{low},{90}{high}",
            clean: leak(format!("{},{}", entry("min", "0", "low", "high", "yshift", ""), entry("max", "90", "high", "low", "-xshift", ""))),
        },
        Range {
            name: "quadrant",
            angle: "scaling=min at 0 and max at 90, ticks",
            unit: "",
            zero: "north east",
            inner: "{min}{low},{max}{high}",
            clean: leak(format!("{},{}", entry("min", "min", "low", "high", "yshift", ""), entry("max", "max", "high", "low", "-xshift", ""))),
        },
        Range {
            name: "quadrant clockwise",
            angle: "scaling=min at 90 and max at 0, ticks",
            unit: "",
            zero: "north east",
            inner: "{min}{low},{max}{high}",
            clean: leak(format!("{},{}", entry("min", "max", "high", "low", "yshift", ""), entry("max", "min", "low", "high", "-xshift", ""))),
        },
        Range {
            name: "-pi half to 0",
            angle: leak(format!("min value=-{HALF_PI}, max value=0")),
            unit: "@rad",
            zero: "south east",
            inner: leak(format!("{{-{HALF_PI}}}{{low}},{{0}}{{high}}")),
            clean: leak(format!("{},{}", entry("min", &format!("-{HALF_PI}"), "low", "high", "xshift", ""), entry("max", "0", "high", "low", "yshift", ""))),
        },
        Range {
            name: "-90 to 0",
            angle: "min value=-90, max value=0",
            unit: "@degrees",
            zero: "south east",
            inner: "{-90}{low},{0}{high}",
            clean: leak(format!("{},{}", entry("min", "-90", "low", "high", "xshift", ""), entry("max", "0", "high", "low", "yshift", ""))),
        },
        Range {
            name: "fourth quadrant",
            angle: "scaling=min at -90 and max at 0, ticks",
            unit: "",
            zero: "south east",
            inner: "{min}{low},{max}{high}",
            clean: leak(format!("{},{}", entry("min", "min", "low", "high", "xshift", ""), entry("max", "max", "high", "low", "yshift", ""))),
        },
        Range {
            name: "fourth quadrant clockwise",
            angle: "scaling=min at 0 and max at -90, ticks",
            unit: "",
            zero: "south east",
            inner: "{min}{low},{max}{high}",
            clean: leak(format!("{},{}", entry("min", "max", "high", "low", "xshift", ""), entry("max", "min", "low", "high", "yshift", ""))),
        },
        Range {
            name: "0 to pi",
            angle: leak(format!("min value=0, max value={PI}")),
            unit: "@rad",
            zero: "north",
            inner: leak(format!("{{0}}{{low}},{{{HALF_PI}}}{{high}},{{{PI}}}{{high}}")),
            clean: leak(format!("{},{}", entry("min", "0", "low", "high", "yshift", ""), entry("max", PI, "high", "low", "-yshift", nt))),
        },
        Range {
            name: "0 to 180",
            angle: "min value=0, max value=180",
            unit: "@degrees",
            zero: "north",
            inner: "{0}{low},{90}{high},{180}{high}",
            clean: leak(format!("{},{}", entry("min", "0", "low", "high", "yshift", ""), entry("max", "180", "high", "low", "-yshift", nt))),
        },
        Range {
            name: "upper half",
            angle: "scaling=min at 0 and max at 180, ticks",
            unit: "",
            zero: "north",
            inner: "{min}{low},{max}{high}",
            clean: leak(format!("{},{}", entry("min", "min", "low", "high", "yshift", ""), entry("max", "max", "high", "low", "-yshift", nt))),
        },
        Range {
            name: "upper half clockwise",
            angle: "scaling=min at 180 and max at 0, ticks",
            unit: "",
            zero: "north",
            inner: "{min}{low},{max}{high}",
            clean: leak(format!("{},{}", entry("min", "min", "low", "high", "yshift", ""), entry("max", "max", "high", "low", "-yshift", nt))),
        },
        Range {
            name: "lower half",
            angle: "scaling=min at -180 and max at 0, ticks",
            unit: "",
            zero: "south",
            inner: "{min}{low},{max}{high}",
            clean: leak(format!("{},{}", entry("min", "min", "low", "high", "-yshift", ""), entry("max", "max", "high", "low", "yshift", nt))),
        },
        Range {
            name: "lower half clockwise",
            angle: "scaling=min at 0 and max at -180, ticks",
            unit: "",
            zero: "south",
            inner: "{min}{low},{max}{high}",
            clean: leak(format!("{},{}", entry("min", "min", "low", "high", "-yshift", ""), entry("max", "max", "high", "low", "yshift", nt))),
        },
        Range {
            name: "-pi half to pi half",
            angle: leak(format!("min value=-{HALF_PI}, max value={HALF_PI}")),
            unit: "@rad",
            zero: "east",
            inner: leak(format!("{{0}}{{low}},{{{HALF_PI}}}{{low}},{{-{HALF_PI}}}{{high}}")),
            clean: leak(format!("{},{}", entry("min", &format!("-{HALF_PI}"), "low", "high", "xshift", ""), entry("max", HALF_PI, "high", "low", "-xshift", nt))),
        },
        Range {
            name: "-90 to 90",
            angle: "min value=-90, max value=90",
            unit: "@degrees",
            zero: "east",
            inner: "{-90}{low},{0}{low},{90}{high}",
            clean: leak(format!("{},{}", entry("min", "-90", "low", "high", "xshift", ""), entry("max", "90", "high", "low", "-xshift", nt))),
        },
        Range {
            name: "right half",
            angle: "scaling=min at -90 and max at 90, ticks",
            unit: "",
            zero: "east",
            inner: "{min}{low},{max}{high}",
            clean: leak(format!("{},{}", entry("min", "min", "low", "high", "xshift", ""), entry("max", "max", "high", "low", "-xshift", nt))),
        },
        Range {
            name: "right half clockwise",
            angle: "scaling=min at 90 and max at -90, ticks",
            unit: "",
            zero: "east",
            inner: "{min}{low},{max}{high}",
            clean: leak(format!("{},{}", entry("min", "min", "low", "high", "xshift", ""), entry("max", "max", "high", "low", "-xshift", nt))),
        },
        Range {
            name: "left half",
            angle: "scaling=min at 90 and max at 270, ticks",
            unit: "",
            zero: "west",
            inner: "{min}{low},{max}{high}",
            clean: leak(format!("{},{}", entry("min", "min", "low", "high", "-xshift", ""), entry("max", "max", "high", "low", "xshift", nt))),
        },
        Range {
            name: "left half clockwise",
            angle: "scaling=min at 270 and max at 90, ticks",
            unit: "",
            zero: "west",
            inner: "{min}{low},{max}{high}",
            clean: leak(format!("{},{}", entry("min", "min", "low", "high", "-xshift", ""), entry("max", "max", "high", "low", "xshift", nt))),
        },
        Range {
            name: "0 to 2pi",
            angle: "min value=0, max value=6.283185307179586",
            unit: "@rad",
            zero: "north east",
            inner: leak(format!("{{0}}{{low}},{{{HALF_PI}}}{{high}},{{{PI}}}{{high}},{{4.71238898038469}}{{low}}")),
            clean: "{0}{low}{high}{}{}",
        },
        Range {
            name: "-pi to pi",
            angle: leak(format!("min value=-{PI}, max value={PI}")),
            unit: "@rad",
            zero: "north east",
            inner: leak(format!("{{0}}{{low}},{{{HALF_PI}}}{{high}},{{{PI}}}{{high}},{{-1.57079632679489}}{{low}}")),
            clean: "{0}{low}{high}{}{}",
        },
        Range {
            name: "0 to 360",
            angle: "min value=0, max value=360",
            unit: "@degrees",
            zero: "north east",
            inner: "{0}{low},{90}{high},{180}{high},{270}{low}",
            clean: "{0}{low}{high}{}{}",
        },
        Range {
            name: "-180 to 180",
            angle: "min value=-180, max value=180",
            unit: "@degrees",
            zero: "north east",
            inner: "{0}{low},{90}{high},{180}{high},{-90}{low}",
            clean: "{0}{low}{high}{}{}",
        },
    ]
}

/// `scientific polar axes`: a circle (or part of one) for the angles, radius axes at some angles.
fn polar() -> String {
    let p = "/tikz/data visualization";
    let hidden = "[{style={draw=none},no tick text}]";
    let mut out = String::from(
        "\\tikzdatavisualizationset{new axis system={scientific polar axes}{new polar axes,angle axis={attribute=angle},radius axis={attribute=radius},\
         every axis/.append style={style={draw=black!50}},every ticks/.append style={style={draw=black!50}}}{0 to 360,outer ticks}\
         {@make axes,scientific polar axes/@setup axes,@radius axes,@make labels},\
         @which radius axes at/.initial=@inner radius axes at,@which visualize radius axis/.initial=@visualize radius axis,\
         @inner radius axes at/.initial=,@clean radius axes at/.initial=,@make labels/.style=,\
         scientific polar axes/radius/.initial=3.25cm}\n",
    );
    // a radius axis at an angle, with its ticks' texts on one side
    out.push_str(&format!(
        "\\tikzdatavisualizationset{{@visualize radius axis/.style 2 args={{radius axis={{visualize axis={{angle axis={{goto=#1}},high=padded max,low=0}},\
         visualize ticks={{common={{angle axis={{goto=#1}},direction axis=angle axis}},major={{tick text at #2,options at=0 as {hidden}}}}}}}}},\
         @visualize clean radius axis/.style n args={{5}}{{radius axis={{\
         visualize axis={{angle axis={{goto=#1}},high=max,low=0,style={{black!25,line cap=rect}}}},\
         visualize axis={{angle axis={{goto=#1}},high=max,low=0,style={{line cap=rect,#4}}}},\
         visualize ticks={{style={{#4}},common={{angle axis={{goto=#1}},direction axis=angle axis,#3=0}},major={{tick text at #2,#5}}}}}}}},\
         @inner visualize zero/.style={{radius axis={{visualize ticks={{common={{angle axis={{goto=0}},direction axis=angle axis}},\
         major={{tick text at low,low=0,high=0,at=0 as [{{style={{draw=none}},node style={{anchor=#1}}}}]}}}}}}}},\
         @clean visualize zero/.style=,@visualize zero/.style={{@inner visualize zero={{#1}}}},\
         @degrees/.style={{angle axis={{degrees,ticks={{step=15,tick suffix=$^\\circ$,major={{options at=360 as {hidden},options at=-180 as {hidden}}}}}}}}}}}\n"
    ));
    // radians: multiples of π/12, as fractions of π; the ends of the range without texts
    let ends = ["6.2831985e0", "6.2831192e0", "6.2830794e0", "6.2828787e0", "6.2831598e0", "-3.1415991e0", "-3.1415802e0", "-3.1415602e0", "-3.14153961e0", "-3.1414406e0"];
    let hide: Vec<String> = ends.iter().map(|v| format!("options at={v} as {hidden}")).collect();
    out.push_str(&format!(
        "\\tikzdatavisualizationset{{@rad/.style={{angle axis={{radians,ticks={{step=(pi/12),node style={{/pgf/number format/frac,/pgf/number format/frac shift={{1}}}},\
         tick typesetter/.code=\\tikra@dv@pi{{##1}},major={{{}}}}}}}}}}}\n",
        hide.join(",")
    ));
    // the three ways to draw the axes
    for (name, which, visualize, zero, radius_padding, angle) in [
        ("outer ticks", "@inner radius axes at", "@visualize radius axis", "@inner visualize zero", "0em", "visualize axis={radius axis={goto=padded max},low=min,high=max},visualize ticks={common={radius axis={goto=padded max},direction axis=radius axis},common={low=0},major={tick text at high}},visualize grid={common={direction axis=radius axis},low=0,high=padded max}"),
        ("inner ticks", "@inner radius axes at", "@visualize radius axis", "@inner visualize zero", "0em", "visualize axis={radius axis={goto=padded max},low=min,high=max},visualize ticks={common={radius axis={goto=padded max},direction axis=radius axis},common={high=0},major={tick text at high}},visualize grid={common={direction axis=radius axis},low=0,high=padded max}"),
        ("clean", "@clean radius axes at", "@visualize clean radius axis", "@clean visualize zero", ".5em", "padding=.5em,visualize axis={radius axis={goto=max},style={black!25, line cap=rect},low=min,high=max},visualize axis={radius axis={goto=padded max},style={line cap=rect},low=min,high=max},visualize ticks={common={radius axis={goto=padded max},direction axis=radius axis},common={low=0},major={tick text at high}},visualize grid={common={direction axis=radius axis},low=0,high=max}"),
    ] {
        out.push_str(&format!(
            "\\tikzdatavisualizationset{{scientific polar axes/{name}/.style={{{p}/@make axes/.style={{\
             radius axis={{min value=0,length=\\pgfkeysvalueof{{{p}/scientific polar axes/radius}},padding={radius_padding},ticks={{few}},\
             visualize grid={{common={{direction axis=angle axis,low=min,high=max}}}}}},angle axis={{{angle}}}}},\
             {p}/@which radius axes at={which},{p}/@which visualize radius axis={visualize},{p}/@visualize zero/.style={{{zero}={{##1}}}}}}}}\n"
        ));
    }
    for r in ranges() {
        let unit = if r.unit.is_empty() { String::new() } else { format!("{},", r.unit) };
        out.push_str(&format!(
            "\\tikzdatavisualizationset{{scientific polar axes/{}/.style={{@setup axes/.style={{angle axis={{{}}},{unit}@visualize zero={},\
             @inner radius axes at={{{}}},@clean radius axes at={{{}}}}}}}}}\n",
            r.name, r.angle, r.zero, r.inner, r.clean
        ));
    }
    out
}

/// Everything, as TeX to run once.
pub fn definitions() -> String {
    let mut out = String::from(BASE);
    for (kind, ticks, grid) in KINDS {
        out.push_str(&format!(
            "\\tikzdatavisualizationset{{every {kind} ticks/.style={{{ticks}}},every {kind} grid/.style={{{grid}}},\
             {kind} at/.style={{{kind}={{at={{#1}}}}}},{kind} also at/.style={{{kind}={{also at={{#1}}}}}}}}\n"
        ));
    }
    for side in ["low", "high"] {
        for parity in ["odd", "even"] {
            out.push_str(&format!("\\tikzdatavisualizationset{{tick text {side} {parity} padding/.initial=0pt}}\n"));
        }
    }
    for parity in ["odd", "even"] {
        out.push_str(&format!(
            "\\tikzdatavisualizationset{{tick text {parity} padding/.style={{tick text low {parity} padding={{-#1}},tick text high {parity} padding={{#1}}}}}}\n"
        ));
    }
    for (name, list) in STRATEGIES {
        out.push_str(&format!("\\tikzdatavisualizationset{{{name} about strategy/.style={{about strategy={{{list}}}}}}}\n"));
    }
    out.push_str("\\tikzdatavisualizationset{standard about strategy}\n");
    // the visualizers' handlers
    for (option, handler, sample) in HANDLERS {
        out.push_str(&format!("\\tikzdatavisualizationset{{visualizer options/{option}/.style={{@handler={{{handler}}}{{{sample}}}}}}}\n"));
    }
    // style sheets
    for (name, default, numbered) in SHEETS {
        let mut styles = format!("default style/.style={{{default}}}");
        for (i, s) in numbered.iter().enumerate() {
            let s = if *name == "strong colors" { format!("visualizer color={s}") } else { s.to_string() };
            styles.push_str(&format!(",{}/.style={{{s}}}", i + 1));
        }
        out.push_str(&format!("\\pgfdvdeclarestylesheet{{{name}}}{{{styles}}}\n"));
    }
    let thickness = ["thin", "thick"];
    let mut both = String::from("default style/.style=thin");
    for i in 0..14 {
        let dash = match i / 2 {
            0 => String::new(),
            k => format!("{},", SHEETS[1].2[k].trim_start_matches("solid")),
        };
        let dash = if dash == "," { String::new() } else { dash };
        both.push_str(&format!(",{}/.style={{{dash}{}}}", i + 1, thickness[i % 2]));
    }
    out.push_str(&format!("\\pgfdvdeclarestylesheet{{vary thickness and dashing}}{{{both}}}\n"));
    let mut cross = String::from("default style/.style={mark=x,every mark/.append style=thin,mark size=2pt}");
    for (i, m) in CROSS_MARKS.iter().enumerate() {
        cross.push_str(&format!(",{}/.style={{mark={m},every mark/.append style=thin,mark size=2pt}}", i + 1));
    }
    cross.push_str(",6/.style={mark=10-pointed star,every mark/.append style={line width=.3pt,mark size=1.8pt}}");
    out.push_str(&format!("\\pgfdvdeclarestylesheet{{cross marks}}{{{cross}}}\n"));
    for (name, model, start, step) in SERIES {
        out.push_str(&format!("\\tikzdvdeclarestylesheetcolorseries{{{name}}}{{{model}}}{{{start}}}{{{step}}}\n"));
    }
    // legends: arrangement, placement, styling
    let mut legend = String::from("legend options/.cd,");
    for (key, sub, value) in [("ideal number of columns", "@columns or rows", "columns"), ("ideal number of rows", "@columns or rows", "rows")] {
        legend.push_str(&format!("{key}/.style={{\\tikra@dv@legend/{sub}={value},\\tikra@dv@legend/@ideal={{#1}}}},"));
    }
    legend.push_str("columns/.style={ideal number of columns={#1}},rows/.style={ideal number of rows={#1}},");
    legend.push_str("max rows/.style={\\tikra@dv@legend/@columns or rows=columns,\\tikra@dv@legend/max entries={#1}},");
    legend.push_str("max columns/.style={\\tikra@dv@legend/@columns or rows=rows,\\tikra@dv@legend/max entries={#1}},");
    for first in ["down", "up", "right", "left"] {
        let thens: &[&str] = if first == "down" || first == "up" { &["right", "left"] } else { &["down", "up"] };
        for then in thens {
            legend.push_str(&format!("{first} then {then}/.style={{\\tikra@dv@legend/@strategy={first} then {then}}},"));
        }
    }
    legend.push_str("anchor/.style={\\tikra@dv@legend/anchor={#1}},at/.style={\\tikra@dv@legend/at={#1}},");
    legend.push_str("right/.style={east outside},left/.style={west outside},above/.style={north outside},below/.style={south outside},");
    for (name, count, at, anchor, more) in OUTSIDE {
        legend.push_str(&format!("{name}/.style={{{more}{count}=1,at={{({at})}},anchor={anchor}}},"));
    }
    for (name, count, at, anchor) in INSIDE {
        legend.push_str(&format!("{name}/.style={{{count}=1,at={{({at})}},anchor={anchor},every legend inside}},"));
    }
    for (name, anchor) in AT_VALUES {
        legend.push_str(&format!("{name}/.style={{at={{(visualization cs:#1)}},anchor={anchor},every legend inside}},"));
    }
    legend.push_str("every new legend/.style={east outside,label style=text right},");
    legend.push_str("every legend inside/.style={opaque=white,label style={node style={font=\\footnotesize}}},");
    legend.push_str(
        "opaque/.style={matrix node style={inner sep=.333ex,outer sep=.5ex,rounded corners=3pt,execute at begin cell={\\tikzset{sharp corners}},fill=#1}},opaque/.default=white,",
    );
    legend.push_str("transparent/.style={matrix node style={fill=none}},");
    legend.push_str("label style/.style={matrix node style={/tikz/data visualization/every label in legend/.append style={#1}}},");
    legend.push_str("matrix node style/.style={\\tikra@dv@legend/matrix node styling/.append style={#1}}");
    out.push_str(&format!("\\tikzdatavisualizationset{{{legend}}}\n"));
    // legend entry samples
    let mut samples = String::from("legend entry options/.cd,");
    for (name, line, marks) in SAMPLES {
        samples.push_str(&format!("{name}/.style={{label in legend line coordinates={{{line}}},label in legend mark coordinates={{{marks}}}}},"));
    }
    let circle = ellipse_points(&[0, 45, 90, 135, 180, 225, 270, 315], "xshift=-1em", ".9em and .35ex");
    let circle_marks = ellipse_points(&[120, -60], "xshift=-1em", ".9em and .35ex");
    samples.push_str(&format!("circular label in legend line/.style={{label in legend line coordinates={{{circle}}},label in legend mark coordinates={{{circle_marks}}}}},"));
    let gap = ellipse_points(&[90, 162, 234, 306, 18], "xshift=-1em,yshift=.2ex", "1.4ex and 0.9ex");
    samples.push_str(&format!("gap circular label in legend line/.style={{label in legend line coordinates={{{gap}}},label in legend mark coordinates={{{gap}}}}}"));
    out.push_str(&format!("\\tikzdatavisualizationset{{{samples}}}\n"));
    // the basic axis systems
    for (name, names, z) in [("xy Cartesian", ["x", "y"], ""), ("uv Cartesian", ["u", "v"], "")] {
        out.push_str(&format!(
            "\\tikzdatavisualizationset{{{name}/.style={{new Cartesian axis={0} axis,{0} axis={{attribute={0},unit vector={{(1pt,0cm)}}}},\
             new Cartesian axis={1} axis,{1} axis={{attribute={1},unit vector={{(0cm,1pt)}}}}{z}}}}}\n",
            names[0], names[1]
        ));
    }
    for (name, base, third) in [("xyz Cartesian cabinet", "xy Cartesian", "z"), ("uvw Cartesian cabinet", "uv Cartesian", "w")] {
        out.push_str(&format!(
            "\\tikzdatavisualizationset{{{name}/.style={{{base},new Cartesian axis={third} axis,{third} axis={{attribute={third},unit vector={{(-0.353553pt,-0.353553pt)}}}}}}}}\n"
        ));
    }
    out.push_str(&school_book());
    out.push_str(&scientific());
    out.push_str(&polar());
    out
}
