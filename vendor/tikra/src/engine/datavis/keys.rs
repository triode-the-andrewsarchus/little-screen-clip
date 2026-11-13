//! The keys of data visualizations that do something: axes and their options, the actions that
//! draw them, visualizers, legends, labels in data, style sheets and the phases' hooks. They are
//! native here; the keys that only hold values or name a list of other keys are in [`super::library`].
//!
//! Keys that many share one function (`x axis`, `y axis`… or the visualizers' names) tell
//! themselves apart by the key's path (`Engine::current_key`).

use crate::keys::{self, macro_name};
use crate::scene::Point;
use crate::tex::{self, Cat, Meaning, Token};
use crate::{Error, PT};

use super::{group, key_text, macro_text, set_key, Engine, Hooks, Object, Signal, Slot};
use crate::engine::{macro_tokens, options::KeyFn, tikra_tokens};

/// The data visualization keys' root.
pub const DV: &str = "/tikz/data visualization";

/// Native keys, by path, and their defaults.
pub const NATIVE: &[(&str, KeyFn)] = &[
    ("/tikz/data visualization/.unknown", k_unknown),
    ("/tikz/data visualization/data/.unknown", k_data_unknown),
    ("/tikz/data visualization/new object", k_new_object),
    ("/tikz/data visualization/before survey", k_hook),
    ("/tikz/data visualization/at start survey", k_hook),
    ("/tikz/data visualization/at end survey", k_hook),
    ("/tikz/data visualization/after survey", k_hook),
    ("/tikz/data visualization/before visualization", k_hook),
    ("/tikz/data visualization/at start visualization", k_hook),
    ("/tikz/data visualization/at end visualization", k_hook),
    ("/tikz/data visualization/after visualization", k_hook),
    ("/tikz/data visualization/data point", k_data_point),
    ("/tikz/data visualization/new axis base", k_new_axis_base),
    ("/tikz/data visualization/new Cartesian axis", k_new_cartesian_axis),
    ("/tikz/data visualization/new polar axes", k_new_polar_axes),
    ("/tikz/data visualization/all axes", k_all_axes),
    ("/tikz/data visualization/new axis system", k_new_axis_system),
    ("/tikz/data visualization/common", k_setter),
    ("/tikz/data visualization/major", k_setter),
    ("/tikz/data visualization/minor", k_setter),
    ("/tikz/data visualization/subminor", k_setter),
    ("/tikz/data visualization/node styling", k_node_styling),
    ("/tikz/data visualization/visualize as line", k_visualize_as),
    ("/tikz/data visualization/visualize as smooth line", k_visualize_as),
    ("/tikz/data visualization/visualize as smooth cycle", k_visualize_as),
    ("/tikz/data visualization/visualize as scatter", k_visualize_as),
    ("/tikz/data visualization/visualize as rectangles", k_visualize_as),
    ("/tikz/data visualization/new visualizer", k_new_visualizer),
    ("/tikz/data visualization/style sheet", k_style_sheet),
    ("/tikz/data visualization/new legend", k_new_legend),
    ("/tikz/data visualization/legend", k_legend_style),
    ("/tikz/data visualization/new legend entry", k_new_legend_entry),
    ("/tikz/data visualization/@radius axes", k_radius_axes),
    ("/tikz/data visualization/axis options/attribute", k_axis_value),
    ("/tikz/data visualization/axis options/function", k_axis_value),
    ("/tikz/data visualization/axis options/scaling", k_axis_value),
    ("/tikz/data visualization/axis options/@default scaling", k_axis_value),
    ("/tikz/data visualization/axis options/padding min", k_axis_value),
    ("/tikz/data visualization/axis options/padding max", k_axis_value),
    ("/tikz/data visualization/axis options/tick placement strategy", k_axis_value),
    ("/tikz/data visualization/axis options/label", k_axis_label),
    ("/tikz/data visualization/axis options/goto", k_goto),
    ("/tikz/data visualization/axis options/goto pos", k_goto_pos),
    ("/tikz/data visualization/axis options/anchor at min", k_anchor_at),
    ("/tikz/data visualization/axis options/anchor at max", k_anchor_at),
    ("/tikz/data visualization/axis options/visualize axis", k_visualize_part),
    ("/tikz/data visualization/axis options/visualize ticks", k_visualize_part),
    ("/tikz/data visualization/axis options/visualize grid", k_visualize_part),
    ("/tikz/data visualization/axis options/visualize label", k_visualize_part),
    ("/tikz/data visualization/axis options/ticks", k_ticks_or_grid),
    ("/tikz/data visualization/axis options/grid", k_ticks_or_grid),
    ("/tikz/data visualization/axis options/ticks and grid", k_ticks_or_grid),
    ("/tikz/data visualization/axis options/include value", k_include_value),
    ("/tikz/data visualization/axis options/min value", k_range_end),
    ("/tikz/data visualization/axis options/max value", k_range_end),
    ("/tikz/data visualization/axis options/unit vector", k_unit_vector),
    ("/tikz/data visualization/axis options/unit vectors", k_unit_vectors),
    ("/tikz/data visualization/axis options/unit length", k_unit_length),
    ("/tikz/data visualization/visualizer options/@handler", k_handler),
    ("/tikz/data visualization/visualizer options/label in legend", k_label_in_legend),
    ("/tikz/data visualization/visualizer options/label in data", k_label_in_data),
    ("/tikz/data visualization/visualizer options/ignore style sheets", k_ignore_style_sheets),
    ("/tikz/data visualization/visualizer label options/when", k_label_when),
    ("/tikz/data visualization/visualizer label options/index", k_label_at),
    ("/tikz/data visualization/visualizer label options/pos", k_label_at),
    ("/tikz/data visualization/visualizer label options/auto", k_label_at),
    ("/handlers/.style sheet", k_style_sheet_handler),
    ("/data point/.unknown", k_data_point_unknown),
];

