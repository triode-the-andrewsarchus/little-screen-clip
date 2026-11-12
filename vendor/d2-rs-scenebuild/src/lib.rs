// SPDX-License-Identifier: MPL-2.0
//! Scene building: a Rust port of upstream D2's `d2renderers/d2scenebuild`
//! package, which turns a compiled target diagram into a `d2scene` document.
//! The `diagnostic-traces` feature enables traces; `oracle-fixtures` compiles
//! tests that read regenerated Go-oracle artifacts.
//!
//! [`build`] mirrors `d2scenebuild.Build`: it preflights the diagram, builds
//! every shape, connection, positioned tooltip, appendix badge, legend and
//! appendix node in Go's order, resolves font fallbacks and materializes
//! glyphs. Go's context cancellation has no equivalent here; every
//! cancellation check is a no-op.

mod appendix;
mod assets;
mod code_shape;
mod color;
mod connection;
mod fallback;
mod fonts;
pub mod golib;
mod image_resolver;
mod labels;
mod latex;
mod legend;
mod markdown;
mod metadata;
#[cfg(not(target_arch = "wasm32"))]
pub mod netpolicy;
#[cfg(not(target_arch = "wasm32"))]
pub mod netpolicy_client;
mod pattern_sources;
mod patterns;
mod shape_effects;
mod shaping;
mod sketch;
mod special_shapes;
mod svgimport;
mod svgimport_path;
#[cfg(any(
    target_os = "macos",
    target_os = "linux",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
))]
mod system_font_discovery;
#[cfg(any(
    target_os = "macos",
    target_os = "linux",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
))]
mod system_font_resolver;
#[cfg(any(
    target_os = "macos",
    target_os = "linux",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
))]
pub use system_font_resolver::{SystemFallbackLimits, SystemFallbackResolver};
#[allow(dead_code)] // Used by the system fallback resolver under development.
mod system_font_priority;
mod target_ext;
#[cfg(test)]
mod tests;
mod tooltip;
mod util;

use std::collections::{BTreeMap, HashMap};

use d2_rs_scene::{
    Asset, AssetId, Box as SceneBox, Document, Font, LineCap, LineJoin, LinkRegion, Mask, Node,
    Path, PathCommand, Primitive, Rect, Stroke, TextAnchor, TextRun, ViewportAlign, ViewportFit,
};
use d2_rs_target::theme::Theme;
use d2_rs_target::{Connection, Diagram, Point as TargetPoint, Shape, Text, ThemeOverrides};

pub use assets::{ImageKind, ImageResolveError, ImageResolver, ImageResource};
pub use color::gradient_paint;
pub use fallback::{
    BundledFallbackLimits, BundledFallbackResolver, FallbackFont, FallbackRequest, FallbackResolver,
};
pub use golib::color::is_gradient;
use golib::geo;
use golib::label::Position;
use golib::shape::{self as libshape, Shape as _};
use golib::svgpath;
#[cfg(not(target_arch = "wasm32"))]
pub use image_resolver::NativeImageHttpClient;
pub use image_resolver::{
    IMAGE_HTTP_HEADERS, ImageAssetLimits, ImageAssetUsage, ImageCacheUsage, ImageFileReader,
    ImageHttpClient, ImageHttpResponse, ImageLocalFiles, ImageMemoryCache, LocalFileError,
    RasterImageResolver, ResolvedImageAsset, UnrestrictedImageFiles,
};
pub use markdown::is_dangerous_link;
pub use metadata::parse_request_uri_has_host;
use target_ext::*;
use util::*;
pub use util::{Error, Result};

pub const DEFAULT_PADDING: i64 = 100;

/// AssetOptions. A resolver supplies immutable, already-probed resources;
/// the builder owns its document copies. SVG imports require finite per-import
/// limits and cumulative document budgets.
#[derive(Clone, Debug, Default)]
pub struct AssetOptions {
    /// Whether a caller-provided image resolver is configured (Go's non-nil
    /// `Resolver`).
    pub resolver: bool,
    /// Native counterpart of the imageasset resolver handoff. The legacy
    /// marker alone cannot resolve resources. Fetch/probe limits belong to
    /// this resolver, just as they do to upstream's imageasset.Resolver.
    pub resolver_impl: Option<std::sync::Arc<dyn ImageResolver>>,
    pub svg_import_limits: SvgImportLimits,
    pub svg_import_budget: SvgImportBudget,
}

impl AssetOptions {
    fn has_resolver(&self) -> bool {
        self.resolver || self.resolver_impl.is_some()
    }
}

/// d2svgimport.Limits (the fields d2scenebuild checks).
#[derive(Clone, Copy, Debug, Default)]
pub struct SvgImportLimits {
    pub max_bytes: i64,
    pub max_depth: i64,
    pub max_elements: i64,
    pub max_attributes: i64,
    pub max_attribute_bytes: i64,
    pub max_path_commands: i64,
    pub max_transform_functions: i64,
    pub max_use_depth: i64,
    pub max_resources: i64,
}

/// SVGImportBudget.
#[derive(Clone, Copy, Debug, Default)]
pub struct SvgImportBudget {
    pub max_source_bytes: i64,
    pub max_elements: i64,
    pub max_attributes: i64,
    pub max_attribute_bytes: i64,
    pub max_path_commands: i64,
    pub max_transform_functions: i64,
    pub max_declared_resources: i64,
    pub max_expanded_use_instances: i64,
}

/// LinkBudget.
#[derive(Clone, Copy, Debug, Default)]
pub struct LinkBudget {
    pub max_regions: i64,
    pub max_string_bytes: i64,
}

impl LinkBudget {
    /// Go `%v` of the struct.
    pub(crate) fn go_v(&self) -> String {
        format!("{{{} {}}}", self.max_regions, self.max_string_bytes)
    }
}

/// FontFallbackOptions. Configure `resolver_impl` with a bundled or caller
/// resolver. With no resolver, Build retains its placeholder-only policy.
#[derive(Clone, Debug, Default)]
pub struct FontFallbackOptions {
    pub resolver: bool,
    /// Native resolver implementation. The legacy `resolver` marker alone
    /// still reports unsupported; it cannot stand in for an implementation.
    pub resolver_impl: Option<std::sync::Arc<dyn FallbackResolver>>,
    pub max_assets: i64,
    pub max_bytes: i64,
    pub max_runes_per_text: i64,
    pub max_total_runes: i64,
    pub max_coverage_checks: i64,
    pub max_font_faces_per_text: i64,
    pub max_shaping_runs: i64,
    pub max_shaped_glyphs: i64,
}

