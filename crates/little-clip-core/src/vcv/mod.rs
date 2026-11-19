//! VCV Rack / Cardinal `.vcv` patch parsing, topology analysis, layout, and preview rendering
//! adapted from `neilbgr/ModularPatchTools`.

pub mod analyze;
pub mod archive;
pub mod catalog;
pub mod layout;
mod panels;
pub mod rack;
pub mod render;

pub use analyze::{
    AnalyzedCable, AnalyzedModule, Band, PatchAnalysis, PatchGroup, SignalType, Voice,
    analyze_patch, build_patch_groups,
};
pub use archive::{
    MAX_PATCH_BYTES, PatchCable, PatchDocument, PatchModule, PortRef, is_vcv_strip_path,
    read_patch_bytes, read_patch_bytes_for_path, read_patch_path,
};
pub use catalog::{ModuleFunction, ModuleInfo, Role, RoleSource, VcvCatalog};
pub use layout::{
    DiagramBand, DiagramEdge, DiagramLayout, DiagramNode, ModulePanelSpec, MonitorView, PortTab,
    build_diagram_layout, build_dynamic_hover_layout, build_strip_layout,
};
pub use panels::{
    PanelAssets, ScreenshotBatchResult, clear_downloaded_panel_cache,
    count_missing_local_screenshot_plugins, download_viewer_panels,
    ensure_local_screenshots_for_patch, generate_all_local_rack_screenshots, load_viewer_panels,
    local_rack_available,
};
pub use rack::{
    PlacedRackModule, RACK_HP_WIDTH, RACK_ROW_GAP, RACK_ROW_HEIGHT, RackRailLayout,
    RankedStripModule, StripLineup, build_full_single_row_strip, build_rack_rail_layout,
    build_ranked_strip,
};
pub use render::{
    DiagramTheme, RenderedDiagram, render_layout_outlined_svg, render_layout_rgba,
    render_layout_svg, render_rack_rails_rgba, render_rack_rails_svg, render_ranked_strip_rgba,
    render_ranked_strip_svg,
};

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::{
    OnceLock,
    atomic::{AtomicBool, Ordering},
};

/// High-level summary metadata extracted from a `.vcv` patch.
#[derive(Debug)]
pub struct PatchSummary {
    pub title: String,
    pub total_modules: usize,
    pub active_modules: usize,
    pub cable_count: usize,
    pub voice_count: usize,
    pub monitor_count: usize,
    pub loop_count: usize,
    pub plugins: Vec<String>,
    pub ranked_strip: StripLineup,
    pub full_strip: Option<StripLineup>,
    pub rack: RackRailLayout,
    pub analysis: PatchAnalysis,
    assets: PanelAssets,
    signal_flow: OnceLock<DiagramLayout>,
}

impl PatchSummary {
    /// Builds the signal-flow overview on its first explicit request.
    pub fn signal_flow(&self) -> &DiagramLayout {
        self.signal_flow
            .get_or_init(|| build_strip_layout(&self.analysis, &self.title))
    }
}

fn path_title(path: &Path) -> &str {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("VCV Patch")
}

/// Reads and analyzes a patch from bytes for a local-only preview.
pub fn inspect_bytes(title: &str, bytes: &[u8]) -> Result<PatchSummary, String> {
    prepare_preview(title, read_patch_bytes(bytes)?, &AtomicBool::new(false))
}

/// Reads and analyzes raw patch bytes associated with a path.
pub fn inspect_path_bytes(path: &Path, bytes: &[u8]) -> Result<PatchSummary, String> {
    inspect_path_bytes_cancellable(path, bytes, &AtomicBool::new(false))
}

/// Prepares a cancellable local-only preview from an already admitted source.
pub fn inspect_path_bytes_cancellable(
    path: &Path,
    bytes: &[u8],
    cancel: &AtomicBool,
) -> Result<PatchSummary, String> {
    prepare_preview(
        path_title(path),
        read_patch_bytes_for_path(path, bytes)?,
        cancel,
    )
}