/// Defaults of the native keys used without a value.
pub const DEFAULTS: &[(&str, &str)] = &[
    ("/tikz/data visualization/new polar axes", "{angle axis}{radius axis}"),
    ("/tikz/data visualization/visualize as line", "line"),
    ("/tikz/data visualization/visualize as smooth line", "line"),
    ("/tikz/data visualization/visualize as smooth cycle", "line"),
    ("/tikz/data visualization/visualize as scatter", "scatter"),
    ("/tikz/data visualization/visualize as rectangles", "rectangles"),
    ("/tikz/data visualization/new legend", "main legend"),
    ("/tikz/data visualization/axis options/ticks", "some"),
    ("/tikz/data visualization/axis options/grid", "at default ticks"),
];

/// The base options of each kind of visualizer (and whether it draws a line), its handler, the
/// sample its legend entries show, and when its object is made.
struct Kind {
    key: &'static str,
    options: &'static str,
    handlers: &'static str,
    handler: Option<&'static str>,
    sample: &'static str,
}

const KINDS: &[Kind] = &[
    Kind {
        key: "visualize as line",
        options: "every path/.style={draw},style={every mark/.append style={color=visualizer color}},mark size=2pt,semithick,color=visualizer color,mark=none,/tikz/data visualization/every visualize as line/.try",
        handlers: "\\tikra@dv@handler,\\tikra@dv@marks",
        handler: Some("straight line"),
        sample: "\\tikra@dv@sample",
    },
    Kind {
        key: "visualize as smooth line",
        options: "",
        handlers: "",
        handler: Some("smooth line"),
        sample: "",
    },
    Kind {
        key: "visualize as smooth cycle",
        options: "",
        handlers: "",
        handler: Some("smooth cycle"),
        sample: "",
    },
    Kind {
        key: "visualize as scatter",
        options: "mark=x,mark size=2pt,style={every mark/.append style={color=visualizer color}},/tikz/data visualization/every visualize as scatter/.try",
        handlers: "\\tikra@dv@marks",
        handler: Some("no lines"),
        sample: "\\tikra@dv@sample",
    },
    Kind {
        key: "visualize as rectangles",
        options: "every path/.style={draw},semithick,color=visualizer color,/tikz/data visualization/every visualize as rectangles/.try",
        handlers: "",
        handler: None,
        sample: "\\tikra@dv@rectsample",
    },
];

fn name_of(e: &Engine) -> String {
    keys::name(&e.current_key).to_string()
}

fn text_of(e: &mut Engine, v: Option<Vec<Token>>) -> Result<String, Error> {
    Ok(e.tex.expand_to_string(v.unwrap_or_default())?.trim().to_string())
}

/// Runs keys with `/tikz/data visualization` as the path (`\tikzdatavisualizationset`).
pub fn dv_keys(e: &mut Engine, v: Vec<Token>) -> Result<(), Error> {
    e.keys_command(v, Some(DV), true)
}

fn dv_text(e: &mut Engine, s: &str) -> Result<(), Error> {
    dv_keys(e, tikra_tokens(s))
}

// ---------------------------------------------------------------- generic keys

/// An unknown key of `/tikz/data visualization` is tried as a TikZ key.
fn k_unknown(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    let name = macro_text(e, "pgfkeyscurrentname");
    let path = format!("/tikz/{name}");
    if e.run_key(&path, v.clone())? {
        return Ok(());
    }
    let old = std::mem::replace(&mut e.st.key_path, "/tikz".into());
    let r = e.invoke(&path, v);
    e.st.key_path = old;
    r
}