/// SketchBudget.
#[derive(Clone, Copy, Debug, Default)]
pub struct SketchBudget {
    pub max_operation_sets: i64,
    pub max_operations: i64,
    pub max_path_commands: i64,
}

/// Options. Scale changes the logical output dimensions.
#[derive(Clone, Debug, Default)]
pub struct Options {
    pub pad: Option<i64>,
    pub scale: Option<f64>,
    pub center: Option<bool>,
    pub theme_id: Option<i64>,
    pub theme_overrides: Option<ThemeOverrides>,
    pub max_nodes: i64,
    pub max_path_commands: i64,
    pub sketch: bool,
    pub sketch_budget: SketchBudget,
    pub link_budget: LinkBudget,
    pub appendix: bool,
    pub assets: Option<AssetOptions>,
    pub fonts: Option<FontFallbackOptions>,
}

#[derive(Clone, Debug)]
pub(crate) struct AppendixItem {
    pub object: String,
    pub field: String,
    pub text: String,
}

pub(crate) struct Builder<'a> {
    pub diagram: &'a Diagram,
    pub options: &'a Options,
    pub theme: Theme,
    pub assets: BTreeMap<AssetId, Asset>,
    pub source_asset_ids: HashMap<String, AssetId>,
    pub image_asset_ids: HashMap<[u8; 32], AssetId>,
    pub svg_budget: d2_rs_math::svg_budget::SvgDocumentBudget,
    pub latex_assets: HashMap<(String, d2_rs_scene::Nrgba), latex::LatexAsset>,
    pub id_to_shape: Option<HashMap<String, Shape>>,
    /// Shared by every connection geometry node.
    pub connection_mask: Option<Mask>,
    pub links: Vec<LinkRegion>,
    pub link_bytes: i64,
    pub appendix_items: Vec<AppendixItem>,
    pub appendix_string_bytes: i64,
    pub font_runes: i64,
    pub font_coverage_checks: i64,
    pub font_shape_runs: i64,
    pub font_glyphs: i64,
    pub code_tokens: code_shape::CodeTokenBudget,
    pub markdown_ruler: Option<golib::markdown::ruler::Ruler>,
    pub font_cache: shaping::FontCache,
    pub sketch_operation_sets: i64,
    pub sketch_operations: i64,
    pub sketch_path_commands: i64,
    pub sketch_streak_patterns: HashMap<String, d2_rs_scene::PatternPaint>,
}

/// Build preflights diagram features and returns a fully resolved,
/// network-free scene. It never emits a partial scene on error.
pub fn build(diagram: &Diagram, options: &Options) -> Result<Document> {
    if let Some(scale) = options.scale {
        if !finite(scale) || scale <= 0.0 {
            return Err(invalid_field(
                "options",
                "scale",
                Some(&fv(scale)),
                "must be finite and greater than zero",
            ));
        }
    }
    if options.max_nodes < 0 {
        return Err(invalid_field(
            "options",
            "maxNodes",
            Some(&options.max_nodes.to_string()),
            "must be non-negative",
        ));
    }
    if options.max_path_commands < 0 {
        return Err(invalid_field(
            "options",
            "maxPathCommands",
            Some(&options.max_path_commands.to_string()),
            "must be non-negative",
        ));
    }
    if options.sketch {
        validate_sketch_budget(options.sketch_budget)?;
    }
    if diagram.has_children() {
        return Err(unsupported("diagram", "nested boards in a single scene"));
    }
    let theme_id = options.theme_id.unwrap_or(0);
    let Some(mut theme) = Theme::find(theme_id) else {
        return err(format!("scene: unknown theme {theme_id}"));
    };
    theme.apply_overrides(options.theme_overrides.as_ref());
    admit_scene_work(diagram, options)?;

    let mut b = Builder {
        diagram,
        options,
        theme,
        assets: BTreeMap::new(),
        source_asset_ids: HashMap::new(),
        image_asset_ids: HashMap::new(),
        svg_budget: Default::default(),
        latex_assets: HashMap::new(),
        id_to_shape: Some(
            diagram
                .shapes()
                .iter()
                .map(|s| (s.id.clone(), s.clone()))
                .collect(),
        ),
        connection_mask: None,
        links: Vec::new(),
        link_bytes: 0,
        appendix_items: Vec::new(),
        appendix_string_bytes: 0,
        font_runes: 0,
        font_coverage_checks: 0,
        font_shape_runs: 0,
        font_glyphs: 0,
        code_tokens: code_shape::CodeTokenBudget::new(options.max_nodes),
        markdown_ruler: None,
        font_cache: shaping::FontCache::default(),
        sketch_operation_sets: 0,
        sketch_operations: 0,
        sketch_path_commands: 0,
        sketch_streak_patterns: HashMap::new(),
    };
    b.preflight()?;
    b.preflight_assets()?;
    b.build_document()
}

/// sketch.go validateSketchBudget.
fn validate_sketch_budget(budget: SketchBudget) -> Result<()> {
    if budget.max_operation_sets <= 0 || budget.max_operations <= 0 || budget.max_path_commands <= 0
    {
        return Err(invalid_field(
            "options",
            "sketchBudget",
            Some(&format!(
                "{{{} {} {}}}",
                budget.max_operation_sets, budget.max_operations, budget.max_path_commands
            )),
            "must provide positive MaxOperationSets, MaxOperations, and MaxPathCommands for sketch scenes",
        ));
    }
    Ok(())
}

/// admitSceneWork: the allocation-free lower bound on nodes and route path
/// commands.
fn admit_scene_work(diagram: &Diagram, options: &Options) -> Result<()> {
    if options.max_nodes > 0 {
        let mut remaining = options.max_nodes;
        let shapes = diagram.shapes().len() as i64;
        let connections = diagram.connections().len() as i64;
        for count in [2, shapes, connections, connections] {
            if count > remaining {
                return err(format!(
                    "scene: node count exceeds limit {}",
                    options.max_nodes
                ));
            }
            remaining -= count;
        }
    }
    if options.max_path_commands == 0 {
        return Ok(());
    }
    let mut remaining = options.max_path_commands;
    for connection in diagram.connections() {
        let route = connection.route().len() as i64;
        if route < 2 || connection.is_curve && (route - 1) % 3 != 0 {
            continue;
        }
        let commands = if connection.is_curve {
            1 + (route - 1) / 3
        } else {
            route
        };
        if commands > remaining {
            return err(format!(
                "scene: path command count exceeds limit {}",
                options.max_path_commands
            ));
        }
        remaining -= commands;
    }
    Ok(())
}

