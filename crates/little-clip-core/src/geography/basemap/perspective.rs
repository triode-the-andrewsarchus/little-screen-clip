//! A surface-targeted orbit camera shared by terrain, picking and sky rendering.

use super::{GlobeCamera, wrap_lon};

pub const EARTH_RADIUS_M: f64 = 6_371_000.0;
pub const MOON_RADIUS_M: f64 = 1_737_400.0;

/// Camera orientation and the installed terrain under its target and eye.
#[derive(Clone, Copy, Debug, Default)]
pub struct Orbit {
    pub tilt: f64,
    pub bearing: f64,
    pub target_height_m: f64,
    pub eye_ground_m: f64,
}

impl Orbit {
    pub fn active(self) -> bool {
        self.tilt != 0.0 || self.bearing != 0.0
    }

    pub fn drag(&mut self, dx: f32, dy: f32) {
        self.tilt = (self.tilt + f64::from(dy) * 0.20).clamp(0.0, 85.0);
        self.bearing = wrap_lon(self.bearing + f64::from(dx) * 0.25);
    }
}

/// A pinhole camera in body-fixed coordinates, with distances in body radii.
#[derive(Clone, Copy, Debug)]
pub struct Perspective {
    pub eye: [f64; 3],
    /// Screen right, screen up and camera backward in body-fixed coordinates.
    pub axes: [[f64; 3]; 3],
    pub focal: f64,
    pub radius_m: f64,
    pub reference_radius: f64,
    size: [f64; 2],
}

impl Perspective {
    pub fn new(camera: &GlobeCamera, [width, height]: [f32; 2], radius_m: f64) -> Self {
        let (sp, cp) = camera.center_lat.to_radians().sin_cos();
        let (sl, cl) = camera.center_lon.to_radians().sin_cos();
        let normal = [cp * cl, cp * sl, sp];
        let east = [-sl, cl, 0.0];
        let north = [-sp * cl, -sp * sl, cp];
        let (sb, cb) = camera.orbit.bearing.to_radians().sin_cos();
        let (st, ct) = camera.orbit.tilt.to_radians().sin_cos();
        let right = combine(east, cb, north, -sb);
        let forward = combine(north, cb, east, sb);
        let back = combine(normal, ct, forward, -st);
        let focal = f64::from(width.min(height)) * 0.95;
        let ratio = focal / camera.scale_px(width, height);
        // At a distance match the original globe's angular radius. Near the
        // surface match the top-down map's center scale so the zoom feels continuous.
        let t = ((camera.zoom - 1.4) / 2.6).clamp(0.0, 1.0);
        let distance = (ratio.hypot(1.0) - 1.0) * (1.0 - t) + ratio * t;
        let reference_radius = 1.0 + camera.orbit.target_height_m.max(0.0) / radius_m;
        let target = normal.map(|component| component * reference_radius);
        let mut eye = combine(target, 1.0, back, distance);
        let altitude = dot(eye, eye).sqrt();
        let minimum = 1.0 + (camera.orbit.eye_ground_m.max(0.0) + 3.0) / radius_m;
        if altitude < minimum {
            eye = eye.map(|component| component * minimum / altitude);
        }
        let back = normalized(combine(eye, 1.0, target, -1.0));
        Self {
            eye,
            axes: [right, normalized(cross(back, right)), back],
            focal,
            radius_m,
            reference_radius,
            size: [f64::from(width), f64::from(height)],
        }
    }

    pub fn half_fov(self) -> [f32; 2] {
        self.size
            .map(|dimension| (dimension / (2.0 * self.focal)) as f32)
    }

    pub fn altitude(self) -> f64 {
        dot(self.eye, self.eye).sqrt() - 1.0
    }

    pub fn project(self, lat: f64, lon: f64, height_m: f64) -> Option<[f32; 2]> {
        let radius = 1.0 + height_m.max(0.0) / self.radius_m;
        let pos = unit(lat, lon).map(|x| x * radius);
        let delta = combine(pos, 1.0, self.eye, -1.0);
        // Elevated points can remain visible above the base sphere past the
        // horizon, or rise higher than the viewer in a nearby valley.
        if dot(pos, self.eye) < radius * radius {
            let distance = dot(delta, delta).sqrt();
            let ray = delta.map(|v| v / distance);
            if sphere_distance(self.eye, ray, 1.0).is_some_and(|t| t < distance - 1e-9) {
                return None;
            }
        }
        self.project_position(pos)
    }

    /// Projects a Cartesian point from an already resolved surface mesh.
    pub fn project_position(self, point: [f64; 3]) -> Option<[f32; 2]> {
        let relative = combine(point, 1.0, self.eye, -1.0);
        let [x, y, z] = self.axes.map(|axis| dot(axis, relative));
        let depth = -z;
        if depth <= 0.0 {
            return None;
        }
        Some([
            (self.size[0] * 0.5 + self.focal * x / depth) as f32,
            (self.size[1] * 0.5 - self.focal * y / depth) as f32,
        ])
    }

