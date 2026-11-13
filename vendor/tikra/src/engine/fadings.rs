//! Fadings drawn as pictures: `\pgfdeclarefading{name}{code}` and the TikZ environment
//! `tikzfadingfrompicture[name=…]`. The picture is typeset on its own; where it is white what it
//! fades shows, where it is black (`pgftransparent`, `transparent`) or empty it does not. Used by
//! `\pgfsetfading{name}{transformation}` (in the coordinate system, centered at its origin),
//! `\pgfsetfadingforcurrentpath` (fitted to the path being built), `scope fading` and `path fading`.

use crate::canvas::{Canvas, Item};
use crate::scene::{Point, Segment, Transform};
use crate::tex::{Cat, Token};
use crate::Error;

use super::{tikra_tokens, Engine};

/// A `scope fading` of a path statement: its name, where in the items what the statement draws
/// begins, `fit fading`, the transformation when it was given, and the box it is fitted to if known.
pub type PathFading = (String, usize, bool, Transform, Option<(Point, Point)>);

/// A `scope fading` carried to the next path: its name, the box it is fitted to (none: at its
/// natural size), and its transformation.
pub type CarriedFading = (String, Option<(Point, Point)>, Transform);

/// A fading's picture: what it draws, and its box (the picture's center is the fading's).
#[derive(Debug, Clone)]
pub struct PictureFading {
    pub items: Vec<Item>,
    pub min: Point,
    pub max: Point,
}

impl PictureFading {
    fn center(&self) -> Point {
        self.min.lerp(self.max, 0.5)
    }

    /// The mask on the canvas: the picture moved so its center is at the origin, then `t`.
    fn mask(&self, t: &Transform) -> Item {
        let c = self.center();
        let t = t.compose(&Transform::translation(-c.x, -c.y));
        let mut items = self.items.clone();
        items.iter_mut().for_each(|i| i.transform(&t));
        Item::FadePicture { items }
    }

    /// The transformation that fits the fading onto the box `lo`–`hi` (`fit fading`): as pgf does,
    /// 50 bp of it around its center become the box.
    fn fit(&self, lo: Point, hi: Point) -> Transform {
        let side = 50.0 * 2.54 / 72.0;
        let c = lo.lerp(hi, 0.5);
        Transform::translation(c.x, c.y).compose(&Transform::scaling((hi.x - lo.x).max(1e-9) / side, (hi.y - lo.y).max(1e-9) / side))
    }
}

impl Engine {
    /// `\pgfdeclarefading{name}{code}`.
    pub fn declare_fading(&mut self) -> Result<(), Error> {
        let name = self.argument_string()?;
        let code = self.argument()?;
        self.fading_from(name.trim().to_string(), code)
    }

    /// The fading `name` drawn by `code`: typeset as a paragraph of its own (a `\tikz`, a shading,
    /// text), the drawing around it set aside meanwhile.
    fn fading_from(&mut self, name: String, code: Vec<Token>) -> Result<(), Error> {
        // a bare shading or text is a picture's content
        let first = code.iter().find(|t| !t.is_space()).cloned();
        // a shading after settings (`\tikzset{top color=…}\pgfuseshading{axis}`) is a picture's too
        let shading = code.iter().any(|t| t.is_cs("pgfuseshading"));
        let code = match first {
            Some(Token::Cs(n)) if &*n == "tikz" || &*n == "begin" => code,
            Some(Token::Cs(n)) if &*n == "pgfuseshading" || &*n == "pgftext" || shading => {
                let mut v = tikra_tokens("\\begin{pgfpicture}");
                v.extend(code);
                v.extend(tikra_tokens("\\end{pgfpicture}"));
                v
            }
            _ => {
                let mut v = tikra_tokens("\\begin{pgfpicture}\\pgftext");
                v.push(Token::Char('{', Cat::Begin));
                v.extend(code);
                v.push(Token::Char('}', Cat::End));
                v.extend(tikra_tokens("\\end{pgfpicture}"));
                v
            }
        };
        let canvas = self.draw_apart(code)?;
        let Canvas { items, min, max, .. } = canvas;
        if let (Some(min), Some(max)) = (min, max) {
            self.picture_fadings.insert(name, PictureFading { items, min, max });
        }
        Ok(())
    }

