//! The figure in words, for readers who cannot see it: what the engine noted while it drew (the
//! nodes and their text, the connections between them, the axes and their plots) becomes a short
//! alternative text, the SVG's `<title>`, and a longer description, its `<desc>`. Math in the text
//! is read as mathra speaks it.
//!
//! The words come in the 16 languages of mathra, whatever the figure is written in: each language
//! is a JSON file in `src/locales/` of phrases with `{0}`, `{1}`…, its plural forms as `key.one`,
//! `key.two`, `key.few`, `key.many` and `key.other`; a phrase a language lacks is English.

use std::collections::HashMap;
use std::sync::OnceLock;

use mathra::locale::Locale;

use crate::scene::Point;

/// What the engine notes while it draws.
#[derive(Debug, Clone, Default)]
pub struct Facts {
    pub nodes: Vec<NodeFact>,
    pub links: Vec<Link>,
    pub axes: Vec<AxisFact>,
    /// Charts of the data visualization library, told by their text alone.
    pub data_visualizations: usize,
    /// Paths drawn, filled or shaded outside nodes and axes.
    pub shapes: usize,
    /// While above zero (an axis drawing its ticks, labels and legend), nothing is noted.
    pub mute: usize,
    /// The text the author gave (`alt={…}`): told as it is, instead of what tikra would write.
    pub author_alt: String,
    /// The node the coordinate just read names (`(a)`, `(a.east)`), for the path's connections.
    pub touched: Option<String>,
    /// The paths of each statement of the source map, by its place there.
    pub shapes_of: HashMap<usize, usize>,
}

#[derive(Debug, Clone)]
pub struct NodeFact {
    /// The name it is known by (`tikz@f@…` when it has none).
    pub key: String,
    pub named: bool,
    /// The text, as TeX.
    pub text: String,
    pub at: Point,
    /// A label on a connection (`-- node {yes} (b)`), told with it.
    pub on_link: bool,
    /// The statement that placed it, in the source map.
    pub source: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arrow {
    None,
    Forward,
    Backward,
    Both,
}

/// A drawn line or curve from one node to another.
#[derive(Debug, Clone)]
pub struct Link {
    pub from: String,
    pub to: String,
    pub arrow: Arrow,
    /// The nodes placed on it, by index into [`Facts::nodes`].
    pub labels: Vec<usize>,
    /// The statement that drew it, in the source map.
    pub source: Option<usize>,
}

#[derive(Debug, Clone, Default)]
pub struct AxisFact {
    /// Its `\\begin{axis}`, in the source map.
    pub source: Option<usize>,
    /// `polar`, `ternary`, `smith` or empty.
    pub kind: &'static str,
    pub three_d: bool,
    pub title: String,
    pub labels: [String; 3],
    /// The data's range on each axis (with the limits given), when known.
    pub range: [Option<[f64; 2]>; 3],
    pub log: [bool; 3],
    /// `symbolic x coords` and friends.
    pub symbolic: [Vec<String>; 3],
    pub series: Vec<Series>,
    /// The range of the values colors stand for (surfaces, scatter by meta).
    pub colors: Option<[f64; 2]>,
}

#[derive(Debug, Clone, Default)]
pub struct Series {
    /// Its `\\addplot`, in the source map.
    pub source: Option<usize>,
    pub kind: &'static str,
    /// Its legend entry, as TeX.
    pub name: String,
    /// The formula it was sampled from, when it was.
    pub expression: String,
    pub points: Vec<[f64; 3]>,
    /// A box plot: lower whisker, lower quartile, median, upper quartile, upper whisker.
    pub boxplot: Option<[Option<f64>; 5]>,
}

/// The figure in words: `alt` short (the `<title>`, an image's `alt`), `text` the whole of it.
/// When the author wrote it (`alt={…}`), both are the author's text and the SVG has no `<desc>`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Description {
    pub alt: String,
    pub text: String,
    pub by_author: bool,
}

/// Points listed one by one up to this many; beyond, their range.
const LIST_POINTS: usize = 12;
/// Nodes and connections told one by one up to these many.
const LIST_NODES: usize = 40;
const LIST_LINKS: usize = 60;

/// The phrases of each language, in the order of [`Locale::ALL`].
const SOURCES: [&str; 16] = [
    include_str!("locales/en.json"),
    include_str!("locales/pt-BR.json"),
    include_str!("locales/es.json"),
    include_str!("locales/fr.json"),
    include_str!("locales/de.json"),
    include_str!("locales/it.json"),
    include_str!("locales/ru.json"),
    include_str!("locales/zh.json"),
    include_str!("locales/ja.json"),
    include_str!("locales/ko.json"),
    include_str!("locales/hi.json"),
    include_str!("locales/ar.json"),
    include_str!("locales/id.json"),
    include_str!("locales/tr.json"),
    include_str!("locales/nl.json"),
    include_str!("locales/pl.json"),
];

