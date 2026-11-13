//! The animations library, as a still picture shows it: `:fill = {0s="black", 2s="red"}` after a
//! command, `animate={myself:rotate={…}}` in options. A document drawn without `make snapshot of=t`
//! shows everything in its base state, so the animations do nothing; with a snapshot time, each
//! animated attribute takes its value at that time (between two times, numbers, lengths and colors
//! are interpolated; before the first, the first one holds; after the last, the animation is over,
//! unless it freezes, and the attribute is back to its base value), as an option. An
//! animation that waits for an event (`begin on=click`) has not started in a snapshot.

use crate::canvas::{Animation, Item, Moving};
use crate::driver::BP;
use crate::scene::{Point, Transform};
use crate::tex::{self, Cat, Token};
use crate::Error;

use super::{tikra_tokens, Engine};

/// `2s`, `50ms`, `1.5`: seconds.
fn seconds(s: &str) -> Option<f64> {
    let s = s.trim();
    if let Some(ms) = s.strip_suffix("ms") {
        return ms.trim().parse::<f64>().ok().map(|v| v / 1000.0);
    }
    if let Some(min) = s.strip_suffix("min") {
        return min.trim().parse::<f64>().ok().map(|v| v * 60.0);
    }
    s.strip_suffix('s').unwrap_or(s).trim().parse().ok()
}

/// A number followed by a unit (`2mm`, `90`): the number and the unit.
fn number_unit(s: &str) -> Option<(f64, &str)> {
    let s = s.trim();
    let end = s.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+')).unwrap_or(s.len());
    Some((s[..end].parse().ok()?, s[end..].trim()))
}

/// The value between `a` and `b` at `f` (0…1): numbers with their unit, or colors mixed as xcolor
/// mixes them.
fn between(a: &str, b: &str, f: f64) -> String {
    match (number_unit(a), number_unit(b)) {
        (Some((x, u)), Some((y, w))) if u == w || w.is_empty() => format!("{}{u}", super::pgf_number(x + (y - x) * f)),
        // (f is below 1: the times around it differ)
        _ if f <= 0.0 => a.to_string(),
        _ => format!("{b}!{}!{a}", super::pgf_number(f * 100.0)),
    }
}

/// The option an attribute becomes (`fill opacity` stays `fill opacity`, `position` is `shift`).
fn option_name(attribute: &str) -> &str {
    match attribute {
        "position" | "translate" => "shift",
        other => other,
    }
}

impl Engine {
    /// `make snapshot of=t` (in seconds).
    pub fn set_snapshot(&mut self, v: &str) {
        self.st.snapshot = seconds(v);
    }