struct DiagramObject<'a> {
    shape: Option<&'a Shape>,
    connection: Option<&'a Connection>,
    z: i64,
    level: i64,
    shape_object: bool,
    order: usize,
}

impl<'a> Builder<'a> {
    fn preflight(&mut self) -> Result<()> {
        let root = &self.diagram.root;
        validate_shape_numbers("root", root)?;
        patterns::validate_fill_pattern("root", &root.fill_pattern)?;
        if root.three_dee {
            return Err(unsupported("root", "3d effect"));
        }
        if root.multiple {
            return Err(unsupported("root", "multiple effect"));
        }
        if root.shadow || root.animated || root.blend {
            return Err(unsupported("root", "effect"));
        }
        if !root.link.is_empty()
            || !root.tooltip.is_empty()
            || !root.pretty_link.is_empty()
            || !root.tooltip_position.is_empty()
        {
            return Err(unsupported("root", "link/tooltip metadata"));
        }
        for shape in self.diagram.shapes() {
            self.preflight_shape(shape)?;
        }
        for connection in self.diagram.connections() {
            self.preflight_connection(connection)?;
        }
        self.preflight_legend()?;
        self.preflight_appendix()?;
        self.compile_link_regions()
    }

    pub(crate) fn preflight_shape(&self, s: &Shape) -> Result<()> {
        let object = format!("shape {}", q(&s.id));
        validate_shape_numbers(&object, s)?;
        patterns::validate_fill_pattern(&object, &s.fill_pattern)?;
        shape_effects::validate_shape_effects(&object, s)?;
        if s.type_ == SHAPE_IMAGE && s.icon.is_none() {
            return Err(invalid_field(
                &object,
                "icon",
                None,
                "must identify an image source for an image shape",
            ));
        }
        if s.opacity != 0.0 {
            match s.type_.as_str() {
                SHAPE_CLASS | SHAPE_SQL_TABLE => special_shapes::validate_structured_shape(s)?,
                SHAPE_CODE => code_shape::validate_code_shape(s)?,
                _ => {}
            }
            if s.icon.is_some() && s.type_ != SHAPE_IMAGE {
                let position = Position::from_string(&s.icon_position);
                if !position.is_shape_position() {
                    return Err(invalid_field(
                        &object,
                        "iconPosition",
                        Some(&s.icon_position),
                        "must identify a supported icon position",
                    ));
                }
            }
        }
        if assets::shape_asset_is_emitted(s)
            && !self
                .options
                .assets
                .as_ref()
                .is_some_and(AssetOptions::has_resolver)
        {
            return Err(unsupported(
                &object,
                "image/icon asset without a configured resolver",
            ));
        }
        if !s.tooltip_position.is_empty() {
            if s.tooltip.is_empty() {
                return Err(invalid_field(
                    &object,
                    "tooltipPosition",
                    Some(&s.tooltip_position),
                    "requires a non-empty tooltip",
                ));
            }
            markdown::validate_markdown_label(
                &format!("{object} positioned tooltip"),
                &s.tooltip,
                16,
                1,
                1,
            )?;
        }
        if !s.text.language.is_empty() {
            let structured_or_code =
                matches!(s.type_.as_str(), SHAPE_CODE | SHAPE_CLASS | SHAPE_SQL_TABLE);
            match s.text.language.as_str() {
                "latex" => {
                    if structured_or_code {
                        return Err(unsupported(
                            &object,
                            "label language latex on a structured or code shape",
                        ));
                    }
                    latex::validate_latex_input(&object, &s.text.label)?;
                    self.validate_latex_import_options(&object)?;
                }
                "markdown" => {
                    if structured_or_code {
                        return Err(unsupported(
                            &object,
                            "label language markdown on a structured or code shape",
                        ));
                    }
                    markdown::validate_markdown_label(
                        &object,
                        &s.text.label,
                        s.text.font_size,
                        s.text.label_width,
                        s.text.label_height,
                    )?;
                }
                language => {
                    if s.type_ == SHAPE_CLASS || s.type_ == SHAPE_SQL_TABLE {
                        return Err(unsupported(
                            &object,
                            &format!("label language {language} on a structured shape"),
                        ));
                    }
                    code_shape::validate_code_text(&object, &s.text)?;
                }
            }
        }
        Ok(())
    }