fn phrases(locale: Locale) -> &'static HashMap<String, String> {
    static TABLE: OnceLock<Vec<HashMap<String, String>>> = OnceLock::new();
    let table = TABLE.get_or_init(|| {
        Locale::ALL
            .iter()
            .zip(SOURCES)
            .map(|(l, json)| {
                let map: HashMap<String, String> = serde_json::from_str(json).unwrap_or_else(|e| panic!("src/locales/{}.json: {e}", l.code()));
                assert_eq!(map.get("code").map(String::as_str), Some(l.code()), "the file of {l:?} declares another code");
                map
            })
            .collect()
    });
    let i = Locale::ALL.iter().position(|l| *l == locale).unwrap_or(0);
    &table[i]
}

/// The plural form of `n` in the language (CLDR's categories, as far as these phrases need them).
fn plural_form(locale: Locale, n: usize) -> &'static str {
    let (m10, m100) = (n % 10, n % 100);
    match locale {
        Locale::Zh | Locale::Ja | Locale::Ko | Locale::Id | Locale::Tr => "other",
        Locale::PtBr | Locale::Fr | Locale::Hi if n <= 1 => "one",
        Locale::Ru if m10 == 1 && m100 != 11 => "one",
        Locale::Ru | Locale::Pl if (2..=4).contains(&m10) && !(12..=14).contains(&m100) => "few",
        Locale::Ru => "many",
        Locale::Pl if n == 1 => "one",
        Locale::Pl => "many",
        Locale::Ar => match (n, m100) {
            (1, _) => "one",
            (2, _) => "two",
            (_, 3..=10) => "few",
            _ => "other",
        },
        _ if n == 1 => "one",
        _ => "other",
    }
}

/// The words of one language.
struct Words {
    locale: Locale,
}

impl Words {
    fn phrase(&self, key: &str) -> &'static str {
        let own = phrases(self.locale);
        own.get(key).or_else(|| phrases(Locale::En).get(key)).map_or_else(|| panic!("phrase '{key}' missing from src/locales/en.json"), String::as_str)
    }

    /// A phrase with `{0}`, `{1}`… replaced by `args`.
    fn t(&self, key: &str, args: &[&str]) -> String {
        let mut s = self.phrase(key).to_string();
        for (i, a) in args.iter().enumerate() {
            s = s.replace(&format!("{{{i}}}"), a);
        }
        s
    }

    /// `n` things: the language's plural form of `key`, then its `other`, then English's.
    fn count(&self, key: &str, n: usize) -> String {
        let form = plural_form(self.locale, n);
        let own = phrases(self.locale);
        let en = phrases(Locale::En);
        let k = format!("{key}.{form}");
        let other = format!("{key}.other");
        let en_form = format!("{key}.{}", plural_form(Locale::En, n));
        let p = own.get(&k).or_else(|| own.get(&other)).or_else(|| en.get(&en_form)).or_else(|| en.get(&other));
        p.map_or_else(|| panic!("phrase '{other}' missing from src/locales/en.json"), |p| p.replace("{0}", &n.to_string()))
    }

    fn quote(&self, s: &str) -> String {
        self.t("quote", &[s])
    }

    fn quoted_list(&self, items: &[String]) -> String {
        items.iter().map(|t| self.quote(t)).collect::<Vec<_>>().join(self.phrase("sep.items"))
    }

    /// Items joined by the list separator, the first `cap` of them and how many more.
    fn capped(&self, items: &[String], cap: usize) -> String {
        let sep = self.phrase("sep.list");
        let mut s = items.iter().take(cap).cloned().collect::<Vec<_>>().join(sep);
        if items.len() > cap {
            s.push_str(sep);
            s.push_str(&self.t("and_more", &[&(items.len() - cap).to_string()]));
        }
        s
    }

    fn sentence(&self, s: &str) -> String {
        format!("{s}{}", self.phrase("end"))
    }

    /// Between the numbers of a pair or a list: a comma, or a semicolon where the comma is decimal.
    fn between(&self) -> &'static str {
        if self.locale.decimal_separator() == ',' {
            "; "
        } else {
            self.phrase("sep.items")
        }
    }

    fn num(&self, v: f64, span: f64) -> String {
        let s = num_in(v, span);
        match self.locale.decimal_separator() {
            '.' => s,
            c => s.replace('.', &c.to_string()),
        }
    }

    fn spoken(&self, tex: &str) -> String {
        spoken(tex, self.locale)
    }
}