/// `data/format=function` and the other keys of `/tikz/data visualization/data`: `/pgf/data` keys.
fn k_data_unknown(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    let name = macro_text(e, "pgfkeyscurrentname");
    let old = std::mem::replace(&mut e.st.key_path, "/pgf/data".into());
    let r = e.invoke(&format!("/pgf/data/{name}"), v);
    e.st.key_path = old;
    r
}

/// `/data point/x=5` for any attribute: the value is kept.
fn k_data_point_unknown(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    let name = macro_text(e, "pgfkeyscurrentname");
    let v = v.unwrap_or_else(|| vec![Token::cs("pgfkeysnovalue")]);
    e.tex.set_meaning(&Token::cs(&macro_name(&format!("/data point/{name}"))), macro_tokens(v), false);
    Ok(())
}

/// Code for one of the phases (`before survey={…}`…): the first four `at start`/`before` add at
/// the end, the others at the front, as the data visualization class keeps them.
fn k_hook(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    let name = name_of(e);
    add_hook(e, &name, v.unwrap_or_default())
}

pub fn add_hook(e: &mut Engine, name: &str, code: Vec<Token>) -> Result<(), Error> {
    let Some(dv) = e.datavis.last_mut() else { return Ok(()) };
    let h: &mut Hooks = &mut dv.hooks;
    let (list, front) = match name {
        "before survey" => (&mut h.before_survey, false),
        "at start survey" => (&mut h.at_start_survey, false),
        "at end survey" => (&mut h.at_end_survey, true),
        "after survey" => (&mut h.after_survey, true),
        "before visualization" => (&mut h.before_visualization, false),
        "at start visualization" => (&mut h.at_start_visualization, false),
        "at end visualization" => (&mut h.at_end_visualization, true),
        _ => (&mut h.after_visualization, true),
    };
    if front {
        let old = std::mem::take(list);
        list.extend(code);
        list.extend(old);
    } else {
        list.extend(code);
    }
    Ok(())
}

/// A native command and its arguments, as code for a hook.
fn command(name: &str, args: &[Vec<Token>]) -> Vec<Token> {
    let mut v = vec![Token::cs(name)];
    for a in args {
        v.extend(group(a.clone()));
    }
    v
}

fn text_arg(s: &str) -> Vec<Token> {
    tex::string_tokens(s)
}

/// `new object={…}`: created at its `when` (before the survey by default).
fn k_new_object(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    let spec = v.unwrap_or_default();
    let mut store = String::new();
    let mut when = "before survey".to_string();
    for item in keys::split_list(&spec) {
        let value = e.tex.expand_to_string(item.value.unwrap_or_default())?.trim().to_string();
        match item.key.as_str() {
            "store" => store = value,
            "when" => when = value,
            _ => {}
        }
    }
    if !store.is_empty() {
        set_key(e, &store, "");
    }
    let code = command("tikra@dv@newobject", &[text_arg(&store), spec]);
    add_hook(e, &when, code)
}

/// `data point={x=1, y=2}`: one data point, added to the data.
fn k_data_point(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    let code = super::data_point_code(v.unwrap_or_default());
    e.do_add_data(code)
}

// ---------------------------------------------------------------- axes

/// The axis whose options are being set.
pub fn current_axis(e: &mut Engine) -> String {
    macro_text(e, "tikra@dv@axis")
}

fn axis_key(e: &mut Engine, sub: &str) -> String {
    format!("{}/{sub}", current_axis(e))
}

/// `new axis base=name`: an axis' keys, the key `name` for its options, and its scaling mapper,
/// made before the survey.
fn k_new_axis_base(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    let name = text_of(e, v)?;
    new_axis(e, &name)
}

fn new_axis(e: &mut Engine, name: &str) -> Result<(), Error> {
    let axis = format!("{DV}/{name}");
    for (sub, value) in [
        ("attribute", name),
        ("function", ""),
        ("scaling", ""),
        ("scaling/default", "0 at 0 and 1 at 1"),
        ("ticks at", ""),
        ("default ticks at", ""),
        ("grid at", ""),
        ("default grid at", ""),
        ("label", ""),
        ("padding min", ""),
        ("padding max", ""),
        ("tick placement strategy", "linear"),
    ] {
        set_key(e, &format!("{axis}/{sub}"), value);
    }
    define_native(e, &axis, k_axis);
    if let Some(dv) = e.datavis.last_mut() {
        if !dv.axes.iter().any(|a| a == name) {
            dv.axes.push(name.to_string());
        }
    }
    add_hook(e, "before survey", command("tikra@dv@make", &[text_arg("mapper"), text_arg(&axis)]))
}