    /// Horizontal scale at the center of the orbit view.
    pub fn meters_per_pixel(self) -> Option<f64> {
        let (_, depth) = self.ground((self.size[0] * 0.5) as f32, (self.size[1] * 0.5) as f32)?;
        Some(depth * self.radius_m / self.focal)
    }

    pub fn ground(self, x: f32, y: f32) -> Option<([f64; 3], f64)> {
        let x = (f64::from(x) - self.size[0] * 0.5) / self.focal;
        let y = (self.size[1] * 0.5 - f64::from(y)) / self.focal;
        let [right, up, back] = self.axes;
        let ray = normalized(combine(combine(right, x, up, y), 1.0, back, -1.0));
        let distance = sphere_distance(self.eye, ray, self.reference_radius)?;
        Some((combine(self.eye, 1.0, ray, distance), distance))
    }

    pub fn unproject(self, x: f32, y: f32) -> Option<(f64, f64)> {
        self.ground(x, y).map(|(point, _)| coordinates(point))
    }
}

pub fn coordinates([x, y, z]: [f64; 3]) -> (f64, f64) {
    (z.atan2(x.hypot(y)).to_degrees(), y.atan2(x).to_degrees())
}

pub fn sphere_distance(eye: [f64; 3], ray: [f64; 3], radius: f64) -> Option<f64> {
    let along = -dot(eye, ray);
    let across = cross(eye, ray);
    let remainder = radius * radius - dot(across, across);
    (along > 0.0 && remainder >= 0.0).then(|| along - remainder.sqrt())
}

pub fn unit(lat: f64, lon: f64) -> [f64; 3] {
    let (sp, cp) = lat.to_radians().sin_cos();
    let (sl, cl) = lon.to_radians().sin_cos();
    [cp * cl, cp * sl, sp]
}

fn combine(a: [f64; 3], x: f64, b: [f64; 3], y: f64) -> [f64; 3] {
    std::array::from_fn(|i| a[i] * x + b[i] * y)
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross([ax, ay, az]: [f64; 3], [bx, by, bz]: [f64; 3]) -> [f64; 3] {
    [ay * bz - az * by, az * bx - ax * bz, ax * by - ay * bx]
}

fn normalized(v: [f64; 3]) -> [f64; 3] {
    let length = dot(v, v).sqrt();
    v.map(|x| x / length)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tilt_stops_above_the_horizon_and_returns_to_top_down() {
        let mut orbit = Orbit::default();
        orbit.drag(0.0, 900.0);
        assert_eq!(orbit.tilt, 85.0);
        orbit.drag(720.0, -900.0);
        assert_eq!(orbit.tilt, 0.0);
        assert_eq!(orbit.bearing.abs(), 180.0);
        orbit.drag(720.0, 0.0);
        assert!(!orbit.active());
    }

    #[test]
    fn tilted_camera_unprojects_the_target_and_exposes_sky() {
        let camera = GlobeCamera {
            center_lat: 40.0,
            center_lon: 179.9,
            zoom: 180.0,
            orbit: Orbit {
                tilt: 80.0,
                bearing: 41.0,
                target_height_m: 1500.0,
                ..Default::default()
            },
            ..GlobeCamera::default()
        };
        let pose = Perspective::new(&camera, [1200.0, 800.0], EARTH_RADIUS_M);
        let target = pose.project(40.0, 179.9, 1500.0).unwrap();
        assert_eq!(target, [600.0, 400.0]);
        let (lat, lon) = pose.unproject(target[0], target[1]).unwrap();
        assert!((lat - 40.0).abs() < 1e-8 && (lon - 179.9).abs() < 1e-8);
        assert!(pose.unproject(600.0, 0.0).is_none());
        let raised = pose.project(40.0, 179.9, 3100.0).unwrap();
        assert!(raised[1] < 390.0);
    }

    #[test]
    fn lunar_pan_and_scale_use_the_rendered_perspective() {
        let mut camera = GlobeCamera::default();
        let size = [900.0, 600.0];
        let pose = Perspective::new(&camera, size, MOON_RADIUS_M);
        let expected = pose.unproject(410.0, 325.0).unwrap();
        camera.pan_perspective_by([40.0, -25.0], size, MOON_RADIUS_M);
        assert!((camera.center_lat - expected.0).abs() < 1e-8);
        assert!((camera.center_lon - expected.1).abs() < 1e-8);
        assert!(pose.meters_per_pixel().unwrap() < MOON_RADIUS_M / camera.scale_px(900.0, 600.0));
    }

    #[test]
    fn the_camera_clears_ground_behind_a_tilted_target() {
        let camera = GlobeCamera {
            zoom: 800.0,
            orbit: Orbit {
                tilt: 85.0,
                target_height_m: 1000.0,
                eye_ground_m: 4500.0,
                ..Default::default()
            },
            ..GlobeCamera::default()
        };
        let pose = Perspective::new(&camera, [900.0, 600.0], EARTH_RADIUS_M);
        assert!(pose.altitude() * EARTH_RADIUS_M >= 4502.999);
        assert_eq!(
            pose.project(camera.center_lat, camera.center_lon, 1000.0),
            Some([450.0, 300.0])
        );
    }
}
