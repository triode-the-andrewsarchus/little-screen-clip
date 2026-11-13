// SPDX-License-Identifier: MPL-2.0
//! font_fallback.go: the post-build text stage (font coverage, fallback
//! selection and glyph materialization through d2-rs-fontface's mirror of
//! Go's internal fontface), plus the sfnt metric helpers of code_shape.go
//! (codeFontMetrics.advance) and markdown.go (markdownFontAdvance).
//!
//! Font loading and shaping live here; bundled resolver authentication lives
//! in fallback.rs.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use d2_rs_fontface::fixed::Int26_6;
use d2_rs_fontface::sfnt::{self, Hinting};
use d2_rs_fontface::{BundledFaceSource, ParsedFace, ShapeFace, ShapeLimits, ShapingWorkspace};
use d2_rs_scene::{Asset, AssetId, Bounds, Font, Glyph, Node, Paint, Point, Primitive, TextRun};

use crate::Builder;
use crate::util::*;

const DEFAULT_MAX_RUNES_PER_TEXT: i64 = 100_000;
const DEFAULT_MAX_TOTAL_RUNES: i64 = 10_000_000;
const DEFAULT_MAX_COVERAGE_CHECKS: i64 = 50_000_000;
const DEFAULT_MAX_FACES_PER_TEXT: i64 = 64;
const DEFAULT_MAX_SHAPING_RUNS: i64 = 100_000;
const DEFAULT_MAX_SHAPED_GLYPHS: i64 = 10_000_000;
const MAX_SCENE_FONT_COVERAGE_ENTRIES: usize = 4_096;

/// Aligned with d2fonts' shaping placeholders.
const MISSING_GLYPH_PLACEHOLDERS: [char; 4] = ['\u{fffd}', '\u{25a1}', '\u{2610}', '?'];

fn safe_fallback_name(value: &str) -> String {
    let value: String = value
        .chars()
        .map(|c| if c < ' ' || c == '\u{7f}' { '?' } else { c })
        .take(80)
        .collect();
    if value.is_empty() {
        "unnamed font".into()
    } else {
        value
    }
}

#[derive(Clone, Copy)]
struct WorkLimits {
    max_runes_per_text: i64,
    max_total_runes: i64,
    max_coverage_checks: i64,
    max_faces_per_text: i64,
    max_shaping_runs: i64,
    max_shaped_glyphs: i64,
}

fn normalized_work_limits(options: Option<&crate::FontFallbackOptions>) -> Result<WorkLimits> {
    let mut limits = WorkLimits {
        max_runes_per_text: DEFAULT_MAX_RUNES_PER_TEXT,
        max_total_runes: DEFAULT_MAX_TOTAL_RUNES,
        max_coverage_checks: DEFAULT_MAX_COVERAGE_CHECKS,
        max_faces_per_text: DEFAULT_MAX_FACES_PER_TEXT,
        max_shaping_runs: DEFAULT_MAX_SHAPING_RUNS,
        max_shaped_glyphs: DEFAULT_MAX_SHAPED_GLYPHS,
    };
    let Some(o) = options else { return Ok(limits) };
    if [
        o.max_runes_per_text,
        o.max_total_runes,
        o.max_coverage_checks,
        o.max_font_faces_per_text,
        o.max_shaping_runs,
        o.max_shaped_glyphs,
    ]
    .iter()
    .any(|v| *v < 0)
    {
        return err("scene: font fallback work limits must not be negative");
    }
    if o.max_runes_per_text != 0 {
        limits.max_runes_per_text = o.max_runes_per_text;
    }
    if o.max_total_runes != 0 {
        limits.max_total_runes = o.max_total_runes;
    }
    if o.max_coverage_checks != 0 {
        limits.max_coverage_checks = o.max_coverage_checks;
    }
    if o.max_font_faces_per_text != 0 {
        limits.max_faces_per_text = o.max_font_faces_per_text;
    } else if o.max_assets > 0 {
        limits.max_faces_per_text = limits.max_faces_per_text.min(o.max_assets);
    }
    if o.max_shaping_runs != 0 {
        limits.max_shaping_runs = o.max_shaping_runs;
    }
    if o.max_shaped_glyphs != 0 {
        limits.max_shaped_glyphs = o.max_shaped_glyphs;
    }
    Ok(limits)
}

