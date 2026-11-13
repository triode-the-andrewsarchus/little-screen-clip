//! The keys of `/pgfplots`. The ones that only hold a value or a style are described by the tables
//! below, with their defaults under `compat=default`, and turned into TeX once; the axis reads
//! their values when it is drawn. The keys whose code does work
//! (`compat`, `axis x line`, `/pgf/number format/…`, the plot handlers) are native.

use crate::keys;
use crate::tex::{self, Meaning, Token};
use crate::Error;

use super::super::{options::KeyFn, tikra_tokens, Engine, NATIVE_KEY_BASE};
use super::number::{SciStyle, Style};

/// Keys that hold a value, and the value each starts with.
const VALUES: &[(&str, &str)] = &[
    ("3d box", "background"),
    ("/pgf/bar interval shift", "0.5"),
    ("/pgf/bar interval width", "1"),
    ("/pgf/bar shift", "0pt"),
    ("/pgf/bar width", "10pt"),
    ("/pgf/decoration/soft clip path", ""),
    ("/tikz/@plot handler", ""),
    ("/tikz/column sep", "0pt"),
    ("/tikz/mark indices", ""),
    ("/tikz/mark phase", "1"),
    ("/tikz/mark repeat", "1"),
    ("/tikz/mark size", "2pt"),
    ("/tikz/row sep", "0pt"),
    ("@axis type", "cart"),
    ("@boxplot", ""),
    ("@colorbar axis", "false"),
    ("@colorbar line", "false"),
    ("@colorbar orientation", "v"),
    ("@colorbar sampled", ""),
    ("@compat", "0"),
    ("@contour format", "standard"),
    ("@date coordinates in", ""),
    ("@disabledatascaling", "false"),
    ("@empty {a}ticklabels", "false"),
    ("@hist", "false"),
    ("@legend image", "line"),
    ("@log fixed point", "false"),
    ("@mesh", ""),
    ("@nodes near coords", ""),
    ("@patch", "false"),
    ("@quiver", "false"),
    ("@restrict expr", ""),
    ("@restrict expr domain", ""),
    ("@substitute pgfpoint", "false"),
    ("@ticklabel strut", "false"),
    ("@visualization depends on", ""),
    ("@{a} line", "box"),
    ("@{a} scale label off", "false"),
    ("anchor", "south west"),
    ("at", ""),
    ("axis x discontinuity", "none"),
    ("axis y discontinuity", "none"),
    ("axis {a} line shift", ""),
    ("bar direction", "auto"),
    ("boxplot/average", ""),
    ("boxplot/box extend", "0.8"),
    ("boxplot/data", "y"),
    ("boxplot/data value", ""),
    ("boxplot/draw direction", "x"),
    ("boxplot/draw position", r"1+\plotnumofactualtype"),
    ("boxplot/draw relative anchor", "0.5"),
    ("boxplot/estimator", "legacy"),
    ("boxplot/lower quartile", "auto"),
    ("boxplot/lower quartile off", "0.25"),
    ("boxplot/lower whisker", "auto"),
    ("boxplot/median", "auto"),
    ("boxplot/sample size", "auto"),
    ("boxplot/sample size max", ""),
    ("boxplot/sample size min", ""),
    ("boxplot/upper quartile", "auto"),
    ("boxplot/upper quartile off", "0.75"),
    ("boxplot/upper whisker", "auto"),
    ("boxplot/variable width min target", "0.2"),
    ("boxplot/whisker extend", r"\pgfkeysvalueof{/pgfplots/boxplot/box extend}*0.8"),
    ("boxplot/whisker range", "1.5"),
    ("clip mode", "global"),
    ("colorbar pos*", ""),
    ("colorbar to name", ""),
    ("colorbar/width", "0.5cm"),
    ("colormap access", "map"),
    ("colormap name", "hot"),
    ("compat", "default"),
    ("contour/draw color", "mapped color"),
    ("contour/fill color", "mapped color"),
    ("contour/label distance", "70pt"),
    ("contour/levels", ""),
    ("contour/levels from colormap", ""),
    ("contour/number", "5"),
    ("cycle list name", "color"),
    ("cycle list shift", ""),
    ("data cs", "cart"),
    ("date ZERO", ""),
    ("domain", ""),
    ("error bars/@x mode", "fixed"),
    ("error bars/@x value", "0"),
    ("error bars/@y mode", "fixed"),
    ("error bars/@y value", "0"),
    ("error bars/error mark", "-"),
    ("error bars/error mark options", "rotate=90"),
    ("error bars/x dir", "none"),
    ("error bars/y dir", "none"),
    ("every axis title shift", "6pt"),
    ("execute at begin axis", ""),
    ("execute at begin plot", ""),
    ("execute at begin plot visualization", ""),
    ("execute at end axis", ""),
    ("execute at end plot", ""),
    ("execute at end plot visualization", ""),
    ("extra x tick label", ""),
    ("extra x tick labels", ""),
    ("extra y tick label", ""),
    ("extra y tick labels", ""),
    ("extra {a} ticks", ""),
    ("faceted color", "mapped color!80!black"),
    ("filter discard warning", "true"),
    ("group/columns", "1"),
    ("group/group name", "group"),
    ("group/horizontal sep", "1cm"),
    ("group/rows", "1"),
    ("group/vertical sep", "1cm"),
    ("group/xlabels at", "all"),
    ("group/xticklabels at", "all"),
    ("group/ylabels at", "all"),
    ("group/yticklabels at", "all"),
    ("height", ""),
    ("hist/@symbolic coords", ""),
    ("hist/bins", "10"),
    ("hist/data", "y"),
    ("hist/data max", ""),
    ("hist/data min", ""),
    ("hist/data value", ""),
    ("late options", ""),
    ("legend columns", "1"),
    ("legend entries", ""),
    ("legend plot pos", "left"),
    ("legend to name", ""),
    ("log basis {a}", ""),
    ("major tick length", "0.15cm"),
    ("mark layer", ""),
    ("mark list fill", ".!80!black"),
    ("max space between ticks", "35"),
    ("mesh input", "lattice"),
    ("mesh/check", "error"),
    ("mesh/color input", "colormap"),
    ("mesh/cols", ""),
    ("mesh/interior colormap name", ""),
    ("mesh/interior colormap thresh", "0"),
    ("mesh/ordering", "x varies"),
    ("mesh/rows", ""),
    ("minor tick length", "0.1cm"),
    ("minor {a} tick num", "0"),
    ("minor {a}tick", ""),
    ("name", ""),
    ("near ticklabel align", "center"),
    ("nodes near coords align", "auto"),
    ("nodes near coords bar offset", "0.5"),
    ("on layer", ""),
    ("parametric/var 1d", "t"),
    ("parent axis height", "6cm"),
    ("parent axis width", "6cm"),
    ("patch refines", "0"),
    ("patch table", ""),
    ("patch table with individual point meta", ""),
    ("patch table with point meta", ""),
    ("patch type", ""),
    ("plot box ratio", "1 1 1"),
    ("plot graphics/includegraphics", ""),
    ("plot graphics/points", ""),
    ("plot graphics/snap z", "0.02"),
    ("plot graphics/squeeze tol", "2pt"),
    ("plot graphics/src", ""),
    ("plot graphics/{a}max", ""),
    ("plot graphics/{a}min", ""),
    ("point meta", ""),
    ("point meta max", ""),
    ("point meta min", ""),
    ("point meta rel", "axis wide"),
    ("quiver/arrow color", ""),
    ("quiver/scale arrows", "1"),
    ("quiver/u", ""),
    ("quiver/v", ""),
    ("quiver/w", ""),
    ("restrict x to domain", ""),
    ("restrict x to domain*", ""),
    ("restrict y to domain", ""),
    ("restrict y to domain*", ""),
    ("samples", ""),
    ("samples at", ""),
    ("samples y", ""),
    ("scale mode", "auto"),
    ("scale ticks above exponent", "3"),
    ("scale ticks below exponent", "-1"),
    ("scale uniformly strategy", "auto"),
    ("scaled {a} ticks", "true"),
    ("scatter", "false"),
    ("scatter/classes", ""),
    ("scatter/use mapped color", "draw=mapped color!80!black,fill=mapped color"),
    ("shader", "faceted"),
    ("stack dir", "plus"),
    ("stack negative", "on previous"),
    ("stack plots", "false"),
    ("symbolic {a} coords", ""),
    ("table/@search path", "."),
    ("table/col sep", "space"),
    ("table/columns", ""),
    ("table/comment chars", ""),
    ("table/create col/linear regression/variance format", "log"),
    ("table/format", "auto"),
    ("table/header", "true"),
    ("table/ignore chars", ""),
    ("table/meta", ""),
    ("table/meta expr", ""),
    ("table/meta index", ""),
    ("table/row sep", "newline"),
    ("table/skip first n", "0"),
    ("table/tie end {a}", ""),
    ("table/tie end {a} index", ""),
    ("table/trim cells", "false"),
    ("table/x error", ""),
    ("table/x error expr", ""),
    ("table/x error index", ""),
    ("table/x error minus", ""),
    ("table/x error minus expr", ""),
    ("table/x error plus", ""),
    ("table/x error plus expr", ""),
    ("table/y error", ""),
    ("table/y error expr", ""),
    ("table/y error index", ""),
    ("table/y error minus", ""),
    ("table/y error minus expr", ""),
    ("table/y error minus index", ""),
    ("table/y error plus", ""),
    ("table/y error plus expr", ""),
    ("table/y error plus index", ""),
    ("table/{a}", ""),
    ("table/{a} expr", ""),
    ("table/{a} index", ""),
    ("ternary start {a}", ""),
    ("tick scale binop", r"\cdot"),
    ("tieline/each nth tie", ""),
    ("title", ""),
    ("trig format plots", "default"),
    ("try min ticks", "4"),
    ("try min ticks log", "3"),
    ("unbounded coords", "discard"),
    ("unit vector ratio", ""),
    ("unit vector ratio axis", ""),
    ("variable y", "y"),
    ("vertex count", ""),
    ("view/az", "25"),
    ("view/el", "30"),
    ("width", ""),
    ("xgrid each nth passes y", ""),
    ("xgrid each nth passes y start", "0"),
    ("xgrid stop at y", ""),
    ("xticklabel shift", ""),
    ("xtickmax", ""),
    ("xtickmin", ""),
    ("y domain", ""),
    ("ygrid each nth passes x", ""),
    ("ygrid each nth passes x start", "0"),
    ("ygrid stop at x", ""),
    ("yticklabel shift", ""),
    ("ytickmax", ""),
    ("ytickmin", ""),
    ("z buffer", "default"),
    ("{a}", ""),
    ("{a} dir", "normal"),
    ("{a} post scale", ""),
    ("{a} unit", ""),
    ("{a} unit prefix", ""),
    ("{a}label", ""),
    ("{a}label shift", "0pt"),
    ("{a}max", ""),
    ("{a}min", ""),
    ("{a}mode", "linear"),
    ("{a}tick", ""),
    ("{a}tick align", "inside"),
    ("{a}tick distance", ""),
    ("{a}tick pos", "both"),
    ("{a}ticklabel", ""),
    ("{a}ticklabel pos", "default"),
    ("{a}ticklabels", ""),
    ("{a}tickten", ""),
];