/// The figure in words, in `locale`; the author's text (`alt={…}`) when there is one.
pub fn describe(f: &Facts, locale: Locale) -> Description {
    let w = Words { locale };
    let author = w.spoken(&f.author_alt);
    if !author.is_empty() {
        return Description { alt: author.clone(), text: author, by_author: true };
    }
    let nodes = visible_nodes(f);
    let (links, joined) = told_links(f, &w);
    let texts: Vec<(String, bool)> = nodes.iter().map(|n| (w.spoken(&n.text), is_number(&n.text))).filter(|t| !t.0.is_empty()).collect();
    let words: Vec<String> = texts.iter().map(|t| t.0.clone()).collect();

    let mut parts: Vec<String> = Vec::new();
    let mut alt = String::new();
    let axes: Vec<&AxisFact> = f.axes.iter().collect();
    for (i, a) in axes.iter().enumerate() {
        let (short, long) = describe_axis(&w, a);
        if i == 0 {
            alt = short.clone();
        }
        if axes.len() > 1 {
            parts.push(w.t("plot_of", &[&(i + 1).to_string(), &axes.len().to_string(), &long]));
        } else {
            parts.push(long);
        }
    }
    if axes.len() > 1 {
        let kinds: Vec<String> = axes.iter().map(|a| describe_axis(&w, a).0).collect();
        let count = w.count("plot", axes.len());
        alt = if kinds.iter().all(|k| *k == kinds[0]) { w.t("plots", &[&count, &kinds[0]]) } else { count };
    }
    if !links.is_empty() {
        let summary = w.t("diagram", &[&w.count("node", joined.len()), &w.count("connection", links.len())]);
        if alt.is_empty() {
            alt = summary.clone();
        }
        parts.push(w.sentence(&summary));
        if !words.is_empty() {
            parts.push(w.sentence(&w.t("nodes", &[&w.capped(&words, LIST_NODES)])));
        }
        parts.push(w.sentence(&w.t("connections", &[&w.capped(&links, LIST_LINKS)])));
    } else if !words.is_empty() {
        let charts = f.data_visualizations;
        let what = if !axes.is_empty() {
            "text.other"
        } else if charts > 0 {
            "text.chart"
        } else {
            "text.figure"
        };
        if axes.is_empty() && charts > 0 {
            parts.push(w.sentence(&w.count("datavis", charts)));
        }
        parts.push(w.sentence(&w.t(what, &[&w.capped(&words, LIST_NODES)])));
        if alt.is_empty() {
            // words before numbers: tick labels say less than titles and names
            let mut first: Vec<String> = texts.iter().filter(|t| !t.1).take(3).map(|t| t.0.clone()).collect();
            if first.is_empty() {
                first = words.iter().take(3).cloned().collect();
            }
            let mut list = w.quoted_list(&first);
            if words.len() > first.len() {
                list.push_str(w.phrase("sep.items"));
                list.push('…');
            }
            alt = if charts > 0 {
                w.t("alt.chart_text", &[&list])
            } else if f.shapes == 0 {
                w.t("alt.drawing_text", &[&list])
            } else {
                w.t("alt.shapes_text", &[&w.count("shape", f.shapes), &list])
            };
        }
    }
    if alt.is_empty() {
        alt = match f.shapes {
            0 => w.t("alt.empty", &[]),
            n => w.t("alt.shapes", &[&w.count("shape", n)]),
        };
    }
    let text = if parts.is_empty() { w.sentence(&alt) } else { parts.join(w.phrase("sep.sentence")) };
    Description { alt, text, by_author: false }
}

/// What one statement of the source map drew, in a sentence: the figure's own words about only
/// that part of it, so the 16 languages come with them: a short name for the part, and a whole
/// sentence for a caption, empty when all there is to say is that a shape was drawn. Both are empty
/// when there is nothing to tell.
pub fn describe_part(f: &Facts, source: usize, locale: Locale) -> (String, String) {
    let here = Some(source);
    let mut part = Facts {
        data_visualizations: 0,
        shapes: 0,
        mute: 0,
        author_alt: String::new(),
        touched: None,
        shapes_of: HashMap::new(),
        nodes: f.nodes.clone(),
        links: Vec::new(),
        axes: Vec::new(),
    };
    // the other nodes stay, unseen, to name what this part connects
    for n in &mut part.nodes {
        n.on_link = n.on_link || n.source != here;
    }
    part.links = f.links.iter().filter(|l| l.source == here).cloned().collect();
    for a in &f.axes {
        if a.source == here {
            part.axes.push(AxisFact { series: Vec::new(), ..a.clone() });
        } else if a.series.iter().any(|s| s.source == here) {
            part.axes.push(AxisFact {
                source: None,
                title: String::new(),
                labels: Default::default(),
                series: a.series.iter().filter(|s| s.source == here).cloned().collect(),
                ..a.clone()
            });
        }
    }
    let shapes = f.shapes_of.get(&source).copied().unwrap_or(0);
    let w = Words { locale };
    if !part.axes.is_empty() {
        let alt = describe(&part, locale).alt;
        let sentence = w.sentence(&alt);
        return (alt, sentence);
    }
    let (links, _) = told_links(&part, &w);
    if !links.is_empty() {
        let list = links.join(w.phrase("sep.list"));
        let sentence = w.sentence(&w.t("connections", &[&list]));
        return (list, sentence);
    }
    let texts: Vec<String> = visible_nodes(&part).iter().map(|n| w.spoken(&n.text)).filter(|t| !t.is_empty()).collect();
    if !texts.is_empty() {
        return (w.quoted_list(&texts), w.sentence(&w.t("text.figure", &[&w.capped(&texts, LIST_NODES)])));
    }
    if shapes > 0 {
        return (w.count("shape", shapes), String::new());
    }
    (String::new(), String::new())
}