/// d2fonts registerBundledFontFaces: authenticate every bundled face once.
fn register_bundled_faces() -> std::result::Result<(), String> {
    static REGISTERED: std::sync::OnceLock<std::result::Result<(), String>> =
        std::sync::OnceLock::new();
    REGISTERED
        .get_or_init(|| {
            use sha2::{Digest, Sha256};
            for family in ["SourceSansPro", "SourceCodePro", "HandDrawn"] {
                for style in ["regular", "bold", "semibold", "italic"] {
                    // Unmodified bundled faces carry precomputed digests; the
                    // registry still authenticates each face's bytes on load.
                    let registered = match crate::fonts::lookup_digest(family, style) {
                        Some((size, digest)) => {
                            d2_rs_fontface::register_bundled_face_size(size, digest)
                        }
                        None => {
                            let data = crate::fonts::lookup(family, style).expect("bundled font");
                            let digest: [u8; 32] = Sha256::digest(&data).into();
                            d2_rs_fontface::register_bundled_face(&data, digest)
                        }
                    };
                    registered
                        .map_err(|e| format!("register bundled font {family}/{style}: {e}"))?;
                }
            }
            Ok(())
        })
        .clone()
}

/// The builder's per-document parsed faces (Go's b.fontFaces) and
/// coverage caches.
#[derive(Default)]
pub(crate) struct FontCache {
    faces: HashMap<AssetId, Arc<ParsedFace>>,
    sources: HashMap<AssetId, Option<Arc<BundledFaceSource>>>,
    coverage: HashMap<AssetId, HashMap<char, bool>>,
}

fn font_asset<'a>(assets: &'a BTreeMap<AssetId, Asset>, id: &str) -> Option<(&'a [u8], u16)> {
    match assets.get(id) {
        Some(Asset::FontAsset {
            data, face_index, ..
        }) => Some((data, *face_index)),
        _ => None,
    }
}

impl FontCache {
    /// fontFace.
    fn face(
        &mut self,
        assets: &BTreeMap<AssetId, Asset>,
        id: &str,
    ) -> std::result::Result<Arc<ParsedFace>, String> {
        if let Some(face) = self.faces.get(id) {
            return Ok(face.clone());
        }
        let Some((data, face_index)) = font_asset(assets, id) else {
            return Err(format!("font asset {} is missing or invalid", q(id)));
        };
        register_bundled_faces()?;
        let parse_error = |e: d2_rs_fontface::Error| {
            format!("parse face {face_index} from font asset {}: {e}", q(id))
        };
        let source =
            d2_rs_fontface::registered_bundled_face(data, face_index).map_err(parse_error)?;
        let source = match source {
            Some(source) => Some(source),
            None => d2_rs_fontface::registered_bundled_noto_color_emoji(data, face_index)
                .map_err(parse_error)?,
        };
        let face = match source {
            Some(source) => source.clone_read_only().map_err(parse_error)?,
            None => d2_rs_fontface::parse_face(data, face_index).map_err(parse_error)?,
        };
        let face = Arc::new(face);
        self.faces.insert(id.to_owned(), face.clone());
        Ok(face)
    }

    /// fontCoverage + sceneFontCoverage.supports.
    fn supports(
        &mut self,
        assets: &BTreeMap<AssetId, Asset>,
        id: &str,
        value: char,
    ) -> std::result::Result<bool, String> {
        if let Some(supported) = self.coverage.get(id).and_then(|m| m.get(&value)) {
            return Ok(*supported);
        }
        let face = self.face(assets, id)?;
        if !self.sources.contains_key(id) {
            let source = match font_asset(assets, id) {
                Some((data, face_index)) => {
                    let source = d2_rs_fontface::registered_bundled_face(data, face_index)
                        .map_err(|e| e.to_string())?;
                    match source {
                        Some(source) => Some(source),
                        None => {
                            d2_rs_fontface::registered_bundled_noto_color_emoji(data, face_index)
                                .map_err(|e| e.to_string())?
                        }
                    }
                }
                None => None,
            };
            self.sources.insert(id.to_owned(), source);
        }
        let supported = match &self.sources[id] {
            Some(source) => source.supports_renderable_rune(value),
            None => face.supports_renderable_rune(value),
        }
        .map_err(|e| e.to_string())?;
        let cache = self.coverage.entry(id.to_owned()).or_default();
        if cache.len() < MAX_SCENE_FONT_COVERAGE_ENTRIES {
            cache.insert(value, supported);
        }
        Ok(supported)
    }
}