/// Keys that are on or off: how each starts; given alone, a switch turns on.
const SWITCHES: &[(&str, &str)] = &[
    ("axis on top", "false"),
    ("boxplot/ensure mark", "false"),
    ("boxplot/variable width", "false"),
    ("change {a} base", "false"),
    ("clip", "true"),
    ("clip marker paths", "false"),
    ("colorbar", "false"),
    ("contour/filled", "false"),
    ("contour/labels", "true"),
    ("enlarge {a} limits", "auto"),
    ("forget plot", "false"),
    ("group/shared labels", "false"),
    ("hide {a} axis", "false"),
    ("hist/cumulative", "false"),
    ("hist/density", "false"),
    ("hist/intervals", "true"),
    ("is smithchart cs", "false"),
    ("log identify minor tick positions", "false"),
    ("patch to triangles", "false"),
    ("patch type sampling", "false"),
    ("plot graphics/auto adjust axis", "true"),
    ("plot graphics/debug", ""),
    ("quiver/update limits", "true"),
    ("reverse legend", "false"),
    ("reverse stacked plots", "true"),
    ("scale only axis", "false"),
    ("separate axis lines", "false"),
    ("show origin", "false"),
    ("smithchart mirrored", "false"),
    ("stacked ignores zero", "false"),
    ("ternary limits relative", "true"),
    ("transpose legend", "false"),
    ("unit rescale keep size", "unless limits declared"),
    ("use units", "true"),
    ("x tick label as interval", "false"),
    ("y tick label as interval", "false"),
    ("{a}majorgrids", "false"),
    ("{a}majorticks", "true"),
    ("{a}minorgrids", "false"),
    ("{a}minorticks", "true"),
];

/// Styles that start empty, for documents to fill.
const HOOKS: &[&str] = &[
    "/tikz/every cell",
    "/tikz/every mark",
    "/tikz/fill between/every even segment",
    "/tikz/fill between/every last segment",
    "/tikz/fill between/every odd segment",
    "/tikz/fill between/every segment",
    "axis background",
    "boxplot/every box",
    "boxplot/every median",
    "boxplot/every whisker",
    "contour/every contour plot",
    "default smithchart xtick",
    "default smithchart xytick",
    "default smithchart ytick",
    "every 3d box foreground",
    "every axis",
    "every axis label",
    "every axis plot",
    "every axis plot except legend",
    "every axis plot no 0",
    "every axis plot post",
    "every axis post",
    "every axis {a} grid",
    "every boxed {a} axis",
    "every boxplot",
    "every forget plot",
    "every inner {a} axis line",
    "every legend image post",
    "every linear axis",
    "every loglog axis",
    "every major grid",
    "every major tick",
    "every major {a} grid",
    "every major {a} tick",
    "every mesh",
    "every minor grid",
    "every minor tick",
    "every minor {a} grid",
    "every minor {a} tick",
    "every node near coord",
    "every outer {a} axis line",
    "every semilogx axis",
    "every semilogy axis",
    "every tick label",
    "every {a} tick",
    "every {a} tick label",
    "group/every plot",
    "quiver/every arrow",
    "quiver/quiver legend",
    "tieline/every curve",
    "tieline/every tieline",
];

/// Keys that add their options to a style (`legend style` to `every axis legend`).
const ADDS_TO: &[(&str, &str)] = &[
    ("/tikz/cells", "/tikz/every cell"),
    ("/tikz/nodes", "/tikz/every node"),
    ("3d box foreground style", "every 3d box foreground"),
    ("axis background style", "axis background"),
    ("axis plot except legend style", "every axis plot except legend"),
    ("colorbar sampled line style", "every colorbar sampled line"),
    ("colorbar style", "every colorbar"),
    ("contour/contour label style", "contour/every contour label"),
    ("error bars/error bar style", "every error bar"),
    ("extra x tick style", "every extra x tick"),
    ("extra y tick style", "every extra y tick"),
    ("forget plot style", "every forget plot"),
    ("grid style", "every axis grid"),
    ("legend image post style", "every legend image post"),
    ("legend style", "every axis legend"),
    ("major grid style", "every major grid"),
    ("major tick style", "every major tick"),
    ("major x grid style", "every major x grid"),
    ("major x tick style", "every major x tick"),
    ("major y grid style", "every major y grid"),
    ("major y tick style", "every major y tick"),
    ("minor grid style", "every minor grid"),
    ("minor tick style", "every minor tick"),
    ("minor x grid style", "every minor x grid"),
    ("minor x tick style", "every minor x tick"),
    ("minor y grid style", "every minor y grid"),
    ("minor y tick style", "every minor y tick"),
    ("node near coord style", "every node near coord"),
    ("node near coords style", "every node near coord"),
    ("nodes near coords style", "every node near coord"),
    ("polar labels style", "polar labels"),
    ("tick label style", "every tick label"),
    ("tick style", "every tick"),
    ("tieline/curve style", "tieline/every curve"),
    ("tieline/tieline style", "tieline/every tieline"),
    ("title style", "every axis title"),
    ("x grid style", "every axis x grid"),
    ("x tick scale label style", "every x tick scale label"),
    ("x tick style", "every x tick"),
    ("y grid style", "every axis y grid"),
    ("y tick scale label style", "every y tick scale label"),
    ("y tick style", "every y tick"),
    ("{a} label style", "every axis {a} label"),
    ("{a} tick label style", "every {a} tick label"),
];

/// Keys that tikra accepts and leaves without effect.
const IGNORED: &[&str] = &[
    "/tikz/pos segment",
    "after end axis",
    "alias",
    "before end axis",
    "boxplot/data filter",
    "cell picture",
    "colorbar source",
    "extra description",
    "filter point",
    "hist/data filter",
    "lua backend",
    "lua debug",
    "pre filter",
    "quiver/after arrow",
    "quiver/before arrow",
    "quiver/u filter",
    "quiver/v filter",
    "quiver/w filter",
    "scatter/@post marker code",
    "set layers",
    "{a} filter",
];

/// Keys that store a fixed value (`#1`: their own) where the drawing reads it.
const SETS: &[(&str, &str, &str)] = &[
    ("area legend", "@legend image", "area"),
    ("colorbar horizontal/lowlevel", "@colorbar orientation", "h"),
    ("colorbar vertical/lowlevel", "@colorbar orientation", "v"),
    ("contour prepared format", "@contour format", "#1"),
    ("disabledatascaling", "@disabledatascaling", "true"),
    ("empty legend", "@legend image", "empty"),
    ("error bars/x explicit", "error bars/@x mode", "explicit"),
    ("error bars/x explicit relative", "error bars/@x mode", "explicit relative"),
    ("error bars/y explicit", "error bars/@y mode", "explicit"),
    ("error bars/y explicit relative", "error bars/@y mode", "explicit relative"),
    ("legend image code", "@legend image", "line"),
    ("legend image with text", "@legend image", "empty"),
    ("line legend", "@legend image", "line"),
    ("log ticks with fixed point", "@log fixed point", "true"),
    ("mesh legend", "@legend image", "line"),
    ("polar comb", "/tikz/@plot handler", "polar comb"),
    ("quiver/u value", "quiver/u", "#1"),
    ("quiver/v value", "quiver/v", "#1"),
    ("quiver/w value", "quiver/w", "#1"),
    ("single xbar legend", "@legend image", "single xbar"),
    ("single ybar legend", "@legend image", "single ybar"),
    ("table/search path", "table/@search path", "#1"),
    ("xbar interval legend", "@legend image", "xbar interval"),
    ("xbar legend", "@legend image", "xbar"),
    ("ybar interval legend", "@legend image", "ybar interval"),
    ("ybar legend", "@legend image", "ybar"),
];

/// Keys that pass their value to several keys.
const SPREADS: &[(&str, &[&str])] = &[
    ("axis line shift", &["axis x line shift", "axis y line shift", "axis z line shift"]),
    ("axis line style", &["inner axis line style", "outer axis line style"]),
    ("axis lines", &["axis x line", "axis y line", "axis z line"]),
    ("axis lines*", &["axis x line*", "axis y line*", "axis z line*"]),
    ("enlargelimits", &["enlarge x limits", "enlarge y limits", "enlarge z limits"]),
    ("hide axis", &["hide x axis", "hide y axis", "hide z axis"]),
    ("hist/symbolic coords", &["hist/@symbolic coords", "symbolic x coords"]),
    ("inner axis line style", &["every inner x axis line/.append style", "every inner y axis line/.append style", "every inner z axis line/.append style"]),
    ("label shift", &["xlabel shift", "ylabel shift", "zlabel shift"]),
    ("label style", &["every axis x label/.append style", "every axis y label/.append style", "every axis z label/.append style"]),
    ("max", &["xmax", "ymax", "zmax"]),
    ("min", &["xmin", "ymin", "zmin"]),
    ("minor tick", &["minor xtick", "minor ytick", "minor ztick"]),
    ("minor tick num", &["minor x tick num", "minor y tick num", "minor z tick num"]),
    ("outer axis line style", &["every outer x axis line/.append style", "every outer y axis line/.append style", "every outer z axis line/.append style"]),
    ("scale", &["x post scale", "y post scale", "z post scale"]),
    ("scaled ticks", &["scaled x ticks", "scaled y ticks", "scaled z ticks"]),
    ("tick align", &["xtick align", "ytick align", "ztick align"]),
    ("ticklabel pos", &["xticklabel pos", "yticklabel pos", "zticklabel pos"]),
    ("ticklabel shift", &["xticklabel shift", "yticklabel shift", "zticklabel shift"]),
    ("tickpos", &["xtick pos", "ytick pos", "ztick pos"]),
    ("x axis line style", &["every outer x axis line/.append style", "every inner x axis line/.append style"]),
    ("y axis line style", &["every outer y axis line/.append style", "every inner y axis line/.append style"]),
    ("z axis line style", &["every outer z axis line/.append style", "every inner z axis line/.append style"]),
];

/// Other names of a key, or keys that hand their value to another key (`bar width` to
/// `/pgf/bar width`).
const ALIASES: &[(&str, &str)] = &[
    ("/tikz/bar interval shift", "/pgf/bar interval shift"),
    ("/tikz/bar interval width", "/pgf/bar interval width"),
    ("/tikz/bar shift", "/pgf/bar shift"),
    ("/tikz/bar width", "/pgf/bar width"),
    ("/tikz/legend cell align", "legend cell align"),
    ("/tikz/legend columns", "legend columns"),
    ("/tikz/legend plot pos", "legend plot pos"),
    ("bar interval shift", "/pgf/bar interval shift"),
    ("bar interval width", "/pgf/bar interval width"),
    ("bar shift", "/pgf/bar shift"),
    ("bar width", "/pgf/bar width"),
    ("domain y", "y domain"),
    ("legend reversed", "reverse legend"),
    ("legend transposed", "transpose legend"),
    ("log base {b}", "log basis {b}"),
    ("scatter src", "point meta"),
    ("ternary relative limits", "ternary limits relative"),
    ("tick pos", "tickpos"),
    ("ticklabel style", "tick label style"),
    ("ticklabelpos", "ticklabel pos"),
    ("subtickwidth", "minor tick length"),
    ("tickwidth", "major tick length"),
    ("variable", "/tikz/variable"),
    ("view/h", "view/az"),
    ("view/v", "view/el"),
    ("{a}label style", "{a} label style"),
    ("{a}ticklabel style", "{a} tick label style"),
];

/// The value a key takes when it is given alone (switches aside).
const DEFAULTS: &[(&str, &str)] = &[
    ("3d box", "complete"),
    ("axis equal", "true"),
    ("axis equal image", "true"),
    ("bar shift auto", "2pt"),
    ("colorbar sampled", "surf,mark=none,shader=flat"),
    ("colorbar sampled line", "scatter,only marks"),
    ("enlargelimits", "true"),
    ("grid", "major"),
    ("hide axis", "true"),
    ("legend reversed", "true"),
    ("legend transposed", "true"),
    ("nodes near coords", r"\pgfmathprintnumber\pgfplotspointmeta"),
    ("nodes near coords*", r"\pgfmathprintnumber\pgfplotspointmeta"),
    ("quiver/colored", "mapped color"),
    ("{b}bar", "2pt"),
    ("{b}bar interval", "1"),
    ("{b}bar interval stacked", "plus"),
    ("{b}bar stacked", "plus"),
];

/// Keys whose value is one of a few names, each its own key below them.
const CHOICES: &[&str] = &["axis equal", "axis equal image", "grid", "legend cell align", "legend pos", "ticks"];

