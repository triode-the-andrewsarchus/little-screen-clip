//! Retained canonical SVG paths painted directly at the current camera scale.
use super::{Canvas, OpenGl, femtovg as vg};
use std::{
    collections::HashMap,
    sync::{
        Arc, Weak,
        atomic::{AtomicU64, Ordering},
    },
};
use vg::{Color, FillRule, Paint, Path, Transform2D};

type ClipPaths = Vec<(usvg::tiny_skia_path::Path, usvg::FillRule)>;

static NEXT_SCENE: AtomicU64 = AtomicU64::new(1);

/// Immutable SVG and decoded images prepared on a bounded worker.
pub struct Scene {
    id: u64,
    tree: usvg::Tree,
    images: HashMap<usize, Arc<image::RgbaImage>>,
    clips: HashMap<usize, ClipPaths>,
}
impl Scene {
    /// Adopts a canonical, outlined tree and its admitted decoded PNG assets.
    ///
    /// # Errors
    ///
    /// Returns an error for effects not yet supported by this native backend.
    /// The caller can keep its existing raster preview with a visible notice.
    pub fn new(
        tree: usvg::Tree,
        images: HashMap<usize, Arc<image::RgbaImage>>,
    ) -> Result<Arc<Self>, String> {
        if images.len() > 64
            || images
                .values()
                .map(|image| u64::from(image.width()) * u64::from(image.height()))
                .sum::<u64>()
                > 8 * 1024 * 1024
        {
            return Err("Native SVG raster assets exceed the preview budget.".into());
        }
        validate_group(tree.root(), &images)?;
        let clips = tree
            .clip_paths()
            .iter()
            .map(|clip| {
                let mut paths = Vec::new();
                compile_clip(clip.root(), &mut paths)?;
                Ok((clip.as_ref() as *const usvg::ClipPath as usize, paths))
            })
            .collect::<Result<HashMap<_, _>, String>>()?;
        Ok(Arc::new(Self {
            id: NEXT_SCENE.fetch_add(1, Ordering::Relaxed),
            tree,
            images,
            clips,
        }))
    }

    /// Paints the scene with optional per-group translations for live gestures.
    ///
    /// `image_rect` places normalized SVG coordinates inside the clipped widget.
    /// Translations are in SVG units and keyed by retained SVG group IDs.
    ///
    /// # Errors
    ///
    /// Returns an error if this host has no healthy native vector renderer.
    pub fn paint(
        self: &Arc<Self>,
        ui: &egui::Ui,
        rect: egui::Rect,
        image_rect: egui::Rect,
        motion: HashMap<String, [f32; 2]>,
    ) -> Result<(), String> {
        let scene = self.clone();
        super::paint_with(ui, rect, move |canvas, scenes| {
            let cached = scenes.entry(scene.id).or_insert_with(|| Cached {
                scene: Arc::downgrade(&scene),
                paths: HashMap::new(),
                textures: HashMap::new(),
                clips: HashMap::new(),
            });
            canvas.save();
            canvas.translate(image_rect.left(), image_rect.top());
            canvas.scale(
                image_rect.width() / scene.tree.size().width(),
                image_rect.height() / scene.tree.size().height(),
            );
            let result = cached.group(canvas, scene.tree.root(), &scene, &motion);
            canvas.restore();
            result
        })
    }
}

fn validate_group(
    group: &usvg::Group,
    images: &HashMap<usize, Arc<image::RgbaImage>>,
) -> Result<(), String> {
    if group.mask().is_some()
        || !group.filters().is_empty()
        || group.blend_mode() != usvg::BlendMode::Normal
    {
        return Err(
            "This drawing's masks, filters or blend effects still use the raster preview.".into(),
        );
    }
    for node in group.children() {
        match node {
            usvg::Node::Group(child) => validate_group(child, images)?,
            usvg::Node::Path(path) => {
                for paint in [
                    path.fill().map(|fill| fill.paint()),
                    path.stroke().map(|stroke| stroke.paint()),
                ]
                .into_iter()
                .flatten()
                {
                    validate_brush(paint)?;
                }
            }
            usvg::Node::Image(image) => {
                let usvg::ImageKind::PNG(bytes) = image.kind() else {
                    return Err("Native SVG images need worker-decoded PNG assets.".into());
                };
                if !images.contains_key(&(Arc::as_ptr(bytes) as usize)) {
                    return Err("Native SVG image pixels were not prepared on the worker.".into());
                }
            }
            usvg::Node::Text(_) => {
                return Err("Native SVG text must be outlined on its worker.".into());
            }
        }
    }
    Ok(())
}