/// A TextRun's shaping inputs and outputs, borrowed field by field so its
/// pattern paint roots can be traversed too.
struct TextReference<'a> {
    id: String,
    text: &'a str,
    font: &'a Font,
    fallbacks: &'a mut Option<Vec<AssetId>>,
    glyphs: &'a mut Option<Vec<Glyph>>,
    missing: BTreeSet<char>,
}

fn push_paint_root<'a>(paint: Option<&'a mut Paint>, out: &mut Vec<&'a mut Node>) {
    if let Some(Paint::PatternPaint(pattern)) = paint {
        if let Some(root) = pattern.root.as_mut() {
            // Textless tiles need no mutation. Preserve shared tile identity
            // through shaping, so raster resource planning sees the same root
            // for every use of a cached sketch streak or built-in pattern.
            if Arc::strong_count(root) == 1 || subtree_has_text(root) {
                out.push(Arc::make_mut(root));
            }
        }
    }
}

fn subtree_has_text(root: &Node) -> bool {
    fn push_paint<'a>(paint: Option<&'a Paint>, stack: &mut Vec<&'a Node>) {
        if let Some(Paint::PatternPaint(pattern)) = paint {
            stack.extend(pattern.root.as_deref());
        }
    }
    let mut stack = vec![root];
    let mut seen = std::collections::HashSet::new();
    while let Some(n) = stack.pop() {
        if !seen.insert(n as *const Node) {
            continue;
        }
        stack.extend(n.children());
        stack.extend(n.mask.as_ref().and_then(|m| m.root.as_deref()));
        let (fill, stroke) = match &n.primitive {
            Some(Primitive::TextRun(_)) => return true,
            Some(Primitive::Rect(p)) => (p.fill.as_ref(), p.stroke.as_ref()),
            Some(Primitive::Ellipse(p)) => (p.fill.as_ref(), p.stroke.as_ref()),
            Some(Primitive::Path(p)) => (p.fill.as_ref(), p.stroke.as_ref()),
            _ => continue,
        };
        push_paint(fill, &mut stack);
        push_paint(stroke.and_then(|s| s.paint.as_ref()), &mut stack);
    }
    false
}

fn push_stroke_root<'a>(stroke: Option<&'a mut d2_rs_scene::Stroke>, out: &mut Vec<&'a mut Node>) {
    if let Some(stroke) = stroke {
        push_paint_root(stroke.paint.as_mut(), out);
    }
}

/// sceneTextReferences: every TextRun reachable from the scene, its masks,
/// pattern paints and imported vector assets, in Go's stack order.
fn collect_text_runs<'a>(roots: Vec<&'a mut Node>, out: &mut Vec<TextReference<'a>>) {
    let mut stack: Vec<&'a mut Node> = roots;
    while let Some(n) = stack.pop() {
        let Node {
            id,
            children,
            mask,
            primitive,
            ..
        } = n;
        if let Some(children) = children.as_mut() {
            stack.extend(children.iter_mut());
        }
        if let Some(root) = mask.as_mut().and_then(|m| m.root.as_mut()) {
            // The shared connection-label mask has only rectangles. Keep its
            // identity, as Go does, when shaping has no text to change.
            if Arc::strong_count(root) == 1 || subtree_has_text(root) {
                stack.push(Arc::make_mut(root));
            }
        }
        match primitive {
            Some(Primitive::TextRun(TextRun {
                text,
                font,
                fallbacks,
                glyphs,
                fill,
                stroke,
                ..
            })) => {
                out.push(TextReference {
                    id: id.clone(),
                    text,
                    font,
                    fallbacks,
                    glyphs,
                    missing: BTreeSet::new(),
                });
                push_paint_root(fill.as_mut(), &mut stack);
                push_stroke_root(stroke.as_mut(), &mut stack);
            }
            Some(Primitive::Rect(rect)) => {
                push_paint_root(rect.fill.as_mut(), &mut stack);
                push_stroke_root(rect.stroke.as_mut(), &mut stack);
            }
            Some(Primitive::Ellipse(ellipse)) => {
                push_paint_root(ellipse.fill.as_mut(), &mut stack);
                push_stroke_root(ellipse.stroke.as_mut(), &mut stack);
            }
            Some(Primitive::Path(path)) => {
                push_paint_root(path.fill.as_mut(), &mut stack);
                push_stroke_root(path.stroke.as_mut(), &mut stack);
            }
            _ => {}
        }
    }
}