    /// The option an animated attribute gives at the snapshot time (none without a snapshot).
    pub fn animation_option(&mut self, attribute: &str, spec: Vec<Token>) -> Result<Option<Vec<Token>>, Error> {
        // `whom:attribute`: the object is the one the options belong to here
        let whom = attribute.split(':').next().filter(|_| attribute.contains(':')).map(str::trim).unwrap_or("");
        let whom = whom.to_string();
        let attribute = attribute.rsplit(':').next().unwrap_or(attribute).trim().to_string();
        let Some(t) = self.st.snapshot else {
            self.animation_room(&attribute, &spec);
            if self.play {
                self.note_animation(&whom, &attribute, &spec);
            }
            return Ok(None);
        };
        let mut times: Vec<(f64, String)> = Vec::new();
        let mut begin = 0.0;
        let mut freeze = false;
        for item in crate::keys::split_list(&spec) {
            let value = item.value.map(|v| tex::tokens_to_string(&v)).unwrap_or_default();
            let value = value.trim().trim_matches('"').to_string();
            match item.key.as_str() {
                "begin" => begin = seconds(&value).unwrap_or(0.0),
                // waiting for an event: not started in a still picture
                "begin on" => return Ok(None),
                // in a snapshot, the animation started at this time
                "begin snapshot" => begin = seconds(&value).unwrap_or(0.0),
                "freeze" => freeze = true,
                "end" | "repeats" | "forever" | "restart" | "remove" | "whom" | "entry control" | "exit control" | "stay" | "jump" => {}
                key => {
                    if let Some(s) = seconds(key) {
                        times.push((s, value));
                    }
                }
            }
        }
        if times.is_empty() {
            return Ok(None);
        }
        times.sort_by(|a, b| a.0.total_cmp(&b.0));
        let at = t - begin;
        // over: the attribute is back to its base value, unless the animation freezes
        if at > times[times.len() - 1].0 && !freeze {
            return Ok(None);
        }
        let value = match times.iter().position(|(s, _)| *s > at) {
            Some(0) => times[0].1.clone(),
            None => times[times.len() - 1].1.clone(),
            Some(i) => {
                let ((s0, a), (s1, b)) = (&times[i - 1], &times[i]);
                between(a, b, if s1 > s0 { (at - s0) / (s1 - s0) } else { 1.0 })
            }
        };
        let mut v = tikra_tokens(&format!("{}=", option_name(&attribute)));
        v.push(Token::Char('{', Cat::Begin));
        v.extend(tikra_tokens(&value));
        v.push(Token::Char('}', Cat::End));
        // pgf animates the canvas: a rotation or a scaling turns the text and lines too
        if matches!(attribute.as_str(), "rotate" | "scale" | "xscale" | "yscale" | "xslant" | "yslant") {
            let mut w = tikra_tokens("transform canvas=");
            w.push(Token::Char('{', Cat::Begin));
            w.extend(v);
            w.push(Token::Char('}', Cat::End));
            return Ok(Some(w));
        }
        Ok(Some(v))
    }