    pub(crate) fn preflight_connection(&self, c: &Connection) -> Result<()> {
        let object = format!("connection {}", q(&c.id));
        validate_connection_numbers(&object, c)?;
        let route = c.route();
        if route.len() < 2 {
            return Err(invalid_field(
                &object,
                "route",
                Some(&route.len().to_string()),
                "must contain at least two points",
            ));
        }
        if c.is_curve && !(route.len() - 1).is_multiple_of(3) {
            return Err(invalid_field(
                &object,
                "route",
                Some(&route.len().to_string()),
                "must contain 1+3n points for a cubic connection",
            ));
        }
        for (i, point) in route.iter().enumerate() {
            let field = format!("route[{i}]");
            let Some(point) = point else {
                return err(format!(
                    "scene: {object} field {} must not be nil",
                    q(&field)
                ));
            };
            if !finite(point.x) {
                return Err(invalid_field(
                    &object,
                    &format!("{field}.x"),
                    Some(&fv(point.x)),
                    "must be finite",
                ));
            }
            if !finite(point.y) {
                return Err(invalid_field(
                    &object,
                    &format!("{field}.y"),
                    Some(&fv(point.y)),
                    "must be finite",
                ));
            }
            validate_route_coordinate(&object, &format!("{field}.x"), point.x, c.stroke_width)?;
            validate_route_coordinate(&object, &format!("{field}.y"), point.y, c.stroke_width)?;
            if i > 0 {
                let previous = route[i - 1].as_ref().expect("checked");
                let (dx, dy) = (point.x - previous.x, point.y - previous.y);
                if !finite(dx) || !finite(dy) || !finite(geo::Vector::new(&[dx, dy]).length()) {
                    return Err(invalid_field(
                        &object,
                        &format!("route[{}:{i}]", i - 1),
                        None,
                        "must have a finite segment length",
                    ));
                }
            }
        }
        let points = route_points(c);
        if c.src_arrow != NO_ARROWHEAD && points[0].vector_to(&points[1]).length() == 0.0 {
            return Err(invalid_field(
                &object,
                "route",
                None,
                "must have a non-zero source arrowhead tangent",
            ));
        }
        let last = points.len() - 1;
        if c.dst_arrow != NO_ARROWHEAD && points[last - 1].vector_to(&points[last]).length() == 0.0
        {
            return Err(invalid_field(
                &object,
                "route",
                None,
                "must have a non-zero destination arrowhead tangent",
            ));
        }
        if c.animated {
            connection::validate_animated_connection(&object, c)?;
        }
        if (!c.tooltip.is_empty() || !c.link.is_empty()) && c.text.label.is_empty() {
            return Err(unsupported(
                &object,
                "link/tooltip metadata without a label (the box-only link representation cannot encode route hit geometry)",
            ));
        }
        if !c.text.label.is_empty() {
            let position = Position::from_string(&c.label_position);
            if !position.is_edge_position() {
                return Err(invalid_field(
                    &object,
                    "labelPosition",
                    Some(&c.label_position),
                    "must identify a connection label position",
                ));
            }
            let zero_markdown_viewport = c.text.language == "markdown"
                && (c.text.label_width == 0 || c.text.label_height == 0);
            if c.text.language != "latex" && c.text.font_size <= 0 && !zero_markdown_viewport {
                return Err(invalid_field(
                    &object,
                    "fontSize",
                    Some(&c.text.font_size.to_string()),
                    "must be positive for a connection with a label",
                ));
            }
            if c.text.language != "markdown"
                && (c.text.label_width <= 0 || c.text.label_height <= 0)
            {
                return Err(invalid_field(
                    &object,
                    "labelDimensions",
                    Some(&format!("{}x{}", c.text.label_width, c.text.label_height)),
                    "must be positive for a connection with a label",
                ));
            }
        }
        if c.icon.is_some() {
            if c.text.label.is_empty()
                && !Position::from_string(&c.icon_position).is_edge_position()
            {
                return Err(invalid_field(
                    &object,
                    "iconPosition",
                    Some(&c.icon_position),
                    "must identify a connection icon position",
                ));
            }
            let top_left = connection_icon_position(c);
            if top_left.is_none_or(|p| !finite(p.x) || !finite(p.y)) {
                return Err(invalid_field(
                    &object,
                    "iconPosition",
                    Some(&c.icon_position),
                    "must resolve to a finite point on the route",
                ));
            }
            if !self
                .options
                .assets
                .as_ref()
                .is_some_and(AssetOptions::has_resolver)
            {
                return Err(unsupported(
                    &object,
                    "icon asset without a configured resolver",
                ));
            }
        }
        for (name, text) in [("srcLabel", &c.src_label), ("dstLabel", &c.dst_label)] {
            let Some(text) = text.as_ref().filter(|t| !t.label.is_empty()) else {
                continue;
            };
            if c.text.font_size <= 0 {
                return Err(invalid_field(
                    &object,
                    "fontSize",
                    Some(&c.text.font_size.to_string()),
                    "must be positive for an arrowhead label",
                ));
            }
            if text.label_width <= 0 || text.label_height <= 0 {
                return Err(invalid_field(
                    &object,
                    &format!("{name}Dimensions"),
                    Some(&format!("{}x{}", text.label_width, text.label_height)),
                    "must be positive for an arrowhead label",
                ));
            }
        }
        match c.text.language.as_str() {
            "latex" => {
                latex::validate_latex_input(&object, &c.text.label)?;
                self.validate_latex_import_options(&object)?;
            }
            "markdown" => markdown::validate_markdown_label(
                &object,
                &c.text.label,
                c.text.font_size,
                c.text.label_width,
                c.text.label_height,
            )?,
            "" => {}
            _ => code_shape::validate_code_text(&object, &c.text)?,
        }
        for arrowhead in [&c.src_arrow, &c.dst_arrow] {
            match arrowhead.as_str() {
                "none" | "arrow" | "triangle" | "unfilled-triangle" | "line" | "filled-diamond"
                | "diamond" | "filled-circle" | "circle" | "cross" | "filled-box" | "box"
                | "cf-one" | "cf-many" | "cf-one-required" | "cf-many-required" => {}
                other => return Err(unsupported(&object, &format!("arrowhead {other}"))),
            }
        }
        Ok(())
    }

