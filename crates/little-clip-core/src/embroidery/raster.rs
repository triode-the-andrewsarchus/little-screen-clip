use super::{Design, Stitch, check_cancelled};
use crate::ImageThumbnail;
use resvg::tiny_skia::{LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap, Stroke, Transform};
use std::sync::atomic::AtomicBool;

mod bands;

const FABRIC: [u8; 3] = [191, 187, 178];
const MARGIN: f32 = 4.0;
const MAX_FRAMES: usize = 64;
const COLUMNS: usize = 8;
const MAX_DETAIL_EDGE: u32 = 4096;
const MAX_DETAIL_PIXELS: f32 = 8_388_608.0;
const SHADOW_WIDTH: f32 = 2.2;
const THREAD_WIDTH: f32 = 1.3;

/// Stitch-order previews packed into one bounded premultiplied texture.
pub struct Timeline {
    pub image: ImageThumbnail,
    pub layout: TimelineLayout,
}

/// Frame geometry and chronological thread colors, independent of the texture.
pub struct TimelineLayout {
    pub width: u32,
    pub height: u32,
    colors: Vec<[u8; 3]>,
    pub stitch_count: usize,
    /// Width and height of the stitches in millimeters, excluding raster padding.
    pub design_size_mm: [f32; 2],
}

impl TimelineLayout {
    /// Thread color at the end of each interval on the chronological strip.
    pub fn colors(&self) -> &[[u8; 3]] {
        &self.colors
    }

    /// The frame nearest a normalized position on the sewing timeline.
    pub fn frame(&self, fraction: f32) -> usize {
        (fraction.clamp(0.0, 1.0) * self.colors.len() as f32).round() as usize
    }

    /// Number of stitches shown at this frame. Frame zero is bare fabric.
    pub fn stitches_at(&self, frame: usize) -> usize {
        self.stitch_count * frame.min(self.colors.len()) / self.colors.len()
    }

    /// Texture coordinates of a frame, inset to keep filtering within its tile.
    pub fn uv(&self, frame: usize) -> [[f32; 2]; 2] {
        let frame = frame.min(self.colors.len());
        let [columns, rows] = grid(self.colors.len() + 1);
        let [x, y] = [(frame % COLUMNS) as f32, (frame / COLUMNS) as f32];
        let [inset_x, inset_y] = [0.5 / self.width as f32, 0.5 / self.height as f32];
        [
            [(x + inset_x) / columns as f32, (y + inset_y) / rows as f32],
            [
                (x + 1.0 - inset_x) / columns as f32,
                (y + 1.0 - inset_y) / rows as f32,
            ],
        ]
    }
}

impl Design {
    /// Renders the completed design with neither travel lines nor a hoop.
    pub fn thumbnail(
        &self,
        max: [u32; 2],
        cancelled: &AtomicBool,
    ) -> Result<ImageThumbnail, String> {
        let mut canvas = Canvas::new(self, max, None, FABRIC)?;
        canvas.sew(&self.stitches, cancelled)?;
        Ok(thumbnail(canvas.pixmap))
    }

    /// Renders the first `stitches` stitches at the requested size within the raster budget.
    ///
    /// `reference` is the size limit of the timeline tile whose framing and thread weights
    /// should be preserved. Only the selected prefix is rasterized.
    pub fn detailed_prefix(
        &self,
        reference: [u32; 2],
        max: [u32; 2],
        stitches: usize,
        cancelled: &AtomicBool,
    ) -> Result<ImageThumbnail, String> {
        check_cancelled(cancelled)?;
        let mut canvas = Canvas::new(self, reference, Some(max), FABRIC)?;
        let stitches = &self.stitches[..stitches.min(self.stitch_count())];
        if bands::worth_parallelizing(&canvas.pixmap, stitches.len()) {
            bands::sew(&mut canvas, stitches, cancelled)?;
        } else {
            canvas.sew(stitches, cancelled)?;
        }
        Ok(thumbnail(canvas.pixmap))
    }

    /// Rasterizes increasing stitch prefixes while keeping the finished framing fixed.
    pub fn timeline(
        &self,
        max: [u32; 2],
        samples: usize,
        cancelled: &AtomicBool,
    ) -> Result<Timeline, String> {
        self.timeline_on(max, samples, FABRIC, cancelled)
    }

