//! Three dimensions, as the `3d` and `perspective` libraries do them (`tikzlibrary3d.code.tex`,
//! `tikzlibraryperspective.code.tex`).
//!
//! The `3d` library adds the `xyz cylindrical` and `xyz spherical` coordinate systems, and puts the
//! canvas on a plane: `canvas is xy plane at z=1` (and the five others) and `canvas is plane` with
//! `plane origin`, `plane x`, `plane y` transform the picture so that the plane's points (0,0),
//! (1,0) and (0,1) land on those three points of the xyz system, with plain unit vectors on it.
//!
//! The `perspective` library sets the unit vectors from a view (`3d view={azimuth}{elevation}`,
//! `isometric view`), and gives the `three point perspective` (`tpp`) coordinate system: a point
//! (x,y,z) goes through the homogeneous matrix that `perspective={p=…,q=…,r=…}` sets from the
//! three vanishing points, then onto the unit vectors.

use crate::scene::{Point, Transform};
use crate::tex::Token;
use crate::Error;

use super::coords::Coord;
use super::options::{split_arguments, KeyFn};
use super::Engine;

/// The perspective's homogeneous matrix, rows `a`, `b`, `c` (x, y, z) and `d` (w), by columns x, y, z.
pub type Perspective = [[f64; 3]; 4];

/// Without vanishing points: the identity.
pub const NO_PERSPECTIVE: Perspective = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [0.0, 0.0, 0.0]];

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

/// A number, or 0 when empty (`canvas is xy plane at z` without a value).
fn number_or_zero(e: &mut Engine, s: &str) -> Result<f64, Error> {
    if s.trim().is_empty() {
        Ok(0.0)
    } else {
        e.number(s)
    }
}

impl Engine {
    /// `(x,y,z)` with the current unit vectors, before the transformation (`\pgfpointxyz`).
    fn xyz_local(&self, x: f64, y: f64, z: f64) -> Point {
        self.st.x.scale(x).plus(self.st.y.scale(y)).plus(self.st.z.scale(z))
    }

    /// A point of the picture in the current coordinate system, before the transformation, as
    /// `\tikz@scan@one@point` gives it.
    fn local_point(&mut self, text: &str) -> Result<Point, Error> {
        let inner = crate::keys::strip_braces(text.trim()).trim().trim_start_matches('(').trim_end_matches(')').to_string();
        let p = self.coordinate(&inner)?.p;
        Ok(self.st.transform.compose(&self.st.pending).inverse().apply(p))
    }

    /// `\tikz@canvas@is@plane`: the plane through `origin` with `x` and `y` at its (1,0) and (0,1)
    /// becomes the canvas, with unit vectors of 1cm and no z.
    fn canvas_is_plane(&mut self, origin: Point, x: Point, y: Point) {
        let (a, b) = (x.minus(origin), y.minus(origin));
        self.st.pending = self.st.pending.compose(&Transform { a: a.x, b: a.y, c: b.x, d: b.y, e: origin.x, f: origin.y });
        self.st.x = Point::new(1.0, 0.0);
        self.st.y = Point::new(0.0, 1.0);
        self.st.z = Point::new(0.0, 0.0);
    }

    /// The coordinate systems of the two libraries, or `None` for another name.
    pub fn three_d_coordinate(&mut self, system: &str, items: &[(String, String)]) -> Result<Option<Coord>, Error> {
        let get = |k: &str| items.iter().rev().find(|(a, _)| a == k).map(|(_, b)| b.clone());
        let num = |e: &mut Engine, k: &str| match get(k) {
            Some(v) => number_or_zero(e, &v),
            None => Ok(0.0),
        };
        let local = match system {
            "xyz cylindrical" => {
                // \pgfpointcylindrical: polar in the xy plane, then up along z
                let (angle, radius, z) = (num(self, "angle")?, num(self, "radius")?, num(self, "z")?);
                let (s, c) = angle.to_radians().sin_cos();
                self.xyz_local(radius * c, radius * s, z)
            }
            "xyz spherical" => {
                // \pgfpointspherical: the longitude turns from y toward x, the latitude up to z
                let longitude = match get("longitude") {
                    Some(v) => number_or_zero(self, &v)?,
                    None => num(self, "angle")?,
                };
                let (latitude, radius) = (num(self, "latitude")?, num(self, "radius")?);
                let (ls, lc) = longitude.to_radians().sin_cos();
                let (ts, tc) = latitude.to_radians().sin_cos();
                let around = self.st.x.scale(ls).plus(self.st.y.scale(lc));
                around.scale(tc).plus(self.st.z.scale(ts)).scale(radius)
            }
            "three point perspective" | "tpp" => {
                let (x, y, z) = (num(self, "x")?, num(self, "y")?, num(self, "z")?);
                let h = self.st.perspective;
                let w = h[3][0] * x + h[3][1] * y + h[3][2] * z + 1.0;
                let row = |r: [f64; 3]| (r[0] * x + r[1] * y + r[2] * z) / w;
                self.xyz_local(row(h[0]), row(h[1]), row(h[2]))
            }
            _ => return Ok(None),
        };
        Ok(Some(Coord { p: self.st.transform.apply(local), node: None }))
    }
}