    fn build_document(&mut self) -> Result<Document> {
        let (diagram_tl, diagram_br) = bounding_box(self.diagram);
        let (mut tl, mut br) = self.visual_bounding_box();
        if self.options.appendix {
            // On-shape badges are part of the base viewbox.
            (tl, br) = (diagram_tl, diagram_br);
        }
        let legend_layout = self.measure_legend(diagram_tl, diagram_br)?;
        let pad = self.options.pad.unwrap_or(DEFAULT_PADDING);
        let (mut left, mut top, mut width, mut height) = dimensions(tl, br, pad)?;
        (left, top, width, height) =
            legend::expand_view_box(legend_layout.as_ref(), left, top, width, height, pad)?;

        let root_shape = &self.diagram.root;
        let root_fill = self.paint(&root_shape.fill, "root fill")?;
        let root_stroke = self.stroke(
            &root_shape.stroke,
            root_shape.stroke_width,
            root_shape.stroke_dash,
            LineCap::BUTT,
            LineJoin::MITER,
            "root stroke",
        )?;
        let stroke_padding = (root_shape.stroke_width as f64 / 2.0).ceil() as i64;
        let Some((background_left, background_top, background_width, background_height)) =
            expand_dimensions(left, top, width, height, stroke_padding)
        else {
            return Err(invalid_padding_error(pad));
        };
        match expand_dimensions(
            background_left,
            background_top,
            background_width,
            background_height,
            stroke_padding,
        ) {
            Some((l, t, w, h)) if w >= 0 && h >= 0 => (left, top, width, height) = (l, t, w, h),
            _ => return Err(invalid_padding_error(pad)),
        }
        let root_pattern = self
            .builtin_pattern(&root_shape.fill_pattern)
            .map_err(|e| Error(format!("scene: root fill pattern: {e}")))?;

        let mut root = node("root", None);
        let mut root_children: Vec<Node> = Vec::new();
        let mut background_fill = root_fill.clone();
        // Index of the node appendix.Append resizes.
        let mut appendix_background: Option<usize> = None;
        if root_shape.double_border {
            let Some((outer_left, outer_top, outer_width, outer_height)) = expand_dimensions(
                left,
                top,
                width,
                height,
                stroke_padding + INNER_BORDER_OFFSET,
            ) else {
                return Err(invalid_padding_error(pad));
            };
            match expand_dimensions(
                outer_left,
                outer_top,
                outer_width,
                outer_height,
                stroke_padding,
            ) {
                Some((l, t, w, h)) if w >= 0 && h >= 0 => (left, top, width, height) = (l, t, w, h),
                _ => return Err(invalid_padding_error(pad)),
            }
            let outer_radius = clamp_border_radius(
                root_shape.border_radius as f64,
                outer_width as f64,
                outer_height as f64,
            );
            let outer = node(
                "root:double-border:outer",
                Some(Primitive::Rect(Rect {
                    r#box: SceneBox {
                        x: outer_left as f64,
                        y: outer_top as f64,
                        width: outer_width as f64,
                        height: outer_height as f64,
                    },
                    radius_x: outer_radius,
                    radius_y: outer_radius,
                    fill: root_fill.clone(),
                    stroke: root_stroke.clone(),
                })),
            );
            let overlay = match &root_pattern {
                Some(pattern) => Some(patterns::overlay_pattern_node(
                    &outer,
                    pattern,
                    "root:double-border:outer:fill-pattern",
                )?),
                None => None,
            };
            appendix_background = Some(root_children.len());
            root_children.push(outer);
            root_children.extend(overlay);
            background_fill = self.paint("transparent", "root double-border inner fill")?;
        }
        let root_radius = clamp_border_radius(
            root_shape.border_radius as f64,
            background_width as f64,
            background_height as f64,
        );
        let background = node(
            "root:background",
            Some(Primitive::Rect(Rect {
                r#box: SceneBox {
                    x: background_left as f64,
                    y: background_top as f64,
                    width: background_width as f64,
                    height: background_height as f64,
                },
                radius_x: root_radius,
                radius_y: root_radius,
                fill: background_fill,
                stroke: root_stroke,
            })),
        );
        let overlay = match &root_pattern {
            Some(pattern) => Some(patterns::overlay_pattern_node(
                &background,
                pattern,
                "root:background:fill-pattern",
            )?),
            None => None,
        };
        if appendix_background.is_none() {
            appendix_background = Some(root_children.len());
        }
        root_children.push(background);
        root_children.extend(overlay);

        self.connection_mask = self.build_connection_mask(SceneBox {
            x: left as f64,
            y: top as f64,
            width: width as f64,
            height: height as f64,
        })?;

        let shapes = self.diagram.shapes();
        let connections = self.diagram.connections();
        let mut objects: Vec<DiagramObject> = Vec::with_capacity(shapes.len() + connections.len());
        for (i, s) in shapes.iter().enumerate() {
            objects.push(DiagramObject {
                shape: Some(s),
                connection: None,
                z: s.z_index,
                level: s.level,
                shape_object: true,
                order: i,
            });
        }
        for (i, c) in connections.iter().enumerate() {
            objects.push(DiagramObject {
                shape: None,
                connection: Some(c),
                z: c.z_index,
                level: 0,
                shape_object: false,
                order: shapes.len() + i,
            });
        }
        // sort.SliceStable with the same less function.
        objects.sort_by(|a, b| {
            use std::cmp::Ordering;
            if a.z != b.z {
                return a.z.cmp(&b.z);
            }
            if a.shape_object && b.shape_object && a.level != b.level {
                return a.level.cmp(&b.level);
            }
            if a.shape_object != b.shape_object {
                return if a.shape_object {
                    Ordering::Less
                } else {
                    Ordering::Greater
                };
            }
            a.order.cmp(&b.order)
        });

        let mut rendered_objects = Vec::with_capacity(objects.len());
        let mut positioned_tooltips: Vec<Option<Node>> = Vec::with_capacity(objects.len());
        for object in &objects {
            if let Some(s) = object.shape {
                rendered_objects.push(self.build_shape(s)?);
                positioned_tooltips.push(self.build_positioned_tooltip(s)?);
            } else {
                rendered_objects
                    .push(self.build_connection(object.connection.expect("connection"))?);
                positioned_tooltips.push(None);
            }
        }

        let appendix_layout = self.measure_appendix(diagram_tl, diagram_br)?;
        (left, top, width, height) =
            appendix::expand_view_box(appendix_layout.as_ref(), left, top, width, height)?;
        if appendix_layout.is_some() {
            // appendix.Append rewrites only the first rendered background
            // rectangle's width and height.
            let index = appendix_background.expect("background");
            match &mut root_children[index].primitive {
                Some(Primitive::Rect(rect)) => {
                    rect.r#box.width = width as f64;
                    rect.r#box.height = height as f64;
                }
                _ => return err("scene: appendix background is not a rectangle"),
            }
        }

        root_children.extend(rendered_objects);
        let (tooltip_numbers, link_numbers) = appendix::appendix_icon_numbers(self.diagram);
        let mut metadata_nodes = Vec::new();
        for (index, object) in objects.iter().enumerate() {
            if let Some(s) = object.shape {
                if let Some(tooltip) = positioned_tooltips[index].take() {
                    metadata_nodes.push(tooltip);
                }
                let icons = self.build_appendix_icons(
                    appendix_layout.as_ref(),
                    s,
                    object.order,
                    tooltip_numbers[object.order],
                    link_numbers[object.order],
                )?;
                metadata_nodes.extend(icons);
            }
        }
        root_children.extend(metadata_nodes);
        if let Some(legend_node) = self.build_legend(legend_layout.as_ref())? {
            root_children.push(legend_node);
        }
        if let Some(appendix_node) = self.build_appendix(appendix_layout.as_ref())? {
            root_children.push(appendix_node);
        }
        append_children(&mut root, root_children);
        self.resolve_and_shape_text(&mut root)?;

        let view_box = SceneBox {
            x: left as f64,
            y: top as f64,
            width: width as f64,
            height: height as f64,
        };
        let mut document = Document::new(view_box, root);
        document.assets = std::mem::take(&mut self.assets);
        document.links = opt_vec(std::mem::take(&mut self.links));
        document.viewport_fit = ViewportFit::MEET;
        if self.options.center == Some(true) {
            document.viewport_align = ViewportAlign::X_MID_Y_MID;
        }
        let mut logical_scale = self.options.scale.unwrap_or(1.0);
        if appendix_layout.is_some() {
            // SVG exports with appendix items use the expanded viewbox
            // dimensions without applying the requested scale.
            logical_scale = 1.0;
        }
        let scaled_width = logical_scale * width as f64;
        let scaled_height = logical_scale * height as f64;
        if !finite(scaled_width)
            || !finite(scaled_height)
            || scaled_width <= 0.0
            || scaled_height <= 0.0
        {
            return Err(invalid_field(
                "options",
                "scale",
                Some(&fv(logical_scale)),
                "produces non-finite or non-positive logical dimensions",
            ));
        }
        document.logical_width = scaled_width.ceil();
        document.logical_height = scaled_height.ceil();
        Ok(document)
    }

    pub(crate) fn build_shape(&mut self, s: &Shape) -> Result<Node> {
        let (fill_value, stroke_value) = if s.type_ == SHAPE_SQL_TABLE || s.type_ == SHAPE_CLASS {
            (s.stroke.clone(), s.fill.clone())
        } else {
            (s.fill.clone(), s.stroke.clone())
        };
        let fill = self.paint(&fill_value, &format!("shape {} fill", q(&s.id)))?;
        let stroke = self.stroke(
            &stroke_value,
            s.stroke_width,
            s.stroke_dash,
            LineCap::BUTT,
            LineJoin::ROUND,
            &format!("shape {} stroke", q(&s.id)),
        )?;
        let mut group = node(s.id.clone(), None);
        group.classes = s.classes.clone().filter(|c| !c.is_empty());
        group.opacity = s.opacity;
        if s.opacity == 0.0 {
            return Ok(group);
        }
        if s.animated {
            shape_effects::configure_animated_shape(&mut group);
        }
        if s.type_ == SHAPE_CODE {
            let children = self.build_code_shape(s, stroke.as_ref())?;
            self.append_shape_geometry(&mut group, s, Vec::new());
            if let Some(icon) = self.build_shape_icon(s, false)? {
                append_children(&mut group, vec![icon]);
            }
            append_children(&mut group, children);
            return Ok(shape_effects::finish_shape(group, s));
        }
        if s.type_ == SHAPE_CLASS || s.type_ == SHAPE_SQL_TABLE {
            let mut children = if self.options.sketch {
                self.build_sketch_structured_shape(s, fill.clone(), stroke.clone())?
            } else {
                self.build_structured_shape(s, fill.clone(), stroke.clone())?
            };
            if !self.options.sketch {
                let pattern = self
                    .builtin_pattern(&s.fill_pattern)
                    .map_err(|e| Error(format!("scene: shape {} fill pattern: {e}", q(&s.id))))?;
                let id = s.id.clone();
                children =
                    patterns::interleave_pattern(children, pattern.as_ref(), &s.id, |n: &Node| {
                        n.id == format!("{id}:outline")
                            || n.id == format!("{id}:class-header")
                            || n.id == format!("{id}:table-header")
                    })?;
            }
            if let Some(icon) = self.build_shape_icon(s, true)? {
                children.push(icon);
            }
            self.append_shape_geometry(&mut group, s, children);
            return Ok(shape_effects::finish_shape(group, s));
        }

        let mut geometry_children = Vec::new();
        if s.type_ == SHAPE_IMAGE {
            geometry_children.push(self.build_shape_image(s)?);
        } else if s.three_dee || s.multiple || s.double_border {
            if self.options.sketch {
                geometry_children.extend(self.build_sketch_shape_effects(
                    s,
                    fill.clone(),
                    stroke.clone(),
                )?);
            } else {
                let effect_nodes = self.build_shape_effects(s, fill.clone(), stroke.clone())?;
                let pattern = self
                    .builtin_pattern(&s.fill_pattern)
                    .map_err(|e| Error(format!("scene: shape {} fill pattern: {e}", q(&s.id))))?;
                let effect_nodes = patterns::interleave_pattern(
                    effect_nodes,
                    pattern.as_ref(),
                    &s.id,
                    |n: &Node| patterns::effect_pattern_node(s, n),
                )?;
                geometry_children.extend(effect_nodes);
            }
        } else {
            if self.options.sketch {
                geometry_children.extend(self.build_sketch_ordinary_shape(s, "")?);
            } else {
                let mut outline_nodes =
                    self.build_ordinary_shape_outline(s, fill.clone(), stroke.clone(), "")?;
                if !outline_nodes.is_empty() {
                    let pattern = self.builtin_pattern(&s.fill_pattern).map_err(|e| {
                        Error(format!("scene: shape {} fill pattern: {e}", q(&s.id)))
                    })?;
                    outline_nodes = patterns::interleave_pattern(
                        outline_nodes,
                        pattern.as_ref(),
                        &s.id,
                        |_: &Node| true,
                    )?;
                }
                geometry_children.extend(outline_nodes);
            }
        }
        self.append_shape_geometry(&mut group, s, geometry_children);
        if let Some(icon) = self.build_shape_icon(s, false)? {
            append_children(&mut group, vec![icon]);
        }
        if !s.text.label.is_empty() {
            let text_nodes = if !s.text.language.is_empty()
                && s.text.language != "latex"
                && s.text.language != "markdown"
            {
                self.build_code_shape(s, stroke.as_ref())?
            } else {
                self.build_shape_text(s)?
            };
            append_children(&mut group, text_nodes);
        }
        Ok(shape_effects::finish_shape(group, s))
    }

    pub(crate) fn build_shape_text(&mut self, s: &Shape) -> Result<Vec<Node>> {
        if s.text.language == "latex" {
            let top_left = latex::latex_shape_label_top_left(s);
            let node = self.build_latex_label_node(
                &format!("shape {}", q(&s.id)),
                &format!("{}:label:0", s.id),
                &s.text.label,
                &s.stroke,
                top_left,
            )?;
            return Ok(vec![node]);
        }
        if s.text.language == "markdown" {
            return self.build_shape_markdown(s);
        }
        let (_, top_left) = labels::shape_label_placement(s);
        let font = self
            .font(&s.text)
            .map_err(|e| Error(format!("scene: shape {}: {e}", q(&s.id))))?;
        let fill = self.paint(&s.font_color(), &format!("shape {} label color", q(&s.id)))?;
        let lines: Vec<&str> = s.text.label.split('\n').collect();
        let mut nodes = Vec::with_capacity(lines.len() + 1);
        if !s.text.label_fill.is_empty() {
            let label_fill = self.paint(
                &s.text.label_fill,
                &format!("shape {} label fill", q(&s.id)),
            )?;
            nodes.push(node(
                format!("{}:label-fill", s.id),
                Some(Primitive::Rect(Rect {
                    r#box: SceneBox {
                        x: top_left.x,
                        y: top_left.y,
                        width: s.text.label_width as f64,
                        height: s.text.label_height as f64,
                    },
                    fill: label_fill,
                    ..Default::default()
                })),
            ));
        }
        let line_advance = s.text.label_height as f64 / lines.len() as f64;
        for (i, line) in lines.iter().enumerate() {
            let mut baseline = top_left.y + s.text.font_size as f64;
            if i > 0 {
                // ARM64 build.go:1012: FMADDD.
                baseline = geo::go_fmadd(i as f64, line_advance, baseline);
            }
            let run = TextRun {
                text: (*line).to_owned(),
                origin: d2_rs_scene::Point::new(
                    top_left.x + s.text.label_width as f64 / 2.0,
                    baseline,
                ),
                anchor: TextAnchor::MIDDLE,
                font: font.clone(),
                fill: fill.clone(),
                underline: s.text.underline,
                // ARM64 build.go:1021: both row edges are FMADDD.
                ink: d2_rs_scene::Bounds::new(
                    top_left.x,
                    geo::go_fmadd(i as f64, line_advance, top_left.y),
                    top_left.x + s.text.label_width as f64,
                    geo::go_fmadd((i + 1) as f64, line_advance, top_left.y),
                ),
                ..Default::default()
            };
            nodes.push(node(
                format!("{}:label:{i}", s.id),
                Some(Primitive::TextRun(run)),
            ));
        }
        Ok(nodes)
    }

    /// font: the bundled face for a Text, registering its asset.
    pub(crate) fn font(&mut self, text: &Text) -> std::result::Result<Font, String> {
        let mut family = self
            .diagram
            .font_family
            .clone()
            .unwrap_or_else(|| "SourceSansPro".into());
        let font_family = &text.font_family;
        if font_family.is_empty() || font_family.eq_ignore_ascii_case("default") {
            // DEFAULT is a role, not a concrete bundled font.
        } else if font_family.eq_ignore_ascii_case("mono") {
            family = self
                .diagram
                .mono_font_family
                .clone()
                .unwrap_or_else(|| "SourceCodePro".into());
        } else {
            match font_family.to_lowercase().as_str() {
                "default" => family = "SourceSansPro".into(),
                "mono" => family = "SourceCodePro".into(),
                _ => {}
            }
        }
        let style = if text.bold {
            "bold"
        } else if text.italic {
            "italic"
        } else {
            "regular"
        };
        self.font_for(&family, style, text.font_size as f64)
    }

    /// The shared tail of font and markdownFont.
    pub(crate) fn font_for(
        &mut self,
        family: &str,
        style: &str,
        size: f64,
    ) -> std::result::Result<Font, String> {
        let asset_id = format!("font:{family}:{style}");
        // The face is looked up (and copied) once per document; later text in
        // the same family and style reuses the retained asset.
        if !self.assets.contains_key(&asset_id) {
            let Some(bytes) = fonts::lookup_vec(family, style).filter(|b| !b.is_empty()) else {
                return Err(format!("font {family}/{style} is not loaded"));
            };
            self.assets.insert(
                asset_id.clone(),
                Asset::FontAsset {
                    mime_type: "font/ttf".into(),
                    data: bytes,
                    data_file: None,
                    face_index: 0,
                },
            );
        }
        let weight = match style {
            "bold" => 700,
            "semibold" => 600,
            _ => 400,
        };
        Ok(Font {
            family: family.to_owned(),
            style: style.to_owned(),
            weight,
            size: size.to_bits(),
            asset: asset_id,
        })
    }

    /// stroke.
    pub(crate) fn stroke(
        &self,
        raw: &str,
        width: i64,
        dash: f64,
        cap: LineCap,
        join: LineJoin,
        description: &str,
    ) -> Result<Option<Stroke>> {
        let paint = self.paint(raw, description)?;
        let Some(paint) = paint.filter(|_| width > 0) else {
            return Ok(None);
        };
        let mut stroke = Stroke {
            paint: Some(paint),
            width: width as f64,
            cap,
            join,
            miter_limit: 4.0,
            ..Default::default()
        };
        if dash != 0.0 {
            let (dash_size, gap_size) = svgpath::get_stroke_dash_attributes(width as f64, dash);
            stroke.dashes = Some(vec![dash_size, gap_size]);
        }
        Ok(Some(stroke))
    }
}

