//! Worker-side geography thumbnails for the hardware and workbench cards.
use little_clip_core::{
    ImageThumbnail,
    geography::{
        Document, Kind,
        basemap::{EquirectTexture, wrap_lon_delta},
        preview::{City, Context},
        trajectory::Trajectory,
    },
};
use std::{
    path::{Path, PathBuf},
    time::Instant,
};
use tiny_skia::{Color, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Stroke, Transform};

const WIDTH: u32 = 400;
const HEIGHT: u32 = 220;
const SIZE: [f32; 2] = [WIDTH as f32, HEIGHT as f32];

pub(crate) struct Map {
    pub thumbnail: ImageThumbnail,
    pub label: Option<String>,
}

type Surface = (PathBuf, Instant, Option<EquirectTexture>);

#[derive(Default)]
pub(crate) struct Cache {
    surface: Option<Surface>,
}

impl Cache {
    pub fn earth(&mut self, root: &Path) -> Option<EquirectTexture> {
        let reusable = self.surface.as_ref().is_some_and(|(dir, time, texture)| {
            dir == root && (texture.is_some() || time.elapsed().as_secs() < 10)
        });
        if !reusable {
            self.surface = Some((
                root.to_owned(),
                Instant::now(),
                EquirectTexture::cached_overview(root),
            ));
        }
        self.surface
            .as_ref()
            .and_then(|(_, _, texture)| texture.clone())
    }
}

impl Map {
    pub fn earth(document: &Document, texture: Option<&EquirectTexture>) -> Option<Self> {
        let context = Context::new(document.geographic_bounds?);
        let camera = context.camera(SIZE);
        let mut pixmap = Pixmap::new(WIDTH, HEIGHT)?;
        for (i, pixel) in pixmap.pixels_mut().iter_mut().enumerate() {
            let [x, y] = [(i as u32 % WIDTH) as f32, (i as u32 / WIDTH) as f32];
            let color = camera.unproject(x + 0.5, y + 0.5, SIZE[0], SIZE[1]).map_or(
                [12, 28, 41],
                |(lat, lon)| {
                    texture.map_or_else(
                        || graticule(lat, lon, &camera),
                        |texture| {
                            texture
                                .sample_rgb(lat, lon)
                                .map(|channel| (f32::from(channel) * 0.82) as u8)
                        },
                    )
                },
            );
            *pixel = Color::from_rgba8(color[0], color[1], color[2], 255)
                .premultiply()
                .to_color_u8();
        }

        for feature in &document.features {
            for path in &feature.paths {
                let mut relative = None;
                let points = path.iter().filter_map(|sample| {
                    let lon = sample.point.lon - camera.center_lon;
                    let previous = relative.unwrap_or(0.0);
                    let next = previous + wrap_lon_delta(lon - previous);
                    relative = Some(next);
                    camera.project_relative(sample.point.lat, next, SIZE[0], SIZE[1])
                });
                draw_track(&mut pixmap, points, feature.kind == Kind::Point);
            }
        }

        let label = context.city.map(|city: City| {
            let [lat, lon] = city.lat_lon;
            if let Some(point) = camera.project(lat, lon, SIZE[0], SIZE[1]) {
                draw_dot(
                    &mut pixmap,
                    point,
                    Color::from_rgba8(255, 210, 102, 255),
                    2.5,
                );
            }
            city.name.to_owned()
        });
        Some(Self::finish(pixmap, label))
    }

    pub fn solar(path: &Trajectory) -> Option<Self> {
        let mut pixmap = Pixmap::new(WIDTH, HEIGHT)?;
        pixmap.fill(Color::from_rgba8(12, 21, 35, 255));
        let projected =
            |[x, y, z]: [f64; 3]| [x * 0.98 + y * 0.20, -(y * 0.58 - x * 0.12 + z * 0.80)];
        let mut min = [0.0_f64; 2];
        let mut max = [0.0_f64; 2];
        for point in path.positions().map(projected) {
            for (i, axis) in point.into_iter().enumerate() {
                min[i] = min[i].min(axis);
                max[i] = max[i].max(axis);
            }
        }
        let span = std::array::from_fn::<_, 2, _>(|i| (max[i] - min[i]).max(0.001));
        let scale = (f64::from(SIZE[0] - 36.0) / span[0]).min(f64::from(SIZE[1] - 36.0) / span[1]);
        let project = |point| {
            let xy = projected(point);
            std::array::from_fn(|i| {
                SIZE[i] * 0.5 + ((xy[i] - min[i].midpoint(max[i])) * scale) as f32
            })
        };
        draw_track(&mut pixmap, path.positions().map(project), false);
        draw_dot(
            &mut pixmap,
            project([0.0; 3]),
            Color::from_rgba8(255, 169, 73, 255),
            5.0,
        );
        Some(Self::finish(pixmap, Some(path.name.clone())))
    }

    fn finish(pixmap: Pixmap, label: Option<String>) -> Self {
        let thumbnail = ImageThumbnail {
            width: WIDTH,
            height: HEIGHT,
            premultiplied_rgba: pixmap.take(),
        };
        Self { thumbnail, label }
    }
}

fn graticule(
    lat: f64,
    lon: f64,
    camera: &little_clip_core::geography::basemap::GlobeCamera,
) -> [u8; 3] {
    let degrees_per_pixel = camera.scale_px(SIZE[0], SIZE[1]).recip().to_degrees();
    let step = 10.0_f64.powf((degrees_per_pixel * 36.0).log10().ceil());
    let near = |axis: f64| {
        let fraction = (axis / step).rem_euclid(1.0);
        fraction.min(1.0 - fraction) * step < degrees_per_pixel
    };
    if near(lat) || near(lon) {
        [39, 70, 83]
    } else {
        [22, 48, 66]
    }
}

fn draw_track(pixmap: &mut Pixmap, mut points: impl Iterator<Item = [f32; 2]>, dots: bool) {
    let Some(first) = points.next() else { return };
    let track = Color::from_rgba8(115, 241, 211, 255);
    if dots {
        for point in std::iter::once(first).chain(points) {
            draw_dot(pixmap, point, track, 4.0);
        }
        return;
    }
    let mut path = PathBuilder::new();
    path.move_to(first[0], first[1]);
    let mut last = first;
    for point in points {
        path.line_to(point[0], point[1]);
        last = point;
    }
    if let Some(path) = path.finish() {
        let mut paint = Paint {
            anti_alias: true,
            ..Default::default()
        };
        let mut stroke = Stroke {
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            ..Default::default()
        };
        for (color, width) in [(Color::BLACK, 4.5), (track, 2.3)] {
            paint.set_color(color);
            stroke.width = width;
            pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
        }
    }
    // A track smaller than the contextual viewport still needs a visible position.
    if (last[0] - first[0]).abs() < 4.0 && (last[1] - first[1]).abs() < 4.0 {
        draw_dot(pixmap, first, track, 3.2);
    }
}

fn draw_dot(pixmap: &mut Pixmap, [x, y]: [f32; 2], color: Color, radius: f32) {
    let mut paint = Paint {
        anti_alias: true,
        ..Default::default()
    };
    for (color, radius) in [(Color::BLACK, radius + 1.4), (color, radius)] {
        if let Some(path) = PathBuilder::from_circle(x, y, radius) {
            paint.set_color(color);
            pixmap.fill_path(
                &path,
                &paint,
                tiny_skia::FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }
}
