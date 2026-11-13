//! The slides of a figure with overlays as one SVG that advances on a click, with SMIL alone, so
//! it needs no script. Open it in a browser, or inline in a page: as an `<img>` it takes no clicks.

use std::fmt::Write;

use crate::{driver, Output};

/// The ids of one slide apart from the others', but the glyphs', the same in all of them.
fn apart(s: &str, slide: usize) -> String {
    let mut out = s.to_string();
    for key in [" id=\"", "url(#", "href=\"#"] {
        let mut text = String::with_capacity(out.len());
        let mut rest = out.as_str();
        while let Some(i) = rest.find(key) {
            let at = i + key.len();
            text.push_str(&rest[..at]);
            if !rest[at..].starts_with("tr-g") {
                let _ = write!(text, "s{slide}-");
            }
            rest = &rest[at..];
        }
        text.push_str(rest);
        out = text;
    }
    out
}

/// What a slide draws, and its box from the picture's origin.
fn parts(o: &Output) -> Option<(String, [f64; 4])> {
    let v = o.svg.split("viewBox=\"").nth(1)?.split('"').next()?;
    let v: Vec<f64> = v.split(' ').filter_map(|n| n.parse().ok()).collect();
    let [x, y, w, h] = v[..] else { return None };
    let body = o.svg.split("</metadata>").nth(1)?.strip_suffix("</svg>")?;
    Some((body.to_string(), [x - o.origin.0, y - o.origin.1, x - o.origin.0 + w, y - o.origin.1 + h]))
}

/// One SVG with every slide, the first on show, each click on to the next and back to the first.
/// The slides are laid over each other by the picture's origin, in the box they all fit in.
pub fn click_through(slides: &[Output]) -> Option<String> {
    let all: Vec<(String, [f64; 4])> = slides.iter().map(parts).collect::<Option<_>>()?;
    let last = slides.last()?;
    let fold = |pick: fn(&[f64; 4]) -> f64, f: fn(f64, f64) -> f64, start: f64| all.iter().map(|(_, b)| pick(b)).fold(start, f);
    let (x0, y0) = (fold(|b| b[0], f64::min, f64::MAX), fold(|b| b[1], f64::min, f64::MAX));
    let (x1, y1) = (fold(|b| b[2], f64::max, f64::MIN), fold(|b| b[3], f64::max, f64::MIN));
    let (w, h) = (x1 - x0, y1 - y0);
    let n = driver::n;
    let count = all.len();
    let mut s = String::new();
    let _ = write!(
        s,
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="{} {} {} {}" width="{}pt" height="{}pt" role="img" aria-label="{}" data-slides="{count}">"#,
        n(x0),
        n(y0),
        n(w),
        n(h),
        n(w),
        n(h),
        driver::escape(&last.alt)
    );
    for (k, (body, _)) in all.iter().enumerate() {
        let (origin, this) = (slides[k].origin, k + 1);
        let before = if this == 1 { count } else { this - 1 };
        // no `-` in the ids of a `begin`: SMIL would read a time offset
        let display = if this == 1 { "" } else { r#" display="none""# };
        let _ = write!(s, r#"<g id="tr-slide-{this}" transform="translate({} {})"{display}>"#, n(-origin.0), n(-origin.1));
        let _ = write!(
            s,
            r#"<set attributeName="display" to="inline" begin="trNext{before}.click" fill="freeze"/><set attributeName="display" to="none" begin="trNext{this}.click" fill="freeze"/>"#
        );
        s.push_str(&apart(body, this));
        let _ = write!(
            s,
            r#"<rect id="trNext{this}" x="{}" y="{}" width="{}" height="{}" fill="white" fill-opacity="0" style="cursor:pointer"/></g>"#,
            n(x0 + origin.0),
            n(y0 + origin.1),
            n(w),
            n(h)
        );
    }
    s.push_str("</svg>");
    Some(s)
}
