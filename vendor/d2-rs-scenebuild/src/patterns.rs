// SPDX-License-Identifier: MPL-2.0
//! patterns.go: built-in fill patterns as scene pattern paints, overlays and
//! interleaving.

use d2_rs_scene::{
    AspectAlign, AspectRatio, Asset, BlendMode, Box as SceneBox, Image, Matrix, Node, Nrgba, Paint,
    PaintUnits, Path, PatternPaint, Primitive, Rect,
};
use d2_rs_target::Shape;

use crate::Builder;
use crate::pattern_sources::*;
use crate::target_ext::*;
use crate::util::*;

fn paper_pattern_asset_id() -> String {
    format!("builtin:fill-pattern:paper:{PAPER_PATTERN_SOURCE_SHA256}")
}

fn grain_pattern_asset_id() -> String {
    format!("builtin:fill-pattern:grain:{GRAIN_PATTERN_PNG_SOURCE_SHA256}")
}

const PAPER_PATTERN_COLORS: [Nrgba; 7] = [
    Nrgba::new(0, 0, 0, 0xff),
    Nrgba::new(0xef, 0xef, 0xef, 0xff),
    Nrgba::new(0xf5, 0xf5, 0xf5, 0xff),
    Nrgba::new(0xf7, 0xf7, 0xf7, 0xff),
    Nrgba::new(0xf9, 0xf9, 0xf9, 0xff),
    Nrgba::new(0xfc, 0xfc, 0xfc, 0xff),
    Nrgba::new(0xff, 0xff, 0xff, 0xff),
];

pub(crate) struct BuiltinPattern {
    pub name: String,
    pub paint: PatternPaint,
}

fn canonical_fill_pattern(value: &str) -> std::result::Result<String, String> {
    let canonical = value.to_lowercase();
    match canonical.as_str() {
        "" | "none" | "dots" | "lines" | "grain" | "paper" => Ok(canonical),
        _ => Err("must be one of none, dots, lines, grain, or paper".into()),
    }
}

/// validateFillPattern.
pub(crate) fn validate_fill_pattern(object: &str, value: &str) -> Result<()> {
    canonical_fill_pattern(value)
        .map(|_| ())
        .map_err(|e| invalid_field(object, "fillPattern", Some(value), &e))
}

fn new_builtin_pattern(name: &str, tile: SceneBox, root: Node) -> BuiltinPattern {
    BuiltinPattern {
        name: name.to_owned(),
        paint: PatternPaint {
            tile,
            root: Some(std::sync::Arc::new(root)),
            units: PaintUnits::USER_SPACE_ON_USE,
            transform: Matrix::identity(),
        },
    }
}

const INK: Nrgba = Nrgba::new(0x0a, 0x0f, 0x25, 0xff);