fn validate_brush(paint: &usvg::Paint) -> Result<(), String> {
    let supported = match paint {
        usvg::Paint::Color(_) => true,
        usvg::Paint::LinearGradient(g) => g.spread_method() == usvg::SpreadMethod::Pad,
        usvg::Paint::RadialGradient(g) => g.spread_method() == usvg::SpreadMethod::Pad,
        usvg::Paint::Pattern(_) => false,
    };
    if supported {
        Ok(())
    } else {
        Err("SVG patterns and repeating gradients still use the raster preview.".into())
    }
}

// Resolve clip-local transforms once on the worker, instead of allocating
// and rewalking clip geometry for every pointer or camera frame.
fn compile_clip(group: &usvg::Group, output: &mut ClipPaths) -> Result<(), String> {
    if group.clip_path().is_some() {
        return Err("Nested SVG clip unions still use the raster preview.".into());
    }
    for node in group.children() {
        match node {
            usvg::Node::Group(group) => compile_clip(group, output)?,
            usvg::Node::Path(path) => {
                if path.is_visible()
                    && let Some(data) = path.data().clone().transform(path.abs_transform())
                {
                    output.push((
                        data,
                        path.fill()
                            .map_or(usvg::FillRule::NonZero, |fill| fill.rule()),
                    ));
                }
            }
            _ => return Err("Unsupported SVG clip content.".into()),
        }
    }
    Ok(())
}

pub(super) struct Cached {
    scene: Weak<Scene>,
    paths: HashMap<usize, Path>,
    textures: HashMap<usize, vg::ImageId>,
    clips: HashMap<usize, Vec<(Path, FillRule)>>,
}
impl Cached {
    pub(super) fn alive(&self) -> bool {
        self.scene.strong_count() > 0
    }
    pub(super) fn release(&self, canvas: &mut Canvas<OpenGl>) {
        for image in self.textures.values() {
            canvas.delete_image(*image);
        }
    }

    fn group(
        &mut self,
        canvas: &mut Canvas<OpenGl>,
        group: &usvg::Group,
        scene: &Scene,
        motion: &HashMap<String, [f32; 2]>,
    ) -> Result<(), String> {
        canvas.save();
        apply_transform(canvas, group.transform());
        if let Some([x, y]) = motion.get(group.id()) {
            canvas.translate(*x, *y);
        }
        if let Some(clip) = group.clip_path() {
            self.clip(canvas, clip, scene);
        }
        let layer = group.opacity().get() != 1.0 || group.isolate();
        let captured = !layer
            || canvas.begin_layer(&vg::LayerEffects::new().with_opacity(group.opacity().get()));
        let result = if captured {
            self.children(canvas, group, scene, motion)
        } else {
            Err("Native SVG opacity layer allocation failed.".into())
        };
        if layer {
            canvas.end_layer();
        }
        canvas.restore();
        result
    }

    fn children(
        &mut self,
        canvas: &mut Canvas<OpenGl>,
        group: &usvg::Group,
        scene: &Scene,
        motion: &HashMap<String, [f32; 2]>,
    ) -> Result<(), String> {
        for node in group.children() {
            match node {
                usvg::Node::Group(group) => self.group(canvas, group, scene, motion)?,
                usvg::Node::Path(path) => self.path(canvas, path),
                usvg::Node::Image(image) => {
                    canvas.save();
                    // Image fitting/cropping is normalized into its absolute
                    // transform. Retain the caller's camera and current layer
                    // origin by applying only the transform relative to its group.
                    if let Some(inverse) = group.abs_transform().invert() {
                        apply_transform(canvas, inverse.pre_concat(image.abs_transform()));
                        let result = self.image(canvas, image, scene);
                        canvas.restore();
                        result?;
                    } else {
                        canvas.restore();
                    }
                }
                usvg::Node::Text(_) => {} // rejected during worker preparation
            }
        }
        Ok(())
    }