/// `canvas is xy plane at z=…` and the others: the plane's x and y axes among x, y, z (0, 1, 2), and
/// the axis it is at.
fn on_plane(e: &mut Engine, v: Option<Vec<Token>>, x_axis: usize, y_axis: usize) -> Result<(), Error> {
    let s = text(e, &v)?;
    let at = number_or_zero(e, &s)?;
    let point = |e: &Engine, axis: Option<usize>| {
        let mut c = [0.0; 3];
        c[3 - x_axis - y_axis] = at;
        if let Some(a) = axis {
            c[a] = 1.0;
        }
        e.xyz_local(c[0], c[1], c[2])
    };
    let (o, x, y) = (point(e, None), point(e, Some(x_axis)), point(e, Some(y_axis)));
    e.canvas_is_plane(o, x, y);
    Ok(())
}

keyfn!(k_xy_plane, |e, v| on_plane(e, v, 0, 1));
keyfn!(k_yx_plane, |e, v| on_plane(e, v, 1, 0));
keyfn!(k_xz_plane, |e, v| on_plane(e, v, 0, 2));
keyfn!(k_zx_plane, |e, v| on_plane(e, v, 2, 0));
keyfn!(k_yz_plane, |e, v| on_plane(e, v, 1, 2));
keyfn!(k_zy_plane, |e, v| on_plane(e, v, 2, 1));

// `plane origin`, `plane x`, `plane y`: points kept for `canvas is plane`, read when it comes
keyfn!(k_plane_origin, |e, v| {
    e.st.plane[0] = Some(text(e, &v)?);
    Ok(())
});
keyfn!(k_plane_x, |e, v| {
    e.st.plane[1] = Some(text(e, &v)?);
    Ok(())
});
keyfn!(k_plane_y, |e, v| {
    e.st.plane[2] = Some(text(e, &v)?);
    Ok(())
});
keyfn!(k_canvas_is_plane, |e, _v| {
    let plane = e.st.plane.clone();
    let defaults = [Point::default(), e.st.x, e.st.y];
    let mut points = defaults;
    for (k, p) in plane.iter().enumerate() {
        if let Some(t) = p {
            points[k] = e.local_point(t)?;
        }
    }
    e.canvas_is_plane(points[0], points[1], points[2]);
    Ok(())
});

/// `3d view={azimuth}{elevation}`: the unit vectors of a rotated view, 1cm long.
fn view(e: &mut Engine, azimuth: f64, elevation: f64) {
    let (sa, ca) = azimuth.to_radians().sin_cos();
    let (se, ce) = elevation.to_radians().sin_cos();
    e.st.x = Point::new(ca, -sa * se);
    e.st.y = Point::new(sa, ca * se);
    e.st.z = Point::new(0.0, ce);
}

keyfn!(k_3d_view, |e, v| {
    let v = v.filter(|v| !crate::keys::trim(v).is_empty()).unwrap_or_else(|| super::tikra_tokens("{-30}{15}"));
    let args = split_arguments(&v, 2);
    let (az, el) = (e.number_of(args[0].clone())?, e.number_of(args[1].clone())?);
    view(e, az, el);
    Ok(())
});
keyfn!(k_isometric_view, |e, _v| {
    view(e, -45.0, (1.0 / 2.0f64.sqrt()).atan().to_degrees());
    Ok(())
});

/// `p={(x,y,z)}`, `q=…`, `r=…`: the vanishing point of the x, y or z direction (a column of the
/// matrix); one at 0 along its own axis leaves that direction without one.
fn vanishing_point(e: &mut Engine, axis: usize, value: &str) -> Result<(), Error> {
    let inner = crate::keys::strip_braces(value.trim()).trim().trim_start_matches('(').trim_end_matches(')').to_string();
    let parts = super::coords::split_commas(&inner);
    if parts.len() != 3 {
        return Err(Error::Figure(format!("vanishing point '{value}': three coordinates expected")));
    }
    let mut v = [0.0; 3];
    for (k, p) in parts.iter().enumerate() {
        v[k] = e.number(p)?;
    }
    let own = v[axis];
    if own != 0.0 {
        let h = &mut e.st.perspective;
        for (row, value) in v.iter().enumerate() {
            if row != axis {
                h[row][axis] = value / own;
            }
        }
        h[3][axis] = 1.0 / own;
    }
    Ok(())
}

keyfn!(k_perspective, |e, v| {
    let given = v.filter(|v| !crate::keys::trim(v).is_empty());
    let v = given.unwrap_or_else(|| super::tikra_tokens("p={(10,0,0)},q={(0,10,0)},r={(0,0,20)}"));
    for item in crate::keys::split_list(&v) {
        let value = e.tex.expand_to_string(item.value.unwrap_or_default())?;
        match item.key.as_str() {
            "p" => vanishing_point(e, 0, &value)?,
            "q" => vanishing_point(e, 1, &value)?,
            "r" => vanishing_point(e, 2, &value)?,
            other => e.warn(format!("perspective: key '{other}' not supported: ignored")),
        }
    }
    Ok(())
});

pub const KEYS: &[(&str, KeyFn)] = &[
    ("/tikz/canvas is xy plane at z", k_xy_plane),
    ("/tikz/canvas is yx plane at z", k_yx_plane),
    ("/tikz/canvas is xz plane at y", k_xz_plane),
    ("/tikz/canvas is zx plane at y", k_zx_plane),
    ("/tikz/canvas is yz plane at x", k_yz_plane),
    ("/tikz/canvas is zy plane at x", k_zy_plane),
    ("/tikz/plane origin", k_plane_origin),
    ("/tikz/plane x", k_plane_x),
    ("/tikz/plane y", k_plane_y),
    ("/tikz/canvas is plane", k_canvas_is_plane),
    ("/tikz/3d view", k_3d_view),
    ("/tikz/isometric view", k_isometric_view),
    ("/tikz/perspective", k_perspective),
];