    /// What `code` draws, typeset as a paragraph of its own (pictures, text), the drawing around it
    /// set aside meanwhile.
    pub fn draw_apart(&mut self, code: Vec<Token>) -> Result<Canvas, Error> {
        let (boxes, layout, words) = self.run_apart(code)?;
        Ok(super::typeset(&boxes, &layout, &words, 0.0, None))
    }

    /// The pictures and words `code` makes and how they are laid out, the drawing around it set
    /// aside meanwhile.
    pub(super) fn run_apart(&mut self, code: Vec<Token>) -> Result<super::Apart, Error> {
        let saved_canvas = std::mem::take(&mut self.canvas);
        let saved_boxes = std::mem::take(&mut self.boxes);
        let saved_layout = std::mem::take(&mut self.layout);
        let saved_words = std::mem::take(&mut self.words);
        let saved_depth = std::mem::replace(&mut self.picture_depth, 0);
        let saved_frozen = std::mem::replace(&mut self.canvas_frozen, false);
        self.begin_group();
        let r = self.run_tokens(code);
        while self.picture_depth > 0 {
            self.end_picture();
        }
        self.end_group();
        self.canvas = saved_canvas;
        let apart = (std::mem::replace(&mut self.boxes, saved_boxes), std::mem::replace(&mut self.layout, saved_layout), std::mem::replace(&mut self.words, saved_words));
        self.picture_depth = saved_depth;
        self.canvas_frozen = saved_frozen;
        r?;
        Ok(apart)
    }

    /// `\begin{tikzfadingfrompicture}[name=…] … \end{tikzfadingfrompicture}` (the `\begin` read).
    pub fn fading_from_picture(&mut self) -> Result<(), Error> {
        let options = self.optional()?.unwrap_or_default();
        let mut name = String::new();
        let mut rest = Vec::new();
        for item in crate::keys::split_list(&options) {
            if item.key == "name" {
                name = self.tex.expand_to_string(item.value.unwrap_or_default())?.trim().to_string();
            } else {
                if !rest.is_empty() {
                    rest.push(Token::other(','));
                }
                rest.extend(tikra_tokens(&item.key));
                if let Some(v) = item.value {
                    rest.push(Token::other('='));
                    rest.push(Token::Char('{', Cat::Begin));
                    rest.extend(v);
                    rest.push(Token::Char('}', Cat::End));
                }
            }
        }
        // the body, up to \end{tikzfadingfrompicture}
        let mut body = Vec::new();
        while let Some(t) = self.tex.get_token() {
            if t.is_cs("end") {
                let arg = self.tex.read_undelimited()?;
                if crate::tex::tokens_to_string(&arg).trim() == "tikzfadingfrompicture" {
                    break;
                }
                body.push(t);
                body.push(Token::Char('{', Cat::Begin));
                body.extend(arg);
                body.push(Token::Char('}', Cat::End));
                continue;
            }
            body.push(t);
        }
        let mut code = tikra_tokens("\\begin{tikzpicture}[");
        code.extend(rest);
        code.extend(tikra_tokens("]"));
        code.extend(body);
        code.extend(tikra_tokens("\\end{tikzpicture}"));
        self.fading_from(name, code)
    }

    /// `\pgfsetfading{name}{transformation}`: what the scope draws from here on is faded, the
    /// fading centered at the origin of the coordinate system the code makes.
    pub fn set_fading(&mut self) -> Result<(), Error> {
        let name = self.argument_string()?;
        let code = self.argument()?;
        let t = self.fading_transform(code)?;
        self.open_fading(name.trim(), t);
        Ok(())
    }

    /// `\pgfsetfadingforcurrentpath{name}{transformation}` (and `…stroked`, the path's box widened by
    /// half the line width): the fading fitted to the path being built.
    pub fn set_fading_for_path(&mut self, stroked: bool) -> Result<(), Error> {
        let name = self.argument_string()?;
        let code = self.argument()?;
        let mut c = Canvas::default();
        c.include_path(&self.pgf_path.segments);
        let (Some(mut lo), Some(mut hi)) = (c.min, c.max) else { return Ok(()) };
        if stroked {
            let w = self.st.line_width / 2.0;
            (lo, hi) = (Point::new(lo.x - w, lo.y - w), Point::new(hi.x + w, hi.y + w));
        }
        let extra = self.fading_transform(code)?;
        let (fit, inner) = match self.picture_fadings.get(name.trim()) {
            Some(f) => (f.fit(lo, hi), self.st.transform.inverse().compose(&extra)),
            None => (Transform::default(), extra),
        };
        // the code's transformation acts in the fitted box
        let t = fit.compose(&Transform { e: 0.0, f: 0.0, ..inner });
        self.open_fading(name.trim(), t);
        Ok(())
    }