/// The connections in words, each once, and the nodes they join.
fn told_links(f: &Facts, w: &Words) -> (Vec<String>, Vec<String>) {
    let name_of = |key: &str| -> Option<String> {
        let n = f.nodes.iter().rev().find(|n| n.key == key)?;
        let t = w.spoken(&n.text);
        if !t.is_empty() {
            Some(t)
        } else if n.named && !n.key.contains('@') {
            // a name the author gave (internal names have an `@`)
            Some(n.key.clone())
        } else {
            None
        }
    };
    let mut links: Vec<String> = Vec::new();
    let mut joined: Vec<String> = Vec::new();
    for l in &f.links {
        let (Some(a), Some(b)) = (name_of(&l.from), name_of(&l.to)) else { continue };
        for k in [&l.from, &l.to] {
            if !joined.contains(k) {
                joined.push(k.clone());
            }
        }
        let mut s = match l.arrow {
            _ if l.from == l.to => w.t("link.self", &[&a]),
            Arrow::Forward => w.t("link.forward", &[&a, &b]),
            Arrow::Backward => w.t("link.forward", &[&b, &a]),
            Arrow::Both => w.t("link.both", &[&a, &b]),
            Arrow::None => w.t("link.none", &[&a, &b]),
        };
        let labels: Vec<String> = l.labels.iter().map(|&i| w.spoken(&f.nodes[i].text)).filter(|t| !t.is_empty()).collect();
        if !labels.is_empty() {
            s = w.t("link.labeled", &[&s, &w.quoted_list(&labels)]);
        }
        if !links.contains(&s) {
            links.push(s);
        }
    }
    (links, joined)
}

/// The nodes worth telling: with text or a name, not on a connection, the last of each name.
fn visible_nodes(f: &Facts) -> Vec<&NodeFact> {
    let mut out: Vec<&NodeFact> = Vec::new();
    for n in &f.nodes {
        if n.on_link || n.text.trim().is_empty() {
            continue;
        }
        // a node drawn again (measured, then placed) is told once
        if let Some(i) = out.iter().position(|m| (n.named && m.key == n.key) || (m.text == n.text && m.at == n.at)) {
            out[i] = n;
            continue;
        }
        out.push(n);
    }
    out
}

/// The axis in words: its short name and the whole of it.
fn describe_axis(w: &Words, a: &AxisFact) -> (String, String) {
    let kind = w.t(&format!("chart.{}", chart_kind(a)), &[]);
    let title = w.spoken(&a.title);
    let labels: Vec<String> = a.labels.iter().map(|l| w.spoken(l)).collect();
    let short = if !title.is_empty() {
        w.t("axis.short_title", &[&kind, &title])
    } else if !labels[0].is_empty() && !labels[1].is_empty() && !a.three_d {
        w.t("axis.against", &[&kind, &labels[1], &labels[0]])
    } else {
        kind.clone()
    };
    let mut s = Vec::new();
    s.push(w.sentence(&if title.is_empty() { kind.clone() } else { w.t("axis.titled", &[&kind, &w.quote(&title)]) }));
    let dims = if a.three_d { 3 } else { 2 };
    for (k, label) in labels.iter().enumerate().take(dims) {
        if let Some(t) = axis_sentence(w, a, k, label) {
            s.push(w.sentence(&t));
        }
    }
    if let Some([lo, hi]) = a.colors {
        s.push(w.sentence(&w.t("colors", &[&w.num(lo, hi - lo), &w.num(hi, hi - lo)])));
    }
    let series: Vec<&Series> = a.series.iter().collect();
    let same_kind = series.iter().all(|p| p.kind == series[0].kind);
    match series.len() {
        0 => s.push(w.sentence(w.phrase("no_data"))),
        1 => s.push(w.sentence(&series_text(w, a, series[0], 0, false))),
        n => {
            s.push(w.sentence(&w.count("series", n)));
            for (i, p) in series.iter().enumerate() {
                s.push(w.sentence(&series_text(w, a, p, i + 1, !same_kind)));
            }
        }
    }
    (short, s.join(w.phrase("sep.sentence")))
}

/// The chart's kind, as the key of its name.
fn chart_kind(a: &AxisFact) -> &'static str {
    match a.kind {
        "polar" | "ternary" | "smith" => return a.kind,
        _ => {}
    }
    let kinds: Vec<&str> = a.series.iter().map(|p| p.kind).collect();
    let all = |k: &str| !kinds.is_empty() && kinds.iter().all(|x| *x == k);
    let named = [
        ("bar", "bar"),
        ("horizontal bar", "hbar"),
        ("histogram", "histogram"),
        ("scatter", "scatter"),
        ("area", "area"),
        ("surface", "surface"),
        ("mesh", "surface"),
        ("contour", "contour"),
        ("box plot", "box"),
        ("stem", "stem"),
        ("vector field", "field"),
    ];
    if let Some((_, k)) = named.iter().find(|(series, _)| all(series)) {
        k
    } else if all("line") {
        if a.three_d {
            "line3d"
        } else {
            "line"
        }
    } else if a.three_d {
        "plot3d"
    } else {
        "chart"
    }
}