/// Makes `path` a key with native code `f` (for keys defined while running, like axes).
pub fn define_native(e: &mut Engine, path: &str, f: KeyFn) {
    let id = match e.key_fns.iter().position(|g| *g as usize == f as usize) {
        Some(i) => super::NATIVE_KEY_BASE + i as u16,
        None => {
            e.key_fns.push(f);
            super::NATIVE_KEY_BASE + (e.key_fns.len() - 1) as u16
        }
    };
    e.tex.define(&macro_name(&format!("{path}/.@cmd")), Meaning::Native(id));
}

/// `reset cm` in a legend's cell: back to the cell's origin, as TeX's cell box keeps it.
pub fn k_cell_reset_cm(e: &mut Engine, _v: Option<Vec<Token>>) -> Result<(), Error> {
    let t = e.st.transform;
    e.st.transform = crate::scene::Transform { a: 1.0, b: 0.0, c: 0.0, d: 1.0, ..t };
    e.st.pending = crate::scene::Transform::default();
    Ok(())
}

/// `new Cartesian axis=name`: an axis whose scaled attribute moves along its unit vector.
fn k_new_cartesian_axis(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    let name = text_of(e, v)?;
    new_axis(e, &name)?;
    let axis = format!("{DV}/{name}");
    set_key(e, &format!("{axis}/scaling/default"), "0 at 0 and 1 at 1cm");
    set_key(e, &format!("{axis}/unit vector"), "\\pgfqpoint{1pt}{0pt}");
    add_hook(e, "before survey", command("tikra@dv@make", &[text_arg("linear"), text_arg(&axis)]))
}

/// `new polar axes={angle axis}{radius axis}`: two axes and their polar transformer.
fn k_new_polar_axes(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    let args = crate::engine::options::split_arguments(&v.unwrap_or_default(), 2);
    let angle = e.tex.expand_to_string(args[0].clone())?.trim().to_string();
    let radius = e.tex.expand_to_string(args[1].clone())?.trim().to_string();
    new_axis(e, &angle)?;
    new_axis(e, &radius)?;
    let (a, r) = (format!("{DV}/{angle}"), format!("{DV}/{radius}"));
    set_key(e, &format!("{a}/attribute"), "angle");
    set_key(e, &format!("{r}/attribute"), "radius");
    set_key(e, &format!("{r}/scaling/default"), "0 at 0 and 1 at 1cm");
    set_key(e, &format!("{a}/unit vector 0 degrees"), "\\pgfqpoint{1pt}{0pt}");
    set_key(e, &format!("{a}/unit vector 90 degrees"), "\\pgfqpoint{0pt}{1pt}");
    add_hook(e, "before survey", command("tikra@dv@make", &[text_arg("polar"), text_arg(&a), text_arg(&r)]))
}

/// `x axis={options}` (and every other axis): the options with `axis options` as the path.
fn k_axis(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    let axis = e.current_key.clone();
    let old = e.tex.meaning_of("tikra@dv@axis");
    e.tex.define_string("tikra@dv@axis", &axis, false);
    let r = e.keys_command(v.unwrap_or_default(), Some(&format!("{DV}/axis options")), true);
    e.tex.set_meaning(&Token::cs("tikra@dv@axis"), old, false);
    r
}

/// `all axes={options}`: the options for each axis made so far.
fn k_all_axes(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    let v = v.unwrap_or_default();
    let axes = e.datavis.last().map(|d| d.axes.clone()).unwrap_or_default();
    for a in axes {
        let mut item = tikra_tokens(&format!("{a}="));
        item.extend(group(v.clone()));
        dv_keys(e, item)?;
    }
    Ok(())
}

/// `new axis system={name}{axes}{default options}{after}`: the style `name={options}` sets the axes
/// up, then the default options, `every name`, the options, and what comes after.
fn k_new_axis_system(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    let args = crate::engine::options::split_arguments(&v.unwrap_or_default(), 4);
    let name = e.tex.expand_to_string(args[0].clone())?.trim().to_string();
    let mut body = args[1].clone();
    body.extend(tikra_tokens(&format!(",{name}/.cd,")));
    body.extend(args[2].clone());
    body.extend(tikra_tokens(&format!(",{DV}/every {name}/.try,")));
    body.push(Token::Char('#', Cat::Param));
    body.push(Token::other('1'));
    body.extend(tikra_tokens(&format!(",{DV}/.cd,")));
    body.extend(args[3].clone());
    let mut def = tikra_tokens(&format!("{name}/.style="));
    def.extend(group(body));
    dv_keys(e, def)
}