    fn path(&mut self, canvas: &mut Canvas<OpenGl>, path: &usvg::Path) {
        if !path.is_visible() {
            return;
        }
        let native = self
            .paths
            .entry(path as *const usvg::Path as usize)
            .or_insert_with(|| path_data(path.data()));
        let fill = path.fill().map(|fill| {
            brush(fill.paint(), fill.opacity().get()).with_fill_rule(rule(fill.rule()))
        });
        let stroke = path.stroke().map(|stroke| {
            brush(stroke.paint(), stroke.opacity().get())
                .with_line_width(stroke.width().get())
                .with_miter_limit(stroke.miterlimit().get())
                .with_line_cap(match stroke.linecap() {
                    usvg::LineCap::Butt => vg::LineCap::Butt,
                    usvg::LineCap::Round => vg::LineCap::Round,
                    usvg::LineCap::Square => vg::LineCap::Square,
                })
                .with_line_join(match stroke.linejoin() {
                    usvg::LineJoin::Round => vg::LineJoin::Round,
                    usvg::LineJoin::Bevel => vg::LineJoin::Bevel,
                    _ => vg::LineJoin::Miter,
                })
                .with_line_dash(stroke.dasharray().unwrap_or_default())
                .with_line_dash_offset(stroke.dashoffset())
        });
        let fill_path = |canvas: &mut Canvas<OpenGl>| {
            if let Some(fill) = &fill {
                canvas.fill_path(native, fill);
            }
        };
        let stroke_path = |canvas: &mut Canvas<OpenGl>| {
            if let Some(stroke) = &stroke {
                canvas.stroke_path(native, stroke);
            }
        };
        match path.paint_order() {
            usvg::PaintOrder::FillAndStroke => {
                fill_path(canvas);
                stroke_path(canvas);
            }
            usvg::PaintOrder::StrokeAndFill => {
                stroke_path(canvas);
                fill_path(canvas);
            }
        }
    }

    fn clip(&mut self, canvas: &mut Canvas<OpenGl>, clip: &usvg::ClipPath, scene: &Scene) {
        if let Some(parent) = clip.clip_path() {
            self.clip(canvas, parent, scene);
        }
        let key = clip as *const usvg::ClipPath as usize;
        let paths = self.clips.entry(key).or_insert_with(|| {
            scene
                .clips
                .get(&key)
                .into_iter()
                .flatten()
                .map(|(path, fill)| (path_data(path), rule(*fill)))
                .collect()
        });
        let before = canvas.transform();
        apply_transform(canvas, clip.transform());
        let refs: Vec<_> = paths.iter().map(|(path, fill)| (path, *fill)).collect();
        canvas.clip_paths(&refs);
        canvas.reset_transform();
        canvas.set_transform(&before);
    }

    fn image(
        &mut self,
        canvas: &mut Canvas<OpenGl>,
        image: &usvg::Image,
        scene: &Scene,
    ) -> Result<(), String> {
        use vg::rgb::FromSlice;
        if !image.is_visible() {
            return Ok(());
        }
        let usvg::ImageKind::PNG(bytes) = image.kind() else {
            return Err("Missing native SVG image.".into());
        };
        let key = Arc::as_ptr(bytes) as usize;
        let texture = if let Some(texture) = self.textures.get(&key) {
            *texture
        } else {
            let pixels = scene
                .images
                .get(&key)
                .ok_or("Missing worker-decoded SVG raster.")?;
            let source = vg::imgref::Img::new(
                pixels.as_raw().as_rgba(),
                pixels.width() as usize,
                pixels.height() as usize,
            );
            let texture = canvas
                .create_image(vg::ImageSource::Rgba(source), vg::ImageFlags::empty())
                .map_err(|error| error.to_string())?;
            self.textures.insert(key, texture);
            texture
        };
        let mut path = Path::new();
        path.rect(0.0, 0.0, image.size().width(), image.size().height());
        canvas.fill_path(
            &path,
            &Paint::image(
                texture,
                0.0,
                0.0,
                image.size().width(),
                image.size().height(),
                0.0,
                1.0,
            ),
        );
        Ok(())
    }
}