/// targetGeometry.
pub(crate) fn target_geometry(s: &Shape) -> libshape::D2Shape {
    let mut geometry = libshape::new_d2_shape(
        dsl_shape_to_shape_type(&s.type_),
        geo::Box::new(
            geo::Point::new(s.pos.x as f64, s.pos.y as f64),
            s.width as f64,
            s.height as f64,
        ),
    );
    if s.type_ == SHAPE_CLOUD {
        if let Some(ratio) = s.content_aspect_ratio {
            geometry.set_inner_box_aspect_ratio(ratio);
        }
    }
    geometry
}

/// scenePath.
pub(crate) fn scene_path(
    commands: &[svgpath::PathCommand],
    fill: Option<d2_rs_scene::Paint>,
    stroke: Option<Stroke>,
) -> Path {
    let mut path = Path {
        fill,
        stroke,
        commands: Vec::with_capacity(commands.len()),
        ..Default::default()
    };
    for command in commands {
        path.commands.push(match command.kind {
            svgpath::PathCommandKind::Move => PathCommand::move_to(command.end.x, command.end.y),
            svgpath::PathCommandKind::Line => PathCommand::line_to(command.end.x, command.end.y),
            svgpath::PathCommandKind::Cubic => PathCommand::cubic_to(
                command.control1.x,
                command.control1.y,
                command.control2.x,
                command.control2.y,
                command.end.x,
                command.end.y,
            ),
            svgpath::PathCommandKind::Close => PathCommand::close(),
        });
    }
    path
}