fn axis_sentence(w: &Words, a: &AxisFact, k: usize, label: &str) -> Option<String> {
    if a.kind == "ternary" || a.kind == "smith" {
        return None;
    }
    let name = match (a.kind, a.three_d, k) {
        ("polar", _, 0) => "axis.angle",
        ("polar", _, _) => "axis.radius",
        (_, true, 0) => "axis.x",
        (_, true, 1) => "axis.y",
        (_, true, _) => "axis.z",
        (_, false, 0) => "axis.horizontal",
        _ => "axis.vertical",
    };
    let mut s = w.t(name, &[]);
    if !label.is_empty() {
        s = w.t("axis.label", &[&s, &w.quote(label)]);
    }
    if !a.symbolic[k].is_empty() {
        s = w.t("axis.categories", &[&s, &a.symbolic[k].join(w.phrase("sep.items"))]);
    } else if let Some([lo, hi]) = a.range[k] {
        let (lo, hi) = (value(w, a, k, lo), value(w, a, k, hi));
        s = w.t(if a.kind == "polar" && k == 0 { "axis.range_degrees" } else { "axis.range" }, &[&s, &lo, &hi]);
        if a.log[k] {
            s = w.t("axis.log", &[&s]);
        }
    } else if label.is_empty() {
        return None;
    }
    Some(s)
}

/// A series in words; `number` counts it among several (0: it is alone), and then `with_kind`
/// says what kind it is when they differ.
fn series_text(w: &Words, a: &AxisFact, p: &Series, number: usize, with_kind: bool) -> String {
    let name = w.spoken(&p.name);
    let mut head = String::new();
    if !name.is_empty() {
        head = w.quote(&name);
    } else if number > 0 {
        head = w.t("series.number", &[&number.to_string()]);
    }
    if with_kind {
        head = w.t("series.kind", &[&head, &w.t(&format!("kind.{}", p.kind), &[])]);
    }
    let data = series_data(w, a, p);
    match (head.is_empty(), data.is_empty()) {
        (true, true) => w.t("no_data", &[]),
        (true, false) => data,
        (false, true) => head,
        (false, false) => w.t("series.data", &[&head, &data]),
    }
}

fn series_data(w: &Words, a: &AxisFact, p: &Series) -> String {
    let n = |v: f64| w.num(v, v);
    if let Some([lw, lq, m, uq, uw]) = p.boxplot {
        let mut v = Vec::new();
        if let Some(m) = m {
            v.push(w.t("box.median", &[&n(m)]));
        }
        if let (Some(lq), Some(uq)) = (lq, uq) {
            v.push(w.t("box.quartiles", &[&n(lq), &n(uq)]));
        }
        if let (Some(lw), Some(uw)) = (lw, uw) {
            v.push(w.t("box.whiskers", &[&n(lw), &n(uw)]));
        }
        if !p.points.is_empty() {
            v.push(w.count("outlier", p.points.len()));
        }
        return v.join(w.phrase("sep.items"));
    }
    let pts: Vec<[f64; 3]> = p.points.iter().copied().filter(|c| c.iter().take(if a.three_d { 3 } else { 2 }).all(|v| v.is_finite())).collect();
    if pts.is_empty() {
        return String::new();
    }
    let x = |v: f64| value(w, a, 0, v);
    let y = |v: f64| value(w, a, 1, v);
    let z = |v: f64| value(w, a, 2, v);
    if !p.expression.is_empty() && pts.len() > LIST_POINTS {
        let formula = format!("{} = {}", if a.three_d { "z" } else { "y" }, p.expression.trim());
        let (x0, x1) = extent(&pts, 0);
        let s = if a.three_d {
            let (y0, y1) = extent(&pts, 1);
            w.t("formula.xy", &[&formula, &x(x0), &x(x1), &y(y0), &y(y1)])
        } else {
            w.t("formula.x", &[&formula, &x(x0), &x(x1)])
        };
        return format!("{s}{}{}", w.phrase("sep.list"), extremes(w, a, &pts));
    }
    if p.kind == "histogram" && pts.len() >= 2 && pts.len() <= LIST_POINTS + 1 {
        let bins: Vec<String> = pts.windows(2).map(|b| w.t("bin", &[&x(b[0][0]), &x(b[1][0]), &y(b[0][1])])).collect();
        return w.t("counted", &[&w.count("bin", bins.len()), &bins.join(w.phrase("sep.list"))]);
    }
    if pts.len() <= LIST_POINTS {
        let between = w.between();
        let list: Vec<String> = pts
            .iter()
            .map(|c| match p.kind {
                "bar" if !a.three_d => format!("{}: {}", x(c[0]), y(c[1])),
                "horizontal bar" if !a.three_d => format!("{}: {}", y(c[1]), x(c[0])),
                _ if a.three_d => format!("({}{between}{}{between}{})", x(c[0]), y(c[1]), z(c[2])),
                _ => format!("({}{between}{})", x(c[0]), y(c[1])),
            })
            .collect();
        let what = if matches!(p.kind, "bar" | "horizontal bar") { "bar" } else { "point" };
        return w.t("counted", &[&w.count(what, pts.len()), &list.join(between)]);
    }
    let (x0, x1) = extent(&pts, 0);
    let s = w.t("points.range", &[&w.count("point", pts.len()), &x(x0), &x(x1)]);
    format!("{s}{}{}", w.phrase("sep.list"), extremes(w, a, &pts))
}