impl Builder<'_> {
    /// builtinPattern (errors are unwrapped messages; callers prefix them).
    pub(crate) fn builtin_pattern(
        &mut self,
        value: &str,
    ) -> std::result::Result<Option<BuiltinPattern>, String> {
        let name = canonical_fill_pattern(value)?;
        let tile15 = SceneBox {
            x: 0.0,
            y: 0.0,
            width: 15.0,
            height: 15.0,
        };
        match name.as_str() {
            "" | "none" => Ok(None),
            "dots" => {
                let mut root = node("builtin:fill-pattern:dots:tile", None);
                root.opacity = 0.1;
                root.blend = BlendMode::MULTIPLY;
                let mut children = Vec::new();
                for [x, y] in [
                    [2.0, 2.0],
                    [12.0, 2.0],
                    [12.0, 12.0],
                    [2.0, 12.0],
                    [2.0, 7.0],
                    [12.0, 7.0],
                    [7.0, 2.0],
                    [7.0, 12.0],
                    [7.0, 7.0],
                ] {
                    children.push(node(
                        format!("builtin:fill-pattern:dots:{x:.0}:{y:.0}"),
                        Some(Primitive::Rect(Rect {
                            r#box: SceneBox {
                                x,
                                y,
                                width: 1.0,
                                height: 1.0,
                            },
                            fill: Some(Paint::solid(INK)),
                            ..Default::default()
                        })),
                    ));
                }
                append_children(&mut root, children);
                Ok(Some(new_builtin_pattern(&name, tile15, root)))
            }
            "lines" => {
                let mut root = node("builtin:fill-pattern:lines:tile", None);
                root.opacity = 0.05;
                root.blend = BlendMode::MULTIPLY;
                let mut children = Vec::new();
                for y in [2.0, 7.0, 12.0] {
                    children.push(node(
                        format!("builtin:fill-pattern:lines:{y:.0}"),
                        Some(Primitive::Rect(Rect {
                            r#box: SceneBox {
                                x: 0.0,
                                y,
                                width: 15.0,
                                height: 1.0,
                            },
                            fill: Some(Paint::solid(INK)),
                            ..Default::default()
                        })),
                    ));
                }
                append_children(&mut root, children);
                Ok(Some(new_builtin_pattern(&name, tile15, root)))
            }
            "grain" => {
                self.ensure_grain_pattern_asset()?;
                let mut root = node("builtin:fill-pattern:grain:tile", None);
                root.opacity = 0.8;
                let mut image = node(
                    "builtin:fill-pattern:grain:image",
                    Some(Primitive::Image(Image {
                        asset: grain_pattern_asset_id(),
                        r#box: SceneBox {
                            x: 0.0,
                            y: 0.0,
                            width: 300.0,
                            height: 300.0,
                        },
                        aspect: AspectRatio {
                            align: AspectAlign::NONE,
                            ..Default::default()
                        },
                    })),
                );
                image.opacity = 0.9;
                root.children = Some(vec![image]);
                Ok(Some(new_builtin_pattern(
                    &name,
                    SceneBox {
                        x: 0.0,
                        y: 0.0,
                        width: 300.0,
                        height: 300.0,
                    },
                    root,
                )))
            }
            "paper" => {
                self.ensure_paper_pattern_asset()?;
                let root = node(
                    "builtin:fill-pattern:paper:image",
                    Some(Primitive::Image(Image {
                        asset: paper_pattern_asset_id(),
                        r#box: SceneBox {
                            x: 0.0,
                            y: 0.0,
                            width: 75.0,
                            height: 75.0,
                        },
                        aspect: AspectRatio {
                            align: AspectAlign::NONE,
                            ..Default::default()
                        },
                    })),
                );
                Ok(Some(new_builtin_pattern(
                    &name,
                    SceneBox {
                        x: 0.0,
                        y: 0.0,
                        width: 75.0,
                        height: 75.0,
                    },
                    root,
                )))
            }
            _ => unreachable!("unreachable fill pattern"),
        }
    }

    fn ensure_grain_pattern_asset(&mut self) -> std::result::Result<(), String> {
        let id = grain_pattern_asset_id();
        if self.assets.contains_key(&id) {
            return Ok(());
        }
        let source = shared_grain_pattern_source()
            .map_err(|e| format!("scene: load built-in grain fill pattern: {e}"))?;
        self.assets.insert(
            id,
            Asset::RasterAsset {
                mime_type: "image/png".into(),
                data: source.png.to_vec(),
                pixel_width: source.pixel_width,
                pixel_height: source.pixel_height,
                decoded_bytes: source.decoded_bytes,
            },
        );
        Ok(())
    }

    fn ensure_paper_pattern_asset(&mut self) -> std::result::Result<(), String> {
        let id = paper_pattern_asset_id();
        if self.assets.contains_key(&id) {
            return Ok(());
        }
        let source = shared_paper_pattern_source()
            .map_err(|e| format!("scene: load built-in paper fill pattern: {e}"))?;
        let asset = new_paper_pattern_asset(source)
            .map_err(|e| format!("scene: build built-in paper fill pattern: {e}"))?;
        self.assets.insert(id, asset);
        Ok(())
    }
}

fn new_paper_pattern_asset(source: &PaperPatternSource) -> std::result::Result<Asset, String> {
    if source.paths.len() != PAPER_PATTERN_PATH_COUNT {
        return Err("invalid shared paper pattern source".into());
    }
    let mut root = node("builtin:fill-pattern:paper:content", None);
    root.opacity = 0.3;
    root.blend = BlendMode::MULTIPLY;
    let mut children = Vec::with_capacity(source.paths.len());
    let mut command_count = 0;
    for (index, path) in source.paths.iter().enumerate() {
        if usize::from(path.color) >= PAPER_PATTERN_COLORS.len() || path.commands.is_empty() {
            return Err(format!("paper path {index} has invalid cached data"));
        }
        children.push(node(
            format!("builtin:fill-pattern:paper:path:{index}"),
            Some(Primitive::Path(Path {
                commands: path.commands.clone(),
                fill: Some(Paint::solid(PAPER_PATTERN_COLORS[usize::from(path.color)])),
                ..Default::default()
            })),
        ));
        command_count += path.commands.len();
    }
    if command_count != PAPER_PATTERN_COMMAND_COUNT {
        return Err(format!(
            "paper command count {command_count}, want {PAPER_PATTERN_COMMAND_COUNT}"
        ));
    }
    root.children = Some(children);
    Ok(Asset::VectorAsset {
        view_box: SceneBox {
            x: 0.0,
            y: 0.0,
            width: 75.0,
            height: 75.0,
        },
        root: Some(std::boxed::Box::new(root)),
    })
}

