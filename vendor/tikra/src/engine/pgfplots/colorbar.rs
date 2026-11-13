//! Colorbars (`\pgfplots@colorbar@move@outside@scope@with@globalmacro` and
//! `\pgfplots@colorbar@draw@`): once an axis is drawn, a second axis next to it, with the options
//! `every colorbar, colorbar shift, colorbar=false`, shows the colormap from `point meta min` to
//! `point meta max`. As in pgfplots it is drawn after the parent axis' group, with only the
//! colorbar styles, the tick, grid and tick label styles, the colormap, the meta range and the
//! parent's size carried over; `parent axis` names the axis it belongs to.

use crate::keys as pk;
use crate::tex::{Meaning, Token};
use crate::{Error, PT};

use super::keys;
use super::{toks, Engine};

/// What a colorbar keeps of its axis.
#[derive(Debug, Clone)]
pub struct Colorbar {
    /// Macros restored around the colorbar: styles and values.
    saved: Vec<(String, Meaning)>,
    meta: [f64; 2],
    /// The parent axis box, in pt.
    width: f64,
    height: f64,
    /// The parent axis, for `at={(0.5,1.03)}` in its description coordinates.
    parent: std::rc::Rc<super::draw::Frame>,
    /// The options of the axis' `contour filled`: the colorbar shows its bands.
    pub bands: Option<Vec<Token>>,
}

thread_local! {
    /// The axis whose colorbar is being drawn.
    static PARENT: std::cell::RefCell<Option<std::rc::Rc<super::draw::Frame>>> = const { std::cell::RefCell::new(None) };
}

/// The parent of the colorbar being drawn: plain coordinates of its `at` are the parent's
/// `axis description cs`.
pub fn parent() -> Option<std::rc::Rc<super::draw::Frame>> {
    PARENT.with(|p| p.borrow().clone())
}

/// The keys pgfplots moves out of the axis' group for the colorbar (`\pgfplots@colorbar@foreach@saved@key`).
const SAVED: &[&str] = &[
    "/pgfplots/every colorbar/.@cmd",
    "/pgfplots/colorbar shift/.@cmd",
    "/pgfplots/colorbar horizontal/lowlevel/.@cmd",
    "/pgfplots/colorbar vertical/lowlevel/.@cmd",
    "/pgfplots/every tick/.@cmd",
    "/pgfplots/every minor tick/.@cmd",
    "/pgfplots/every major tick/.@cmd",
    "/pgfplots/every axis grid/.@cmd",
    "/pgfplots/every minor grid/.@cmd",
    "/pgfplots/every major grid/.@cmd",
    "/pgfplots/every tick label/.@cmd",
    "/pgfplots/colormap name",
    "/pgfplots/colorbar/width",
    "/pgfplots/colorbar pos*",
    "/pgfplots/@colorbar sampled",
    "/pgfplots/@colorbar line",
    "/pgfplots/colormap access",
];

/// The colorbar of the axis being drawn, if it has one.
pub fn snapshot(e: &mut Engine, meta: [f64; 2], box_size: (f64, f64), parent: super::draw::Frame) -> Option<Colorbar> {
    if !keys::flag(e, "/pgfplots/colorbar") {
        return None;
    }
    // `colorbar to name`: kept for a `\ref` (that a single run of LaTeX leaves as ??), not drawn
    if !keys::value(e, "/pgfplots/colorbar to name").is_empty() {
        return None;
    }
    let mut saved: Vec<(String, Meaning)> = SAVED.iter().map(|k| (pk::macro_name(k), e.tex.meaning_of(&pk::macro_name(k)))).collect();
    // the colormap itself
    let name = keys::value(e, "/pgfplots/colormap name");
    let cm = pk::macro_name(&format!("/pgfplots/@colormaps/{name}"));
    saved.push((cm.clone(), e.tex.meaning_of(&cm)));
    let meta = if meta[0].is_finite() && meta[1].is_finite() && meta[1] > meta[0] {
        meta
    } else {
        e.warn("colorbar: the axis has no point meta, it shows 0 to 1");
        [0.0, 1.0]
    };
    Some(Colorbar { saved, meta, width: box_size.0, height: box_size.1, parent: std::rc::Rc::new(parent), bands: None })
}