fn validate_shape_numbers(object: &str, s: &Shape) -> Result<()> {
    for (name, value) in [
        ("width", s.width),
        ("height", s.height),
        ("strokeWidth", s.stroke_width),
        ("borderRadius", s.border_radius),
        ("iconBorderRadius", s.icon_border_radius),
    ] {
        if value < 0 {
            return Err(invalid_field(
                object,
                name,
                Some(&value.to_string()),
                "must be non-negative",
            ));
        }
    }
    validate_opacity(object, s.opacity)?;
    validate_dash(object, s.stroke_dash, s.stroke_width)?;
    if let Some(ratio) = s.content_aspect_ratio {
        if !finite(ratio) || ratio <= 0.0 {
            return Err(invalid_field(
                object,
                "contentAspectRatio",
                Some(&fv(ratio)),
                "must be finite and greater than zero",
            ));
        }
    }
    validate_text_numbers(object, "", &s.text)?;
    validate_shape_integer_bounds(object, s)
}

fn validate_connection_numbers(object: &str, c: &Connection) -> Result<()> {
    if c.stroke_width < 0 {
        return Err(invalid_field(
            object,
            "strokeWidth",
            Some(&c.stroke_width.to_string()),
            "must be non-negative",
        ));
    }
    validate_opacity(object, c.opacity)?;
    validate_dash(object, c.stroke_dash, c.stroke_width)?;
    if !finite(c.border_radius) || c.border_radius < 0.0 {
        return Err(invalid_field(
            object,
            "borderRadius",
            Some(&fv(c.border_radius)),
            "must be finite and non-negative",
        ));
    }
    if !finite(c.icon_border_radius) || c.icon_border_radius < 0.0 {
        return Err(invalid_field(
            object,
            "iconBorderRadius",
            Some(&fv(c.icon_border_radius)),
            "must be finite and non-negative",
        ));
    }
    if !finite(c.label_percentage) || c.label_percentage < 0.0 || c.label_percentage > 1.0 {
        return Err(invalid_field(
            object,
            "labelPercentage",
            Some(&fv(c.label_percentage)),
            "must be finite and within [0,1]",
        ));
    }
    validate_text_numbers(object, "", &c.text)?;
    if let Some(text) = &c.src_label {
        validate_text_numbers(object, "srcLabel.", text)?;
    }
    if let Some(text) = &c.dst_label {
        validate_text_numbers(object, "dstLabel.", text)?;
    }
    Ok(())
}