    /// The transformation the code (`\pgftransformshift{…}`…) makes, on top of the current one.
    fn fading_transform(&mut self, code: Vec<Token>) -> Result<Transform, Error> {
        self.begin_group();
        let r = self.run_tokens(code);
        let t = self.st.transform;
        self.end_group();
        r?;
        Ok(t)
    }

    /// Opens the fading `name` placed by `t` in the current scope, closed with it.
    pub fn open_fading(&mut self, name: &str, t: Transform) {
        if let Some(mask) = self.natural_fading(name, &t) {
            self.open_in_scope(mask);
        }
    }

    /// The `scope fading`s of a path statement, closed with it: each masks what the statement drew,
    /// fitted to the path's box (`fit fading`, and a path with segments), or else at its natural size
    /// where it was given. A statement that drew nothing hands them to the next one, as TeX's do,
    /// fitted to its own path: pgf fits the fading when the path that gives it is done.
    pub fn close_path_fadings(&mut self, fadings: Vec<PathFading>, settings: (Transform, bool)) {
        let (fading_transform, fit_now) = settings;
        for (name, start, fit, t, fixed) in fadings.into_iter().rev() {
            let start = start.min(self.canvas.items.len());
            // the settings the whole statement gave, after the fading's own key
            let t = t.compose(&fading_transform);
            let fitted = fixed.or(self.st.path_box.filter(|_| fit && fit_now));
            if start == self.canvas.items.len() {
                self.carried_fadings.push((name, fitted, t));
                continue;
            }
            let mask = match fitted {
                Some((lo, hi)) => {
                    let segs = [Segment::MoveTo(lo), Segment::LineTo(Point::new(hi.x, lo.y)), Segment::LineTo(hi), Segment::LineTo(Point::new(lo.x, hi.y)), Segment::Close];
                    if self.picture_fadings.contains_key(&name) {
                        self.picture_fading_for(&name, &segs, true)
                    } else {
                        self.fading_for(&name, &segs).map(|mask| Item::Fade { mask })
                    }
                }
                None => self.natural_fading(&name, &t),
            };
            if let Some(mask) = mask {
                self.canvas.items.insert(start, mask);
                self.canvas.items.push(Item::End);
            }
        }
    }

    /// A fading at its natural size, centered at the origin of `t`.
    fn natural_fading(&mut self, name: &str, t: &Transform) -> Option<Item> {
        if let Some(f) = self.picture_fadings.get(name) {
            return Some(f.mask(t));
        }
        let k = 2.0 * 2.54 / 72.0;
        if let Some(mask) = super::output::library_fading(name, &t.compose(&Transform::scaling(k, k))) {
            return Some(Item::Fade { mask });
        }
        match self.fadings.get(name).cloned() {
            Some(mut sh) => {
                let (crate::canvas::Shading::Axial { transform, .. } | crate::canvas::Shading::Radial { transform, .. }) = &mut sh;
                *transform = t.compose(transform);
                Some(Item::Fade { mask: sh })
            }
            None => {
                self.warn(format!("fading '{name}' not supported: drawn without it"));
                None
            }
        }
    }

    /// A picture fading for one path, fitted to it (`path fading` with `fit fading`).
    pub fn picture_fading_for(&self, name: &str, segs: &[Segment], fit: bool) -> Option<Item> {
        let f = self.picture_fadings.get(name)?;
        let mut c = Canvas::default();
        c.include_path(segs);
        let (lo, hi) = (c.min?, c.max?);
        let t = if fit { f.fit(lo, hi) } else { self.st.transform };
        let t = t.compose(&self.st.fading_transform);
        Some(f.mask(&t))
    }
}