/// `common=`, `major=`, `minor=`, `subminor=`: options for those ticks, gathered (but ignored while
/// a placement strategy is computing the positions).
fn k_setter(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    if e.datavis.last().is_some_and(|d| d.setters_off) {
        return Ok(());
    }
    let key = e.current_key.clone();
    let mut old = keys::value(&e.tex, &key).unwrap_or_default();
    old.push(Token::other(','));
    old.extend(v.unwrap_or_default());
    e.tex.set_meaning(&Token::cs(&macro_name(&key)), macro_tokens(old), false);
    Ok(())
}

/// `node styling`: the node styles gathered by `node style=…`, as TikZ options.
fn k_node_styling(e: &mut Engine, _v: Option<Vec<Token>>) -> Result<(), Error> {
    let v = keys::value(&e.tex, &format!("{DV}/@node styling")).unwrap_or_default();
    let old = std::mem::replace(&mut e.st.key_path, "/tikz".into());
    let r = e.keys_command(v, Some("/tikz"), true);
    e.st.key_path = old;
    r
}

/// The axis options that set one of the axis' keys: `attribute=`, `scaling=`, `padding min=`…
fn k_axis_value(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    let sub = match name_of(e).as_str() {
        "@default scaling" => "scaling/default".to_string(),
        n => n.to_string(),
    };
    let key = axis_key(e, &sub);
    let v = v.unwrap_or_default();
    // expanded where the value is used (lengths, scalings); kept as given otherwise
    e.tex.set_meaning(&Token::cs(&macro_name(&key)), macro_tokens(v), false);
    Ok(())
}

/// `label=text` (`label` alone: the attribute's name in math italics).
fn k_axis_label(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    let v = match v {
        Some(v) => v,
        None => {
            let a = {
                let k = axis_key(e, "attribute");
                key_text(e, &k)
            }
            .unwrap_or_default();
            tikra_tokens(&format!("$\\mathit{{{a}}}$"))
        }
    };
    let key = axis_key(e, "label");
    e.tex.set_meaning(&Token::cs(&macro_name(&key)), macro_tokens(v), false);
    Ok(())
}

fn k_goto(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    let axis = current_axis(e);
    let value = text_of(e, v)?;
    e.goto(&axis, &value)
}

fn k_goto_pos(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    let a = {
        let k = axis_key(e, "attribute");
        key_text(e, &k)
    }
    .unwrap_or_default();
    let value = text_of(e, v)?;
    set_key(e, &format!("/data point/{a}/out pos"), &value);
    Ok(())
}

fn k_anchor_at(e: &mut Engine, _v: Option<Vec<Token>>) -> Result<(), Error> {
    let which = if name_of(e).ends_with("min") { "min" } else { "max" };
    let axis = current_axis(e);
    e.anchor_at(&axis, which)
}

/// `visualize axis=`, `visualize ticks=`, `visualize grid=`, `visualize label=`: drawn after the
/// data, with these options.
fn k_visualize_part(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    let what = name_of(e);
    let axis = current_axis(e);
    let v = v.unwrap_or_default();
    let dv = e.dv()?;
    let list = match what.as_str() {
        "visualize axis" => &mut dv.actions.axes,
        "visualize ticks" => &mut dv.actions.ticks,
        "visualize grid" => &mut dv.actions.grids,
        _ => &mut dv.actions.labels,
    };
    list.push((axis, v));
    Ok(())
}

/// `ticks=…`, `grid=…`, `ticks and grid=…`: added to the axis' tick (grid) settings.
fn k_ticks_or_grid(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    let what = name_of(e);
    let v = v.unwrap_or_default();
    let subs: &[&str] = match what.as_str() {
        "ticks" => &["ticks at"],
        "grid" => &["grid at"],
        _ => &["ticks at", "grid at"],
    };
    for sub in subs {
        let key = axis_key(e, sub);
        let mut old = keys::value(&e.tex, &key).unwrap_or_default();
        old.push(Token::other(','));
        old.extend(v.clone());
        e.tex.set_meaning(&Token::cs(&macro_name(&key)), macro_tokens(old), false);
    }
    Ok(())
}

/// `include value=0,1`: before the survey, the values count as seen.
fn k_include_value(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    let axis = current_axis(e);
    add_hook(e, "before survey", command("tikra@dv@include", &[text_arg(&axis), v.unwrap_or_default()]))
}

/// `min value=…` and `max value=…`: at the end of the survey, the range is set.
fn k_range_end(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    let which = if name_of(e).starts_with("min") { "min" } else { "max" };
    let axis = current_axis(e);
    add_hook(e, "at end survey", command("tikra@dv@setrange", &[text_arg(&axis), v.unwrap_or_default(), text_arg(which)]))
}