/// Styles: what each key turns on. Unqualified keys in them are pgfplots keys.
const STYLES: &[(&str, &str)] = &[
    ("/pgf/decoration/soft clip", "/pgf/decoration/name=soft clip"),
    ("/tikz/mark options", "/tikz/every mark/.style={#1}"),
    ("/tikz/trim axis left", "/pgfplots/trim axis left"),
    ("/tikz/trim axis right", "/pgfplots/trim axis right"),
    ("area cycle list", "bar cycle list"),
    ("area style", "area cycle list, area legend, axis on top"),
    ("axis equal image/false", "/pgfplots/unit vector ratio="),
    ("axis equal image/true", "/pgfplots/unit vector ratio=1 1 1, /pgfplots/unit rescale keep size=false"),
    ("axis equal/false", "/pgfplots/unit vector ratio="),
    ("axis equal/true", "/pgfplots/unit vector ratio=1 1 1, /pgfplots/unit rescale keep size=true"),
    ("bar shift auto", r"/pgf/bar shift={-0.5*(\numplotsofactualtype*\pgfplotbarwidth + (\numplotsofactualtype-1)*(#1)) + (.5+\plotnumofactualtype)*\pgfplotbarwidth + \plotnumofactualtype*(#1)}"),
    ("boxplot/every average", "/tikz/mark=diamond*"),
    ("colorbar as legend", "/pgfplots/colorbar sampled={surf, shader=flat corner}"),
    ("colorbar as palette", "/pgfplots/colorbar sampled={surf, shader=flat corner}"),
    ("contour prepared filled", "/pgfplots/contour prepared={draw color=mapped color!80!black, labels=false, filled}"),
    ("contour/every contour label", r"inner sep=2pt, font=\scriptsize, every node/.style={mapped color!50!black, fill=white}, /pgf/number format/relative*={\pgfplotspointmetarangeexponent}"),
    ("contour/every filled contour plot", "/pgfplots/area legend"),
    ("contour/labels over line", "/pgfplots/contour/every contour label/.append style={every node/.append style={fill=none, anchor=base, yshift=1pt}}"),
    ("date coordinates in", r"/pgfplots/@date coordinates in={#1}, /pgfplots/#1ticklabel={\tick}, /pgfplots/scaled #1 ticks=false"),
    ("every 3d description", "/pgfplots/every axis x label/.style={at={(ticklabel cs:0.5)}, anchor=near ticklabel}, /pgfplots/every axis y label/.style={at={(ticklabel cs:0.5)}, anchor=near ticklabel}, /pgfplots/every x tick scale label/.style={at={(xticklabel cs:0.95,5pt)}, anchor=near xticklabel, inner sep=0pt}, /pgfplots/every y tick scale label/.style={at={(yticklabel cs:0.95,5pt)}, anchor=near yticklabel, inner sep=0pt}, /pgfplots/try min ticks=3"),
    ("every 3d view {0}{90}", "/pgfplots/xlabel near ticks, /pgfplots/ylabel near ticks, /pgfplots/axis on top=true"),
    ("every axis grid", "thin, black!25"),
    ("every axis legend", "cells={anchor=center}, inner xsep=3pt, inner ysep=2pt, nodes={inner sep=2pt, text depth=0.15em}, anchor=north east, shape=rectangle, fill=white, draw=black, at={(0.98,0.98)}"),
    ("every axis title", r"at={(0.5,1)}, above, yshift=\pgfkeysvalueof{/pgfplots/every axis title shift}"),
    ("every axis z label", "at={(ticklabel cs:0.5)}, rotate=90, anchor=near ticklabel"),
    ("every colorbar global", "axis on top, title=, xlabel=, ylabel=, zlabel=, legend entries=, grid=none, enlargelimits=false, scale only axis, xtick=, xticklabel=, ytick=, yticklabel=, zmin=, zmax="),
    ("every colorbar sampled line", "help lines"),
    ("every error bar", "thin"),
    ("every extra {a} tick", "/pgfplots/log identify minor tick positions=true"),
    ("every fill between plot", "/pgfplots/area legend, /tikz/fill"),
    ("every non boxed {a} axis", "{a}tick align=center, enlarge {a} limits=false, {a} axis line style={-stealth}"),
    ("every patch", "miter limit=1"),
    ("every polar axis", r"grid=major, xtick pos=right, ytick pos=right, every axis title shift=1.2\baselineskip, legend style={anchor=center}, /pgfplots/polar labels"),
    ("every tick", "very thin, gray"),
    ("every z tick scale label", "at={(zticklabel* cs:1.2,-0.3em)}, anchor=near zticklabel, inner sep=0pt"),
    ("extra tick style", "every extra x tick/.append style={#1}, every extra y tick/.append style={#1}, every extra z tick/.append style={#1}"),
    ("footnotesize", r"width=5cm, height=, legend style={font=\footnotesize}, tick label style={font=\footnotesize}, label style={font=\small}, title style={font=\small}, every axis title shift=0pt, max space between ticks=15, every mark/.append style={mark size=8}, major tick length=0.1cm, minor tick length=0.066cm"),
    ("group/empty plot", "/pgfplots/hide axis=true"),
    ("hist/handler", "/tikz/ybar interval"),
    ("matrix plot", "/pgfplots/late options={/pgfplots/y dir=reverse, /pgfplots/axis on top}, /pgfplots/matrix plot*"),
    ("matrix plot*", "/pgfplots/surf, /pgfplots/mesh input=image, /pgfplots/shader=flat corner, /pgfplots/z buffer=none"),
    ("no marks", "/pgfplots/no markers"),
    ("no markers", "/pgfplots/every axis plot post/.append style={mark=none}"),
    ("normalsize", "/pgfplots/width=240pt, /pgfplots/height=207pt, /pgfplots/max space between ticks=35"),
    ("only marks scatter", "/pgfplots/scatter, /tikz/only marks"),
    ("polar labels", "every axis x label/.style={at={(rel axis cs:0,0.5)}, anchor=north}, every axis y label/.style={at={(0.2,0.85)}, anchor=center}"),
    ("small", r"width=6.5cm, height=, tick label style={font=\footnotesize}, label style={font=\small}, max space between ticks=25"),
    ("tiny", r"width=4cm, height=, legend style={font=\tiny}, tick label style={font=\tiny}, label style={font=\tiny}, title style={font=\footnotesize}, every axis title shift=0pt, max space between ticks=12, every mark/.append style={mark size=6}, major tick length=0.1cm, minor tick length=0.066cm, every legend image post/.append style={scale=0.8}"),
    ("trim axis left", "/tikz/trim left={(current axis.south west)}"),
    ("trim axis right", "/tikz/trim right={(current axis.south east)}"),
    ("typeset ticklabels with strut", "/pgfplots/@ticklabel strut=true"),
    ("unit vector ratio*", "/pgfplots/unit vector ratio={#1}, /pgfplots/unit rescale keep size=false"),
    ("xlabel absolute", "/pgfplots/every axis x label/.style={at={(0.5,0)}, below, yshift=-15pt}, /pgfplots/every x tick scale label/.style={at={(1,0)}, yshift=-2em, left, inner sep=0pt}"),
    ("xlabel near ticks", "/pgfplots/every axis x label/.style={at={(ticklabel cs:0.5)}, anchor=near ticklabel}, /pgfplots/every x tick scale label/.style={at={(xticklabel cs:0.9,5pt)}, anchor=near xticklabel, inner sep=0pt}"),
    ("xticklabel in circle", "xtick align=inside, xticklabel style={rotate=90, anchor=south west}"),
    ("ylabel absolute", "/pgfplots/every axis y label/.style={at={(0,0.5)}, xshift=-35pt, rotate=90}, /pgfplots/every y tick scale label/.style={at={(0,1)}, above right, inner sep=0pt, yshift=0.3em}"),
    ("ylabel near ticks", "/pgfplots/every axis y label/.style={at={(ticklabel cs:0.5)}, rotate=90, anchor=near ticklabel}, /pgfplots/every y tick scale label/.style={at={(yticklabel* cs:1.03,-0.3em)}, /pgfplots/near ticklabel align=outside, anchor=near yticklabel opposite, inner sep=0pt}"),
    ("xbar", "bar direction=x, bar cycle list, ytick align=outside, /pgfplots/xbar legend, /pgfplots/set point meta if empty=x, /pgfplots/bar shift auto={#1}, /tikz/xbar"),
    ("ybar", "bar direction=y, bar cycle list, xtick align=outside, ybar legend, /pgfplots/bar shift auto={#1}, /tikz/ybar"),
    ("{b}bar interval", r"bar direction={b}, bar cycle list, {o} tick label as interval, {o}majorgrids, {o}tick align=outside, {o}tick=data, {b}bar interval legend, bar interval width={#1/\numplotsofactualtype}, bar interval shift={(\plotnumofactualtype+0.5)/\numplotsofactualtype}, /tikz/{b}bar interval"),
    ("{b}bar interval stacked", "bar direction={b}, bar cycle list, single {b}bar legend, stack plots={b}, stack dir=#1, /tikz/{b}bar interval"),
    ("{b}bar stacked", "bar direction={b}, bar cycle list, single {b}bar legend, stack plots={b}, stack dir=#1, /tikz/{b}bar"),
    ("{b}ticklabel interval boundaries", r"{b} tick label as interval, {b}ticklabel={$\pgfmathprintnumber{\tick}$ -- $\pgfmathprintnumber{\nexttick}$}"),
];