    /// Played, not a snapshot: the animation, kept for the object being drawn.
    fn note_animation(&mut self, whom: &str, attribute: &str, spec: &[Token]) {
        if !matches!(whom, "" | "myself") {
            self.warn(format!("animation of {whom}: only an object's own animations are played"));
            return;
        }
        let mut raw: Vec<(f64, String)> = Vec::new();
        let (mut begin, mut on, mut repeats, mut freeze) = (0.0, None, None, false);
        for item in crate::keys::split_list(spec) {
            let value = item.value.map(|v| tex::tokens_to_string(&v)).unwrap_or_default();
            let value = value.trim().trim_matches('"').trim().to_string();
            match item.key.as_str() {
                "begin" => begin = seconds(&value).unwrap_or(0.0),
                "begin on" => {
                    let mut event = "click".to_string();
                    for part in value.split(',').map(str::trim) {
                        match part {
                            "click" | "mouse over" | "mouse down" | "mouse up" | "mouse out" | "mouse move" => event = part.replace(' ', ""),
                            p if p.starts_with("delay") => begin = p.split('=').nth(1).and_then(seconds).unwrap_or(0.0),
                            _ => {}
                        }
                    }
                    on = Some(event);
                }
                "repeats" if value.is_empty() || value == "forever" => repeats = Some("indefinite".to_string()),
                "repeats" => repeats = Some(value.split_whitespace().next().unwrap_or("1").to_string()),
                "forever" => repeats = Some("indefinite".to_string()),
                "freeze" => freeze = true,
                "begin snapshot" | "end" | "restart" | "remove" | "whom" | "entry control" | "exit control" | "stay" | "jump" => {}
                key => {
                    if let Some(s) = seconds(key) {
                        raw.push((s, value));
                    }
                }
            }
        }
        raw.sort_by(|a, b| a.0.total_cmp(&b.0));
        if raw.is_empty() {
            return;
        }
        let times: Vec<f64> = raw.iter().map(|r| r.0).collect();
        let one = |e: &mut Engine, moving: Moving, f: &dyn Fn(&mut Engine, &str) -> Option<String>| {
            let values: Option<Vec<String>> = raw.iter().map(|r| f(e, &r.1)).collect();
            match values {
                Some(values) => e.animations.push(Animation { moving, times: times.clone(), values, begin, on: on.clone(), repeats: repeats.clone(), freeze }),
                None => e.warn(format!("animation of {attribute}: a value is not understood; not played")),
            }
        };
        let color = |e: &mut Engine, v: &str| e.color(v).map(|c| c.hex());
        let number = |_: &mut Engine, v: &str| v.parse::<f64>().ok().map(|x| x.to_string());
        let width = |e: &mut Engine, v: &str| e.length(v).ok().map(|x| (x * BP).to_string());
        let shift = |e: &mut Engine, v: &str| -> Option<String> {
            let origin = e.st.transform.apply(Point::default());
            let d = e.coordinate(point_text(v)).ok()?.p.minus(origin);
            Some(format!("{} {}", d.x * BP, -d.y * BP))
        };
        match attribute {
            "fill" => one(self, Moving::Attribute("fill", false), &color),
            "draw" => one(self, Moving::Attribute("stroke", false), &color),
            "color" => {
                one(self, Moving::Attribute("fill", false), &color);
                one(self, Moving::Attribute("stroke", false), &color);
            }
            "text" => one(self, Moving::Attribute("fill", true), &color),
            "opacity" => one(self, Moving::Attribute("opacity", false), &number),
            "fill opacity" => one(self, Moving::Attribute("fill-opacity", false), &number),
            "draw opacity" => one(self, Moving::Attribute("stroke-opacity", false), &number),
            "line width" => one(self, Moving::Attribute("stroke-width", false), &width),
            "shift" | "position" | "translate" => one(self, Moving::Shift, &shift),
            "xshift" => one(self, Moving::Shift, &|e, v| e.length(v).ok().map(|x| format!("{} 0", x * BP))),
            "yshift" => one(self, Moving::Shift, &|e, v| e.length(v).ok().map(|y| format!("0 {}", -y * BP))),
            "rotate" => one(self, Moving::Rotation, &|_, v| v.parse::<f64>().ok().map(|a| (-a).to_string())),
            "scale" => one(self, Moving::Scaling, &number),
            "xscale" => one(self, Moving::Scaling, &|_, v| v.parse::<f64>().ok().map(|x| format!("{x} 1"))),
            "yscale" => one(self, Moving::Scaling, &|_, v| v.parse::<f64>().ok().map(|y| format!("1 {y}"))),
            other => self.warn(format!("animation of {other}: not played")),
        }
        self.animation_center.get_or_insert(self.st.transform.apply(Point::default()));
    }

    /// After a statement: what it drew, from `start`, moves as it asked (the animations it noted,
    /// from `from`). A scope's animations go over what it will draw, up to its end.
    pub fn place_animations(&mut self, start: usize, opens: bool, from: usize) {
        let animations = self.animations.split_off(from);
        let center = self.animation_center.take().unwrap_or_default();
        let item = Item::Animated { animations, center };
        if opens {
            self.canvas.push(item);
            self.animated_scopes.push(self.tex.group_depth());
        } else {
            let start = start.min(self.canvas.items.len());
            self.canvas.items.insert(start, item);
            self.canvas.push(Item::End);
        }
    }

    /// After `\\end{scope}`: the animations of the scopes that closed.
    pub fn close_animated_scopes(&mut self) {
        while self.animated_scopes.last().is_some_and(|&d| d > self.tex.group_depth()) {
            self.animated_scopes.pop();
            self.canvas.push(Item::End);
        }
    }