pub(crate) fn validate_opacity(object: &str, opacity: f64) -> Result<()> {
    if !finite(opacity) || !(0.0..=1.0).contains(&opacity) {
        return Err(invalid_field(
            object,
            "opacity",
            Some(&fv(opacity)),
            "must be finite and within [0,1]",
        ));
    }
    Ok(())
}

pub(crate) fn validate_dash(object: &str, dash: f64, stroke_width: i64) -> Result<()> {
    if !finite(dash) || dash < 0.0 {
        return Err(invalid_field(
            object,
            "strokeDash",
            Some(&fv(dash)),
            "must be finite and non-negative",
        ));
    }
    if dash > 0.0 && stroke_width > 0 {
        let (dash_size, gap_size) = svgpath::get_stroke_dash_attributes(stroke_width as f64, dash);
        if !finite(dash_size) || !finite(gap_size) || dash_size < 0.0 || gap_size < 0.0 {
            return Err(invalid_field(
                object,
                "strokeDash",
                Some(&fv(dash)),
                "must produce finite non-negative dash lengths with strokeWidth",
            ));
        }
    }
    Ok(())
}

fn validate_text_numbers(object: &str, prefix: &str, text: &Text) -> Result<()> {
    if text.font_size < 0 {
        return Err(invalid_field(
            object,
            &format!("{prefix}fontSize"),
            Some(&text.font_size.to_string()),
            "must be non-negative",
        ));
    }
    if text.label_width < 0 {
        return Err(invalid_field(
            object,
            &format!("{prefix}labelWidth"),
            Some(&text.label_width.to_string()),
            "must be non-negative",
        ));
    }
    if text.label_height < 0 {
        return Err(invalid_field(
            object,
            &format!("{prefix}labelHeight"),
            Some(&text.label_height.to_string()),
            "must be non-negative",
        ));
    }
    Ok(())
}

fn validate_shape_integer_bounds(object: &str, s: &Shape) -> Result<()> {
    let half_stroke = s.stroke_width / 2 + s.stroke_width % 2;
    for (name, position, dimension) in [("pos.x", s.pos.x, s.width), ("pos.y", s.pos.y, s.height)] {
        if checked_sub(position, half_stroke, MIN_INT, MAX_INT).is_none() {
            return Err(invalid_field(
                object,
                name,
                Some(&position.to_string()),
                "must fit raster bounds arithmetic with strokeWidth",
            ));
        }
        let Some(end) = checked_add(position, dimension, MIN_INT, MAX_INT) else {
            return Err(invalid_field(
                object,
                name,
                Some(&position.to_string()),
                "must fit raster bounds arithmetic with its dimension",
            ));
        };
        if checked_add(end, half_stroke, MIN_INT, MAX_INT).is_none() {
            return Err(invalid_field(
                object,
                name,
                Some(&position.to_string()),
                "must fit raster bounds arithmetic with strokeWidth",
            ));
        }
    }
    Ok(())
}

fn validate_route_coordinate(
    object: &str,
    field: &str,
    coordinate: f64,
    stroke_width: i64,
) -> Result<()> {
    let half_stroke = (stroke_width as f64 / 2.0).ceil();
    if coordinate.floor() - half_stroke <= MIN_INT as f64
        || coordinate.ceil() + half_stroke >= MAX_INT as f64
    {
        return Err(invalid_field(
            object,
            field,
            Some(&fv(coordinate)),
            "must fit raster integer bounds arithmetic",
        ));
    }
    Ok(())
}

pub(crate) fn invalid_padding_error(pad: i64) -> Error {
    Error(format!("padding {pad} produces invalid scene dimensions"))
}

fn dimensions(tl: TargetPoint, br: TargetPoint, pad: i64) -> Result<(i64, i64, i64, i64)> {
    let fail = || invalid_padding_error(pad);
    let double_pad = checked_add(pad, pad, MIN_INT, MAX_INT).ok_or_else(fail)?;
    let left = checked_sub(tl.x, pad, MIN_INT, MAX_INT).ok_or_else(fail)?;
    let top = checked_sub(tl.y, pad, MIN_INT, MAX_INT).ok_or_else(fail)?;
    let width = checked_sub(br.x, tl.x, MIN_INT, MAX_INT).ok_or_else(fail)?;
    let width = checked_add(width, double_pad, MIN_INT, MAX_INT).ok_or_else(fail)?;
    let height = checked_sub(br.y, tl.y, MIN_INT, MAX_INT).ok_or_else(fail)?;
    let height = checked_add(height, double_pad, MIN_INT, MAX_INT).ok_or_else(fail)?;
    if width < 0 || height < 0 {
        return Err(fail());
    }
    Ok((left, top, width, height))
}

fn expand_dimensions(
    left: i64,
    top: i64,
    width: i64,
    height: i64,
    amount: i64,
) -> Option<(i64, i64, i64, i64)> {
    let double_amount = checked_add(amount, amount, MIN_INT, MAX_INT)?;
    Some((
        checked_sub(left, amount, MIN_INT, MAX_INT)?,
        checked_sub(top, amount, MIN_INT, MAX_INT)?,
        checked_add(width, double_amount, MIN_INT, MAX_INT)?,
        checked_add(height, double_amount, MIN_INT, MAX_INT)?,
    ))
}