/// Where the values (y, or z in 3d) are lowest and highest.
fn extremes(w: &Words, a: &AxisFact, pts: &[[f64; 3]]) -> String {
    let k = if a.three_d { 2 } else { 1 };
    let var = if a.three_d { "z" } else { "y" };
    // the first of equal extremes, as one reads from the left
    let lo = pts.iter().fold(pts[0], |m, p| if p[k] < m[k] { *p } else { m });
    let hi = pts.iter().fold(pts[0], |m, p| if p[k] > m[k] { *p } else { m });
    let at = |c: [f64; 3]| {
        if a.three_d {
            format!("x = {}{}y = {}", value(w, a, 0, c[0]), w.between(), value(w, a, 1, c[1]))
        } else {
            format!("x = {}", value(w, a, 0, c[0]))
        }
    };
    if (hi[k] - lo[k]).abs() <= 1e-12 * hi[k].abs().max(1.0) {
        return w.t("extremes.flat", &[var, &value(w, a, k, lo[k])]);
    }
    w.t("extremes", &[var, &value(w, a, k, lo[k]), &at(lo), &value(w, a, k, hi[k]), &at(hi)])
}

fn extent(pts: &[[f64; 3]], k: usize) -> (f64, f64) {
    pts.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), p| (lo.min(p[k]), hi.max(p[k])))
}

/// A coordinate as the axis shows it: a category's name, or the number, as precise as the axis'
/// range calls for.
fn value(w: &Words, a: &AxisFact, k: usize, v: f64) -> String {
    let names = &a.symbolic[k];
    if !names.is_empty() && (v - v.round()).abs() < 1e-9 && v >= 0.0 && (v as usize) < names.len() {
        return names[v as usize].clone();
    }
    match a.range[k] {
        Some([lo, hi]) if !a.log[k] => w.num(v, hi - lo),
        _ => w.num(v, v),
    }
}

/// A number with up to four significant digits, without trailing zeros.
pub fn num(v: f64) -> String {
    num_in(v, v)
}

/// A number with four significant digits of `span` (a range it is in): TeX's arithmetic leaves
/// 0.000000000069 where 0 was meant, and in a range of 4 that is 0.
pub fn num_in(v: f64, span: f64) -> String {
    if !v.is_finite() {
        return v.to_string();
    }
    let scale = if span.is_finite() && span.abs() > 0.0 { span.abs() } else { v.abs() };
    if v == 0.0 || scale == 0.0 {
        return "0".into();
    }
    let mag = scale.log10().floor() as i32;
    let decimals = (3 - mag).clamp(0, 12) as usize;
    let mut s = format!("{v:.decimals$}");
    if s.contains('.') {
        s = s.trim_end_matches('0').trim_end_matches('.').to_string();
    }
    if s == "-0" {
        s = "0".into();
    }
    s
}

/// A tick label: a number, perhaps signed, in math or with a unit sign (`$-1$`, `50\%`), in any
/// language, read from its TeX.
fn is_number(tex: &str) -> bool {
    let mut letters = false;
    let mut digits = false;
    let mut command = false;
    for c in tex.chars() {
        match c {
            '\\' => command = true,
            c if c.is_alphabetic() => letters |= !command,
            c => {
                command = false;
                digits |= c.is_ascii_digit();
            }
        }
    }
    digits && !letters
}

/// A node's TeX as it is read: the math spoken by mathra in `locale`, the commands gone.
pub fn spoken(tex: &str, locale: Locale) -> String {
    let c: Vec<char> = tex.chars().collect();
    let mut out = String::new();
    let mut text = String::new();
    let mut i = 0;
    while i < c.len() {
        // `$…$`, `$$…$$`, `\(…\)`, `\[…\]`
        let open = match (c[i], c.get(i + 1)) {
            ('$', Some('$')) => Some(("$$", 2)),
            ('$', _) => Some(("$", 1)),
            ('\\', Some('(')) => Some(("\\)", 2)),
            ('\\', Some('[')) => Some(("\\]", 2)),
            ('\\', Some(_)) => {
                text.push(c[i]);
                text.push(c[i + 1]);
                i += 2;
                continue;
            }
            _ => None,
        };
        let Some((close, skip)) = open else {
            text.push(c[i]);
            i += 1;
            continue;
        };
        let start = i + skip;
        let rest: String = c[start..].iter().collect();
        let Some(end) = find_close(&rest, close) else {
            text.extend(&c[i..]);
            break;
        };
        out.push_str(&plain(&text));
        text.clear();
        let math = &rest[..end];
        out.push(' ');
        out.push_str(&speak(math, locale));
        out.push(' ');
        i = start + rest[..end].chars().count() + close.chars().count();
    }
    out.push_str(&plain(&text));
    // math spoken next to brackets and punctuation: `($^\circ$C)` is "(degrees C)", `$x$?` "x?",
    // `$36$\%` "36%"
    let mut out = out.split_whitespace().collect::<Vec<_>>().join(" ").replace("( ", "(");
    for p in [")", ",", ".", ";", ":", "?", "!", "%"] {
        out = out.replace(&format!(" {p}"), p);
    }
    out
}