/// A TikZ point without the current transformation, in pt.
pub fn scan_point(e: &mut Engine, v: Vec<Token>) -> Result<Point, Error> {
    let s = e.tex.expand_to_string(v)?;
    let s = s.trim();
    let inner = s.strip_prefix('(').and_then(|r| r.strip_suffix(')')).unwrap_or(s).to_string();
    e.begin_group();
    e.st.transform = crate::scene::Transform::default();
    let r = e.coordinate(&inner);
    e.end_group();
    let p = r?.p;
    Ok(Point::new(p.x / PT, p.y / PT))
}

pub fn qpoint(p: Point) -> String {
    format!("\\pgfqpoint{{{}pt}}{{{}pt}}", super::num(p.x), super::num(p.y))
}

fn k_unit_vector(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    let p = scan_point(e, v.unwrap_or_default())?;
    let key = axis_key(e, "unit vector");
    set_key(e, &key, &qpoint(p));
    Ok(())
}

/// `unit vectors={(a)}{(b)}`: a polar axis' vectors at 0 and 90 degrees.
fn k_unit_vectors(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    let args = crate::engine::options::split_arguments(&v.unwrap_or_default(), 2);
    let a = scan_point(e, args[0].clone())?;
    let b = scan_point(e, args[1].clone())?;
    let (ka, kb) = (axis_key(e, "unit vector 0 degrees"), axis_key(e, "unit vector 90 degrees"));
    set_key(e, &ka, &qpoint(a));
    set_key(e, &kb, &qpoint(b));
    Ok(())
}

/// `unit length=1cm per 10 units`: 10 units of the attribute are 1cm.
fn k_unit_length(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    let s = text_of(e, v)?;
    let (length, units) = match s.split_once("per") {
        Some((l, u)) => (l.trim().to_string(), u.trim().trim_end_matches("units").trim().to_string()),
        None => (s.trim().to_string(), "1".to_string()),
    };
    let key = axis_key(e, "scaling");
    set_key(e, &key, &format!("0 at 0 and {{{units}}} at {{{length}}}"));
    Ok(())
}

/// For polar axes: the radius axes drawn at the angles the range's style lists.
fn k_radius_axes(e: &mut Engine, _v: Option<Vec<Token>>) -> Result<(), Error> {
    let which = key_text(e, &format!("{DV}/@which radius axes at")).unwrap_or_default();
    let style = key_text(e, &format!("{DV}/@which visualize radius axis")).unwrap_or_default();
    let list = keys::value(&e.tex, &format!("{DV}/{which}")).unwrap_or_default();
    // (the ranges list no empty item)
    for item in super::axes_split(&list).into_iter().filter(|i| !keys::trim(i).is_empty()) {
        let mut v = tikra_tokens(&format!("{style}="));
        v.extend(group(item));
        dv_keys(e, v)?;
    }
    Ok(())
}

// ---------------------------------------------------------------- visualizers

/// `visualize as line=name` and the other kinds: a visualizer, its object and its handler.
fn k_visualize_as(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    let kind = name_of(e);
    let name = text_of(e, v)?;
    let Some(k) = KINDS.iter().find(|k| k.key == kind) else { return Ok(()) };
    match kind.as_str() {
        "visualize as smooth line" => dv_text(e, &format!("visualize as line={{{name}}}"))?,
        "visualize as smooth cycle" => dv_text(e, &format!("visualize as smooth line={{{name}}}"))?,
        "visualize as rectangles" => {
            add_hook(e, "before survey", command("tikra@dv@make", &[text_arg("rectangle"), text_arg(&name)]))?;
            set_key(e, &format!("/data point/{name}/attribute 1"), "x");
            set_key(e, &format!("/data point/{name}/attribute 2"), "y");
            new_visualizer(e, &name, tikra_tokens(k.options), tikra_tokens(&format!("visualizer in legend={}", k.sample)))?;
        }
        _ => {
            add_hook(e, "after survey", command("tikra@dv@make", &[text_arg("plot"), text_arg(&name), tikra_tokens(k.handlers)]))?;
            new_visualizer(e, &name, tikra_tokens(k.options), tikra_tokens(&format!("visualizer in legend={}", k.sample)))?;
        }
    }
    // the visualizer's options key, run once even with no handler: it sets the visualizer up
    let h = k.handler.unwrap_or_default();
    dv_text(e, &format!("{name}={{{h}}}"))?;
    Ok(())
}

/// `new visualizer={name}{options}{legend options}`.
fn k_new_visualizer(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    let args = crate::engine::options::split_arguments(&v.unwrap_or_default(), 3);
    let name = e.tex.expand_to_string(args[0].clone())?.trim().to_string();
    new_visualizer(e, &name, args[1].clone(), args[2].clone())
}

