use super::{Design, Stitch, check_cancelled};
use crate::ImageThumbnail;
use resvg::tiny_skia::{LineCap, LineJoin, Paint, PathBuilder, Pixmap, Stroke, Transform};
use std::sync::atomic::AtomicBool;

const FABRIC: [u8; 3] = [191, 187, 178];
const MARGIN: f32 = 4.0;
const MAX_FRAMES: usize = 64;
const COLUMNS: usize = 8;

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
        let mut canvas = Canvas::new(self, max)?;
        canvas.sew(&self.stitches, cancelled)?;
        Ok(thumbnail(canvas.pixmap))
    }

    /// Rasterizes increasing stitch prefixes while keeping the finished framing fixed.
    pub fn timeline(
        &self,
        max: [u32; 2],
        samples: usize,
        cancelled: &AtomicBool,
    ) -> Result<Timeline, String> {
        let mut canvas = Canvas::new(self, max)?;
        let samples = samples.clamp(1, MAX_FRAMES - 1).min(self.stitch_count());
        let [columns, rows] = grid(samples + 1);
        let [width, height] = [canvas.pixmap.width(), canvas.pixmap.height()];
        let mut atlas = pixmap(width * columns as u32, height * rows as u32)?;
        let mut layout = TimelineLayout {
            width,
            height,
            colors: Vec::with_capacity(samples),
            stitch_count: self.stitch_count(),
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
}

impl Canvas {
    fn new(design: &Design, max: [u32; 2]) -> Result<Self, String> {
        let max = max.map(|n| n.clamp(16, 512));
        let span: [f32; 2] = std::array::from_fn(|axis| {
            (i64::from(design.max[axis]) - i64::from(design.min[axis])) as f32
        });
        let scale = ((max[0] as f32 - 2.0 * MARGIN) / span[0].max(1.0))
            .min((max[1] as f32 - 2.0 * MARGIN) / span[1].max(1.0));
        let size: [u32; 2] = std::array::from_fn(|axis| {
            ((span[axis] * scale + 2.0 * MARGIN).ceil() as u32).clamp(16, max[axis])
        });
        let mut pixmap = pixmap(size[0], size[1])?;
        pixmap.fill(resvg::tiny_skia::Color::from_rgba8(
            FABRIC[0], FABRIC[1], FABRIC[2], 255,
        ));
        Ok(Self {
            pixmap,
            origin: design.min,
            scale,
            offset: std::array::from_fn(|axis| (size[axis] as f32 - span[axis] * scale) * 0.5),
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
        for run in stitches.chunk_by(|a, b| a.color == b.color) {
            for group in run.chunks(2048) {
                check_cancelled(cancelled)?;
                self.stroke(group);
            }
        }
        Ok(())
    }

    fn stroke(&mut self, stitches: &[Stitch]) {
        let mut path = PathBuilder::new();
        for stitch in stitches {
            let [x1, y1] = self.project(stitch.from);
            let [mut x2, y2] = self.project(stitch.to);
            if x1 == x2 && y1 == y2 {
                // A stationary needle entry still leaves a visible point.
                x2 += 0.01;
            }
            path.move_to(x1, y1);
            path.line_to(x2, y2);
        }
        let Some(path) = path.finish() else { return };
        let [red, green, blue] = stitches[0].color;
        let mut paint = Paint::default();
        let mut stroke = Stroke {
            width: 2.2,
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            ..Default::default()
        };
        paint.set_color_rgba8(27, 24, 21, 155);
        self.pixmap
            .stroke_path(&path, &paint, &stroke, Transform::identity(), None);
        stroke.width = 1.3;
        paint.set_color_rgba8(red, green, blue, 255);
        self.pixmap
            .stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }
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
    }
}