impl Engine {
    /// Draws a colorbar after its axis' group is closed.
    pub fn draw_colorbar(&mut self, cb: &Colorbar) -> Result<(), Error> {
        self.begin_group();
        let r = (|| -> Result<(), Error> {
            for (name, meaning) in &cb.saved {
                self.tex.set_meaning(&Token::cs(name), meaning.clone(), false);
            }
            let num = super::super::pgf_number;
            keys::set(self, "/pgfplots/@colorbar axis", "true");
            keys::set(self, "/pgfplots/point meta min", &num(cb.meta[0]));
            keys::set(self, "/pgfplots/point meta max", &num(cb.meta[1]));
            keys::set(self, "/pgfplots/parent axis width", &format!("{}pt", num(cb.width / PT)));
            keys::set(self, "/pgfplots/parent axis height", &format!("{}pt", num(cb.height / PT)));
            // the orientation, from the styles
            self.begin_group();
            self.st.key_path = "/pgfplots".into();
            keys::run(self, "every colorbar")?;
            let horizontal = keys::value(self, "/pgfplots/@colorbar orientation") == "h";
            self.end_group();
            let sampled = keys::value_tokens(self, "/pgfplots/@colorbar sampled").filter(|v| !pk::trim(v).is_empty());
            let (style, n) = match (&cb.bands, &sampled) {
                (Some(b), _) => (format!("contour filled={{{}}}", crate::tex::tokens_to_string(b)), 2),
                (None, Some(s)) => (crate::tex::tokens_to_string(s), keys::number(self, "/pgfplots/samples").unwrap_or(25.0).max(2.0) as usize),
                (None, None) => ("surf,shader=interp".to_string(), 2),
            };
            let (lo, hi) = (cb.meta[0], cb.meta[1]);
            let at = |k: usize| lo + (hi - lo) * k as f64 / (n - 1) as f64;
            let mut coords = String::new();
            // `colorbar sampled line`: one line of samples in the middle, with the axis line on the
            // tick labels' side only
            let line = sampled.is_some() && keys::flag(self, "/pgfplots/@colorbar line");
            let mut extra = String::new();
            if line {
                for k in 0..n {
                    let v = num(at(k));
                    coords.push_str(&if horizontal { format!("({v},0.5) ") } else { format!("(0.5,{v}) ") });
                }
                let pos = keys::value(self, "/pgfplots/colorbar pos*");
                extra = if horizontal {
                    format!("axis x line*={},y axis line style={{/pgfplots/every colorbar sampled line}},", if pos == "t" { "top" } else { "bottom" })
                } else {
                    format!("axis y line*={},x axis line style={{/pgfplots/every colorbar sampled line}},", if pos == "l" { "left" } else { "right" })
                };
            }
            for side in [0, 1].into_iter().filter(|_| !line) {
                for k in 0..n {
                    let v = num(at(k));
                    coords.push_str(&if horizontal { format!("({v},{side}) ") } else { format!("({side},{v}) ") });
                }
                if side == 0 {
                    coords.push_str("\\par ");
                }
            }
            let meta = if horizontal { "x" } else { "y" };
            let code = format!(
                "\\begin{{axis}}[every colorbar,colorbar shift,colorbar=false,{extra}]\\addplot[{style},point meta={meta},forget plot] coordinates {{{coords}}};\\end{{axis}}"
            );
            let before = PARENT.with(|p| p.replace(Some(cb.parent.clone())));
            let r = self.run_tokens(toks(&code));
            PARENT.with(|p| *p.borrow_mut() = before);
            r
        })();
        self.end_group();
        r
    }
}