/// The ternary, smith chart and colorbar styles, longer than a line.
const LONG_STYLES: &[(&str, &str)] = &[
    (
        "every ternary axis",
        r"grid=major, /pgfplots/tick align=outside, xticklabel style={anchor=west},
        /pgfplots/every 3d description/.style={}, every axis title shift=15pt,
        every axis x label/.style={anchor=near ticklabel, at={(ticklabel cs:0.5)}},
        every axis y label/.style={anchor=near ticklabel, at={(ticklabel cs:0.5)}},
        every axis z label/.style={anchor=near ticklabel, at={(ticklabel cs:0.5)}},
        every x tick scale label/.style={inner sep=0pt, anchor=near xticklabel, at={(xticklabel cs:0.95,5pt)}},
        every y tick scale label/.style={inner sep=0pt, anchor=near yticklabel, at={(yticklabel cs:0.95,5pt)}},
        every z tick scale label/.style={inner sep=0pt, anchor=near yticklabel, at={(yticklabel cs:0.95,5pt)}},
        every axis legend/.style={shape=rectangle, draw=black, fill=white, anchor=north west, at={(1.03,1.03)},
            cells={anchor=center}, inner xsep=3pt, inner ysep=2pt, nodes={inner sep=2pt, text depth=0.15em}}",
    ),
    (
        "every smithchart axis",
        r"xmin=0, xmax=16000, ymin=-16000, ymax=16000, grid=both, scaled ticks=false,
        xtick align=center, ytick align=center, major tick style={draw=black},
        legend style={anchor=center}, every axis title shift=1.2\baselineskip",
    ),
    // the ticks by the chart's size: few below 14cm, many below 20cm, dense beyond
    (
        "few smithchart ticks*",
        "default smithchart xtick/.style={xtick={0.2,0.5,1,2,5}},
        default smithchart ytick/.style={ytick={0,0.2,0.5,1,2,5,-0.2,-0.5,-1,-2,-5}},
        default smithchart xytick/.style={xgrid each nth passes y={2}, ygrid each nth passes x={2}}",
    ),
    ("few smithchart ticks", "/pgfplots/few smithchart ticks*"),
    (
        "many smithchart ticks*",
        "default smithchart xtick/.style={xtick={0.1,0.2,0.3,0.4,0.5,1,1.5,2,3,4,5,10,20},
            minor xtick={0.6,0.7,0.8,0.9,1.1,1.2,1.3,1.4,1.6,1.7,1.8,1.9,2.2,2.4,2.6,2.8,3.2,3.4,3.6,3.8,4.5,6,7,8,9,50}},
        default smithchart ytick/.style={
            ytick={0,0.1,0.2,...,1,1.5,2,3,4,5,10,20,-0.1,-0.2,...,-1,-1.5,-2,-3,-4,-5,-10,-20},
            minor ytick={1.1,1.2,1.3,1.4,1.6,1.7,1.8,1.9,2.2,2.4,2.6,2.8,3.2,3.4,3.6,3.8,4.5,6,7,8,9,50,
                -1.1,-1.2,-1.3,-1.4,-1.6,-1.7,-1.8,-1.9,-2.2,-2.4,-2.6,-2.8,-3.2,-3.4,-3.6,-3.8,-4.5,-6,-7,-8,-9,-50}},
        default smithchart xytick/.style={xgrid each nth passes y={1,2,4,5,10,20}, ygrid each nth passes x={1,2,3,5,10:3,20:3}}",
    ),
    ("many smithchart ticks", "many smithchart ticks*, every axis title shift=6pt, yticklabel in circle, show origin=true"),
    (
        "dense smithchart ticks*",
        "default smithchart xtick/.style={xtick={0.1,0.2,0.3,0.4,0.5,0.6,0.7,0.8,0.9,1,1.2,1.4,1.6,1.8,2,3,4,5,10,20},
            minor xtick={0.01,0.02,0.03,0.04,0.05,0.06,0.07,0.08,0.09,0.11,0.12,0.13,0.14,0.15,0.16,0.17,0.18,0.19,
                0.22,0.24,0.26,0.28,0.32,0.34,0.36,0.38,0.42,0.44,0.46,0.48,0.52,0.55,0.65,0.75,0.85,0.95,
                1.1,1.3,1.5,1.7,1.9,2.2,2.4,2.6,2.8,3.2,3.4,3.6,3.8,4.5,6,7,8,9,50}},
        default smithchart ytick/.style={
            ytick={0,0.1,0.2,...,1,1.2,1.4,1.6,1.8,2,3,4,5,10,20,-0.1,-0.2,...,-1,-1.2,-1.4,-1.6,-1.8,-2,-3,-4,-5,-10,-20},
            minor ytick={0.01,0.02,0.03,0.04,0.05,0.06,0.07,0.08,0.09,0.11,0.12,0.13,0.14,0.15,0.16,0.17,0.18,0.19,
                0.22,0.24,0.26,0.28,0.32,0.34,0.36,0.38,0.42,0.44,0.46,0.48,0.55,0.65,0.75,0.85,0.95,
                1.1,1.3,1.5,1.7,1.9,2.2,2.4,2.6,2.8,3.2,3.4,3.6,3.8,4.5,6,7,8,9,50,
                -0.01,-0.02,-0.03,-0.04,-0.05,-0.06,-0.07,-0.08,-0.09,-0.11,-0.12,-0.13,-0.14,-0.15,-0.16,-0.17,-0.18,-0.19,
                -0.22,-0.24,-0.26,-0.28,-0.32,-0.34,-0.36,-0.38,-0.42,-0.44,-0.46,-0.48,-0.55,-0.65,-0.75,-0.85,-0.95,
                -1.1,-1.3,-1.5,-1.7,-1.9,-2.2,-2.4,-2.6,-2.8,-3.2,-3.4,-3.6,-3.8,-4.5,-6,-7,-8,-9,-50}},
        default smithchart xytick/.style={
            xgrid each nth passes y={0.2 if < 0.2001,0.5 if < 0.50001,1 if < 1.001,2,4,5,10,20},
            ygrid each nth passes x={0.2 if < 0.2001,0.52 if < 0.52001,1 if < 1.001,2,3,5,10:3,20:3}}",
    ),
    (
        "dense smithchart ticks",
        "yticklabel in circle, every axis title shift=6pt,
        dense smithchart ticks*, show origin=true, every major grid/.style={black!60}",
    ),
    // a colorbar is an axis of its own, beside its parent
    (
        "colorbar right",
        r"/pgfplots/colorbar=true, /pgfplots/colorbar pos*=r, /pgfplots/colorbar shift/.style={xshift=0.3cm},
        /pgfplots/every colorbar/.style={/pgfplots/every colorbar global, at={(parent axis.right of north east)}, anchor=north west,
            xmin=0, xmax=1, ymin=\pgfkeysvalueof{/pgfplots/point meta min}, ymax=\pgfkeysvalueof{/pgfplots/point meta max},
            height=\pgfkeysvalueof{/pgfplots/parent axis height}, width=\pgfkeysvalueof{/pgfplots/colorbar/width},
            xtick=\empty, yticklabel pos=right, colorbar vertical/lowlevel}",
    ),
    (
        "colorbar left",
        "/pgfplots/colorbar right, /pgfplots/colorbar pos*=l, /pgfplots/colorbar shift/.style={xshift=-0.3cm},
        /pgfplots/every colorbar/.append style={at={(parent axis.left of north west)}, anchor=north east, yticklabel pos=left}",
    ),
    (
        "colorbar horizontal",
        r"/pgfplots/colorbar=true, /pgfplots/colorbar pos*=b, /pgfplots/colorbar shift/.style={yshift=-0.3cm},
        /pgfplots/every colorbar/.style={/pgfplots/every colorbar global, at={(parent axis.below south west)}, anchor=north west,
            ymin=0, ymax=1, xmin=\pgfkeysvalueof{/pgfplots/point meta min}, xmax=\pgfkeysvalueof{/pgfplots/point meta max},
            width=\pgfkeysvalueof{/pgfplots/parent axis width}, height=\pgfkeysvalueof{/pgfplots/colorbar/width},
            xticklabel pos=left, ytick=\empty, colorbar horizontal/lowlevel}",
    ),
];

/// Keys that run TeX code; `#1` is their value.
const CODES: &[(&str, &str)] = &[
    ("/pgf/number format/fixed relative", r"\pgfkeysalso{/pgf/number format/@fixed relative}"),
    ("@boxplot macros", r"\def\pgfplotsboxplotvalue##1{\pgfkeysvalueof{/pgfplots/boxplot/##1}}\let\boxplotvalue=\pgfplotsboxplotvalue"),
    ("colorbar sampled", r"\pgfkeysalso{/pgfplots/colorbar=true}\pgfkeyssetvalue{/pgfplots/@colorbar sampled}{#1}\pgfkeyssetvalue{/pgfplots/@colorbar line}{false}"),
    ("colorbar sampled line", r"\pgfkeysalso{/pgfplots/colorbar=true}\pgfkeyssetvalue{/pgfplots/@colorbar sampled}{#1}\pgfkeyssetvalue{/pgfplots/@colorbar line}{true}"),
    ("error bars/{b} fixed", r"\pgfkeyssetvalue{/pgfplots/error bars/@{b} mode}{fixed}\pgfkeyssetvalue{/pgfplots/error bars/@{b} value}{#1}"),
    ("error bars/{b} fixed relative", r"\pgfkeyssetvalue{/pgfplots/error bars/@{b} mode}{fixed relative}\pgfkeyssetvalue{/pgfplots/error bars/@{b} value}{#1}"),
    ("group/{b} descriptions at", r"\pgfkeyssetvalue{/pgfplots/group/{b}labels at}{#1}\pgfkeyssetvalue{/pgfplots/group/{b}ticklabels at}{#1}"),
    ("nodes near coords", r"\pgfkeyssetvalue{/pgfplots/@nodes near coords}{#1}\pgfkeysalso{/pgfplots/set point meta if empty=f(x)}"),
    ("nodes near coords*", r"\pgfkeyssetvalue{/pgfplots/@nodes near coords}{#1}\pgfkeysalso{/pgfplots/set point meta if empty=f(x)}"),
    ("quiver/colored", r"\pgfkeyssetvalue{/pgfplots/quiver/arrow color}{#1}\pgfkeysalso{/pgfplots/set point meta if empty=f(x)}"),
    ("scatter/@pre marker code", r"\def\tikra@scatter@usemapped{1}"),
];

/// Keys of two values (`view={az}{el}`): styles, then code.
const STYLES_2: &[(&str, &str)] = &[
    ("skip coords between index", r"/pgfplots/x filter/.append code={\ifnum\coordindex<#1 \else\ifnum\coordindex<#2 \def\pgfmathresult{}\fi\fi}"),
    ("view", "/pgfplots/view/az={#1}, /pgfplots/view/el={#2}"),
];
const CODES_2: &[(&str, &str)] = &[
    ("colormap", r"\tikra@colormap{#1}{#2}\pgfkeyssetvalue{/pgfplots/colormap name}{#1}\pgfkeysalso{/pgfplots/@colormap defined={#1}}"),
    ("mesh/interior colormap", r"\tikra@colormap{#1}{#2}\pgfkeyssetvalue{/pgfplots/mesh/interior colormap name}{#1}"),
    (
        "restrict expr to domain",
        r"\pgfkeyssetvalue{/pgfplots/@restrict expr}{#1}\pgfkeyssetvalue{/pgfplots/@restrict expr domain}{#2}\pgfkeysalso{/pgfplots/unbounded coords=jump}",
    ),
    ("restrict expr to domain*", r"\pgfkeyssetvalue{/pgfplots/@restrict expr}{#1}\pgfkeyssetvalue{/pgfplots/@restrict expr domain}{#2}"),
];

/// Where `legend pos` puts the legend: the point of the axis, and the legend's anchor there.
const LEGEND_POS: &[(&str, &str, &str)] = &[
    ("north east", "0.97,0.97", "north east"),
    ("north west", "0.03,0.97", "north west"),
    ("outer north east", "1.03,1", "north west"),
    ("south east", "0.97,0.03", "south east"),
    ("south west", "0.03,0.03", "south west"),
];

/// `legend cell align`: the anchor of the cells for each choice.
const LEGEND_CELL_ALIGN: &[(&str, &str)] = &[("center", "center"), ("left", "west"), ("right", "east")];

/// `ticks=…` and `grid=…`: whether each choice turns on the major and the minor ones, on every axis.
const MAJOR_MINOR: &[(&str, bool, bool)] = &[("both", true, true), ("major", true, false), ("minor", false, true), ("none", false, false)];

/// The y tick labels of a smith chart, along the circle: (key, tick align, the labels' anchor,
/// their anchor when upside down).
const CIRCLE_LABELS: &[(&str, &str, &str, &str)] = &[
    ("yticklabel around circle", "center", "south east", "south west"),
    ("yticklabel around circle*", "center", "north east", "north west"),
    ("yticklabel in circle", "inside", "south west", "north east"),
];

/// Where `cycle list` and its variants keep the list they are given.
const CYCLE_LIST_KEYS: &[(&str, &str)] =
    &[("cycle list", "@internal@"), ("cycle multi list", "@multi@"), ("cycle multiindex list", "@multiindex@"), ("cycle multiindex* list", "@multiindex*@")];

