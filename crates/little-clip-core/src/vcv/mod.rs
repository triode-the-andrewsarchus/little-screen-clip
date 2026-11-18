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
    PatchCable, PatchDocument, PatchModule, PortRef, read_patch_bytes, read_patch_path,
};
pub use catalog::{ModuleFunction, ModuleInfo, Role, RoleSource, VcvCatalog};
pub use layout::{
    DiagramBand, DiagramEdge, DiagramLayout, DiagramNode, ModulePanelSpec, MonitorView, PortTab,
    build_diagram_layout, build_dynamic_hover_layout, build_strip_layout,
};
pub use panels::{
    ResolvedPanel, ScreenshotBatchResult, clear_downloaded_panel_cache,
    count_missing_local_screenshot_plugins, ensure_local_screenshots_for_patch,
    generate_all_local_rack_screenshots, local_rack_available, resolve_panel,
};
pub use rack::{
    PlacedRackModule, RACK_HP_WIDTH, RACK_ROW_GAP, RACK_ROW_HEIGHT, RackRailLayout,
    RankedStripModule, StripLineup, build_rack_rail_layout, build_ranked_strip,
};
pub use render::{
    DiagramTheme, RenderedDiagram, render_layout_outlined_svg, render_layout_rgba,
    render_layout_svg, render_rack_rails_rgba, render_rack_rails_svg, render_ranked_strip_rgba,
    render_ranked_strip_svg,
};

use std::collections::BTreeSet;
use std::path::Path;

/// High-level summary metadata extracted from a `.vcv` patch.
#[derive(Clone, Debug)]
pub struct PatchSummary {
    pub title: String,
    pub total_modules: usize,
    pub active_modules: usize,
    pub cable_count: usize,
    pub voice_count: usize,
    pub monitor_count: usize,
    pub loop_count: usize,
    pub plugins: Vec<String>,
    pub strip_dark: StripLineup,
    pub strip_light: StripLineup,
    pub rack_dark: RackRailLayout,
    pub rack_light: RackRailLayout,
    pub strip: DiagramLayout,
    pub analysis: PatchAnalysis,
}

/// Reads and analyzes a `.vcv` patch from bytes and prepares its overview strip layout.
pub fn inspect_bytes(title: &str, bytes: &[u8]) -> Result<PatchSummary, String> {
    let doc = read_patch_bytes(bytes)?;
    let total_modules = doc.modules.len();
    let cable_count = doc.cables.len();
    let analysis = analyze_patch(doc)?;
    Ok(summarize(title, total_modules, cable_count, analysis))
}

/// Reads and analyzes a `.vcv` patch from disk and prepares its overview strip layout.
pub fn inspect_path(path: &Path) -> Result<PatchSummary, String> {
    let doc = read_patch_path(path)?;
    let total_modules = doc.modules.len();
    let cable_count = doc.cables.len();
    let analysis = analyze_patch(doc)?;
    let title = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("VCV Patch");
    Ok(summarize(title, total_modules, cable_count, analysis))
}

fn summarize(
    title: &str,
    total_modules: usize,
    cable_count: usize,
    analysis: PatchAnalysis,
) -> PatchSummary {
    let active_modules = analysis.shown_module_count();
    let voice_count = analysis.voices.len();
    let monitor_count = analysis.monitor_count();
    let loop_count = analysis.loops.len();

    let mut plugin_set = BTreeSet::new();
    for m in &analysis.modules {
        plugin_set.insert(m.plugin.clone());
    }
    let plugins: Vec<String> = plugin_set.into_iter().collect();
    let strip_dark = build_ranked_strip(&analysis, true);
    let strip_light = build_ranked_strip(&analysis, false);
    let rack_dark = build_rack_rail_layout(&analysis, true);
    let rack_light = build_rack_rail_layout(&analysis, false);
    let strip = build_strip_layout(&analysis, title);

    PatchSummary {
        title: title.to_string(),
        total_modules,
        active_modules,
        cable_count,
        voice_count,
        monitor_count,
        loop_count,
        plugins,
        strip_dark,
        strip_light,
        rack_dark,
        rack_light,
        strip,
        analysis,
    }
}

