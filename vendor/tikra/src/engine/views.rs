//! View boxes: the views library's `meet={(a) (b) at (c) (d)}` and `slice=…` scope options, and the
//! basic layer's `\begin{pgfviewboxscope}{a}{b}{c}{d}{meet|slice}`. The box from `a` to `b` is
//! scaled (the same along both axes) and shifted onto the box from `c` to `d`: with `meet` all of it
//! fits, centered; with `slice` it fills the box and overflows it (pgf's pdf output does not clip
//! it). Neither the target box nor what is drawn in it counts in the picture's size, as in pgf. It
//! is a canvas transformation: lines and text are scaled too.

use crate::canvas::Item;
use crate::scene::{Point, Transform};
use crate::Error;

use super::Engine;

impl Engine {
    /// The view box `a`–`b` (user coordinates) onto `c`–`d`, from here to the end of the scope.
    pub fn view_box(&mut self, a: Point, b: Point, c: Point, d: Point, slice: bool) {
        let t = self.st.transform.compose(&self.st.pending);
        let (vw, vh) = ((b.x - a.x).abs().max(1e-9), (b.y - a.y).abs().max(1e-9));
        let (tw, th) = ((d.x - c.x).abs(), (d.y - c.y).abs());
        let s = if slice { (tw / vw).max(th / vh) } else { (tw / vw).min(th / vh) };
        let (vc, tc) = (a.lerp(b, 0.5), c.lerp(d, 0.5));
        // pgf neither clips to the target box (`slice` overflows it) nor counts it in the picture's size
        // what is drawn inside does not make the picture larger
        if let Some(sc) = self.scopes.last_mut() {
            sc.clipped = true;
        }
        // a canvas transformation: line widths and text are scaled too
        let m = Transform::translation(tc.x, tc.y).compose(&Transform::scaling(s, s)).compose(&Transform::translation(-vc.x, -vc.y));
        let canvas = t.compose(&m).compose(&t.inverse());
        self.open_in_scope(Item::Transform { transform: canvas });
    }

    /// `meet={(a) (b) at (c) (d)}` (or `slice`).
    pub fn view_key(&mut self, spec: &str, slice: bool) -> Result<(), Error> {
        let spec = spec.trim().trim_start_matches('{').trim_end_matches('}');
        let Some((view, target)) = spec.split_once(" at ") else {
            self.warn(format!("{}: '{spec}' is not '(a) (b) at (c) (d)': ignored", if slice { "slice" } else { "meet" }));
            return Ok(());
        };
        let inverse = self.st.transform.compose(&self.st.pending).inverse();
        let mut points = Vec::new();
        for part in [view, target] {
            for c in part.split(')').map(|c| c.trim().trim_start_matches('(').trim()).filter(|c| !c.is_empty()) {
                let p = self.coordinate(c)?.p;
                points.push(inverse.apply(p));
            }
        }
        let [a, b, c, d] = points[..] else {
            self.warn(format!("view box '{spec}': four points expected: ignored"));
            return Ok(());
        };
        self.view_box(a, b, c, d, slice);
        Ok(())
    }

    /// `\begin{pgfviewboxscope}{a}{b}{c}{d}{meet|slice}` (the name read).
    pub fn begin_view_box_scope(&mut self) -> Result<(), Error> {
        let mut points = [Point::default(); 4];
        for p in points.iter_mut() {
            *p = self.point_arg()?;
        }
        let kind = self.argument_string()?;
        self.begin_group();
        self.begin_scope();
        self.view_box(points[0], points[1], points[2], points[3], kind.trim() == "slice");
        self.st.apply_pending();
        Ok(())
    }
}