/// A cycle list entry: the line's options, whether it is densely dashed, the mark's options
/// (solid, in a dashed line) and the mark.
type Entry = (&'static str, bool, &'static str, &'static str);

/// The predefined cycle lists (pgfplots manual, "Cycle Lists"): each plot of an axis takes the
/// next entry.
const CYCLE_LISTS: &[(&str, &[Entry])] = &[
    (
        "color",
        &[
            ("blue", false, "fill=blue!80!black", "*"),
            ("red", false, "fill=red!80!black", "square*"),
            ("brown!60!black", false, "fill=brown!80!black", "otimes*"),
            ("black", false, "", "star"),
            ("blue", false, "fill=blue!80!black", "diamond*"),
            ("red", true, "solid,fill=red!80!black", "*"),
            ("brown!60!black", true, "solid,fill=brown!80!black", "square*"),
            ("black", true, "solid,fill=gray", "otimes*"),
            ("blue", true, "solid", "star"),
            ("red", true, "solid,fill=red!80!black", "diamond*"),
        ],
    ),
    (
        "exotic",
        &[
            ("teal", false, "fill=teal!80!black", "*"),
            ("orange", false, "fill=orange!80!black", "square*"),
            ("cyan!60!black", false, "fill=cyan!80!black", "otimes*"),
            ("red!70!white", false, "", "star"),
            ("lime!80!black", false, "fill=lime", "diamond*"),
            ("red", true, "solid,fill=red!80!black", "*"),
            ("yellow!60!black", true, "solid,fill=yellow!80!black", "square*"),
            ("black", false, "solid,fill=gray", "otimes*"),
            ("blue", true, "solid", "star"),
            ("red", true, "solid,fill=red!80!black", "diamond*"),
        ],
    ),
    (
        "black white",
        &[
            ("", false, "fill=gray", "*"),
            ("", false, "fill=gray", "square*"),
            ("", false, "fill=gray", "otimes*"),
            ("", false, "", "star"),
            ("", false, "fill=gray", "diamond*"),
            ("", true, "solid,fill=gray", "*"),
            ("", true, "solid,fill=gray", "square*"),
            ("", true, "solid,fill=gray", "otimes*"),
            ("", true, "solid", "star"),
            ("", true, "solid,fill=gray", "diamond*"),
        ],
    ),
    (
        "mark list",
        &[
            ("", false, r"solid,fill=\pgfplotsmarklistfill", "*"),
            ("", false, r"solid,fill=\pgfplotsmarklistfill", "square*"),
            ("", false, r"solid,fill=\pgfplotsmarklistfill", "triangle*"),
            ("", false, "solid", "star"),
            ("", false, r"solid,fill=\pgfplotsmarklistfill", "diamond*"),
            ("", false, r"solid,fill=\pgfplotsmarklistfill!40", "otimes*"),
            ("", false, "solid", "|"),
            ("", false, r"solid,fill=\pgfplotsmarklistfill", "pentagon*"),
            ("", false, "solid", "text,text mark=p"),
            ("", false, "solid", "text,text mark=a"),
        ],
    ),
    (
        "mark list*",
        &[
            ("", false, r"solid,fill=\pgfplotsmarklistfill", "*"),
            ("", false, r"solid,fill=\pgfplotsmarklistfill", "square*"),
            ("", false, r"solid,fill=\pgfplotsmarklistfill", "triangle*"),
            ("", false, r"solid,fill=\pgfplotsmarklistfill", "halfsquare*"),
            ("", false, r"solid,fill=\pgfplotsmarklistfill", "pentagon*"),
            ("", false, r"solid,fill=\pgfplotsmarklistfill", "halfcircle*"),
            ("", false, r"solid,fill=\pgfplotsmarklistfill,rotate=180", "halfdiamond*"),
            ("", false, r"solid,fill=\pgfplotsmarklistfill!40", "otimes*"),
            ("", false, r"solid,fill=\pgfplotsmarklistfill", "diamond*"),
            ("", false, r"solid,fill=\pgfplotsmarklistfill", "halfsquare right*"),
            ("", false, r"solid,fill=\pgfplotsmarklistfill", "halfsquare left*"),
        ],
    ),
];

/// Cycle lists of line styles or colors alone, one option per plot.
const PLAIN_CYCLE_LISTS: &[(&str, &[&str])] = &[
    ("color list", &["red", "blue", "black", "yellow", "brown", "teal", "orange", "violet", "cyan", "green!70!black", "magenta", "gray"]),
    ("linestyles", &["solid", "dashed", "dotted"]),
    ("linestyles*", &["solid", "dashed", "dotted", "dashdotted", "dashdotdotted"]),
];

/// The bars of `bar cycle list`: (line color, fill color).
const BAR_CYCLE: &[(&str, &str)] =
    &[("blue", "blue!30!white"), ("red", "red!30!white"), ("brown!60!black", "brown!30!white"), ("black", "gray"), ("violet!80!black", "violet"), ("green", "green!80!black")];

/// The predefined colormaps (pgfplots manual, "Color maps"), in the colormap syntax.
const COLORMAPS: &[(&str, &str)] = &[
    ("bluered", "rgb255(0cm)=(0,0,180); rgb255(1cm)=(0,255,255); rgb255(2cm)=(100,255,0); rgb255(3cm)=(255,255,0); rgb255(4cm)=(255,0,0); rgb255(5cm)=(128,0,0)"),
    ("blackwhite", "gray(0cm)=(0); gray(1cm)=(1)"),
    ("cool", "rgb255(0cm)=(255,255,255); rgb255(1cm)=(0,128,255); rgb255(2cm)=(255,0,255)"),
    ("greenyellow", "rgb255(0cm)=(0,128,0); rgb255(1cm)=(255,255,0)"),
    ("hot", "color(0cm)=(blue); color(1cm)=(yellow); color(2cm)=(orange); color(3cm)=(red)"),
    ("hot2", "[1cm]rgb255(0cm)=(0,0,0) rgb255(3cm)=(255,0,0) rgb255(6cm)=(255,255,0) rgb255(8cm)=(255,255,255)"),
    ("jet", "rgb255(0cm)=(0,0,128) rgb255(1cm)=(0,0,255) rgb255(3cm)=(0,255,255) rgb255(5cm)=(255,255,0) rgb255(7cm)=(255,0,0) rgb255(8cm)=(128,0,0)"),
    ("redyellow", "rgb255(0cm)=(255,0,0); rgb255(1cm)=(255,255,0)"),
    ("violet", "rgb255=(25,25,122) color=(white) rgb255=(238,140,238)"),
];

/// matplotlib's viridis and plasma, sampled: the rgb of each stop.
const SAMPLED_COLORMAPS: &[(&str, &[&str])] = &[
    (
        "plasma",
        &[
            "0.050383,0.029803,0.527975",
            "0.171574,0.019706,0.580806",
            "0.261183,0.013308,0.617911",
            "0.35015,0.004382,0.646298",
            "0.429719,0.000831,0.659425",
            "0.512206,0.018833,0.655209",
            "0.584391,0.068579,0.632812",
            "0.65558,0.129725,0.592317",
            "0.714883,0.187299,0.546338",
            "0.771958,0.249237,0.494813",
            "0.819651,0.306812,0.448306",
            "0.866078,0.36966,0.400126",
            "0.904601,0.429797,0.356329",
            "0.940313,0.497642,0.309197",
            "0.966798,0.564396,0.265118",
            "0.986345,0.640969,0.217948",
            "0.994324,0.716681,0.177208",
            "0.989587,0.803205,0.146529",
            "0.970533,0.887896,0.145919",
            "0.940015,0.975158,0.131326",
        ],
    ),
    (
        "viridis",
        &[
            "0.267,0.00487,0.32942",
            "0.28192,0.08966,0.41241",
            "0.28026,0.1657,0.4765",
            "0.26366,0.23763,0.51877",
            "0.23744,0.3052,0.54192",
            "0.20862,0.36775,0.55267",
            "0.18225,0.42618,0.55711",
            "0.1592,0.48224,0.55807",
            "0.13777,0.53749,0.5549",
            "0.12115,0.59274,0.54465",
            "0.12808,0.64775,0.5235",
            "0.18065,0.7014,0.48819",
            "0.27415,0.75198,0.4366",
            "0.39517,0.79747,0.36775",
            "0.53561,0.83578,0.2819",
            "0.68895,0.86545,0.18272",
            "0.84557,0.88733,0.0997",
            "0.99324,0.90616,0.14394",
        ],
    ),
];

/// Macros that pgfplots documents, and the values the axis code reads before any axis sets them.
const PRELUDE: &str = r"
\def\pgfplotscreateplotcyclelist#1#2{\expandafter\def\csname pgfk@/pgfplots/@cycle lists/#1\endcsname{#2}}
\let\pgfcreateplotcyclelist\pgfplotscreateplotcyclelist
\def\pgfkeyslet#1#2{\expandafter\let\csname pgfk@#1\endcsname#2}
\def\tikra@colormap#1#2{\expandafter\def\csname pgfk@/pgfplots/@colormaps/#1\endcsname{#2}}
\def\axisdefaultwidth{240pt}\def\axisdefaultheight{207pt}
\def\pgfplotsmarklistfill{.!80!black}
\def\numplots{0}\def\numplotsofactualtype{1}\def\plotnumofactualtype{0}\def\plotnum{0}
\def\pgfplotbarwidth{10pt}\def\pgfplotbarshift{0pt}
\def\pgfplotscolormaplastindexof#1{\csname pgfpl@cm@#1@last\endcsname}
\pgfkeys{/pgfplots/.search also={/tikz}, /tikz/fill between/.search also={/pgfplots,/tikz}}
";

/// What the definitions set up, once they are all made: the default colormap, the colorbar and
/// the labels' places.
const SETUP: &str = r"
\pgfplotsset{@boxplot macros, xlabel absolute, ylabel absolute, colormap/viridis, colormap/hot, colorbar right, colorbar=false}
";

/// A key's full path: the tables name pgfplots keys without their `/pgfplots/`.
fn full(name: &str) -> String {
    if name.starts_with('/') {
        name.to_string()
    } else {
        format!("/pgfplots/{name}")
    }
}

/// The lines a table line stands for: `{a}` is each of x, y and z; `{b}` is x and y, with `{o}`
/// the other of the two.
fn axes<const N: usize>(parts: [&str; N]) -> Vec<[String; N]> {
    let with = |pairs: &[(&str, &str)]| parts.map(|p| pairs.iter().fold(p.to_string(), |p, (from, to)| p.replace(from, to)));
    if parts.iter().any(|p| p.contains("{a}")) {
        ["x", "y", "z"].iter().map(|a| with(&[("{a}", a)])).collect()
    } else if parts.iter().any(|p| p.contains("{b}")) {
        [("x", "y"), ("y", "x")].iter().map(|(b, o)| with(&[("{b}", b), ("{o}", o)])).collect()
    } else {
        vec![parts.map(str::to_string)]
    }
}

/// One definition, `name/handler={value}`, onto the text.
fn put(s: &mut String, name: &str, handler: &str, value: &str) {
    s.push_str(&format!("{}/{handler}={{{value}}},\n", full(name)));
}

/// The pgfplots keys as TeX, built once from the tables.
fn definitions() -> &'static str {
    static TEXT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    TEXT.get_or_init(|| {
        let mut s = String::from(PRELUDE);
        s.push_str("\\pgfkeys{\n");
        let d = &mut s;
        for &(n, v) in VALUES {
            axes([n, v]).iter().for_each(|[n, v]| put(d, n, ".initial", v));
        }
        for &(n, v) in SWITCHES {
            for [n, v] in axes([n, v]) {
                put(d, &n, ".initial", &v);
                put(d, &n, ".default", "true");
            }
        }
        for &n in HOOKS {
            axes([n]).iter().for_each(|[n]| put(d, n, ".style", ""));
        }
        for &n in IGNORED {
            axes([n]).iter().for_each(|[n]| put(d, n, ".code", ""));
        }
        for &(n, t) in ADDS_TO {
            axes([n, t]).iter().for_each(|[n, t]| put(d, n, ".style", &format!("{}/.append style={{#1}}", full(t))));
        }
        for &(n, t) in ALIASES {
            axes([n, t]).iter().for_each(|[n, t]| put(d, n, ".style", &format!("{}={{#1}}", full(t))));
        }
        for &(n, path, v) in SETS {
            for [n, path, v] in axes([n, path, v]) {
                put(d, &n, ".code", &format!("\\pgfkeyssetvalue{{{}}}{{{v}}}", full(&path)));
            }
        }
        for &(n, targets) in SPREADS {
            let body: Vec<String> = targets.iter().map(|t| format!("{}={{#1}}", full(t))).collect();
            put(d, n, ".style", &body.join(", "));
        }
        for &n in CHOICES {
            d.push_str(&format!("{}/.is choice,\n", full(n)));
        }
        for &(n, v) in DEFAULTS {
            axes([n, v]).iter().for_each(|[n, v]| put(d, n, ".default", v));
        }
        for &(n, v) in STYLES.iter().chain(LONG_STYLES) {
            axes([n, v]).iter().for_each(|[n, v]| put(d, n, ".style", v));
        }
        for &(n, v) in CODES {
            axes([n, v]).iter().for_each(|[n, v]| put(d, n, ".code", v));
        }
        for &(n, v) in STYLES_2 {
            put(d, n, ".style 2 args", v);
        }
        for &(n, v) in CODES_2 {
            put(d, n, ".code 2 args", v);
        }
        // the dash patterns pgfplots adds to TikZ's: a dash, then one or two dots
        for (spacing, gap) in [("loosely ", 4), ("", 2), ("densely ", 1)] {
            for (name, dots) in [("dashdotted", 1), ("dashdotdotted", 2)] {
                let pattern = format!("on 3pt off {gap}pt{}", format!(" on \\the\\pgflinewidth off {gap}pt").repeat(dots));
                put(d, &format!("/tikz/{spacing}{name}"), ".style", &format!("dash pattern={pattern}"));
            }
        }
        for &(n, at, anchor) in LEGEND_POS {
            put(d, &format!("legend pos/{n}"), ".style", &format!("/pgfplots/legend style={{at={{({at})}}, anchor={anchor}}}"));
        }
        for &(n, anchor) in LEGEND_CELL_ALIGN {
            put(d, &format!("legend cell align/{n}"), ".style", &format!("/pgfplots/legend style={{cells={{anchor={anchor}}}}}"));
        }
        for (key, kind) in [("ticks", "ticks"), ("grid", "grids")] {
            for &(choice, major, minor) in MAJOR_MINOR {
                let body: Vec<String> = ["x", "y", "z"].iter().map(|a| format!("{a}major{kind}={major}, {a}minor{kind}={minor}")).collect();
                put(d, &format!("{key}/{choice}"), ".style", &body.join(", "));
            }
        }
        for &(n, align, anchor, upside_down) in CIRCLE_LABELS {
            let labels = format!("rotate=90, sloped like y axis={{execute for upside down={{\\tikzset{{anchor={upside_down}}}}}, reset nontranslations=false}}, anchor={anchor}");
            put(d, n, ".style", &format!("ytick align={align}, yticklabel style={{{labels}}}"));
        }
        for &(n, slot) in CYCLE_LIST_KEYS {
            put(d, n, ".code", &format!("\\pgfplotscreateplotcyclelist{{{slot}}}{{#1}}\\pgfkeyssetvalue{{/pgfplots/cycle list name}}{{{slot}}}"));
        }
        let bars: Vec<String> = BAR_CYCLE.iter().map(|(line, fill)| format!("{{{line}, fill={fill}, mark=none}}")).collect();
        put(d, "bar cycle list", ".style", &format!("/pgfplots/cycle list={{{}}}", bars.join(", ")));
        for &(n, spec) in COLORMAPS {
            put(d, &format!("colormap/{n}"), ".style", &format!("/pgfplots/colormap={{{n}}}{{{spec}}}"));
        }
        for &(n, stops) in SAMPLED_COLORMAPS {
            let spec: Vec<String> = stops.iter().map(|rgb| format!("rgb=({rgb})")).collect();
            put(d, &format!("colormap/{n}"), ".style", &format!("/pgfplots/colormap={{{n}}}{{{}}}", spec.join(" ")));
        }
        s.push_str("}\n");
        for &(name, entries) in CYCLE_LISTS {
            let entries: Vec<String> = entries
                .iter()
                .map(|&(line, dashed, mark_style, mark)| {
                    let mut options = vec![];
                    options.extend((!line.is_empty()).then(|| line.to_string()));
                    options.extend(dashed.then(|| "densely dashed".to_string()));
                    options.extend((!mark_style.is_empty()).then(|| format!("every mark/.append style={{{mark_style}}}")));
                    options.push(format!("mark={mark}"));
                    format!("{{{}}}", options.join(","))
                })
                .collect();
            s.push_str(&format!("\\pgfplotscreateplotcyclelist{{{name}}}{{{}\\\\}}\n", entries.join("\\\\")));
        }
        for &(name, options) in PLAIN_CYCLE_LISTS {
            s.push_str(&format!("\\pgfplotscreateplotcyclelist{{{name}}}{{{}\\\\}}\n", options.join("\\\\")));
        }
        s.push_str(SETUP);
        s
    })
}