/// Reads and analyzes a patch from a guarded disk snapshot.
pub fn inspect_path(path: &Path) -> Result<PatchSummary, String> {
    prepare_preview(
        path_title(path),
        read_patch_path(path)?,
        &AtomicBool::new(false),
    )
}

fn prepare_preview(
    title: &str,
    document: PatchDocument,
    cancel: &AtomicBool,
) -> Result<PatchSummary, String> {
    if cancel.load(Ordering::Relaxed) {
        return Err("Cancelled".into());
    }
    let analysis = analyze_patch(document)?;
    let assets = panels::load_panel_thumbnails(&analysis.document.panel_keys(), cancel);
    if cancel.load(Ordering::Relaxed) {
        return Err("Cancelled".into());
    }
    Ok(summarize(title, analysis, assets))
}

fn summarize(title: &str, analysis: PatchAnalysis, assets: PanelAssets) -> PatchSummary {
    let total_modules = analysis.document.modules.len();
    let cable_count = analysis.document.cables.len();
    let active_modules = analysis.shown_module_count();
    let voice_count = analysis.voices.len();
    let monitor_count = analysis.monitor_count();
    let loop_count = analysis.loops.len();

    let mut plugin_set = BTreeSet::new();
    for m in &analysis.modules {
        plugin_set.insert(m.plugin.clone());
    }
    let plugins: Vec<String> = plugin_set.into_iter().collect();
    PatchSummary {
        title: title.to_string(),
        total_modules,
        active_modules,
        cable_count,
        voice_count,
        monitor_count,
        loop_count,
        plugins,
        ranked_strip: build_ranked_strip(&analysis, &assets),
        full_strip: build_full_single_row_strip(&analysis, &assets),
        rack: build_rack_rail_layout(&analysis, &assets),
        analysis,
        assets,
        signal_flow: OnceLock::new(),
    }
}

/// Renders the ranked module lineup strip into RGBA pixels.
pub fn render_strip_rgba(
    summary: &PatchSummary,
    scale: f32,
    dark: bool,
) -> Result<RenderedDiagram, String> {
    render_ranked_strip_rgba(&summary.ranked_strip, &summary.assets, scale, dark)
}

/// Renders the full un-deduplicated single-row module strip in patch order, when applicable.
pub fn render_full_strip_rgba(
    summary: &PatchSummary,
    scale: f32,
    dark: bool,
) -> Option<Result<RenderedDiagram, String>> {
    summary
        .full_strip
        .as_ref()
        .map(|lineup| render_ranked_strip_rgba(lineup, &summary.assets, scale, dark))
}

/// Renders the physical Rack rail layout into RGBA pixels.
pub fn render_rack_rgba(
    summary: &PatchSummary,
    scale: f32,
    dark: bool,
) -> Result<RenderedDiagram, String> {
    render_rack_rails_rgba(&summary.rack, &summary.assets, scale, dark)
}

