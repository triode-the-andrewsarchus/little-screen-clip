//! Shared map framing for compact previews on the desktop and ambient display.
use super::{
    basemap::{GlobeCamera, MAX_MAP_ZOOM, ProjectionMode, wrap_lon, wrap_lon_delta},
    places::{PLACES, PlaceKind},
};

#[derive(Clone, Copy)]
pub struct Context {
    bounds: [f64; 4],
    minimum_span: f64,
    pub city: Option<City>,
}

#[derive(Clone, Copy)]
pub struct City {
    pub name: &'static str,
    pub lat_lon: [f64; 2],
}

impl Context {
    pub fn new(mut bounds: [f64; 4]) -> Self {
        let [west, south, east, north] = bounds;
        let [lat, lon] = [south.midpoint(north), west.midpoint(east)];
        let minimum_span = if west == east && south == north {
            0.55
        } else {
            0.09
        };
        let city = ((east - west).max(north - south) < 2.0)
            .then(|| nearest_city(lat, lon))
            .flatten();
        if let Some(city) = city {
            let [city_lat, city_lon] = city.lat_lon;
            let city_lon = lon + wrap_lon_delta(city_lon - lon);
            bounds = [
                west.min(city_lon),
                south.min(city_lat),
                east.max(city_lon),
                north.max(city_lat),
            ];
        }
        Self {
            bounds,
            minimum_span,
            city,
        }
    }

    pub fn camera(self, [width, height]: [f32; 2]) -> GlobeCamera {
        let [west, south, east, north] = self.bounds;
        let mut camera = GlobeCamera {
            center_lat: south.midpoint(north),
            center_lon: wrap_lon(west.midpoint(east)),
            zoom: 1.0,
            mode: ProjectionMode::Map2D,
            orbit: Default::default(),
        };
        let cos_lat = camera.center_lat.to_radians().cos().abs().max(0.25);
        let horizontal = ((east - west) * cos_lat)
            .max(self.minimum_span)
            .to_radians();
        let vertical = (north - south).max(self.minimum_span).to_radians();
        let scale = (f64::from((width - 34.0).max(1.0)) / horizontal)
            .min(f64::from((height - 28.0).max(1.0)) / vertical);
        camera.zoom = (scale / camera.scale_px(width, height)).min(MAX_MAP_ZOOM);
        camera
    }
}

fn nearest_city(lat: f64, lon: f64) -> Option<City> {
    let longitude_scale = lat.to_radians().cos().abs();
    PLACES
        .iter()
        .filter_map(|(kind, name, _, _, [city_lat, city_lon], population)| {
            if *kind != PlaceKind::City || *population < 40_000.0 {
                return None;
            }
            let distance = (lat - city_lat).hypot(wrap_lon_delta(lon - city_lon) * longitude_scale);
            (distance < 1.5).then_some((
                distance,
                City {
                    name,
                    lat_lon: [*city_lat, *city_lon],
                },
            ))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, city)| city)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harbor_frame_includes_seattle_and_track() {
        let context = Context::new([-122.352, 47.616, -122.338, 47.630]);
        let city = context.city.unwrap();
        assert_eq!(city.name, "Seattle");
        for size @ [w, h] in [[240.0, 66.0], [400.0, 220.0]] {
            let camera = context.camera(size);
            for [lat, lon] in [city.lat_lon, [47.616, -122.352], [47.630, -122.338]] {
                let [x, y] = camera.project(lat, lon, w, h).unwrap();
                assert!(x > 0.0 && x < w && y > 0.0 && y < h);
            }
        }
    }

    #[test]
    fn lincoln_pin_keeps_the_nearby_city_and_regional_context_in_frame() {
        let [lat, lon] = [40.813599, -96.702606];
        let context = Context::new([lon, lat, lon, lat]);
        let city = context.city.unwrap();
        assert_eq!(city.name, "Lincoln");
        let [w, h] = [400.0, 220.0];
        let camera = context.camera([w, h]);
        let [x, y] = camera
            .project(city.lat_lon[0], city.lat_lon[1], w, h)
            .unwrap();
        assert!((w * 0.35..w * 0.65).contains(&x));
        assert!((h * 0.35..h * 0.65).contains(&y));
        let (north, _) = camera.unproject(w * 0.5, 0.0, w, h).unwrap();
        assert!(north - camera.center_lat > 0.25);
    }
}