    /// The shifts an `xshift`, `yshift` or `shift` animation goes through: as pgf does, the picture
    /// makes room for the animated object at each of them.
    fn animation_room(&mut self, attribute: &str, spec: &[Token]) {
        if !matches!(attribute, "xshift" | "yshift" | "shift" | "position" | "translate") {
            return;
        }
        let linear = Transform { e: 0.0, f: 0.0, ..self.st.transform };
        for item in crate::keys::split_list(spec) {
            if seconds(&item.key).is_none() {
                continue;
            }
            let value = item.value.map(|v| tex::tokens_to_string(&v)).unwrap_or_default();
            let value = value.trim().trim_matches('"').trim().to_string();
            let d = match attribute {
                "xshift" => self.length(&value).map(|x| Point::new(x, 0.0)),
                "yshift" => self.length(&value).map(|y| Point::new(0.0, y)),
                _ => {
                    let origin = self.st.transform.apply(Point::default());
                    self.coordinate(point_text(&value)).map(|c| c.p.minus(origin))
                }
            };
            if let Ok(d) = d {
                self.anim_shifts.push(if attribute == "xshift" || attribute == "yshift" { linear.apply(d) } else { d });
            }
        }
    }

    /// `\pgfsysanimate{attribute}` in snapshot mode: the value the timeline gives the attribute at
    /// the snapshot time, an option of the next object named `whom` (pgf attaches it to the id).
    pub fn sys_animate(&mut self, attribute: &str, whom: &str, spec: Vec<Token>) -> Result<(), Error> {
        let attribute = match attribute {
            "fillopacity" => "fill opacity",
            "strokeopacity" => "draw opacity",
            "linewidth" => "line width",
            "opacity" => "opacity",
            // the other attributes keep their base value
            _ => return Ok(()),
        };
        if let Some(o) = self.animation_option(attribute, spec)? {
            let options = self.sys_animations.entry(whom.to_string()).or_default();
            if !options.is_empty() {
                options.push(Token::other(','));
            }
            options.extend(o);
        }
        Ok(())
    }

    /// `:attribute = {…}` after a command or in a path (the `:` read): the option it gives, if any.
    pub fn read_animation(&mut self) -> Result<Option<Vec<Token>>, Error> {
        let mut name = String::new();
        loop {
            match self.tex.get_token() {
                Some(t) if t.is_char('=') => break,
                Some(Token::Char(c, _)) => name.push(c),
                Some(t) => name.push_str(&tex::show_token(&t)),
                None => return Ok(None),
            }
        }
        let spec = match self.tex.next_nonspace() {
            Some(Token::Char(_, Cat::Begin)) => self.tex.read_group_body()?,
            Some(t) => vec![t],
            None => Vec::new(),
        };
        self.animation_option(name.trim(), spec)
    }

    /// `animate={…}`: its `whom:attribute={…}` entries (and styles that give them) at the snapshot.
    pub fn animate_key(&mut self, v: Vec<Token>) -> Result<(), Error> {
        for item in crate::keys::split_list(&v) {
            match item.value {
                // `myself: = { :rotate = {…}, :color = {…} }`: one object, several attributes
                Some(value) if item.key.trim_end().ends_with(':') => {
                    for sub in crate::keys::split_list(&value) {
                        if let Some(o) = self.animation_option(&sub.key, sub.value.unwrap_or_default())? {
                            self.process_keys(o, Some("/tikz"))?;
                        }
                    }
                }
                Some(value) if item.key.contains(':') => {
                    if let Some(o) = self.animation_option(&item.key, value)? {
                        self.process_keys(o, Some("/tikz"))?;
                    }
                }
                // a style of /tikz/animate (`shake = 1mm`): its entries come back here as TikZ keys
                value => {
                    let mut keys = tikra_tokens(&format!("/tikz/animate/{}", item.key));
                    if let Some(value) = value {
                        keys.push(Token::other('='));
                        keys.push(Token::Char('{', Cat::Begin));
                        keys.extend(value);
                        keys.push(Token::Char('}', Cat::End));
                    }
                    self.process_keys(keys, Some("/tikz"))?;
                }
            }
        }
        Ok(())
    }
}

/// The coordinate of a shift's value, `(x,y)` or braced `{(x,y)}`: what is inside the parentheses.
fn point_text(v: &str) -> &str {
    crate::keys::strip_braces(v).trim_start_matches('(').trim_end_matches(')')
}