/// Grapheme clusters (go-text segmenter; the UAX #29 rules are shared with
/// d2-rs-text's uniseg translation).
fn grapheme_clusters(text: &str) -> Vec<&str> {
    let mut clusters = Vec::new();
    let mut start = 0;
    for end in d2_rs_text::uniseg::grapheme_boundaries(text) {
        if end > start {
            clusters.push(&text[start..end]);
        }
        start = end;
    }
    if start < text.len() {
        clusters.push(&text[start..]);
    }
    clusters
}

impl Builder<'_> {
    /// resolveFontFallbacks followed by shapeTextRuns.
    pub(crate) fn resolve_and_shape_text(&mut self, root: &mut Node) -> Result<()> {
        // Imported vector assets are scene roots too (sorted by asset ID);
        // font assets are read from a snapshot so the vector roots can be
        // borrowed mutably.
        let mut assets = std::mem::take(&mut self.assets);
        let mut fonts: BTreeMap<AssetId, Asset> = assets
            .iter()
            .filter(|(_, a)| matches!(a, Asset::FontAsset { .. }))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let mut cache = std::mem::take(&mut self.font_cache);
        let result = {
            let mut roots: Vec<&mut Node> = vec![root];
            for asset in assets.values_mut() {
                if let Asset::VectorAsset {
                    root: Some(vector_root),
                    ..
                } = asset
                {
                    roots.push(vector_root);
                }
            }
            let mut references = Vec::new();
            collect_text_runs(roots, &mut references);
            self.resolve_font_fallbacks(&mut references, &mut fonts, &mut cache)
                .and_then(|()| self.shape_text_runs(&mut references, &fonts, &mut cache))
        };
        for (id, asset) in fonts {
            assets.entry(id).or_insert(asset);
        }
        self.assets = assets;
        self.font_cache = cache;
        result
    }

    fn resolve_font_fallbacks(
        &mut self,
        references: &mut [TextReference<'_>],
        assets: &mut BTreeMap<AssetId, Asset>,
        cache: &mut FontCache,
    ) -> Result<()> {
        let limits = normalized_work_limits(self.options.fonts.as_ref())?;
        if self
            .options
            .fonts
            .as_ref()
            .is_some_and(|f| f.resolver && f.resolver_impl.is_none())
        {
            return err(
                "scene: font fallback resolvers are not supported by the native scene builder",
            );
        }
        let mut missing_all = BTreeSet::new();
        // Buckets of references keyed by (family, style, weight), in order.
        let mut bucket_keys: Vec<(String, String, i64)> = Vec::new();
        let mut buckets: HashMap<(String, String, i64), Vec<usize>> = HashMap::new();
        for (reference_index, reference) in references.iter_mut().enumerate() {
            if reference.glyphs.as_ref().is_some_and(|g| !g.is_empty()) || reference.text.is_empty()
            {
                continue;
            }
            let fallbacks = reference.fallbacks.clone().unwrap_or_default();
            if fallbacks.len() as i64 >= limits.max_faces_per_text {
                return err(format!(
                    "scene: text node {} fallback font reference count {} exceeds per-text face limit {}",
                    q(&reference.id),
                    fallbacks.len(),
                    limits.max_faces_per_text
                ));
            }
            let mut font_ids = vec![reference.font.asset.clone()];
            font_ids.extend(fallbacks);
            let key = (
                reference.font.family.clone(),
                reference.font.style.clone(),
                reference.font.weight,
            );
            let mut runes = Vec::new();
            for value in reference.text.chars() {
                if runes.len() as i64 >= limits.max_runes_per_text {
                    return err(format!(
                        "scene: text node {} rune count exceeds per-text limit {}",
                        q(&reference.id),
                        limits.max_runes_per_text
                    ));
                }
                if self.font_runes >= limits.max_total_runes {
                    return err(format!(
                        "scene: text rune count exceeds total limit {}",
                        limits.max_total_runes
                    ));
                }
                self.font_runes += 1;
                runes.push(value);
            }
            let text: String = runes.iter().collect();
            let mut bucketed = false;
            for cluster in grapheme_clusters(&text) {
                let mut covered = false;
                for id in &font_ids {
                    let mut face_covers_cluster = true;
                    for value in cluster.chars() {
                        if d2_rs_fontface::is_default_ignorable_rune(value) {
                            continue;
                        }
                        if self.font_coverage_checks >= limits.max_coverage_checks {
                            return err(format!(
                                "scene: font coverage checks exceed limit {}",
                                limits.max_coverage_checks
                            ));
                        }
                        self.font_coverage_checks += 1;
                        let supported = cache.supports(assets, id, value).map_err(|e| {
                            Error(format!("scene: text node {}: {e}", q(&reference.id)))
                        })?;
                        if !supported {
                            face_covers_cluster = false;
                            break;
                        }
                    }
                    if face_covers_cluster {
                        covered = true;
                        break;
                    }
                }
                if covered {
                    continue;
                }
                for value in cluster.chars() {
                    if d2_rs_fontface::is_default_ignorable_rune(value) {
                        continue;
                    }
                    reference.missing.insert(value);
                    missing_all.insert(value);
                    if !bucketed {
                        bucketed = true;
                        if !buckets.contains_key(&key) {
                            bucket_keys.push(key.clone());
                        }
                        buckets
                            .entry(key.clone())
                            .or_default()
                            .push(reference_index);
                    }
                }
            }
        }
        let mut retained_assets = assets.len() as i64;
        let mut retained_bytes: i64 = assets
            .values()
            .map(|a| {
                if let Asset::FontAsset { data, .. } = a {
                    data.len() as i64
                } else {
                    0
                }
            })
            .sum();
        if let Some(fonts) = &self.options.fonts {
            if fonts.max_assets <= 0 || fonts.max_bytes <= 0 {
                return err(
                    "scene: font fallback resolver requires positive asset and byte limits",
                );
            }
            if retained_assets > fonts.max_assets {
                return err(format!(
                    "scene: retained font asset count {retained_assets} exceeds limit {}",
                    fonts.max_assets
                ));
            }
            if retained_bytes > fonts.max_bytes {
                return err(format!(
                    "scene: retained font bytes {retained_bytes} exceed limit {}",
                    fonts.max_bytes
                ));
            }
        }
        if missing_all.is_empty() {
            return Ok(());
        }
        let mut reusable = Vec::<AssetId>::new();
        for key in &bucket_keys {
            let mut resolver_missing = BTreeSet::new();
            for &index in &buckets[key] {
                resolver_missing.extend(references[index].missing.iter().copied());
            }
            let mut fallback_ids = Vec::new();
            for id in &reusable {
                let mut used = false;
                for value in resolver_missing.clone() {
                    if self.fallback_supports(cache, assets, id, value, limits)? {
                        resolver_missing.remove(&value);
                        used = true;
                    }
                }
                if used {
                    fallback_ids.push(id.clone());
                }
            }
            if !resolver_missing.is_empty() {
                if let Some(fonts) = self.options.fonts.as_ref() {
                    if let Some(resolver) = fonts.resolver_impl.as_ref() {
                        let request = crate::FallbackRequest {
                            runes: resolver_missing.into_iter().collect(),
                            family: key.0.clone(),
                            style: key.1.clone(),
                            weight: key.2,
                        };
                        let resources = resolver.resolve_fallbacks(&request).map_err(|e| {
                            Error(format!(
                                "scene: font fallback for family {} style {} weight {}: {e}",
                                q(&key.0),
                                q(&key.1),
                                key.2
                            ))
                        })?;
                        if resources.len() as i64 > fonts.max_assets {
                            return err(format!(
                                "scene: font fallback resolver returned {} resources, exceeding result limit {}",
                                resources.len(),
                                fonts.max_assets
                            ));
                        }
                        let mut returned_bytes = 0;
                        for (index, resource) in resources.iter().enumerate() {
                            if resource.data.len() as i64 > fonts.max_bytes - returned_bytes {
                                return err(format!(
                                    "scene: font fallback resolver bytes exceed limit {} at resource {index} ({})",
                                    fonts.max_bytes,
                                    safe_fallback_name(&resource.name)
                                ));
                            }
                            returned_bytes += resource.data.len() as i64;
                        }
                        for (index, resource) in resources.iter().enumerate() {
                            let name = safe_fallback_name(&resource.name);
                            if resource.data.is_empty() {
                                return err(format!(
                                    "scene: font fallback {index} ({name}) has no data"
                                ));
                            }
                            if resource.mime_type.trim().is_empty() {
                                return err(format!(
                                    "scene: font fallback {index} ({name}) has no MIME type"
                                ));
                            }
                            let id = crate::fallback::asset_id(resource);
                            if !assets.contains_key(&id) {
                                if retained_assets >= fonts.max_assets {
                                    return err(format!(
                                        "scene: retained font asset count exceeds limit {}",
                                        fonts.max_assets
                                    ));
                                }
                                if resource.data.len() as i64 > fonts.max_bytes - retained_bytes {
                                    return err(format!(
                                        "scene: retained font bytes exceed limit {}",
                                        fonts.max_bytes
                                    ));
                                }
                                assets.insert(
                                    id.clone(),
                                    Asset::FontAsset {
                                        mime_type: resource.mime_type.clone(),
                                        data: resource.data.to_vec(),
                                        data_file: None,
                                        face_index: resource.face_index,
                                    },
                                );
                                retained_assets += 1;
                                retained_bytes += resource.data.len() as i64;
                            }
                            let face = cache.face(assets, &id).map_err(|e| {
                                Error(format!("scene: font fallback {index} ({name}): {e}"))
                            })?;
                            if face.is_bundled_noto_color_emoji() && !reusable.contains(&id) {
                                reusable.push(id.clone());
                            }
                            if !fallback_ids.contains(&id) {
                                fallback_ids.push(id);
                            }
                        }
                    }
                }
            }
            for &reference_index in &buckets[key] {
                let reference = &mut references[reference_index];
                let mut selected: Vec<AssetId> = reference.fallbacks.clone().unwrap_or_default();
                for id in &fallback_ids {
                    for &value in &reference.missing {
                        if self.fallback_supports(cache, assets, id, value, limits)? {
                            if !selected.contains(id) {
                                selected.push(id.clone());
                            }
                            break;
                        }
                    }
                }
                let mut placeholder = false;
                let mut candidates = vec![reference.font.asset.clone()];
                candidates.extend(selected.iter().cloned());
                'faces: for id in &candidates {
                    for value in MISSING_GLYPH_PLACEHOLDERS {
                        if self.font_coverage_checks >= limits.max_coverage_checks {
                            return err(format!(
                                "scene: font coverage checks exceed limit {}",
                                limits.max_coverage_checks
                            ));
                        }
                        self.font_coverage_checks += 1;
                        if cache.supports(assets, id, value).map_err(Error)? {
                            placeholder = true;
                            break 'faces;
                        }
                    }
                }
                if !placeholder {
                    let (id, added) = self
                        .ensure_missing_glyph_font(assets, cache, retained_assets, retained_bytes)
                        .map_err(|e| {
                            Error(format!("scene: text node {}: {e}", q(&reference.id)))
                        })?;
                    if added != 0 {
                        retained_assets += 1;
                        retained_bytes += added;
                    }
                    if !selected.contains(&id) {
                        selected.push(id);
                    }
                }
                *reference.fallbacks = opt_vec(selected);
            }
        }
        Ok(())
    }

    fn fallback_supports(
        &mut self,
        cache: &mut FontCache,
        assets: &BTreeMap<AssetId, Asset>,
        id: &str,
        value: char,
        limits: WorkLimits,
    ) -> Result<bool> {
        if self.font_coverage_checks >= limits.max_coverage_checks {
            return err(format!(
                "scene: font coverage checks exceed limit {}",
                limits.max_coverage_checks
            ));
        }
        self.font_coverage_checks += 1;
        cache.supports(assets, id, value).map_err(Error)
    }

    fn ensure_missing_glyph_font(
        &self,
        assets: &mut BTreeMap<AssetId, Asset>,
        cache: &mut FontCache,
        retained_assets: i64,
        retained_bytes: i64,
    ) -> std::result::Result<(AssetId, i64), String> {
        let id = "font:SourceSansPro:regular".to_owned();
        let supports_placeholder = |cache: &mut FontCache,
                                    assets: &BTreeMap<AssetId, Asset>|
         -> std::result::Result<bool, String> {
            let face = cache.face(assets, &id)?;
            for value in MISSING_GLYPH_PLACEHOLDERS {
                if face
                    .supports_renderable_rune(value)
                    .map_err(|e| e.to_string())?
                {
                    return Ok(true);
                }
            }
            Ok(false)
        };
        if assets.contains_key(&id) {
            if !supports_placeholder(cache, assets)? {
                return Err("bundled missing-glyph font has no drawable placeholder".into());
            }
            return Ok((id, 0));
        }
        let data = crate::fonts::lookup("SourceSansPro", "regular")
            .filter(|d| !d.is_empty())
            .ok_or("bundled missing-glyph font is not loaded")?;
        if let Some(fonts) = &self.options.fonts {
            if retained_assets >= fonts.max_assets {
                return Err(format!(
                    "retained font asset count exceeds limit {} while adding missing-glyph font",
                    fonts.max_assets
                ));
            }
            if data.len() as i64 > fonts.max_bytes - retained_bytes {
                return Err(format!(
                    "retained font bytes exceed limit {} while adding missing-glyph font",
                    fonts.max_bytes
                ));
            }
        }
        assets.insert(
            id.clone(),
            Asset::FontAsset {
                mime_type: "font/ttf".into(),
                data: data.to_vec(),
                data_file: None,
                face_index: 0,
            },
        );
        match supports_placeholder(cache, assets) {
            Ok(true) => Ok((id, data.len() as i64)),
            other => {
                assets.remove(&id);
                cache.faces.remove(&id);
                Err(other.err().unwrap_or_else(|| {
                    "bundled missing-glyph font has no drawable placeholder".into()
                }))
            }
        }
    }

    fn shape_text_runs(
        &mut self,
        references: &mut [TextReference<'_>],
        assets: &BTreeMap<AssetId, Asset>,
        cache: &mut FontCache,
    ) -> Result<()> {
        let limits = normalized_work_limits(self.options.fonts.as_ref())?;
        let mut workspace = ShapingWorkspace::new();
        for reference in references.iter_mut() {
            if reference.glyphs.as_ref().is_some_and(|g| !g.is_empty()) || reference.text.is_empty()
            {
                continue;
            }
            let fallbacks = reference.fallbacks.clone().unwrap_or_default();
            if fallbacks.len() as i64 >= limits.max_faces_per_text {
                return err(format!(
                    "scene: text node {} fallback font reference count {} exceeds per-text face limit {}",
                    q(&reference.id),
                    fallbacks.len(),
                    limits.max_faces_per_text
                ));
            }
            let mut ids: Vec<AssetId> = Vec::with_capacity(1 + fallbacks.len());
            for id in std::iter::once(reference.font.asset.clone()).chain(fallbacks) {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
            if ids.len() as i64 > limits.max_faces_per_text {
                return err(format!(
                    "scene: text node {} font face count {} exceeds per-text limit {}",
                    q(&reference.id),
                    ids.len(),
                    limits.max_faces_per_text
                ));
            }
            let mut faces = Vec::with_capacity(ids.len());
            for id in &ids {
                faces.push(
                    cache.face(assets, id).map_err(|e| {
                        Error(format!("scene: text node {}: {e}", q(&reference.id)))
                    })?,
                );
            }
            let shape_faces: Vec<ShapeFace<'_>> = ids
                .iter()
                .zip(&faces)
                .map(|(id, face)| ShapeFace {
                    id: id.as_str(),
                    face: face.as_ref(),
                })
                .collect();
            let remaining_coverage = limits.max_coverage_checks - self.font_coverage_checks;
            if remaining_coverage <= 0 {
                return err(format!(
                    "scene: font coverage checks exceed limit {}",
                    limits.max_coverage_checks
                ));
            }
            let remaining_runs = limits.max_shaping_runs - self.font_shape_runs;
            if remaining_runs <= 0 {
                return err(format!(
                    "scene: text shaping run count exceeds limit {}",
                    limits.max_shaping_runs
                ));
            }
            let remaining_glyphs = limits.max_shaped_glyphs - self.font_glyphs;
            if remaining_glyphs <= 0 {
                return err(format!(
                    "scene: shaped glyph count exceeds limit {}",
                    limits.max_shaped_glyphs
                ));
            }
            let size = reference.font.size();
            if !finite(size) || size <= 0.0 || size > f64::from(i32::MAX) / 64.0 {
                return err(format!(
                    "scene: text node {} has an invalid font size for shaping",
                    q(&reference.id)
                ));
            }
            let ppem = (size * 64.0).round() as Int26_6;
            let shaped = workspace
                .shape_text_transient(
                    &(),
                    reference.text,
                    ppem,
                    &shape_faces,
                    ShapeLimits {
                        runes: limits.max_runes_per_text,
                        faces: limits.max_faces_per_text,
                        coverage_checks: remaining_coverage,
                        runs: remaining_runs,
                        glyphs: remaining_glyphs,
                    },
                )
                .map_err(|e| Error(format!("scene: text node {}: {e}", q(&reference.id))))?;
            let mut glyphs = Vec::with_capacity(shaped.glyphs.len());
            for glyph in &shaped.glyphs {
                if glyph.face >= ids.len() {
                    return err(format!(
                        "scene: text node {} shaper returned invalid face {}",
                        q(&reference.id),
                        glyph.face
                    ));
                }
                let ink = if glyph.has_ink {
                    let f = |v: Int26_6| f64::from(v) / 64.0;
                    Bounds::new(
                        f(glyph.ink.min.x),
                        f(glyph.ink.min.y),
                        f(glyph.ink.max.x),
                        f(glyph.ink.max.y),
                    )
                } else {
                    Bounds::default()
                };
                glyphs.push(Glyph {
                    id: glyph.id,
                    empty: glyph.empty,
                    asset: ids[glyph.face].clone(),
                    position: Point::new(glyph.position_x, glyph.position_y),
                    advance: glyph.advance,
                    ink,
                });
            }
            if glyphs.is_empty() {
                return err(format!(
                    "scene: text node {} shaped non-empty text to no glyphs",
                    q(&reference.id)
                ));
            }
            self.font_coverage_checks += shaped.coverage_checks;
            self.font_shape_runs += shaped.runs;
            self.font_glyphs += glyphs.len() as i64;
            *reference.glyphs = Some(glyphs);
        }
        Ok(())
    }
}

/// The parsed sfnt outline of a registered font asset (Go's
/// b.fontFace(id).Outline).
fn outline(b: &mut Builder<'_>, font: &Font) -> std::result::Result<Arc<sfnt::Font>, String> {
    let face = b.font_cache.face(&b.assets, &font.asset)?;
    Ok(face.outline_arc().clone())
}

fn ppem(size: f64) -> Int26_6 {
    (size * 64.0).round() as Int26_6
}

/// codeFontMetrics.advance: float accumulation of sfnt advances and kerns.
pub(crate) fn code_advance(
    b: &mut Builder<'_>,
    font: &Font,
    value: &str,
) -> std::result::Result<f64, String> {
    let face = outline(b, font)?;
    let ppem = ppem(font.size());
    let mut previous = None;
    let mut advance = 0.0;
    for (byte_offset, r) in value.char_indices() {
        let glyph = face.glyph_index(r).map_err(|e| {
            format!(
                "look up glyph for U+{:04X} at byte {byte_offset}: {e}",
                r as u32
            )
        })?;
        if glyph == 0 {
            return Err(format!(
                "missing glyph U+{:04X} at byte {byte_offset}",
                r as u32
            ));
        }
        if let Some(previous) = previous {
            match face.kern(previous, glyph, ppem, Hinting::None) {
                Ok(kern) => advance += f64::from(kern) / 64.0,
                Err(sfnt::Error::NotFound) => {}
                Err(e) => {
                    return Err(format!(
                        "kern glyph for U+{:04X} at byte {byte_offset}: {e}",
                        r as u32
                    ));
                }
            }
        }
        let glyph_advance = face
            .glyph_advance(glyph, ppem, Hinting::None)
            .map_err(|e| {
                format!(
                    "measure glyph for U+{:04X} at byte {byte_offset}: {e}",
                    r as u32
                )
            })?;
        advance += f64::from(glyph_advance) / 64.0;
        previous = Some(glyph);
    }
    Ok(advance)
}

/// markdownFontAdvance: fixed-point accumulation, converted once.
pub(crate) fn markdown_font_advance(
    b: &mut Builder<'_>,
    font: &Font,
    text: &str,
    size: f64,
) -> std::result::Result<f64, String> {
    let face = outline(b, font)?;
    let ppem = ppem(size);
    let mut advance: Int26_6 = 0;
    let mut previous = None;
    for value in text.chars() {
        let glyph = face
            .glyph_index(value)
            .map_err(|e| format!("look up U+{:04X}: {e}", value as u32))?;
        if glyph == 0 {
            return Err(format!("missing glyph U+{:04X}", value as u32));
        }
        if let Some(previous) = previous {
            match face.kern(previous, glyph, ppem, Hinting::None) {
                Ok(kern) => advance = advance.wrapping_add(kern),
                Err(sfnt::Error::NotFound) => {}
                Err(e) => return Err(format!("kern U+{:04X}: {e}", value as u32)),
            }
        }
        let glyph_advance = face
            .glyph_advance(glyph, ppem, Hinting::None)
            .map_err(|e| format!("advance U+{:04X}: {e}", value as u32))?;
        advance = advance.wrapping_add(glyph_advance);
        previous = Some(glyph);
    }
    Ok(f64::from(advance) / 64.0)
}