macro_rules! keyfn {
    ($name:ident, |$e:ident, $v:ident| $body:expr) => {
        fn $name($e: &mut Engine, $v: Option<Vec<Token>>) -> Result<(), Error> {
            $body
        }
    };
}

fn text(e: &mut Engine, v: &Option<Vec<Token>>) -> Result<String, Error> {
    Ok(match v {
        Some(v) => e.tex.expand_to_string(v.clone())?.trim().to_string(),
        None => String::new(),
    })
}

/// The key's last name, for keys that share their code (`/pgfplots/axis x line`).
fn current_name(e: &Engine) -> String {
    keys::name(&e.current_key).to_string()
}

/// Sets a stored value (`\pgfkeyssetvalue`).
pub fn set(e: &mut Engine, path: &str, value: &str) {
    e.tex.define_string(&keys::macro_name(path), value, false);
}

/// A stored value as tokens, if it is not empty.
pub fn value_tokens(e: &Engine, path: &str) -> Option<Vec<Token>> {
    keys::value(&e.tex, path).filter(|v| !keys::trim(v).is_empty())
}

/// A stored value, expanded, as text (empty if unset).
pub fn value(e: &mut Engine, path: &str) -> String {
    match keys::value(&e.tex, path) {
        Some(v) => e.tex.expand_to_string(v).map(|s| s.trim().to_string()).unwrap_or_default(),
        None => String::new(),
    }
}

/// A boolean value (`true`, `false`).
pub fn flag(e: &mut Engine, path: &str) -> bool {
    value(e, path) == "true"
}

/// A numeric value, if set and valid.
pub fn number(e: &mut Engine, path: &str) -> Option<f64> {
    let s = value(e, path);
    if s.is_empty() {
        return None;
    }
    e.number(&s).ok().filter(|v| v.is_finite())
}

/// A length value in cm, if set and valid (a number without unit is in pt).
pub fn length(e: &mut Engine, path: &str) -> Option<f64> {
    let s = value(e, path);
    if s.is_empty() {
        return None;
    }
    e.length(&s).ok().filter(|v| v.is_finite())
}

/// Runs a key list under `/pgfplots`.
pub fn run(e: &mut Engine, list: &str) -> Result<(), Error> {
    let saved = e.st.key_path.clone();
    let r = e.process_keys(tikra_tokens(list), Some("/pgfplots"));
    e.st.key_path = saved;
    r
}

/// The compatibility level, as a number: 1.18 → 118, `default` → 0.
pub fn compat(e: &Engine) -> u32 {
    keys::value(&e.tex, "/pgfplots/@compat").map(|v| tex::tokens_to_string(&v)).and_then(|s| s.trim().parse().ok()).unwrap_or(0)
}

// `/pgfplots/table/.unknown`: a key of `table[…]` that is not the table's is the plot's
// (`\pgfplots@addplotimpl@table@installkeypath`), `table[point meta=…]` for one.
keyfn!(k_table_unknown, |e, v| {
    let name = match e.tex.meaning_of("pgfkeyscurrentname") {
        Meaning::Macro(m) => tex::tokens_to_string(&tex::substitute(&m.body, &[])),
        _ => String::new(),
    };
    let saved = e.st.key_path.clone();
    let r = e.invoke(&format!("/pgfplots/{name}"), v);
    e.st.key_path = saved;
    r
});

// `/pgfplots/.unknown`: a key pgfplots does not know is a TikZ key (`.search also=/tikz`).
keyfn!(k_unknown, |e, v| {
    let name = match e.tex.meaning_of("pgfkeyscurrentname") {
        Meaning::Macro(m) => tex::tokens_to_string(&tex::substitute(&m.body, &[])),
        _ => String::new(),
    };
    let saved = e.st.key_path.clone();
    let r = e.invoke(&format!("/tikz/{name}"), v);
    e.st.key_path = saved;
    r
});

// `compat=1.x`: the level, and the label placement it implies (`compat/labels`).
// `visualization depends on={expr \as \macro}`: one more, kept as a group
keyfn!(k_visualization_depends_on, |e, v| {
    let path = keys::macro_name("/pgfplots/@visualization depends on");
    let mut list = keys::value(&e.tex, "/pgfplots/@visualization depends on").unwrap_or_default();
    list.push(Token::Char('{', tex::Cat::Begin));
    list.extend(v.unwrap_or_default());
    list.push(Token::Char('}', tex::Cat::End));
    e.tex.set_meaning(&Token::cs(&path), super::super::macro_tokens(list), false);
    Ok(())
});

keyfn!(k_compat, |e, v| {
    let s = text(e, &v)?;
    let level = match s.as_str() {
        "newest" | "1.18" => 118,
        "default" | "pre 1.3" | "" => 0,
        other => {
            let mut parts = other.split('.');
            let major: u32 = parts.next().and_then(|p| p.trim().parse().ok()).unwrap_or(1);
            let minor: u32 = parts.next().and_then(|p| p.trim().parse().ok()).unwrap_or(0);
            major * 100 + minor
        }
    };
    set(e, "/pgfplots/compat", &s);
    set(e, "/pgfplots/@compat", &level.to_string());
    set(e, "/pgfplots/@substitute pgfpoint", if level >= 111 { "true" } else { "false" });
    if level >= 103 {
        run(e, "xlabel near ticks,ylabel near ticks")?;
    } else {
        run(e, "xlabel absolute,ylabel absolute")?;
    }
    Ok(())
});

// `compat/pgfpoint substitution=1.x`: from 1.11 on, `(1,2)` and `\pgfqpointxy` inside an axis
// are `axis cs`; before, they are in the units of the data scale transformation.
keyfn!(k_pgfpoint_substitution, |e, v| {
    let s = text(e, &v)?;
    let minor = s.trim().strip_prefix("1.").and_then(|m| m.parse::<u32>().ok());
    let on = s.trim() == "newest" || minor.is_some_and(|m| m >= 11);
    set(e, "/pgfplots/@substitute pgfpoint", if on { "true" } else { "false" });
    Ok(())
});

/// Whether plain coordinates inside an axis are `axis cs` (`\ifpgfplots@substitute@pgfpoint`).
pub fn substitute_pgfpoint(e: &Engine) -> bool {
    keys::value(&e.tex, "/pgfplots/@substitute pgfpoint").is_some_and(|v| tex::tokens_to_string(&v).trim() == "true")
}

/// `axis x line*=bottom`: where the line is, and the ticks go with it.
fn line_star(e: &mut Engine, axis: char, value: &str) {
    let (line, pos) = match value {
        "box" => ("box", "both"),
        "bottom" | "left" => ("low", "left"),
        "middle" | "center" => ("center", "center"),
        "top" | "right" => ("high", "right"),
        _ => ("none", "none"),
    };
    set(e, &format!("/pgfplots/@{axis} line"), line);
    set(e, &format!("/pgfplots/{axis}tick pos"), pos);
}

keyfn!(k_line_star, |e, v| {
    let name = current_name(e);
    let axis = name.chars().nth(5).unwrap_or('x');
    let value = text(e, &v)?;
    line_star(e, axis, &value);
    Ok(())
});