/// overlayPatternNode.
pub(crate) fn overlay_pattern_node(
    base: &Node,
    pattern: &BuiltinPattern,
    fallback_id: &str,
) -> Result<Node> {
    if !base.children().is_empty() {
        return err(format!(
            "scene: cannot apply fill pattern to node {} with children",
            q(&base.id)
        ));
    }
    if base.animations.as_ref().is_some_and(|a| !a.is_empty()) {
        return err(format!(
            "scene: cannot apply fill pattern to animated node {}",
            q(&base.id)
        ));
    }
    let paint = Some(Paint::PatternPaint(pattern.paint.clone()));
    let primitive = match &base.primitive {
        Some(Primitive::Rect(rect)) => Primitive::Rect(Rect {
            fill: paint,
            stroke: None,
            ..rect.clone()
        }),
        Some(Primitive::Ellipse(ellipse)) => {
            let mut e = ellipse.clone();
            e.fill = paint;
            e.stroke = None;
            Primitive::Ellipse(e)
        }
        Some(Primitive::Path(path)) => Primitive::Path(Path {
            fill: paint,
            stroke: None,
            ..path.clone()
        }),
        Some(Primitive::TextRun(_)) => {
            return err(format!(
                "scene: node {} primitive d2scene.TextRun cannot carry a fill pattern",
                q(&base.id)
            ));
        }
        Some(Primitive::Image(_)) => {
            return err(format!(
                "scene: node {} primitive d2scene.Image cannot carry a fill pattern",
                q(&base.id)
            ));
        }
        None => {
            return err(format!(
                "scene: node {} primitive <nil> cannot carry a fill pattern",
                q(&base.id)
            ));
        }
    };
    let mut overlay = Node::new(Some(primitive));
    overlay.id = if base.id.is_empty() {
        fallback_id.to_owned()
    } else {
        format!("{}:fill-pattern", base.id)
    };
    overlay.classes = Some(vec![format!("{}-overlay", pattern.name)]);
    overlay.transform = base.transform;
    overlay.opacity = base.opacity;
    overlay.blend = BlendMode::MULTIPLY;
    overlay.clip = base.clip.clone();
    overlay.mask = base.mask.clone();
    overlay.filters = base.filters.clone().filter(|f| !f.is_empty());
    Ok(overlay)
}

/// interleavePattern.
pub(crate) fn interleave_pattern(
    nodes: Vec<Node>,
    pattern: Option<&BuiltinPattern>,
    id_prefix: &str,
    eligible: impl Fn(&Node) -> bool,
) -> Result<Vec<Node>> {
    let Some(pattern) = pattern else {
        return Ok(nodes);
    };
    let mut output = Vec::with_capacity(nodes.len() * 2);
    for (index, n) in nodes.into_iter().enumerate() {
        let overlay = if eligible(&n) {
            Some(overlay_pattern_node(
                &n,
                pattern,
                &format!("{id_prefix}:fill-pattern:{index}"),
            )?)
        } else {
            None
        };
        output.push(n);
        output.extend(overlay);
    }
    Ok(output)
}

/// effectPatternNode.
pub(crate) fn effect_pattern_node(s: &Shape, n: &Node) -> bool {
    if s.three_dee {
        return n.id == format!("{}:3d-main", s.id);
    }
    if s.double_border {
        if n.id == format!("{}:double-border:outer", s.id) {
            return true;
        }
        if s.multiple && n.id == format!("{}:multiple:outer", s.id) {
            return matches!(s.type_.as_str(), "" | SHAPE_RECTANGLE | SHAPE_SQUARE);
        }
        return false;
    }
    if s.multiple {
        return n.id == format!("{}:main", s.id)
            || n.id.starts_with(&format!("{}:main:path:", s.id));
    }
    false
}