    /// Rasterizes a stitch timeline on the given opaque RGB background.
    pub fn timeline_on(
        &self,
        max: [u32; 2],
        samples: usize,
        background: [u8; 3],
        cancelled: &AtomicBool,
    ) -> Result<Timeline, String> {
        let mut canvas = Canvas::new(self, max, None, background)?;
        let samples = samples.clamp(1, MAX_FRAMES - 1).min(self.stitch_count());
        let [columns, rows] = grid(samples + 1);
        let [width, height] = [canvas.pixmap.width(), canvas.pixmap.height()];
        let mut atlas = pixmap(width * columns as u32, height * rows as u32)?;
        let mut layout = TimelineLayout {
            width,
            height,
            colors: Vec::with_capacity(samples),
            stitch_count: self.stitch_count(),
            design_size_mm: self.size_mm(),
        };
        let mut previous = 0;
        for frame in 0..=samples {
            let end = self.stitch_count() * frame / samples;
            canvas.sew(&self.stitches[previous..end], cancelled)?;
            if end > 0 {
                layout.colors.push(self.stitches[end - 1].color);
            }
            previous = end;
            copy_tile(&mut atlas, &canvas.pixmap, frame);
        }
        Ok(Timeline {
            image: thumbnail(atlas),
            layout,
        })
    }
}

fn grid(frames: usize) -> [usize; 2] {
    [frames.min(COLUMNS), frames.div_ceil(COLUMNS)]
}

fn copy_tile(atlas: &mut Pixmap, tile: &Pixmap, frame: usize) {
    let width = tile.width() as usize * 4;
    let height = tile.height() as usize;
    let stride = atlas.width() as usize * 4;
    let x = frame % COLUMNS * width;
    let y = frame / COLUMNS * height;
    for (row, source) in tile.data().chunks_exact(width).enumerate() {
        let start = (row + y) * stride + x;
        atlas.data_mut()[start..start + width].copy_from_slice(source);
    }
}

struct Canvas {
    pixmap: Pixmap,
    origin: [i32; 2],
    scale: f32,
    offset: [f32; 2],
    raster_transform: Transform,
}

impl Canvas {
    fn new(
        design: &Design,
        reference: [u32; 2],
        target: Option<[u32; 2]>,
        [r, g, b]: [u8; 3],
    ) -> Result<Self, String> {
        let max = reference.map(|n| n.clamp(16, 512));
        let span: [f32; 2] = std::array::from_fn(|axis| {
            (i64::from(design.max[axis]) - i64::from(design.min[axis])) as f32
        });
        let scale = ((max[0] as f32 - 2.0 * MARGIN) / span[0].max(1.0))
            .min((max[1] as f32 - 2.0 * MARGIN) / span[1].max(1.0));
        let size: [u32; 2] = std::array::from_fn(|axis| {
            ((span[axis] * scale + 2.0 * MARGIN).ceil() as u32).clamp(16, max[axis])
        });
        let output = target.map_or(size, |target| detail_size(size, target));
        let mut pixmap = pixmap(output[0], output[1])?;
        pixmap.fill(resvg::tiny_skia::Color::from_rgba8(r, g, b, 255));
        Ok(Self {
            pixmap,
            origin: design.min,
            scale,
            offset: std::array::from_fn(|axis| (size[axis] as f32 - span[axis] * scale) * 0.5),
            raster_transform: Transform::from_scale(
                output[0] as f32 / size[0] as f32,
                output[1] as f32 / size[1] as f32,
            ),
        })
    }

    fn project(&self, point: [i32; 2]) -> [f32; 2] {
        std::array::from_fn(|axis| {
            (i64::from(point[axis]) - i64::from(self.origin[axis])) as f32 * self.scale
                + self.offset[axis]
        })
    }

    fn sew(&mut self, stitches: &[Stitch], cancelled: &AtomicBool) -> Result<(), String> {
        check_cancelled(cancelled)?;
        for group in stitch_groups(stitches) {
            check_cancelled(cancelled)?;
            self.stroke(group);
        }
        Ok(())
    }

    fn path(&self, stitches: &[Stitch]) -> Option<Path> {
        let mut path = PathBuilder::new();
        let mut previous = None;
        for stitch in stitches {
            let [x1, y1] = self.project(stitch.from);
            let [x2, y2] = self.project(stitch.to);
            if x1 == x2 && y1 == y2 {
                path.move_to(x1, y1);
                path.line_to(x2 + 0.01, y2);
                previous = None;
                continue;
            }
            // Join only touching stitches. Travel gaps still start a fresh contour.
            if previous != Some(stitch.from) {
                path.move_to(x1, y1);
            }
            path.line_to(x2, y2);
            previous = Some(stitch.to);
        }
        path.finish()
    }

    fn stroke(&mut self, stitches: &[Stitch]) {
        let Some(path) = self.path(stitches) else {
            return;
        };
        self.pixmap.stroke_path(
            &path,
            &shadow_paint(),
            &stroke_style(SHADOW_WIDTH),
            self.raster_transform,
            None,
        );
        self.pixmap.stroke_path(
            &path,
            &thread_paint(stitches[0].color),
            &stroke_style(THREAD_WIDTH),
            self.raster_transform,
            None,
        );
    }
}

fn stitch_groups(stitches: &[Stitch]) -> impl Iterator<Item = &[Stitch]> {
    stitches
        .chunk_by(|a, b| a.color == b.color)
        .flat_map(|run| run.chunks(2048))
}