// `axis x line=middle`: the line, and the styles of `compat/axis line style` for it.
keyfn!(k_line, |e, v| {
    let name = current_name(e);
    let axis = name.chars().nth(5).unwrap_or('x');
    let value = text(e, &v)?;
    line_star(e, axis, &value);
    let post18 = compat(e) >= 108;
    let other = if axis == 'x' { 'y' } else { 'x' };
    let other_line = keys::value(&e.tex, &format!("/pgfplots/@{other} line")).map(|v| tex::tokens_to_string(&v)).unwrap_or_default();
    let list = match (value.as_str(), axis) {
        ("box", _) if post18 => format!("{axis}label near ticks,every boxed {axis} axis"),
        ("box", _) => format!("every boxed {axis} axis"),
        ("none", _) => format!("hide {axis} axis"),
        ("middle" | "center", 'x') if post18 => {
            let at = if other_line == "high" { "0" } else { "1" };
            format!("every axis x label/.style={{/pgfplots/near ticklabel align=inside,at={{(ticklabel* cs:{at})}},anchor=near ticklabel opposite}},every non boxed x axis")
        }
        ("middle" | "center", 'x') => {
            if other_line == "high" {
                "every axis x label/.style={at={(current axis.left of origin)},anchor=south west},every non boxed x axis".into()
            } else {
                "every axis x label/.style={at={(current axis.right of origin)},anchor=south east},every non boxed x axis".into()
            }
        }
        ("middle" | "center", 'y') if post18 => {
            if other_line == "high" {
                "every axis y label/.style={/pgfplots/near ticklabel align=outside,at={(ticklabel* cs:0)},anchor=near ticklabel opposite},every non boxed y axis".into()
            } else {
                "every axis y label/.style={/pgfplots/near ticklabel align=inside,at={(ticklabel* cs:1)},anchor=near ticklabel opposite},every non boxed y axis".into()
            }
        }
        ("middle" | "center", 'y') => {
            if other_line == "high" {
                "every axis y label/.style={at={(current axis.below origin)},anchor=south west},every non boxed y axis".into()
            } else {
                "every axis y label/.style={at={(current axis.above origin)},anchor=north west},every non boxed y axis".into()
            }
        }
        ("top" | "right", 'x') if post18 => "every axis x label/.style={at={(ticklabel cs:0.5)},anchor=near ticklabel},every non boxed x axis".into(),
        ("top" | "right", 'x') => "every axis x label/.style={at={(0.5,1)},anchor=south,yshift=15pt},every non boxed x axis".into(),
        ("top" | "right", 'y') if post18 => "every axis y label/.style={at={(ticklabel cs:0.5)},anchor=near ticklabel,rotate=90},every non boxed y axis".into(),
        ("top" | "right", 'y') => "every axis y label/.style={at={(1,0.5)},xshift=33pt,rotate=90},every non boxed y axis".into(),
        (_, a) => format!("every non boxed {a} axis"),
    };
    run(e, &list)
});

/// The plot handlers of TikZ that pgfplots draws (`const plot`, `ybar`…): stored as a value so
/// that groups restore them.
fn handler(e: &mut Engine, name: &str) {
    set(e, "/tikz/@plot handler", name);
}

keyfn!(k_handler, |e, _v| {
    let name = current_name(e);
    // TikZ's own `plot` draws with the same handler
    e.st.plot_handler = name.clone();
    let h = match name.as_str() {
        "smooth" | "sharp plot" => "",
        "const plot" | "const plot mark left" => "const",
        "const plot mark right" => "const right",
        "const plot mark mid" => "const mid",
        "jump mark left" => "jump left",
        "jump mark right" => "jump right",
        "jump mark mid" => "jump mid",
        other => other,
    };
    handler(e, h);
    Ok(())
});

// `mesh`, `surf`, `patch`: the plot is a mesh colored by its point meta.
keyfn!(k_mesh, |e, v| {
    let name = current_name(e);
    let value = text(e, &v)?;
    if value == "false" {
        set(e, "/pgfplots/@mesh", "");
        return Ok(());
    }
    set(e, "/pgfplots/@mesh", if name == "mesh" { "m" } else { "s" });
    if name == "patch" {
        set(e, "/pgfplots/@patch", "true");
        set(e, "/pgfplots/mesh input", "patches");
    }
    run(e, "set point meta if empty=f(x),every mesh/.try,/tikz/color=mapped color")?;
    if name == "patch" {
        run(e, "every patch")?;
    }
    Ok(())
});

// `scatter`: marks colored by the colormap.
keyfn!(k_scatter, |e, v| {
    let value = text(e, &v)?;
    let on = value != "false";
    set(e, "/pgfplots/scatter", if on { "true" } else { "false" });
    if on {
        run(e, "set point meta if empty=f(x)")?;
    }
    Ok(())
});

// `color of colormap={300 of viridis}` (0 to 1000), `const color of colormap`, `index of
// colormap={4 of viridis}` (the colormap's 5th color): the current color, as
// `\pgfplots@color@of@colormap` sets it.
keyfn!(k_colormap_color, |e, v| {
    let name = current_name(e);
    let s = text(e, &v)?;
    // `\i of jet` reads `0of jet`: the first `of` after the number
    let at = s.char_indices().find(|&(i, _)| s[i..].starts_with("of") && s[..i].trim_end().chars().last().is_some_and(|c| c.is_ascii_digit() || c == '.' || c == ')'));
    let (value, map) = match at {
        Some((i, _)) => (s[..i].trim().to_string(), Some(s[i + 2..].trim().to_string())),
        None => (s.trim().to_string(), None),
    };
    let mut map_name = map.unwrap_or_else(|| keys::value(&e.tex, "/pgfplots/colormap name").map(|v| tex::tokens_to_string(&v)).unwrap_or_default());
    // `of colormap/cool`: the style defines the map
    let spec_of = |e: &Engine, n: &str| keys::value(&e.tex, &format!("/pgfplots/@colormaps/{n}")).map(|v| tex::tokens_to_string(&v)).unwrap_or_default();
    let spec = if let Some(style) = map_name.strip_prefix("colormap/").map(str::to_string) {
        e.begin_group();
        let r = run(e, &format!("/pgfplots/colormap/{style}"));
        let spec = spec_of(e, &style);
        e.end_group();
        r?;
        map_name = style;
        spec
    } else {
        spec_of(e, &map_name)
    };
    let Some(cm) = super::colormap::Colormap::parse(&spec, &e.colors) else {
        e.warn(format!("{name}: colormap '{map_name}' not defined"));
        return Ok(());
    };
    let x = e.number(&value)?;
    let c = if name.starts_with("index") {
        let k = (x.max(0.0) as usize).min(cm.stops.len() - 1);
        cm.stops[k].1
    } else if name.starts_with("const") {
        cm.at_const(x / 1000.0)
    } else {
        cm.at(x / 1000.0)
    };
    let id = format!("pgfplots@colormap@{:.0}@{:.0}@{:.0}", c.r * 1e6, c.g * 1e6, c.b * 1e6);
    e.colors.insert(id.clone(), c);
    e.process_keys(super::toks(&format!("/tikz/color={id}")), Some("/tikz"))
});

// After a colormap is defined: `\pgfplotscolormaplastindexof{name}` (the index of its last color).
keyfn!(k_colormap_defined, |e, v| {
    let name = text(e, &v)?;
    let mut spec = value(e, &format!("/pgfplots/@colormaps/{name}"));
    // `samples of colormap=(10 of viridis)`: that many colors of the other colormap, equally spaced
    if let Some(rest) = spec.trim().strip_prefix("samples of colormap") {
        let arg = rest.trim().trim_start_matches('=').trim().trim_matches(|c| matches!(c, '(' | ')' | '{' | '}'));
        if let Some((n, other)) = arg.split_once(" of ") {
            let n: usize = n.trim().parse().unwrap_or(0);
            if let Some(cm) = super::colormap::named(e, other.split(',').next().unwrap_or("").trim()).filter(|_| n >= 2) {
                let colors: Vec<String> = (0..n).map(|k| cm.at(k as f64 / (n - 1) as f64)).map(|c| format!("rgb=({},{},{})", c.r, c.g, c.b)).collect();
                spec = colors.join(" ");
                set(e, &format!("/pgfplots/@colormaps/{name}"), &spec);
            }
        }
    }
    if let Some(cm) = super::colormap::Colormap::parse(&spec, &e.colors) {
        e.tex.define_string(&format!("pgfpl@cm@{name}@last"), &(cm.stops.len() - 1).to_string(), true);
    }
    Ok(())
});

// `x SI prefix=kilo` (the units library): the prefix of the unit in the label; with `change x base`
// the tick labels in that unit too (`scaled x ticks=base 10:-3`, without a scale label).
keyfn!(k_si_prefix, |e, v| {
    let c = current_key_axis(e);
    let name = text(e, &v)?;
    let table: &[(&str, i32, &str)] = &[
        ("yocto", 24, "y"),
        ("zepto", 21, "z"),
        ("atto", 18, "a"),
        ("femto", 15, "f"),
        ("pico", 12, "p"),
        ("nano", 9, "n"),
        ("micro", 6, "\\mu"),
        ("milli", 3, "m"),
        ("centi", 2, "c"),
        ("deci", 1, "d"),
        ("deca", -1, "da"),
        ("hecto", -2, "h"),
        ("kilo", -3, "k"),
        ("mega", -6, "M"),
        ("giga", -9, "G"),
        ("tera", -12, "T"),
        ("peta", -15, "P"),
        ("exa", -18, "E"),
        ("zetta", -21, "Z"),
        ("yotta", -24, "Y"),
    ];
    let Some(&(_, exp, prefix)) = table.iter().find(|(n, _, _)| *n == name) else {
        e.warn(format!("{c} SI prefix={name} not supported: ignored"));
        return Ok(());
    };
    set(e, &format!("/pgfplots/{c} unit prefix"), prefix);
    if flag(e, &format!("/pgfplots/change {c} base")) {
        set(e, &format!("/pgfplots/scaled {c} ticks"), &format!("base 10:{exp}"));
        set(e, &format!("/pgfplots/@{c} scale label off"), "true");
    }
    Ok(())
});

/// The axis letter of the key being run (`/pgfplots/x SI prefix` → `x`).
fn current_key_axis(e: &Engine) -> char {
    current_name(e).chars().next().unwrap_or('x')
}

// `colormap/PuBu`, `colormap/RdPu-6`: the ColorBrewer colormaps (`\usepgfplotslibrary{colorbrewer}`),
// equally spaced colors.
keyfn!(k_colormap_unknown, |e, _v| {
    let name = e.tex.expand_to_string(vec![Token::cs("pgfkeyscurrentname")])?;
    let Some(colors) = crate::color::brewer::colormap(&name) else {
        e.warn(format!("option '{name}' not supported: ignored"));
        return Ok(());
    };
    let spec: Vec<String> = colors.iter().map(|c| format!("rgb=({},{},{})", c.r, c.g, c.b)).collect();
    run(e, &format!("/pgfplots/colormap={{{name}}}{{{}}}", spec.join(" ")))
});

// `contour prepared={…}`: the contour handler, its keys in /pgfplots/contour.
keyfn!(k_contour_prepared, |e, v| {
    set(e, "/tikz/@plot handler", "contour prepared");
    let saved = e.st.key_path.clone();
    let mut list = super::toks("every contour plot,");
    list.extend(v.unwrap_or_default());
    let r = e.process_keys(list, Some("/pgfplots/contour"));
    e.st.key_path = saved;
    r
});

// `contour filled={…}`: a mesh drawn in bands of the levels, its keys in /pgfplots/contour.
keyfn!(k_contour_filled, |e, v| {
    run(e, "/pgfplots/surf")?;
    set(e, "/tikz/@plot handler", "contour filled");
    let saved = e.st.key_path.clone();
    let mut list = super::toks("every filled contour plot,");
    list.extend(v.unwrap_or_default());
    let r = e.process_keys(list, Some("/pgfplots/contour"));
    e.st.key_path = saved;
    r
});

// `sloped like x axis`: the node turned to the axis' direction on paper, never upside down; it
// replaces rotations given before (`\pgfplotstransformtoaxisdirection`).
// Its options: `execute for upside down={…}` (kept for after the node's other options, as
// pgfplots runs it when the node is placed), `allow upside down`, `reset nontranslations=false`.
fn sloped_like(e: &mut Engine, a: usize, v: Option<Vec<Token>>) -> Result<(), Error> {
    let Some(f) = e.frame.clone() else { return Ok(()) };
    let (code, allow, reset) = super::smith::sloped_options(&v.unwrap_or_default());
    // the direction at the tick being labelled (a smith chart's circle), else the axis'
    let d = f.slope.get().unwrap_or_else(|| f.e[a].scale(if f.max[a] < f.min[a] { -1.0 } else { 1.0 }));
    if d.norm() < 1e-12 {
        return Ok(());
    }
    let mut d = d;
    if d.x < 0.0 {
        if let Some(code) = code {
            // `\tikzset{anchor=…}`: its keys
            let t = crate::keys::trim(&code).to_vec();
            let keys = match t.split_first() {
                Some((first, rest)) if first.is_cs("tikzset") => crate::tex::strip_outer_braces(crate::keys::trim(rest).to_vec()),
                _ => t,
            };
            f.upside_down.replace(Some(keys));
        }
        if !allow {
            d = d.scale(-1.0);
        }
    }
    let p = e.st.pending;
    let base = if reset { crate::scene::Transform { a: 1.0, b: 0.0, c: 0.0, d: 1.0, ..p } } else { p };
    e.st.pending = base.compose(&crate::scene::Transform::rotation(d.y.atan2(d.x).to_degrees()));
    Ok(())
}
keyfn!(k_sloped_x, |e, v| sloped_like(e, 0, v));
keyfn!(k_sloped_y, |e, v| sloped_like(e, 1, v));
keyfn!(k_sloped_z, |e, v| sloped_like(e, 2, v));