/// Renders the ranked module lineup strip into RGBA pixels.
pub fn render_strip_rgba(
    summary: &PatchSummary,
    scale: f32,
    dark: bool,
) -> Result<RenderedDiagram, String> {
    let lineup = if dark {
        &summary.strip_dark
    } else {
        &summary.strip_light
    };
    render_ranked_strip_rgba(lineup, scale, dark)
}

/// Renders the physical Rack rail layout into RGBA pixels.
pub fn render_rack_rgba(
    summary: &PatchSummary,
    scale: f32,
    dark: bool,
) -> Result<RenderedDiagram, String> {
    let rack = if dark {
        &summary.rack_dark
    } else {
        &summary.rack_light
    };
    render_rack_rails_rgba(rack, scale, dark)
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
    let rendered = render_layout_rgba(&layout, scale, dark)?;
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
    }

    #[test]
    fn analyzes_voices_monitors_and_badge_watchers() {
        let summary = inspect_bytes("Test Synth", &sample_patch_json()).expect("analyze sample");
        assert_eq!(summary.voice_count, 2);
        assert_eq!(summary.monitor_count, 1);
        assert!(!summary.strip.nodes.is_empty());

        // Scopes and displays default to MonitorView::Badge: no Scope box in diagram,
        // and the watched node carries the watcher badge.
        assert!(
            summary.strip.nodes.iter().all(|n| n.band != Band::Monitor),
            "monitor band should not create separate boxes in Badge mode"
        );
        assert!(
            summary.strip.nodes.iter().any(|n| !n.watchers.is_empty()),
            "watched module/voice should carry a Scope badge"
        );
        assert!(
            summary.strip.nodes.iter().all(|n| !n.panels.is_empty()),
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
        assert!(tall.nodes.len() > summary.strip.nodes.len());
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
    fn validates_downloaded_modular_patch_tools_fixtures_if_present() {
        let Ok(home) = std::env::var("HOME") else {
            return;
        };
        let dir = Path::new(&home).join("Pictures/vcv");
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return;
        };
        let mut count = 0usize;
        for entry in entries.flatten() {
            let path = entry.path();
            if path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("vcv"))
            {
                let summary = inspect_path(&path)
                    .unwrap_or_else(|e| panic!("failed to analyze {}: {e}", path.display()));
                assert!(summary.active_modules > 0, "empty {}", path.display());
                let rendered = render_strip_rgba(&summary, 1.0, true)
                    .unwrap_or_else(|e| panic!("failed to render {}: {e}", path.display()));
                assert!(rendered.width > 0 && rendered.height > 0);
                count += 1;
            }
        }
        eprintln!(
            "Validated {count} real .vcv fixture patches from {}",
            dir.display()
        );
    }

    #[test]
    fn library_panel_cache_preserves_high_res_and_renders_strip_or_fallback() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("cache");
        let cache_plugin = cache.join("CustomPlugin");
        std::fs::create_dir_all(&cache_plugin).unwrap();

        // Build a synthetic 360x1520 lossless PNG (matching VCV Library 4x WebP height).
        let img = image::RgbaImage::from_pixel(360, 1520, image::Rgba([40, 80, 120, 255]));
        let mut png_bytes = Vec::new();
        img.write_to(
            &mut std::io::Cursor::new(&mut png_bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
        std::fs::write(cache_plugin.join("MyVCO.png"), &png_bytes).unwrap();

        let resolved = panels::resolve_panel_with_dirs("CustomPlugin", "MyVCO", &[], &cache, false)
            .expect("cached high-res library panel should resolve offline");
        assert_eq!(
            resolved.rgba.dimensions(),
            (360, 1520),
            "viewer panel texture must retain full 1520px vertical resolution"
        );
        assert!((resolved.aspect - (360.0 / 1520.0)).abs() < 0.005);
        assert!(resolved.data_uri.starts_with("data:image/png;base64,"));

        // Verify SVG data_uri thumbnail stays compact (320px height) for strip/hover embedding.
        let uri_b64 = resolved
            .data_uri
            .strip_prefix("data:image/png;base64,")
            .unwrap();
        let uri_png =
            base64::Engine::decode(&base64::engine::general_purpose::STANDARD, uri_b64).unwrap();
        let uri_img = image::load_from_memory(&uri_png).unwrap();
        assert_eq!(uri_img.height(), 320);

        // Unsafe slugs and uncached offline modules must return None cleanly.
        assert!(panels::resolve_panel_with_dirs("../etc", "passwd", &[], &cache, false).is_none());
        assert!(
            panels::resolve_panel_with_dirs("MissingPlugin", "Missing", &[], &cache, false)
                .is_none()
        );
        assert!(panels::resolve_panel("", "VCO").is_none());

        // Strip and rail SVGs render cleanly (embedding cached images or stylized fallbacks).
        let summary = inspect_bytes("Library + Fallback", &sample_patch_json()).expect("analyze");
        let strip_svg = render_ranked_strip_svg(&summary.strip_dark, true);
        let rack_svg = render_rack_rails_svg(&summary.rack_dark, true);
        assert!(strip_svg.contains("<svg") && strip_svg.contains("</svg>"));
        assert!(rack_svg.contains("<svg") && rack_svg.contains("</svg>"));
    }

    #[test]
    fn ranked_strip_includes_all_unique_modules_prioritized_left_to_right() {
        let summary = inspect_bytes("Ranked Test", &sample_patch_json()).expect("analyze");
        let models: Vec<&str> = summary
            .strip_dark
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
            .strip_dark
            .modules
            .iter()
            .find(|m| m.model == "VCO")
            .expect("VCO present");
        assert_eq!(vco.count, 2);
        let vcf = summary
            .strip_dark
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
            .strip_dark
            .modules
            .iter()
            .map(|m| m.model.as_str())
            .collect();
        assert_eq!(
            extra_models,
            vec!["VCO", "AudioInterface2", "HostMIDI", "Blank"]
        );

        // Rack rail layout preserves saved multi-row coordinates (rows 0 and 1, all 11 modules).
        assert_eq!(summary.rack_dark.modules.len(), 11);
        assert_eq!(summary.rack_dark.rows.len(), 2);
        let row0_y = summary.rack_dark.rows[0].1;
        let row1_y = summary.rack_dark.rows[1].1;
        assert!(row1_y > row0_y + 300.0);
    }

    #[test]
    fn library_panel_clamps_extreme_heights_while_keeping_standard_1520px() {
        let tall = image::RgbaImage::from_pixel(300, 2000, image::Rgba([20, 40, 60, 255]));
        let mut png_bytes = Vec::new();
        tall.write_to(
            &mut std::io::Cursor::new(&mut png_bytes),
            image::ImageFormat::Png,
        )
        .unwrap();

        let panel = panels::encode_raster_bytes_as_panel(&png_bytes).expect("encode tall raster");
        assert_eq!(panel.rgba.dimensions(), (228, 1520));
    }

    #[test]
    fn local_rack_screenshot_takes_priority_over_library_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let shots = tmp.path().join("screenshots");
        let cache = tmp.path().join("cache");
        std::fs::create_dir_all(shots.join("Valley")).unwrap();
        std::fs::create_dir_all(cache.join("Valley")).unwrap();

        let encode_png = |w: u32, h: u32, rgba: [u8; 4]| {
            let img = image::RgbaImage::from_pixel(w, h, image::Rgba(rgba));
            let mut out = Vec::new();
            img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
                .unwrap();
            out
        };

        std::fs::write(
            shots.join("Valley/Plateau.png"),
            encode_png(360, 1520, [10, 200, 30, 255]),
        )
        .unwrap();
        // Even with a valid cached library fallback, the local Rack screenshot must win.
        std::fs::write(
            cache.join("Valley/Plateau.png"),
            encode_png(180, 380, [220, 20, 20, 255]),
        )
        .unwrap();

        let resolved =
            panels::resolve_panel_with_dirs("Valley", "Plateau", &[shots], &cache, false)
                .expect("resolve panel");
        assert_eq!(resolved.rgba.dimensions(), (360, 1520));
        assert_eq!(resolved.rgba.get_pixel(0, 0).0, [10, 200, 30, 255]);
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
}