/// A visualizer's keys: `name={options}`, its number for style sheets, the options of its scope,
/// its style signal, styling and legend options.
fn new_visualizer(e: &mut Engine, name: &str, options: Vec<Token>, legend: Vec<Token>) -> Result<(), Error> {
    define_native(e, &format!("{DV}/{name}"), k_visualizer);
    let n = {
        let dv = e.dv()?;
        dv.visualizers += 1;
        dv.visualizers
    };
    set_key(e, &format!("/data point/set/{name}"), &n.to_string());
    let base = format!("{DV}/visualizers/{name}");
    e.tex.set_meaning(&Token::cs(&macro_name(&format!("{base}/options"))), macro_tokens(options), false);
    let mut def = tikra_tokens(&format!("{base}/signal/.code=\\tikra@dv@stylesignal,{base}/styling/.style=,{base}/label in legend options/.style="));
    def.extend(group(legend));
    dv_keys(e, def)
}

/// `name={options}` of a visualizer: its options, and its data points from now on are its set.
fn k_visualizer(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    let name = e.current_key.trim_start_matches(DV).trim_start_matches('/').to_string();
    let old = e.tex.meaning_of("tikra@dv@visualizer");
    e.tex.define_string("tikra@dv@visualizer", &name, false);
    let r = e.keys_command(v.unwrap_or_default(), Some(&format!("{DV}/visualizer options")), true);
    e.tex.set_meaning(&Token::cs("tikra@dv@visualizer"), old, false);
    r?;
    let mut set = tikra_tokens("\\pgfkeys");
    set.extend(group(tikra_tokens(&format!("/data point/set={{{name}}}"))));
    e.do_add_data(set)
}

fn current_visualizer(e: &mut Engine) -> String {
    macro_text(e, "tikra@dv@visualizer")
}

/// `@handler={handler}{legend sample}`: how the visualizer joins its points, and the sample its
/// legend entry shows.
fn k_handler(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    let args = crate::engine::options::split_arguments(&v.unwrap_or_default(), 2);
    let handler = e.tex.expand_to_string(args[0].clone())?.trim().to_string();
    let name = current_visualizer(e);
    let base = format!("{DV}/visualizers/{name}");
    let mut v = tikra_tokens(&format!("{base}/styling/.append style={{/utils/exec={{\\def\\tikra@dv@handler{{{handler}}}}}}},{base}/label in legend options/.append style="));
    v.extend(group(args[1].clone()));
    dv_keys(e, v)
}

/// `ignore style sheets`: the visualizer's style signal does nothing, and it is not counted.
fn k_ignore_style_sheets(e: &mut Engine, _v: Option<Vec<Token>>) -> Result<(), Error> {
    let name = current_visualizer(e);
    dv_text(e, &format!("visualizers/{name}/signal/.code="))?;
    let dv = e.dv()?;
    dv.visualizers = dv.visualizers.saturating_sub(1);
    Ok(())
}

/// `style sheet=name`: the visualizers' style comes from the sheet, by their set.
fn k_style_sheet(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    let sheet = text_of(e, v)?;
    add_style_attribute(e, "/data point/set", &sheet)
}

/// `/data point/key/.style sheet=name`: the same for any attribute.
fn k_style_sheet_handler(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    let path = macro_text(e, "pgfkeyscurrentpath");
    let sheet = text_of(e, v)?;
    add_style_attribute(e, &path, &sheet)
}

fn add_style_attribute(e: &mut Engine, attribute: &str, sheet: &str) -> Result<(), Error> {
    let Some(dv) = e.datavis.last_mut() else { return Ok(()) };
    let id = dv.objects.len();
    dv.objects.push(Object::StyleAttribute { attribute: attribute.to_string(), sheet: sheet.to_string() });
    dv.connect(id, Slot::InvokeStyle, Signal::Style);
    Ok(())
}

// ---------------------------------------------------------------- labels in data

/// `label in data={options}`: a label where the visualizer's line passes a place (by default the
/// place of its number among the visualizers), and the count of its data points.
fn k_label_in_data(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    let name = current_visualizer(e);
    let mut options = tikra_tokens("auto,");
    options.extend(v.unwrap_or_default());
    add_hook(e, "before survey", command("tikra@dv@make", &[text_arg("count"), text_arg(&name)]))?;
    add_hook(e, "before visualization", command("tikra@dv@make", &[text_arg("label"), text_arg(&name), options]))
}

/// `when=attribute is value`.
fn k_label_when(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    let s = text_of(e, v)?;
    let (a, t) = s.split_once(" is").unwrap_or((&s, ""));
    e.tex.define_string("tikra@dv@labelattr", a.trim(), false);
    e.tex.define_string("tikra@dv@labelat", t.trim(), false);
    Ok(())
}