// `intersection segments={of=A and B, sequence=…}` (the fillbetween library)
keyfn!(k_intersection_segments, |e, v| super::fillbetween::intersection_segments(e, v));

// `tieline={…}`: the curve through the ties' ends, and the ties.
keyfn!(k_tieline, |e, v| {
    set(e, "/tikz/@plot handler", "tieline");
    let saved = e.st.key_path.clone();
    let r = e.process_keys(v.unwrap_or_default(), Some("/pgfplots/tieline"));
    e.st.key_path = saved;
    r
});

// `quiver={u=…, v=…}`: arrows from each point by (u, v, w).
keyfn!(k_quiver, |e, v| {
    set(e, "/pgfplots/@quiver", "true");
    set(e, "/tikz/@plot handler", "quiver");
    let saved = e.st.key_path.clone();
    let r = e.process_keys(v.unwrap_or_default(), Some("/pgfplots/quiver"));
    e.st.key_path = saved;
    r
});

// `hist={bins=…}`: the plot's values are counted in bins, then drawn with `hist/handler`.
keyfn!(k_hist, |e, v| {
    set(e, "/pgfplots/@hist", "true");
    let saved = e.st.key_path.clone();
    let r = e.process_keys(v.unwrap_or_default(), Some("/pgfplots/hist"));
    e.st.key_path = saved;
    r?;
    run(e, "/pgfplots/hist/handler")
});

// `boxplot={…}`, `boxplot prepared={…}`: the box plot handler, its keys in /pgfplots/boxplot.
keyfn!(k_boxplot, |e, v| {
    let name = current_name(e);
    set(e, "/pgfplots/@boxplot", if name.ends_with("prepared") { "prepared" } else { "raw" });
    set(e, "/tikz/@plot handler", "boxplot");
    run(e, "/pgfplots/every boxplot")?;
    let saved = e.st.key_path.clone();
    let r = e.process_keys(v.unwrap_or_default(), Some("/pgfplots/boxplot"));
    e.st.key_path = saved;
    r?;
    if flag(e, "/pgfplots/boxplot/ensure mark") && e.st.plot_mark.is_none() {
        run(e, "/tikz/mark=*")?;
    }
    Ok(())
});

// `group style={…}`: keys of /pgfplots/group.
keyfn!(k_group_style, |e, v| {
    let saved = e.st.key_path.clone();
    let r = e.process_keys(v.unwrap_or_default(), Some("/pgfplots/group"));
    e.st.key_path = saved;
    r
});

// `group size=2 by 3`: columns by rows.
keyfn!(k_group_size, |e, v| {
    let s = text(e, &v)?;
    let (c, r) = s.split_once("by").unwrap_or((s.as_str(), "1"));
    set(e, "/pgfplots/group/columns", c.trim());
    set(e, "/pgfplots/group/rows", r.trim());
    Ok(())
});

keyfn!(k_set_meta, |e, v| {
    if keys::value(&e.tex, "/pgfplots/point meta").is_none_or(|m| keys::trim(&m).is_empty() || tex::tokens_to_string(&m).trim() == "none") {
        let value = v.unwrap_or_default();
        e.tex.set_meaning(&Token::cs(&keys::macro_name("/pgfplots/point meta")), super::super::macro_tokens(value), false);
    }
    Ok(())
});

// ------------------------------------------------------------------ /pgf/number format

fn nf(e: &mut Engine) -> &mut super::number::NumberFormat {
    &mut e.st.number_format
}

keyfn!(k_nf, |e, v| {
    let name = current_name(e);
    let value = text(e, &v)?;
    let on = |s: &str| !(s == "false");
    let f = nf(e);
    match name.as_str() {
        "fixed" => f.style = Style::Fixed,
        "frac" => f.style = Style::Frac,
        "sci" => f.style = Style::Sci,
        "std" => {
            f.style = match value.split_once(':') {
                Some((a, b)) => Style::Std { lower: a.trim().parse().ok(), upper: b.trim().parse().unwrap_or(4) },
                None if value.is_empty() => Style::Std { lower: None, upper: 4 },
                None => Style::Std { lower: value.trim().parse().ok(), upper: 4 },
            }
        }
        "int detect" => f.style = Style::IntDetect,
        "int trunc" => f.style = Style::IntTrunc,
        "relative" | "relative*" => f.style = Style::Relative(value.trim().parse().unwrap_or(0)),
        "@fixed relative" => f.style = Style::FixedRelative,
        "precision" => f.precision = value.trim().parse::<f64>().map_or(2, |p| p.max(0.0) as usize),
        "sci precision" => f.sci_precision = value.trim().parse().ok(),
        "fixed zerofill" => f.fixed_zerofill = on(&value),
        "sci zerofill" => f.sci_zerofill = on(&value),
        "zerofill" => {
            f.fixed_zerofill = on(&value);
            f.sci_zerofill = on(&value);
        }
        "set thousands separator" | "1000 sep" => f.thousands_sep = value,
        "set decimal separator" | "dec sep" => f.dec_sep = value,
        "use comma" => {
            f.dec_sep = "{,}".into();
            f.thousands_sep = ".".into();
        }
        "use period" => {
            f.dec_sep = ".".into();
            f.thousands_sep = "{,}".into();
        }
        "min exponent for 1000 sep" => f.min_exponent_1000 = value.trim().parse().unwrap_or(0),
        "showpos" | "print sign" => f.showpos = on(&value),
        "skip 0." => f.skip_zero = on(&value),
        "sci 10^e" | "sci 10e" => f.sci_style = SciStyle::TenE,
        "sci e" => f.sci_style = SciStyle::LowerE,
        "sci E" => f.sci_style = SciStyle::UpperE,
        "sci subscript" => f.sci_style = SciStyle::Subscript,
        "sci superscript" => f.sci_style = SciStyle::Superscript,
        "retain unit mantissa" => f.unit_mantissa = on(&value),
        _ => {}
    }
    Ok(())
});

const NATIVE: &[(&str, KeyFn)] = &[
    ("/pgfplots/.unknown", k_unknown),
    ("/pgfplots/table/.unknown", k_table_unknown),
    ("/pgfplots/compat", k_compat),
    ("/pgfplots/compat/pgfpoint substitution", k_pgfpoint_substitution),
    ("/pgfplots/visualization depends on", k_visualization_depends_on),
    ("/pgfplots/mesh", k_mesh),
    ("/pgfplots/surf", k_mesh),
    ("/pgfplots/patch", k_mesh),
    ("/pgfplots/scatter", k_scatter),
    ("/pgfplots/set point meta if empty", k_set_meta),
    ("/pgfplots/axis x line", k_line),
    ("/pgfplots/axis y line", k_line),
    ("/pgfplots/axis z line", k_line),
    ("/pgfplots/axis x line*", k_line_star),
    ("/pgfplots/axis y line*", k_line_star),
    ("/pgfplots/axis z line*", k_line_star),
    ("/pgfplots/group style", k_group_style),
    ("/pgfplots/hist", k_hist),
    ("/pgfplots/quiver", k_quiver),
    ("/pgfplots/tieline", k_tieline),
    ("/tikz/intersection segments", k_intersection_segments),
    ("/tikz/sloped like x axis", k_sloped_x),
    ("/tikz/sloped like y axis", k_sloped_y),
    ("/tikz/sloped like z axis", k_sloped_z),
    ("/pgfplots/contour prepared", k_contour_prepared),
    ("/pgfplots/contour filled", k_contour_filled),
    ("/pgfplots/boxplot", k_boxplot),
    ("/pgfplots/boxplot prepared", k_boxplot),
    ("/pgfplots/colormap/.unknown", k_colormap_unknown),
    ("/pgfplots/x SI prefix", k_si_prefix),
    ("/pgfplots/y SI prefix", k_si_prefix),
    ("/pgfplots/z SI prefix", k_si_prefix),
    ("/pgfplots/@colormap defined", k_colormap_defined),
    ("/pgfplots/color of colormap", k_colormap_color),
    ("/pgfplots/const color of colormap", k_colormap_color),
    ("/pgfplots/index of colormap", k_colormap_color),
    ("/tikz/color of colormap", k_colormap_color),
    ("/tikz/const color of colormap", k_colormap_color),
    ("/tikz/index of colormap", k_colormap_color),
    ("/pgfplots/group/group size", k_group_size),
    ("/tikz/smooth", k_handler),
    ("/tikz/sharp plot", k_handler),
    ("/tikz/const plot", k_handler),
    ("/tikz/const plot mark left", k_handler),
    ("/tikz/const plot mark right", k_handler),
    ("/tikz/const plot mark mid", k_handler),
    ("/tikz/jump mark left", k_handler),
    ("/tikz/jump mark right", k_handler),
    ("/tikz/jump mark mid", k_handler),
    ("/tikz/ycomb", k_handler),
    ("/tikz/xcomb", k_handler),
    ("/tikz/ybar", k_handler),
    ("/tikz/xbar", k_handler),
    ("/tikz/ybar interval", k_handler),
    ("/tikz/xbar interval", k_handler),
    ("/tikz/polar comb", k_handler),
];

const NUMBER_FORMAT: &[&str] = &[
    "frac",
    "fixed",
    "sci",
    "std",
    "int detect",
    "int trunc",
    "relative",
    "relative*",
    "@fixed relative",
    "precision",
    "sci precision",
    "fixed zerofill",
    "sci zerofill",
    "zerofill",
    "set thousands separator",
    "1000 sep",
    "set decimal separator",
    "dec sep",
    "use comma",
    "use period",
    "min exponent for 1000 sep",
    "showpos",
    "print sign",
    "skip 0.",
    "sci 10^e",
    "sci 10e",
    "sci e",
    "sci E",
    "sci subscript",
    "sci superscript",
    "retain unit mantissa",
];

fn native(e: &mut Engine, path: &str, f: KeyFn) {
    let id = NATIVE_KEY_BASE + e.key_fns.len() as u16;
    e.key_fns.push(f);
    e.tex.define(&keys::macro_name(&format!("{path}/.@cmd")), Meaning::Native(id));
}

/// Defines the pgfplots keys (after TikZ's).
pub fn define(e: &mut Engine) {
    // what the colormap gives each point; set while a plot is drawn
    e.colors.insert("mapped color".into(), crate::color::Color::rgb(0.0, 0.0, 0.0));
    for (path, f) in NATIVE {
        native(e, path, *f);
    }
    for name in NUMBER_FORMAT {
        native(e, &format!("/pgf/number format/{name}"), k_nf);
    }
    for (k, v) in [
        ("fixed zerofill", "true"),
        ("sci zerofill", "true"),
        ("zerofill", "true"),
        ("showpos", "true"),
        ("print sign", "true"),
        ("skip 0.", "true"),
        ("retain unit mantissa", "true"),
    ] {
        e.tex.set_meaning(&Token::cs(&keys::macro_name(&format!("/pgf/number format/{k}/.@def"))), super::super::macro_from(v), false);
    }
    let saved = e.st.key_path.clone();
    e.st.key_path = "/pgfplots".into();
    let mut lexer = crate::tex::Tex::new_lexer();
    lexer.push_text(definitions());
    let mut tokens = Vec::new();
    while let Some(t) = lexer.get_token() {
        tokens.push(t);
    }
    // (tikra's own text, run when the engine starts: before any document code)
    e.run_tokens(tokens).expect("pgfplots' definitions run");
    e.tex.warnings.clear();
    e.st.key_path = saved;
}