/// Builds and renders a dynamically unfolded diagram fitting within `(max_width_px, max_height_px)`.
pub fn render_dynamic_hover_rgba(
    summary: &PatchSummary,
    max_width_px: f32,
    max_height_px: f32,
    scale: f32,
    dark: bool,
) -> Result<(DiagramLayout, RenderedDiagram), String> {
    let layout = build_dynamic_hover_layout(
        &summary.analysis,
        &summary.title,
        f64::from(max_width_px),
        f64::from(max_height_px),
    );
    let rendered = render_layout_rgba(&layout, &summary.assets, scale, dark)?;
    Ok((layout, rendered))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn sample_patch_json() -> Vec<u8> {
        let json = serde_json::json!({
            "version": "2.6.0",
            "modules": [
                { "id": 1, "plugin": "Core", "model": "MIDIToCVInterface", "pos": [0, 0] },
                { "id": 2, "plugin": "Fundamental", "model": "VCO", "pos": [10, 0], "params": [{ "id": 0, "value": 0.42 }] },
                { "id": 3, "plugin": "Fundamental", "model": "VCF", "pos": [20, 0], "params": [{ "id": 0, "value": 0.65 }, { "id": 1, "value": 0.3 }] },
                { "id": 4, "plugin": "Fundamental", "model": "ADSR", "pos": [28, 0] },
                { "id": 5, "plugin": "Fundamental", "model": "VCA-1", "pos": [36, 0] },
                { "id": 6, "plugin": "Fundamental", "model": "VCMixer", "pos": [42, 0] },
                { "id": 7, "plugin": "Core", "model": "AudioInterface2", "pos": [52, 0] },
                { "id": 8, "plugin": "Fundamental", "model": "Scope", "pos": [60, 0] },
                { "id": 9, "plugin": "Fundamental", "model": "VCO", "pos": [10, 1], "params": [{ "id": 0, "value": 0.78 }] },
                { "id": 10, "plugin": "Fundamental", "model": "VCF", "pos": [20, 1] },
                { "id": 11, "plugin": "Fundamental", "model": "SEQ3", "pos": [0, 1] }
            ],
            "cables": [
                { "id": 1, "outputModuleId": 1, "outputId": 0, "inputModuleId": 2, "inputId": 0, "color": "#ffd452" },
                { "id": 2, "outputModuleId": 1, "outputId": 1, "inputModuleId": 4, "inputId": 0, "color": "#52beff" },
                { "id": 3, "outputModuleId": 2, "outputId": 0, "inputModuleId": 3, "inputId": 0, "color": "#ff5252" },
                { "id": 4, "outputModuleId": 3, "outputId": 0, "inputModuleId": 5, "inputId": 1, "color": "#ff5252" },
                { "id": 5, "outputModuleId": 4, "outputId": 0, "inputModuleId": 5, "inputId": 0, "color": "#52ff7d" },
                { "id": 6, "outputModuleId": 5, "outputId": 0, "inputModuleId": 6, "inputId": 0, "color": "#ff5252" },
                { "id": 7, "outputModuleId": 6, "outputId": 0, "inputModuleId": 7, "inputId": 0, "color": "#ff5252" },
                { "id": 8, "outputModuleId": 3, "outputId": 0, "inputModuleId": 8, "inputId": 0, "color": "#ff5252" },
                { "id": 9, "outputModuleId": 11, "outputId": 2, "inputModuleId": 9, "inputId": 0, "color": "#ffd452" },
                { "id": 10, "outputModuleId": 9, "outputId": 0, "inputModuleId": 10, "inputId": 0, "color": "#ff5252" },
                { "id": 11, "outputModuleId": 10, "outputId": 0, "inputModuleId": 6, "inputId": 1, "color": "#ff5252" }
            ]
        });
        serde_json::to_vec(&json).expect("serialize sample patch")
    }

    fn compress_tar_vcv(patch_json: &[u8]) -> Vec<u8> {
        let mut tar_buf = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_buf);
            let mut header = tar::Header::new_gnu();
            header.set_path("./patch.json").unwrap();
            header.set_size(patch_json.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append(&header, patch_json).unwrap();
            builder.finish().unwrap();
        }
        let mut encoder = zstd::stream::Encoder::new(Vec::new(), 3).unwrap();
        encoder.write_all(&tar_buf).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn parses_json_and_zst_tar_vcv_formats() {
        let json_bytes = sample_patch_json();
        let tar_bytes = compress_tar_vcv(&json_bytes);

        let doc_json = read_patch_bytes(&json_bytes).expect("read raw json vcv");
        let doc_tar = read_patch_bytes(&tar_bytes).expect("read zstd+tar vcv");
        assert_eq!(doc_json.modules.len(), doc_tar.modules.len());
        assert_eq!(doc_json.cables.len(), doc_tar.cables.len());
        assert_eq!(doc_json.row_count(), 2);
        assert_eq!(doc_tar.row_count(), 2);

        let empty = read_patch_bytes(br#"{"version":"2.6.0","modules":[],"cables":[]}"#)
            .expect("empty patch");
        assert_eq!(empty.row_count(), 0);

        let sparse = read_patch_bytes(
            br#"{
                "version":"2.6.0",
                "modules":[
                    {"id":1,"plugin":"Fundamental","model":"VCO","pos":[0,-1]},
                    {"id":2,"plugin":"Fundamental","model":"VCF","pos":[10,-1]},
                    {"id":3,"plugin":"Fundamental","model":"VCA-1","pos":[0,2]}
                ],
                "cables":[]
            }"#,
        )
        .expect("sparse rows patch");
        assert_eq!(sparse.row_count(), 2);
    }

    #[test]
    fn parses_vcvss_strip_presets_in_physical_order_with_zero_rows() {
        let vcvss = serde_json::to_vec(&serde_json::json!({
            "stripVersion": 1,
            "leftModules": [
                { "id": 20, "plugin": "Fundamental", "model": "VCF" },
                { "id": 10, "plugin": "Fundamental", "model": "VCO" }
            ],
            "leftWidth": 240.0,
            "rightModules": [
                { "id": 30, "plugin": "Fundamental", "model": "VCA-1" },
                { "id": 40, "plugin": "Core", "model": "AudioInterface2" }
            ],
            "rightWidth": 240.0,
            "cables": [
                { "outputModuleId": 10, "outputId": 0, "inputModuleId": 20, "inputId": 0, "color": "#ff5252" },
                { "outputModuleId": 20, "outputId": 0, "inputModuleId": 30, "inputId": 0, "color": "#ffd452" }
            ],
            "version": "2.6.0"
        }))
        .unwrap();

        let doc = read_patch_bytes(&vcvss).expect("read .vcvss strip");
        assert!(doc.is_strip);
        assert_eq!(doc.row_count(), 0);
        assert_eq!(doc.modules.len(), 4);
        assert_eq!(doc.cables.len(), 2);
        let ids: Vec<i64> = doc.modules.iter().map(|m| m.id).collect();
        assert_eq!(ids, [10, 20, 30, 40]);

        let summary = inspect_path_bytes(Path::new("voice.vcvss"), &vcvss).expect("analyze .vcvss");
        assert_eq!(summary.total_modules, 4);
        assert_eq!(summary.cable_count, 2);
        assert_eq!(summary.analysis.document.row_count(), 0);
        assert_eq!(
            summary
                .rack
                .modules
                .iter()
                .map(|m| m.id)
                .collect::<Vec<_>>(),
            [10, 20, 30, 40]
        );
        let strip_img = render_strip_rgba(&summary, 1.0, true).expect("render strip");
        assert!(strip_img.width > 0 && strip_img.height > 0);
        let rack_img = render_rack_rgba(&summary, 0.5, true).expect("render rack");
        assert!(rack_img.width > 0 && rack_img.height > 0);

        // Even a patch JSON saved under a .vcvss filename stays tagged as 0-row strip metadata.
        let flat = inspect_path_bytes(Path::new("compat.vcvss"), &sample_patch_json())
            .expect("inspect .vcvss extension");
        assert_eq!(flat.analysis.document.row_count(), 0);
    }

    #[test]
    fn analyzes_voices_monitors_and_badge_watchers() {
        let summary = inspect_bytes("Test Synth", &sample_patch_json()).expect("analyze sample");
        assert!(summary.signal_flow.get().is_none());
        assert_eq!(summary.voice_count, 2);
        assert_eq!(summary.monitor_count, 1);
        assert!(!summary.signal_flow().nodes.is_empty());
        assert!(std::ptr::eq(summary.signal_flow(), summary.signal_flow()));

        // Scopes and displays default to MonitorView::Badge: no Scope box in diagram,
        // and the watched node carries the watcher badge.
        assert!(
            summary
                .signal_flow()
                .nodes
                .iter()
                .all(|n| n.band != Band::Monitor),
            "monitor band should not create separate boxes in Badge mode"
        );
        assert!(
            summary
                .signal_flow()
                .nodes
                .iter()
                .any(|n| !n.watchers.is_empty()),
            "watched module/voice should carry a Scope badge"
        );
        assert!(
            summary
                .signal_flow()
                .nodes
                .iter()
                .all(|n| !n.panels.is_empty()),
            "every node should include miniature module panel specs"
        );
    }

    #[test]
    fn dynamic_hover_unfolds_more_when_given_more_vertical_space() {
        let summary = inspect_bytes("Dynamic Unfold", &sample_patch_json()).expect("analyze");
        let small = build_dynamic_hover_layout(&summary.analysis, "Small", 900.0, 220.0);
        let tall = build_dynamic_hover_layout(&summary.analysis, "Tall", 1200.0, 900.0);

        assert!(
            tall.nodes.len() >= small.nodes.len(),
            "taller vertical budget should unfold at least as many nodes ({} vs {})",
            tall.nodes.len(),
            small.nodes.len()
        );
        assert!(tall.nodes.len() > summary.signal_flow().nodes.len());
    }

    #[test]
    fn renders_strip_and_hover_to_rgba_pixels() {
        let summary = inspect_bytes("Render Test", &sample_patch_json()).expect("analyze");
        let strip_img = render_strip_rgba(&summary, 1.0, true).expect("render strip");
        assert!(strip_img.width > 100 && strip_img.height > 40);
        assert_eq!(
            strip_img.rgba.len(),
            strip_img.width as usize * strip_img.height as usize * 4
        );

        let (_layout, hover_img) =
            render_dynamic_hover_rgba(&summary, 1200.0, 800.0, 1.0, true).expect("render hover");
        assert!(hover_img.width > 150 && hover_img.height > 80);
        assert_eq!(
            hover_img.rgba.len(),
            hover_img.width as usize * hover_img.height as usize * 4
        );
    }

    #[test]
    fn panel_art_is_size_specific_and_shared_in_svg() {
        let directory = tempfile::tempdir().unwrap();
        let cache = directory.path();
        let plugin = cache.join("CustomPlugin");
        std::fs::create_dir_all(&plugin).unwrap();

        let color = [40, 80, 120, 255];
        image::RgbaImage::from_pixel(360, 1520, image::Rgba(color))
            .save(plugin.join("MyVCO.png"))
            .unwrap();
        let modules = [("CustomPlugin".into(), "MyVCO".into())];
        let cancel = AtomicBool::new(false);
        let viewer = panels::load_panels_with_dirs(&modules, true, &[], Some(cache), &cancel);
        assert_eq!(
            viewer
                .viewer_image("CustomPlugin", "MyVCO")
                .unwrap()
                .dimensions(),
            (360, 1520)
        );
        assert_eq!(viewer.thumbnails().count(), 0);

        let thumbnails = panels::load_panels_with_dirs(&modules, false, &[], Some(cache), &cancel);
        assert!(thumbnails.viewer_image("CustomPlugin", "MyVCO").is_none());
        let (_, uri) = thumbnails.thumbnails().next().expect("local thumbnail");
        let uri_png = base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            uri.strip_prefix("data:image/png;base64,").unwrap(),
        )
        .unwrap();
        assert_eq!(image::load_from_memory(&uri_png).unwrap().height(), 320);
        assert!(
            (thumbnails.aspect("CustomPlugin", "MyVCO").unwrap() - (360.0 / 1520.0)).abs() < 0.005
        );

        let document = archive::parse_patch_value(&serde_json::json!({
            "modules": [
                {"id": 1, "plugin": "CustomPlugin", "model": "MyVCO", "pos":[0, 0]},
                {"id": 2, "plugin": "CustomPlugin", "model": "MyVCO", "pos":[6, 0]}
            ]
        }))
        .unwrap();
        let analysis = analyze_patch(document).unwrap();
        let rack = build_rack_rail_layout(&analysis, &thumbnails);
        let svg = render_rack_rails_svg(&rack, &thumbnails, true);
        assert_eq!(svg.matches("data:image/png;base64,").count(), 1);
        let rendered = render_rack_rails_rgba(&rack, &thumbnails, 1.0, true).unwrap();
        for module in &rack.modules {
            let x = (module.x + module.width / 2.0) as usize;
            let y = (module.y + module.height / 2.0) as usize;
            let pixel = (y * rendered.width as usize + x) * 4;
            assert_eq!(rendered.rgba[pixel..pixel + 4], color);
        }
    }

    #[test]
    fn sparse_rack_uses_one_screw_pattern_across_a_wide_span() {
        let document = archive::parse_patch_value(&serde_json::json!({
            "modules": [
                {"id": 1, "plugin": "Example", "model": "Panel", "pos":[-16_000, 0]},
                {"id": 2, "plugin": "Example", "model": "Panel", "pos":[16_000, 0]}
            ]
        }))
        .unwrap();
        let summary = summarize(
            "Sparse rack",
            analyze_patch(document).unwrap(),
            PanelAssets::default(),
        );
        let svg = render_rack_rails_svg(&summary.rack, &summary.assets, true);
        assert_eq!(svg.matches("<pattern ").count(), 1);
        assert!(svg.len() < 16_000);
        let rendered = render_rack_rgba(&summary, 0.002, true).unwrap();
        assert!(rendered.width < 1024);
        assert_eq!(rendered.height, 1);
    }

    #[test]
    fn ranked_strip_includes_all_unique_modules_prioritized_left_to_right() {
        let summary = inspect_bytes("Ranked Test", &sample_patch_json()).expect("analyze");
        let models: Vec<&str> = summary
            .ranked_strip
            .modules
            .iter()
            .map(|m| m.model.as_str())
            .collect();

        // All distinct modules (including output-only AudioInterface2 and Scope) remain
        // present so full strips display every module when space allows.
        assert!(models.contains(&"AudioInterface2"));
        assert!(models.contains(&"Scope"));

        // Duplicate VCO and VCF are deduplicated with x2 count badges.
        let vco = summary
            .ranked_strip
            .modules
            .iter()
            .find(|m| m.model == "VCO")
            .expect("VCO present");
        assert_eq!(vco.count, 2);
        let vcf = summary
            .ranked_strip
            .modules
            .iter()
            .find(|m| m.model == "VCF")
            .expect("VCF present");
        assert_eq!(vcf.count, 2);

        // Tier 0 (sources/MIDI-in/SEQ3) precedes Tier 1 (VCF), which precedes ADSR/VCA/Mixer,
        // which precede Tier 6 (AudioInterface2 and Scope) when narrow rows need prioritizing.
        let pos_vco = models.iter().position(|&m| m == "VCO").unwrap();
        let pos_vcf = models.iter().position(|&m| m == "VCF").unwrap();
        let pos_adsr = models.iter().position(|&m| m == "ADSR").unwrap();
        let pos_mixer = models.iter().position(|&m| m == "VCMixer").unwrap();
        let pos_audio = models.iter().position(|&m| m == "AudioInterface2").unwrap();
        let pos_scope = models.iter().position(|&m| m == "Scope").unwrap();
        assert!(
            pos_vco < pos_vcf
                && pos_vcf < pos_adsr
                && pos_adsr < pos_mixer
                && pos_mixer < pos_audio
                && pos_mixer < pos_scope
        );

        // Also ensure uncabled modules (Tier 7) and blanks (Tier 8) stay included after Tier 6.
        let extra_json = serde_json::json!({
            "version": "2.6.0",
            "modules": [
                { "id": 1, "plugin": "Fundamental", "model": "VCO", "pos": [0, 0] },
                { "id": 2, "plugin": "Core", "model": "AudioInterface2", "pos": [10, 0] },
                { "id": 3, "plugin": "Cardinal", "model": "HostMIDI", "pos": [20, 0] },
                { "id": 4, "plugin": "Fundamental", "model": "Blank", "pos": [30, 0] }
            ],
            "cables": [
                { "id": 1, "outputModuleId": 1, "outputId": 0, "inputModuleId": 2, "inputId": 0, "color": "#ff5252" }
            ]
        });
        let extra = inspect_bytes(
            "Extra Strip Tiers",
            &serde_json::to_vec(&extra_json).unwrap(),
        )
        .unwrap();
        let extra_models: Vec<&str> = extra
            .ranked_strip
            .modules
            .iter()
            .map(|m| m.model.as_str())
            .collect();
        assert_eq!(
            extra_models,
            vec!["VCO", "AudioInterface2", "HostMIDI", "Blank"]
        );

        // Rack rail layout preserves saved multi-row coordinates (rows 0 and 1, all 11 modules).
        assert_eq!(summary.rack.modules.len(), 11);
        assert_eq!(summary.rack.rows.len(), 2);
        let row0_y = summary.rack.rows[0].1;
        let row1_y = summary.rack.rows[1].1;
        assert!(row1_y > row0_y + 300.0);
    }

    #[test]
    fn local_art_takes_priority_and_respects_decode_limits() {
        let tmp = tempfile::tempdir().unwrap();
        let shots = tmp.path().join("screenshots");
        let cache = tmp.path().join("cache");
        std::fs::create_dir_all(shots.join("Valley")).unwrap();
        std::fs::create_dir_all(cache.join("Valley")).unwrap();

        image::RgbaImage::from_pixel(300, 2000, image::Rgba([10, 200, 30, 255]))
            .save(shots.join("Valley/Plateau.png"))
            .unwrap();
        let fallback = image::RgbaImage::from_pixel(180, 380, image::Rgba([220, 20, 20, 255]));
        fallback.save(cache.join("Valley/Plateau.png")).unwrap();
        image::RgbaImage::new(1, 4097)
            .save(shots.join("Valley/TooTall.png"))
            .unwrap();

        let panels = panels::load_panels_with_dirs(
            &[
                ("Valley".into(), "Plateau".into()),
                ("Valley".into(), "TooTall".into()),
                ("..".into(), "Plateau".into()),
            ],
            true,
            &[shots],
            Some(&cache),
            &AtomicBool::new(false),
        );
        let resolved = panels
            .viewer_image("Valley", "Plateau")
            .expect("local screenshot");
        assert_eq!(resolved.dimensions(), (228, 1520));
        assert_eq!(resolved.get_pixel(0, 0).0, [10, 200, 30, 255]);
        assert!(panels.viewer_image("Valley", "TooTall").is_none());
        assert!(panels.viewer_image("..", "Plateau").is_none());
    }

    #[test]
    fn discovers_rack_env_from_log_prepares_staging_settings_and_detects_gaps() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let user_dir = home.join(".local/share/Rack2");
        let sys_dir = home.join("Rack2App");
        let plugin_dir = user_dir.join("plugins-lin-x64/MyPlugin");
        let shots_dir = user_dir.join("screenshots");
        std::fs::create_dir_all(&user_dir).unwrap();
        std::fs::create_dir_all(&sys_dir).unwrap();
        std::fs::create_dir_all(&plugin_dir).unwrap();
        std::fs::create_dir_all(shots_dir.join("MyPlugin")).unwrap();

        let exe_name = if cfg!(target_os = "windows") {
            "Rack.exe"
        } else {
            "Rack"
        };
        std::fs::write(sys_dir.join(exe_name), b"stub").unwrap();
        std::fs::write(
            user_dir.join("log.txt"),
            format!(
                "[0.000 info] System directory: {}\n[0.000 info] User directory: {}\n",
                sys_dir.display(),
                user_dir.display()
            ),
        )
        .unwrap();

        let env = panels::discover_rack_env_in(Some(&home), None, None);
        assert!(env.can_run_screenshots());
        assert_eq!(env.primary_user_dir.as_deref(), Some(user_dir.as_path()));
        assert_eq!(env.sys_dir.as_deref(), Some(sys_dir.as_path()));
        assert!(env.screenshot_dirs.contains(&shots_dir));

        let src_settings = user_dir.join("settings.json");
        let dst_settings = tmp.path().join("staging_settings.json");
        std::fs::write(
            &src_settings,
            r#"{"windowMaximized":true,"windowSize":[1920.0,1080.0],"preferDarkPanels":true}"#,
        )
        .unwrap();
        panels::write_staging_settings_json(&src_settings, &dst_settings).unwrap();
        let patched: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&dst_settings).unwrap()).unwrap();
        assert_eq!(patched["windowMaximized"], serde_json::json!(false));
        assert_eq!(patched["windowSize"], serde_json::json!([128.0, 128.0]));
        assert_eq!(patched["preferDarkPanels"], serde_json::json!(true));

        // Hidden modules in `plugin.json` do not mark a plugin as missing screenshots once visible modules exist.
        std::fs::write(
            plugin_dir.join("plugin.json"),
            r#"{"slug":"MyPlugin","modules":[{"slug":"VisibleMod"},{"slug":"OldHidden","hidden":true}]}"#,
        )
        .unwrap();
        assert!(panels::plugin_needs_screenshots(
            "MyPlugin",
            Some(&plugin_dir),
            Some(&sys_dir),
            std::slice::from_ref(&shots_dir),
        ));
        std::fs::write(shots_dir.join("MyPlugin/VisibleMod.png"), b"png").unwrap();
        assert!(!panels::plugin_needs_screenshots(
            "MyPlugin",
            Some(&plugin_dir),
            Some(&sys_dir),
            std::slice::from_ref(&shots_dir),
        ));

        // Seeded symlinks are preserved in staging without being re-copied; regular PNGs copy atomically.
        let staging_shots = tmp.path().join("staging_shots");
        panels::seed_existing_screenshot_links(
            &shots_dir.join("MyPlugin"),
            &staging_shots.join("MyPlugin"),
        );
        std::fs::create_dir_all(staging_shots.join("NewPlugin")).unwrap();
        std::fs::write(staging_shots.join("NewPlugin/Osc.png"), b"png-bytes").unwrap();
        let copied = panels::copy_generated_screenshots(&staging_shots, &shots_dir);
        assert_eq!(copied, 1);
        assert_eq!(
            std::fs::read(shots_dir.join("NewPlugin/Osc.png")).unwrap(),
            b"png-bytes"
        );
    }

    #[test]
    fn single_row_full_strip_preserves_duplicates_in_patch_order() {
        // sample_patch_json spans 2 rows, so full_strip must not be built.
        let multi = inspect_bytes("Multi", &sample_patch_json()).expect("analyze multi-row");
        assert!(multi.full_strip.is_none());

        // Single-row .vcv with duplicate VCO modules in [VCF, VCO, Scope, VCO] patch order.
        let single_row = serde_json::to_vec(&serde_json::json!({
            "version": "2.6.0",
            "modules": [
                { "id": 1, "plugin": "Fundamental", "model": "VCF", "pos": [0, 0] },
                { "id": 2, "plugin": "Fundamental", "model": "VCO", "pos": [10, 0] },
                { "id": 3, "plugin": "Fundamental", "model": "Scope", "pos": [20, 0] },
                { "id": 4, "plugin": "Fundamental", "model": "VCO", "pos": [30, 0] }
            ],
            "cables": []
        }))
        .unwrap();
        let single = inspect_bytes("Single", &single_row).expect("analyze single-row");
        let full = single.full_strip.as_ref().expect("single-row full strip");
        let models: Vec<&str> = full.modules.iter().map(|m| m.model.as_str()).collect();
        assert_eq!(models, ["VCF", "VCO", "Scope", "VCO"]);
        assert!(full.modules.iter().all(|m| m.count == 1));
        assert!(full.width > single.ranked_strip.width);

        // .vcvss strip preserves [leftModules.rev(), rightModules] order with duplicates.
        let strip_json = serde_json::to_vec(&serde_json::json!({
            "stripVersion": 1,
            "leftModules": [
                { "id": 2, "plugin": "Fundamental", "model": "VCO" },
                { "id": 1, "plugin": "Fundamental", "model": "AudioInterface2" }
            ],
            "rightModules": [
                { "id": 3, "plugin": "Fundamental", "model": "VCF" },
                { "id": 4, "plugin": "Fundamental", "model": "VCO" }
            ],
            "cables": [],
            "version": "2.6.0"
        }))
        .unwrap();
        let strip =
            inspect_path_bytes(Path::new("strip.vcvss"), &strip_json).expect("analyze vcvss");
        let strip_full = strip.full_strip.as_ref().expect("vcvss full strip");
        let strip_models: Vec<&str> = strip_full
            .modules
            .iter()
            .map(|m| m.model.as_str())
            .collect();
        assert_eq!(strip_models, ["AudioInterface2", "VCO", "VCF", "VCO"]);
        assert!(strip_full.modules.iter().all(|m| m.count == 1));
        let rendered = render_full_strip_rgba(&strip, 1.0, true)
            .expect("full strip present")
            .expect("render full strip");
        assert!(rendered.width > 0 && rendered.height > 0);
    }
}