fn stroke_style(width: f32) -> Stroke {
    Stroke {
        width,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Default::default()
    }
}

fn shadow_paint() -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color_rgba8(27, 24, 21, 155);
    paint
}

fn thread_paint([r, g, b]: [u8; 3]) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color_rgba8(r, g, b, 255);
    paint
}

fn detail_size(reference: [u32; 2], target: [u32; 2]) -> [u32; 2] {
    let [width, height] = reference.map(|n| n as f32);
    let [max_width, max_height] = target.map(|n| n.clamp(1, MAX_DETAIL_EDGE) as f32);
    let scale = (max_width / width)
        .min(max_height / height)
        .min((MAX_DETAIL_PIXELS / (width * height)).sqrt());
    reference.map(|n| (n as f32 * scale).floor().max(1.0) as u32)
}

fn pixmap(width: u32, height: u32) -> Result<Pixmap, String> {
    Pixmap::new(width, height).ok_or_else(|| "Could not allocate embroidery preview".into())
}

fn thumbnail(pixmap: Pixmap) -> ImageThumbnail {
    ImageThumbnail {
        width: pixmap.width(),
        height: pixmap.height(),
        premultiplied_rgba: pixmap.take(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atlas_frames_show_increasing_stitch_prefixes_in_a_fixed_view() {
        let design = Design {
            stitches: vec![
                Stitch {
                    from: [0, 0],
                    to: [0, 100],
                    color: [160, 22, 58],
                },
                Stitch {
                    from: [100, 0],
                    to: [100, 100],
                    color: [20, 77, 119],
                },
            ],
            min: [0, 0],
            max: [100, 100],
        };
        let cancelled = AtomicBool::new(false);
        let Timeline { image, layout } = design.timeline([64, 64], 40, &cancelled).unwrap();
        let pixel = |frame: usize, x: usize, y: usize| {
            let offset = (y * image.width as usize + frame * layout.width as usize + x) * 4;
            &image.premultiplied_rgba[offset..offset + 3]
        };
        assert_eq!(layout.frame(0.0), 0);
        assert_eq!(layout.stitches_at(layout.frame(0.5)), 1);
        assert_eq!(layout.stitches_at(layout.frame(1.0)), 2);
        assert_eq!(pixel(0, 4, 32), FABRIC);
        assert_ne!(pixel(1, 4, 32), FABRIC);
        assert_eq!(pixel(1, 60, 32), FABRIC);
        assert_eq!(pixel(2, 32, 32), FABRIC);
        assert_ne!(pixel(2, 60, 32), FABRIC);

        let detail = design
            .detailed_prefix([64, 64], [768, 512], 1, &cancelled)
            .unwrap();
        assert_eq!([detail.width, detail.height], [512, 512]);
        let detail_pixel = |x: usize, y: usize| {
            let offset = (y * detail.width as usize + x) * 4;
            &detail.premultiplied_rgba[offset..offset + 3]
        };
        assert_eq!(detail_pixel(32, 256), [160, 22, 58]);
        assert_eq!(detail_pixel(46, 256), FABRIC);
        assert_eq!(detail_pixel(480, 256), FABRIC);
    }

    #[test]
    fn parallel_bands_keep_travel_gaps_and_order_across_row_boundaries() {
        let design = Design::from_stitches(vec![
            Stitch {
                from: [0, 0],
                to: [0, 50],
                color: [160, 22, 58],
            },
            Stitch {
                from: [0, 50],
                to: [0, 100],
                color: [160, 22, 58],
            },
            Stitch {
                from: [100, 0],
                to: [100, 100],
                color: [160, 22, 58],
            },
            Stitch {
                from: [0, 62],
                to: [100, 62],
                color: [20, 77, 119],
            },
        ])
        .unwrap();
        let cancelled = AtomicBool::new(false);
        let mut canvas = Canvas::new(&design, [64, 64], Some([512, 512]), FABRIC).unwrap();
        let path = canvas.path(&design.stitches[..3]).unwrap();
        assert_eq!(
            path.segments()
                .filter(|op| matches!(op, resvg::tiny_skia::PathSegment::MoveTo(_)))
                .count(),
            2
        );

        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(4)
            .build()
            .unwrap();
        pool.install(|| bands::sew(&mut canvas, &design.stitches, &cancelled))
            .unwrap();
        let pixel = |x: usize, y: usize| {
            let start = (y * 512 + x) * 4;
            &canvas.pixmap.data()[start..start + 3]
        };
        for y in [127, 128, 255, 256, 383, 384] {
            assert_eq!(pixel(32, y), [160, 22, 58]);
            assert_eq!(pixel(256, y), FABRIC);
        }
        assert_eq!(pixel(32, 310), [20, 77, 119]);
    }
}