fn apply_transform(canvas: &mut Canvas<OpenGl>, transform: usvg::Transform) {
    // FemtoVG's set_transform concatenates, despite its name.
    canvas.set_transform(&matrix(transform));
}
fn matrix(t: usvg::Transform) -> Transform2D {
    Transform2D([t.sx, t.ky, t.kx, t.sy, t.tx, t.ty])
}
fn rule(rule: usvg::FillRule) -> FillRule {
    match rule {
        usvg::FillRule::NonZero => FillRule::NonZero,
        usvg::FillRule::EvenOdd => FillRule::EvenOdd,
    }
}
fn path_data(path: &usvg::tiny_skia_path::Path) -> Path {
    let mut output = Path::new();
    for segment in path.segments() {
        use usvg::tiny_skia_path::PathSegment;
        match segment {
            PathSegment::MoveTo(p) => output.move_to(p.x, p.y),
            PathSegment::LineTo(p) => output.line_to(p.x, p.y),
            PathSegment::QuadTo(a, b) => output.quad_to(a.x, a.y, b.x, b.y),
            PathSegment::CubicTo(a, b, c) => output.bezier_to(a.x, a.y, b.x, b.y, c.x, c.y),
            PathSegment::Close => output.close(),
        }
    }
    output
}

fn rgba(color: usvg::Color, alpha: f32) -> Color {
    Color::rgbaf(
        f32::from(color.red) / 255.0,
        f32::from(color.green) / 255.0,
        f32::from(color.blue) / 255.0,
        alpha,
    )
}
fn gradient_stops(stops: &[usvg::Stop], opacity: f32) -> impl Iterator<Item = (f32, Color)> + '_ {
    stops.iter().map(move |stop| {
        (
            stop.offset().get(),
            rgba(stop.color(), opacity * stop.opacity().get()),
        )
    })
}

fn brush(paint: &usvg::Paint, opacity: f32) -> Paint {
    match paint {
        usvg::Paint::Color(color) => Paint::color(rgba(*color, opacity)),
        usvg::Paint::LinearGradient(g) => Paint::linear_gradient_stops(
            g.x1(),
            g.y1(),
            g.x2(),
            g.y2(),
            gradient_stops(g.stops(), opacity),
        )
        .with_gradient_transform(matrix(g.transform())),
        usvg::Paint::RadialGradient(g) => Paint::two_point_radial_gradient_stops(
            g.fx(),
            g.fy(),
            0.0,
            g.cx(),
            g.cy(),
            g.r().get(),
            gradient_stops(g.stops(), opacity),
        )
        .with_gradient_transform(matrix(g.transform())),
        usvg::Paint::Pattern(_) => Paint::color(Color::black()), // rejected on worker
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_effects_select_an_explicit_fallback_instead_of_disappearing() {
        for (definitions, paint) in [
            (
                r##"<mask id="m"><rect width="10" height="10" fill="white"/></mask>"##,
                r##"<g mask="url(#m)"><path d="M0 0h10v10h-10Z"/></g>"##,
            ),
            (
                r##"<linearGradient id="g" spreadMethod="repeat"><stop stop-color="red"/><stop offset="1" stop-color="blue"/></linearGradient>"##,
                r##"<path d="M0 0h10v10h-10Z" fill="url(#g)"/>"##,
            ),
        ] {
            let source = format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><defs>{definitions}</defs>{paint}</svg>"#
            );
            let tree = usvg::Tree::from_str(&source, &usvg::Options::default()).unwrap();
            assert!(
                Scene::new(tree, HashMap::new())
                    .err()
                    .unwrap()
                    .contains("raster preview")
            );
        }
        let tree = usvg::Tree::from_str(r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><g id="live" opacity="0.4"><path d="M0 0h8v8h-8Z"/><path d="M2 2h8v8h-8Z"/></g></svg>"#, &usvg::Options::default()).unwrap();
        let scene = Scene::new(tree, HashMap::new()).unwrap();
        assert!(
            scene.tree.node_by_id("live").is_some(),
            "group opacity retains its own compositing boundary"
        );
    }
}