/// `index=n`, `pos=fraction`, `auto`: where along the visualizer's points the label goes.
fn k_label_at(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    let name = current_visualizer(e);
    e.tex.define_string("tikra@dv@labelattr", &format!("set/{name}/count"), false);
    let count = e.object_of(&format!("{DV}/visualizers/{name}/count/obj"));
    let total = if let Some(Object::Count { max, .. }) = count.and_then(|c| e.datavis.last().and_then(|d| d.objects.get(c)).cloned()) { max } else { 0.0 };
    let at = match name_of(e).as_str() {
        "index" => text_of(e, v)?,
        "pos" => {
            let p = text_of(e, v)?;
            format!("({p}*{})", super::num(total))
        }
        _ => {
            let n = key_text(e, &format!("/data point/set/{name}")).unwrap_or_else(|| "1".into());
            let all = e.datavis.last().map_or(1, |d| d.visualizers.max(1));
            format!("(({n}-0.5)/{all}*{})", super::num(total))
        }
    };
    e.tex.define_string("tikra@dv@labelat", &at, false);
    Ok(())
}

// ---------------------------------------------------------------- legends

/// `new legend=name`: a legend (once per data visualization), drawn after the axes, with its
/// options key `name={…}`; options set before the data visualization are kept.
fn k_new_legend(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    let name = text_of(e, v)?;
    let base = format!("{DV}/{name}");
    let defined = e.key_exists(&format!("{base}/at"));
    if let Some(dv) = e.datavis.last_mut() {
        if !dv.legends.contains(&name) {
            dv.legends.push(name.clone());
            add_hook(e, "before survey", command("tikra@dv@make", &[text_arg("legend"), text_arg(&name)]))?;
        }
    }
    if defined {
        return Ok(());
    }
    for (sub, value) in [
        ("@columns or rows", "columns"),
        ("@ideal", "1"),
        ("max entries", "100"),
        ("@strategy", "down then right"),
        ("anchor", "west"),
        ("at", "(data visualization bounding box.east)"),
    ] {
        set_key(e, &format!("{base}/{sub}"), value);
    }
    dv_text(e, &format!("{name}/matrix node styling/.style={{row sep=0pt,column sep=.8em}}"))?;
    define_native(e, &base, k_legend);
    dv_text(e, &format!("{name}={{every new legend}}"))
}

/// `legend={options}`: the main legend, with these options.
fn k_legend_style(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    dv_text(e, "new legend")?;
    let mut item = tikra_tokens("main legend=");
    item.extend(group(v.unwrap_or_default()));
    dv_keys(e, item)
}

/// A legend's options key: `legend options` with the legend as the one they set.
fn k_legend(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    let legend = e.current_key.clone();
    let old = e.tex.meaning_of("tikra@dv@legend");
    e.tex.define_string("tikra@dv@legend", &legend, false);
    let r = e.keys_command(v.unwrap_or_default(), Some(&format!("{DV}/legend options")), true);
    e.tex.set_meaning(&Token::cs("tikra@dv@legend"), old, false);
    r
}

/// `new legend entry={options}`: an entry of the legend the options name, added at the start of
/// the survey.
fn k_new_legend_entry(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    let options = v.unwrap_or_default();
    e.begin_group();
    let r = e.keys_command(options.clone(), Some(&format!("{DV}/legend entry options")), true);
    let legend = key_text(e, &format!("{DV}/legend entry options/legend")).unwrap_or_else(|| "main legend".into());
    e.end_group();
    r?;
    dv_text(e, &format!("new legend={{{legend}}}"))?;
    let mut entry = tikra_tokens(&format!("legend={{{legend}}},"));
    entry.extend(options);
    let code = command("tikra@dv@addentry", &[text_arg(&legend), entry]);
    add_hook(e, "at start survey", code)
}

/// `label in legend={options}`: an entry for the visualizer in a legend.
fn k_label_in_legend(e: &mut Engine, v: Option<Vec<Token>>) -> Result<(), Error> {
    let name = current_visualizer(e);
    let mut entry = tikra_tokens(&format!(
        "visualizer in legend style={{/tikz/data visualization/visualizers/{name}/styling/.try}},/tikz/data visualization/every label in legend/.try,/tikz/.cd,every label/.try,/tikz/data visualization/legend entry options/.cd,setup={{/data point/set={{{name}}}}},/tikz/data visualization/visualizers/{name}/label in legend options/.try,"
    ));
    entry.extend(v.unwrap_or_default());
    let mut item = tikra_tokens("new legend entry=");
    item.extend(group(entry));
    dv_keys(e, item)
}