/// Where `close` ends the math, skipping escaped characters.
fn find_close(s: &str, close: &str) -> Option<usize> {
    let mut i = 0;
    let b = s.as_bytes();
    while i < s.len() {
        if s[i..].starts_with(close) {
            return Some(i);
        }
        if b[i] == b'\\' {
            i += 1;
        }
        i += s[i..].chars().next().map_or(1, char::len_utf8);
    }
    None
}

/// Math as mathra speaks it in `locale`, read as the figure draws it (English conventions: `3.14`).
fn speak(math: &str, locale: Locale) -> String {
    let mut registry = mathra::rules::Registry::default();
    match mathra::input::latex::read(math, Locale::En, &mut registry) {
        Ok(tree) => mathra::output::speech::speak(&tree, locale),
        Err(_) => plain(math),
    }
}

/// Text mode without its commands: `\textbf{a}` is `a`, `\\` a space, `\%` a percent sign.
fn plain(tex: &str) -> String {
    // commands whose first argument is not text
    const DROP_ARG: &[&str] = &["color", "textcolor", "colorbox", "hspace", "vspace", "rule", "includegraphics", "fontsize", "label", "ref", "setlength"];
    const WORDS: &[(&str, &str)] = &[("ldots", "…"), ("dots", "…"), ("textbackslash", "\\"), ("LaTeX", "LaTeX"), ("TeX", "TeX"), ("textdegree", "°"), ("euro", "€")];
    let c: Vec<char> = tex.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < c.len() {
        match c[i] {
            '\\' => {
                i += 1;
                if i >= c.len() {
                    break;
                }
                if !c[i].is_alphabetic() {
                    // `\\`, `\,` and friends are space; `\%` and friends the character
                    out.push(if matches!(c[i], '%' | '&' | '$' | '#' | '_' | '{' | '}') { c[i] } else { ' ' });
                    i += 1;
                    if c[i - 1] == '\\' && c.get(i) == Some(&'[') {
                        // `\\[2pt]`
                        while i < c.len() && c[i] != ']' {
                            i += 1;
                        }
                        i += 1;
                    }
                    continue;
                }
                let start = i;
                while i < c.len() && c[i].is_alphabetic() {
                    i += 1;
                }
                let name: String = c[start..i].iter().collect();
                if c.get(i) == Some(&'*') {
                    i += 1;
                }
                if let Some((_, w)) = WORDS.iter().find(|(n, _)| *n == name) {
                    out.push_str(w);
                } else if DROP_ARG.contains(&name.as_str()) {
                    while c.get(i).is_some_and(|ch| ch.is_whitespace()) {
                        i += 1;
                    }
                    if c.get(i) == Some(&'[') {
                        while i < c.len() && c[i] != ']' {
                            i += 1;
                        }
                        i += 1;
                    }
                    if c.get(i) == Some(&'{') {
                        let mut depth = 0;
                        while i < c.len() {
                            match c[i] {
                                '{' => depth += 1,
                                '}' => {
                                    depth -= 1;
                                    if depth == 0 {
                                        i += 1;
                                        break;
                                    }
                                }
                                _ => {}
                            }
                            i += 1;
                        }
                    }
                } else {
                    out.push(' ');
                }
            }
            '{' | '}' => i += 1,
            '~' => {
                out.push(' ');
                i += 1;
            }
            '`' | '\'' if c.get(i + 1) == Some(&c[i]) => {
                out.push(if c[i] == '`' { '\u{201c}' } else { '\u{201d}' });
                i += 2;
            }
            '-' if c.get(i + 1) == Some(&'-') => {
                let three = c.get(i + 2) == Some(&'-');
                out.push(if three { '—' } else { '–' });
                i += if three { 3 } else { 2 };
            }
            ch => {
                out.push(ch);
                i += 1;
            }
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers() {
        assert_eq!(num(0.0), "0");
        assert_eq!(num(2.5), "2.5");
        assert_eq!(num(1.0 / 3.0), "0.3333");
        assert_eq!(num(2019.0), "2019");
        assert_eq!(num(-0.00001), "-0.00001");
        assert_eq!(num(f64::NAN), "NaN");
        // as precise as the range it is in
        assert_eq!(num_in(0.000000000069, 4.0), "0");
        assert_eq!(num_in(0.004432, 0.47), "0.0044");
    }

    #[test]
    fn an_axis_with_nothing_to_tell() {
        let f = Facts { axes: vec![AxisFact { series: vec![Series::default()], ..Default::default() }], ..Default::default() };
        assert_eq!(describe(&f, Locale::En), Description { alt: "Chart".into(), text: "Chart. No data.".into(), by_author: false });
        // a label, without a range
        let a = AxisFact { labels: ["time".into(), String::new(), String::new()], ..Default::default() };
        assert_eq!(describe(&Facts { axes: vec![a], ..Default::default() }, Locale::En).text, "Chart. Horizontal axis “time”. No data.");
    }

    #[test]
    fn plural_forms() {
        let forms = |l: Locale| [0, 1, 2, 3, 5, 11, 12, 21, 22, 25, 101, 111].map(|n| plural_form(l, n));
        assert_eq!(forms(Locale::En), ["other", "one", "other", "other", "other", "other", "other", "other", "other", "other", "other", "other"]);
        assert_eq!(forms(Locale::PtBr)[..3], ["one", "one", "other"]);
        assert_eq!(forms(Locale::Ru), ["many", "one", "few", "few", "many", "many", "many", "one", "few", "many", "one", "many"]);
        assert_eq!(forms(Locale::Pl), ["many", "one", "few", "few", "many", "many", "many", "many", "few", "many", "many", "many"]);
        assert_eq!(forms(Locale::Ar), ["other", "one", "two", "few", "few", "other", "other", "other", "other", "other", "other", "other"]);
        assert_eq!(forms(Locale::Ja), ["other"; 12]);
    }

    #[test]
    fn every_language_has_its_words() {
        for l in Locale::ALL {
            let p = phrases(l);
            assert_eq!(p["code"], l.code());
            // every phrase a language has is one English has, with the same placeholders
            for (k, v) in p.iter().filter(|(k, _)| !["code", "name"].contains(&k.as_str())) {
                let base = k.rsplit_once('.').filter(|(_, f)| ["one", "two", "few", "many", "other"].contains(f)).map_or(k.as_str(), |(b, _)| b);
                let en = phrases(Locale::En).get(k.as_str()).or_else(|| phrases(Locale::En).get(&format!("{base}.other")));
                let en = en.unwrap_or_else(|| panic!("{}: '{k}' is not an English phrase", l.code()));
                for i in 0..5 {
                    let ph = format!("{{{i}}}");
                    // a plural form may leave out the number ("one node")
                    if !(k.ends_with(".one") || k.ends_with(".two")) {
                        assert_eq!(v.contains(&ph), en.contains(&ph), "{}: '{k}' and {ph}", l.code());
                    }
                }
            }
            let w = Words { locale: l };
            assert!(!w.count("node", 3).is_empty() && !w.t("chart.bar", &[]).is_empty());
        }
    }

    #[test]
    fn in_portuguese() {
        let a = AxisFact {
            title: "Vendas".into(),
            range: [Some([0.0, 1.0]), Some([0.0, 10.0]), None],
            series: vec![Series { kind: "line", points: vec![[0.0, 1.5, 0.0], [1.0, 2.25, 0.0]], ..Default::default() }],
            ..Default::default()
        };
        let d = describe(&Facts { axes: vec![a], ..Default::default() }, Locale::PtBr);
        assert_eq!(d.alt, "Gráfico de linhas: Vendas");
        assert_eq!(d.text, "Gráfico de linhas com o título “Vendas”. Eixo horizontal, de 0 a 1. Eixo vertical, de 0 a 10. 2 pontos — (0; 1,5); (1; 2,25).");
        assert_eq!(spoken(r"$\frac{3}{4}$", Locale::PtBr), "três quartos");
    }

    #[test]
    fn spoken_text() {
        assert_eq!(spoken(r"\textbf{Start}", Locale::En), "Start");
        assert_eq!(spoken(r"Line one\\Line two", Locale::En), "Line one Line two");
        assert_eq!(spoken(r"temperature ($^\circ$C)", Locale::En), "temperature (degrees C)");
        assert_eq!(spoken(r"50\% off", Locale::En), "50% off");
        // a number printed by \pgfmathprintnumber is math: its percent sign still touches it
        assert_eq!(spoken(r"$36$\% and $x$ \%", Locale::En), "36% and x%");
        assert_eq!(spoken(r"\textcolor{red}{hot}", Locale::En), "hot");
        assert_eq!(spoken(r"$x$", Locale::En), "x");
        assert_eq!(spoken(r"$\frac{3}{4}$", Locale::En), "3 over 4");
        assert_eq!(spoken(r"$$x$$, \(y\) and \[z\]", Locale::En), "x, y and z");
        assert_eq!(spoken(r"costs $5", Locale::En), "costs $5");
        assert_eq!(spoken(r"{a}\\[2pt]b~c\ldots\", Locale::En), "a b c…");
        assert_eq!(spoken(r"\hspace*{1cm}x \includegraphics [width=1cm]{f{1}.png}y", Locale::En), "x y");
    }
}
