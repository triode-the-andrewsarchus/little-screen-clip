//! Little Screen Clip desktop GUI workbench (`clip-workbench`).
//!
//! An egui-based control center providing multi-slot visual inspection, operation command palette,
//! visual node graph builder, audio/image/video/3D/PDF/MIDI inspection windows, and live screen mirror.

use eframe::egui;
use little_clip_core::color::parse_hex_color_rgba_or_white as parse_hex_color_rgba;
use little_clip_core::config::WorkbenchAppearance;
use little_clip_core::ipc::{IpcRequest, IpcResponse};
use little_clip_core::palette::AppPalette;
use little_clip_core::presentation::{
    midi::MidiData,
    model3d::is_3d_model_file,
    network_preview::{NetworkPreview, detect_network_preview},
    spreadsheet::{
        SpreadsheetData, detect_csv_text, is_spreadsheet_file, load_spreadsheet,
        looks_like_csv_text,
    },
    timestamps::detect_timestamp,
    units::detect_unit_conversion,
};
use little_clip_core::{
    AppConfig, BackgroundJobs, ClipItem, ClipPayload, JobError, JobTicket, SlotId, SlotManager,
};
use little_clip_ops::pipeline::recipes::{find_recipe_by_name, load_all_recipes};
use little_clip_ops::pipeline::types::{NodeKind, Recipe};
use little_clip_ops::{OpDescriptor, get_applicable_ops};
use little_clip_render::{SCREEN_HEIGHT, SCREEN_WIDTH, ScreenRenderer};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::time::Duration;

const MAX_INLINE_MARKDOWN_CLASSIFICATION_BYTES: usize = 64 * 1024;
const MAX_INLINE_TEXT_ROUTING_BYTES: usize = 64 * 1024;
const MAX_FILE_PATH_TEXT_BYTES: usize = 4096;

fn is_large_text_payload(item: &ClipItem) -> bool {
    matches!(&item.payload, ClipPayload::Text(text) | ClipPayload::Markdown(text) if text.len() > MAX_FILE_PATH_TEXT_BYTES)
}

fn bounded_item_file_path(item: &ClipItem) -> Option<PathBuf> {
    (!is_large_text_payload(item))
        .then(|| item.file_path())
        .flatten()
}

mod ansi;
mod appearance_sync;
mod audio_preview;
mod barcode_worker;
mod binary_diff_view;
mod canvas;
mod component_gallery;
mod conversion_decode_worker;
mod diff_view;
pub mod icons;
mod image_preview;
mod midi_view;
mod model3d_view;
mod pdf_view;
mod popup;
mod rolling_audio_strip;
pub mod shell;
mod spreadsheet_view;
mod state_sync;
mod stl_view;
mod text_preview;
mod ui_appearance;
pub mod ui_ext;

use appearance_sync::AppearanceSync;
use icons::Icon;

use audio_preview::AudioPreviewState;
use binary_diff_view::BinaryDiffViewState;
use canvas::*;
use diff_view::DiffViewerState;
use image_preview::ImagePreviewState;
use little_clip_ops::binary_diff::resolve_binary_diff_pair;
use little_clip_ops::diff::{
    DiffDisplayMode, is_image_path, resolve_diff_pair, resolve_diff_target,
};
use midi_view::MidiViewerState;
use model3d_view::Model3dViewerState;
use pdf_view::PdfEditorState;
use rolling_audio_strip::{RollingAudioAction, RollingAudioStripState};
pub use shell::*;
use spreadsheet_view::SpreadsheetViewerState;
use text_preview::TextPreviewState;
pub use ui_ext::*;

/// Primary navigation view tabs in the desktop workbench.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkbenchView {
    /// Command palette for searching and executing transform operations and recipes.
    Palette,
    /// Interactive visual node pipeline recipe editor.
    NodeMachine,
    /// Automated action trigger rules configuration.
    AutoActions,
}

pub(crate) fn hex_to_c32(hex: &str) -> egui::Color32 {
    let (r, g, b, a) = parse_hex_color_rgba(hex);
    egui::Color32::from_rgba_unmultiplied(r, g, b, a)
}

pub(crate) fn hex_to_c32_alpha(hex: &str, alpha: u8) -> egui::Color32 {
    let (r, g, b, _) = parse_hex_color_rgba(hex);
    egui::Color32::from_rgba_unmultiplied(r, g, b, alpha)
}

fn update_palette_search_focus(
    ctx: &egui::Context,
    response: &egui::Response,
    needs_focus: &mut bool,
    audio_region_active: bool,
    activate_from_audio: bool,
) {
    if audio_region_active {
        if response.has_focus() {
            ctx.memory_mut(|memory| memory.surrender_focus(response.id));
        }
        if activate_from_audio {
            response.request_focus();
        }
        *needs_focus = false;
    } else if *needs_focus {
        response.request_focus();
        *needs_focus = false;
    }
}

fn show_workbench_footer(
    ui: &mut egui::Ui,
    view: WorkbenchView,
    status_msg: Option<&str>,
    palette: &AppPalette,
) -> egui::Rect {
    let hint = match view {
        WorkbenchView::Palette => {
            "[Enter] Execute & Paste  |  [F2 / Tab] Big Node Machine  |  [Esc] Dismiss"
        }
        WorkbenchView::NodeMachine => {
            "[Right/Mid Drag] Pan Canvas  |  [Scroll] Zoom  |  [F2] Quick Palette  |  [Esc] Dismiss"
        }
        WorkbenchView::AutoActions => "[Click] Toggle Setting  |  [Esc] Dismiss",
    };
    let wrap_width = ui.available_width().max(1.0);
    let hint_galley = ui.fonts(|fonts| {
        fonts.layout(
            hint.to_owned(),
            egui::FontId::proportional(BODY_FONT_SIZE),
            hex_to_c32(&palette.colors.text_muted),
            wrap_width,
        )
    });
    let status_galley = status_msg.map(|message| {
        ui.fonts(|fonts| {
            fonts.layout(
                message.to_owned(),
                egui::FontId::proportional(BODY_FONT_SIZE),
                hex_to_c32(&palette.diff_del),
                wrap_width,
            )
        })
    });
    let gap = if status_galley.is_some() {
        ui.spacing().item_spacing.y
    } else {
        0.0
    };
    let footer_height = (hint_galley.size().y
        + status_galley.as_ref().map_or(0.0, |galley| galley.size().y)
        + gap
        + 1.0
        + 12.0)
        .ceil();
    let mut captions_rect = egui::Rect::NOTHING;
    egui::TopBottomPanel::bottom("workbench_footer_panel")
        .exact_height(footer_height)
        .frame(
            egui::Frame::none()
                .inner_margin(egui::Margin::symmetric(0.0, crate::ui_ext::FOOTER_INSET_Y)),
        )
        .show_inside(ui, |ui| {
            if let Some(galley) = &status_galley {
                let (rect, _) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), galley.size().y),
                    egui::Sense::hover(),
                );
                ui.painter()
                    .galley(rect.min, galley.clone(), egui::Color32::PLACEHOLDER);
                captions_rect = captions_rect.union(rect);
                ui.add_space(gap);
            }
            let separator_y = ui.cursor().min.y + 0.5;
            ui.painter().hline(
                ui.min_rect().x_range(),
                separator_y,
                egui::Stroke::new(1.0_f32, hex_to_c32(&palette.colors.surface_panel)),
            );
            ui.add_space(1.0);
            let (rect, _) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), hint_galley.size().y),
                egui::Sense::hover(),
            );
            ui.painter()
                .galley(rect.min, hint_galley.clone(), egui::Color32::PLACEHOLDER);
            captions_rect = captions_rect.union(rect);
        });
    captions_rect
}

/// Action item displayed in the command palette search list.
#[derive(Clone)]
enum PaletteAction {
    /// Built-in Rust transformation operation.
    Op(OpDescriptor),
    /// Configured visual pipeline recipe.
    Recipe(Recipe),
}

impl PaletteAction {
    fn label(&self) -> &str {
        match self {
            PaletteAction::Op(op) => op.label,
            PaletteAction::Recipe(r) => &r.name,
        }
    }

    fn category(&self) -> &str {
        match self {
            PaletteAction::Op(op) => op.category,
            PaletteAction::Recipe(_) => "Recipe",
        }
    }

    fn description(&self) -> &str {
        match self {
            PaletteAction::Op(op) => op.description,
            PaletteAction::Recipe(r) => &r.description,
        }
    }
}

struct InAppPromptState {
    recipe_name: String,
    message: String,
    input_text: String,
    first_frame: bool,
}

enum SpreadsheetLoadSource {
    File(PathBuf),
    Text { content: Arc<String>, title: String },
}

enum ViewerSession {
    Diff(DiffViewerState),
    BinaryDiff(BinaryDiffViewState),
    Audio(AudioPreviewState),
    Image(ImagePreviewState),
    ImageLoading(ImageLoadJob),
    Spreadsheet(SpreadsheetViewerState),
    Pdf(PdfEditorState),
    Midi(MidiViewerState),
    Model3d(Model3dViewerState),
    Text(TextPreviewState),
    ComponentGallery(component_gallery::ComponentGalleryState),
}

fn show_viewer_panel(ctx: &egui::Context, contents: impl FnOnce(&mut egui::Ui)) {
    egui::CentralPanel::default()
        .frame(egui::Frame::none())
        .show(ctx, contents);
}

fn is_image_preview_action_item(item: &ClipItem) -> bool {
    !is_large_text_payload(item) && item.is_image() && !is_pdf_clip_item(item)
}

fn is_pdf_clip_item(item: &ClipItem) -> bool {
    bounded_item_file_path(item)
        .as_deref()
        .is_some_and(little_clip_core::pdf::is_pdf_path)
        || matches!(
            &item.payload,
            little_clip_core::ClipPayload::Image { mime, .. }
                if mime.eq_ignore_ascii_case("application/pdf")
        )
}

type SpreadsheetLoadReceiver =
    Receiver<Result<Result<(SpreadsheetData, Option<PathBuf>), String>, JobError>>;
type MidiLoadReceiver = Receiver<Result<Result<(MidiData, PathBuf), String>, JobError>>;

enum ImageLoadSource {
    File(PathBuf),
    ItemFile {
        path: PathBuf,
        mime: String,
        title: String,
    },
    Bytes {
        bytes: Vec<u8>,
        mime: String,
        path: Option<PathBuf>,
        title: String,
    },
}

impl ImageLoadSource {
    fn from_item(item: &ClipItem) -> Result<Self, String> {
        if let ClipPayload::Image {
            bytes, mime, path, ..
        } = &item.payload
        {
            let title = if item.title.is_empty() {
                path.as_ref()
                    .and_then(|path| path.file_name())
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Image Preview".to_string())
            } else {
                item.title.clone()
            };
            if !bytes.is_empty() {
                // ClipPayload owns Vec bytes. Copy only the encoded payload once;
                // decoding, metadata extraction and pixel copies run on the worker.
                return Ok(Self::Bytes {
                    bytes: bytes.clone(),
                    mime: mime.clone(),
                    path: path.clone(),
                    title,
                });
            }
            if let Some(path) = path {
                return Ok(Self::ItemFile {
                    path: path.clone(),
                    mime: mime.clone(),
                    title,
                });
            }
        }
        item.image_path()
            .map(Self::File)
            .ok_or_else(|| "Item payload does not contain a recognizable image".to_string())
    }
}

struct ImageLoadJob {
    ticket: JobTicket,
    receiver: Receiver<Result<Result<ImagePreviewState, String>, JobError>>,
}

struct TextPreviewLoadJob {
    ticket: JobTicket,
    receiver: Receiver<Result<Result<TextPreviewState, String>, JobError>>,
}

struct RollingEditorLoadJob {
    ticket: JobTicket,
    cancelled: Arc<AtomicBool>,
    receiver: Receiver<
        Result<Result<(PathBuf, Option<little_clip_core::audio::AudioMetadata>), String>, JobError>,
    >,
    editor: String,
    ready: Option<Result<(PathBuf, Option<little_clip_core::audio::AudioMetadata>), String>>,
}

impl Drop for RollingEditorLoadJob {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        self.ticket.cancel();
        if let Some(Ok((path, _))) = self.ready.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

impl ImageLoadJob {
    fn new(source: ImageLoadSource, config: AppConfig) -> Self {
        let (ticket, receiver) =
            BackgroundJobs::global().submit(Duration::from_secs(30), move || match source {
                ImageLoadSource::File(path) => ImagePreviewState::from_path(path, &config),
                ImageLoadSource::ItemFile { path, mime, title } => std::fs::read(&path)
                    .map_err(|error| format!("Failed to read file {}: {error}", path.display()))
                    .and_then(|bytes| {
                        ImagePreviewState::from_bytes_with_meta(
                            bytes,
                            mime,
                            Some(path),
                            title,
                            &config,
                        )
                    }),
                ImageLoadSource::Bytes {
                    bytes,
                    mime,
                    path,
                    title,
                } => ImagePreviewState::from_bytes_with_meta(bytes, mime, path, title, &config),
            });
        Self { ticket, receiver }
    }
}

impl Drop for ImageLoadJob {
    fn drop(&mut self) {
        self.ticket.cancel();
    }
}

impl TextPreviewLoadJob {
    fn from_file(path: PathBuf, config: AppConfig) -> Self {
        let (ticket, receiver) =
            BackgroundJobs::global().submit(Duration::from_secs(30), move || {
                let failure_path = path.display().to_string();
                TextPreviewState::from_file(&path, &config)
                    .ok_or_else(|| format!("Could not read or prepare text file {failure_path}"))
            });
        Self { ticket, receiver }
    }

    fn from_text(content: String, title: Option<String>, config: AppConfig) -> Self {
        let (ticket, receiver) = BackgroundJobs::global()
            .submit(Duration::from_secs(30), move || {
                Ok(TextPreviewState::from_text(content, title, &config))
            });
        Self { ticket, receiver }
    }

    fn from_shared_text(content: Arc<String>, title: String, config: AppConfig) -> Self {
        let (ticket, receiver) =
            BackgroundJobs::global().submit(Duration::from_secs(30), move || {
                Ok(TextPreviewState::from_text(
                    content.as_str().to_owned(),
                    Some(title),
                    &config,
                ))
            });
        Self { ticket, receiver }
    }
}

impl Drop for TextPreviewLoadJob {
    fn drop(&mut self) {
        self.ticket.cancel();
    }
}

struct WorkbenchApp {
    state: SlotManager,
    state_rx: Receiver<SlotManager>,
    view: WorkbenchView,
    canvas_state: NodeCanvasState,
    recipes: Vec<Recipe>,
    search_query: String,
    palette_search_needs_focus: bool,
    selected_action_index: usize,
    selected_slots: Vec<SlotId>,
    inspector_text_preview_cache: Option<InspectorTextPreviewCache>,
    status_msg: Option<String>,
    appearance_sync: AppearanceSync,
    renderer: ScreenRenderer,
    ui_palette: AppPalette,
    mirror_texture: Option<egui::TextureHandle>,
    dirty_mirror: bool,
    in_app_prompt: Option<InAppPromptState>,
    viewer: Option<ViewerSession>,
    startup_viewers: VecDeque<ViewerSession>,
    startup_midi: Option<PathBuf>,
    image_load_job: Option<ImageLoadJob>,
    text_load_job: Option<TextPreviewLoadJob>,
    spreadsheet_load_job: Option<(
        JobTicket,
        SpreadsheetLoadReceiver,
        Option<(Arc<String>, String)>,
    )>,
    midi_load_job: Option<(JobTicket, MidiLoadReceiver)>,
    rolling_editor_job: Option<RollingEditorLoadJob>,
    rolling_audio_strip: Option<RollingAudioStripState>,
    dragging_slot: Option<SlotId>,
    config: AppConfig,
    appearance_applied: bool,
}

#[derive(Debug, Clone)]
struct InspectorTextPreviewCache {
    selected_slots: Vec<SlotId>,
    selected_item_ids: Vec<Option<uuid::Uuid>>,
    text_preview: Option<(SlotId, Option<PathBuf>, bool)>,
    spreadsheet_slot: Option<SlotId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkbenchEscapeRoute {
    Handled,
    StopAudio,
    ReturnToPalette,
    CloseWorkbench,
}

fn route_workbench_escape(
    view: &mut WorkbenchView,
    canvas_state: &mut NodeCanvasState,
    palette_search_needs_focus: &mut bool,
    audio_region_active: bool,
) -> WorkbenchEscapeRoute {
    if *view == WorkbenchView::NodeMachine
        && (canvas_state.catalog_open
            || canvas_state.save_dialog_open
            || canvas_state.dragging_from.is_some())
    {
        if canvas_state.catalog_open {
            canvas_state.catalog_open = false;
        } else if canvas_state.save_dialog_open {
            canvas_state.save_dialog_open = false;
        } else {
            canvas_state.dragging_from = None;
        }
        WorkbenchEscapeRoute::Handled
    } else if audio_region_active {
        WorkbenchEscapeRoute::StopAudio
    } else if *view == WorkbenchView::NodeMachine || *view == WorkbenchView::AutoActions {
        *view = WorkbenchView::Palette;
        *palette_search_needs_focus = true;
        WorkbenchEscapeRoute::ReturnToPalette
    } else {
        WorkbenchEscapeRoute::CloseWorkbench
    }
}

struct AutoActionsLayout {
    header: egui::Rect,
    subtitle: egui::Rect,
    card: egui::Rect,
    checkbox: egui::Rect,
    status: egui::Rect,
    explanation: egui::Rect,
    behavior: egui::Rect,
}

impl Default for AutoActionsLayout {
    fn default() -> Self {
        Self {
            header: egui::Rect::NOTHING,
            subtitle: egui::Rect::NOTHING,
            card: egui::Rect::NOTHING,
            checkbox: egui::Rect::NOTHING,
            status: egui::Rect::NOTHING,
            explanation: egui::Rect::NOTHING,
            behavior: egui::Rect::NOTHING,
        }
    }
}

/// Neutral secondary treatment for contextual previews, editors, and copy actions.
fn contextual_palette_action_button<'a>(
    icon: Option<Icon>,
    label: impl Into<String>,
    palette: &AppPalette,
) -> egui::Button<'a> {
    ui_ext::secondary_button_icon(
        icon,
        label,
        hex_to_c32(&palette.colors.surface_panel),
        palette,
    )
}

fn render_auto_actions(
    ui: &mut egui::Ui,
    config: &mut AppConfig,
    palette: &AppPalette,
) -> (Option<bool>, AutoActionsLayout) {
    let mut layout = AutoActionsLayout::default();
    let mut changed = None;
    ui.add_space(crate::ui_ext::SPACE_XS);
    ui.horizontal_wrapped(|ui| {
        Icon::Settings.ui(ui, 18.0, hex_to_c32(&palette.harmonies.base));
        layout.header = ui
            .heading(
                egui::RichText::new("Matched Auto-Actions")
                    .color(hex_to_c32(&palette.harmonies.base))
                    .strong(),
            )
            .rect;
        layout.subtitle = ui
            .label(
                egui::RichText::new("— Actions trigger automatically on clipboard match")
                    .color(hex_to_c32(&palette.colors.text_secondary)),
            )
            .rect;
    });
    ui.add_space(crate::ui_ext::SPACE_SM);

    egui::ScrollArea::vertical().show(ui, |ui| {
        let card_bg = hex_to_c32(&palette.colors.surface_recessed);
        let card_stroke = hex_to_c32(&palette.colors.surface_panel);
        let card = egui::Frame::none()
            .fill(card_bg)
            .stroke(egui::Stroke::new(1.0_f32, card_stroke))
            .rounding(egui::Rounding::same(8.0))
            .inner_margin(egui::Margin::symmetric(16.0, 14.0))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    let mut trim = config.auto_actions.trim_link_tracking;
                    let title_color = contrast_text_c32(
                        card_bg,
                        Some(hex_to_c32(&palette.colors.text_primary)),
                        palette,
                    );
                    let cb = ui.checkbox(
                        &mut trim,
                        egui::RichText::new("Clean Tracking from Links")
                            .strong()
                            .size(BODY_FONT_SIZE)
                            .color(title_color),
                    );
                    layout.checkbox = cb.rect;
                    if cb.changed() {
                        config.auto_actions.trim_link_tracking = trim;
                        changed = Some(trim);
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let (status_text, badge_bg, pref_fg) = if trim {
                            (
                                "ENABLED",
                                hex_to_c32(&palette.diff_add),
                                hex_to_c32(&palette.colors.text_primary),
                            )
                        } else {
                            (
                                "DISABLED",
                                hex_to_c32(&palette.colors.surface_panel),
                                hex_to_c32(&palette.colors.text_secondary),
                            )
                        };
                        let badge_fg = contrast_text_c32(badge_bg, Some(pref_fg), palette);
                        egui::Frame::none()
                            .fill(badge_bg)
                            .rounding(egui::Rounding::same(4.0))
                            .inner_margin(egui::Margin::symmetric(8.0, 3.0))
                            .show(ui, |ui| {
                                layout.status = ui
                                    .label(
                                        egui::RichText::new(status_text)
                                            .size(BODY_FONT_SIZE)
                                            .strong()
                                            .color(badge_fg),
                                    )
                                    .rect;
                            });
                    });
                });

                ui.add_space(crate::ui_ext::ITEM_SPACING_Y);
                layout.explanation = ui
                    .label(
                        egui::RichText::new(
                            "Automatically strips tracking queries (utm_source, utm_medium, fbclid, gclid, twclid, si, etc.) when any HTTP or HTTPS URL is copied to the clipboard.",
                        )
                        .color(hex_to_c32(&palette.colors.text_primary)),
                    )
                    .rect;
                ui.add_space(crate::ui_ext::SPACE_XS);
                layout.behavior = ui
                    .label(
                        egui::RichText::new(
                            "↳ Behavior: Cleaned link is placed directly into the active hand; original untracked URL is preserved in Undo [Z] for instant bypass.",
                        )
                        .size(BODY_FONT_SIZE)
                        .color(hex_to_c32(&palette.colors.text_secondary)),
                    )
                    .rect;
            });
        layout.card = card.response.rect;
        ui.add_space(14.0);
    });
    (changed, layout)
}

impl WorkbenchApp {
    fn set_status_message(&mut self, message: impl Into<String>) {
        self.status_msg = Some(message.into());
    }

    fn clear_status_message(&mut self) {
        self.status_msg = None;
    }

    fn new(
        state: SlotManager,
        start_with_diff: bool,
        start_with_binary_diff: bool,
        initial_audio: Option<PathBuf>,
        initial_image: Option<PathBuf>,
        initial_pdf: Option<PathBuf>,
        initial_midi: Option<PathBuf>,
        open_component_gallery: bool,
    ) -> Self {
        let mut config = AppConfig::load();
        // Desktop monitor mirror is always upright (no 180 flip)
        config.display.rotate_180 = false;
        let ui_palette = ui_appearance::palette(config.workbench.appearance);
        let renderer = ScreenRenderer::new(config.clone())
            .unwrap_or_else(|_| ScreenRenderer::fallback(config.clone()));

        let state_rx = state_sync::spawn_state_poller(state.clone());

        let canvas_state = NodeCanvasState::new();
        let recipes = load_all_recipes();

        let audio_preview = initial_audio.map(|p| {
            let title = p
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "Audio file".to_string());
            AudioPreviewState::new(p, title).with_editor(config.audio.editor.clone())
        });

        let rolling_audio_strip = Some(RollingAudioStripState::new());
        let mut startup_viewers = VecDeque::new();
        if let Some(audio) = audio_preview {
            startup_viewers.push_back(ViewerSession::Audio(audio));
        }
        if let Some(path) = initial_image {
            startup_viewers.push_back(ViewerSession::ImageLoading(ImageLoadJob::new(
                ImageLoadSource::File(path),
                config.clone(),
            )));
        }
        if let Some(path) = initial_pdf {
            startup_viewers.push_back(ViewerSession::Pdf(PdfEditorState::new(path)));
        }

        let mut app = Self {
            state,
            state_rx,
            view: WorkbenchView::Palette,
            canvas_state,
            recipes,
            search_query: String::new(),
            palette_search_needs_focus: true,
            selected_action_index: 0,
            selected_slots: vec![SlotId::LeftHand],
            inspector_text_preview_cache: None,
            status_msg: None,
            appearance_sync: AppearanceSync::default(),
            renderer,
            ui_palette,
            mirror_texture: None,
            dirty_mirror: true,
            in_app_prompt: None,
            viewer: None,
            startup_viewers,
            startup_midi: initial_midi,
            image_load_job: None,
            text_load_job: None,
            spreadsheet_load_job: None,
            midi_load_job: None,
            rolling_editor_job: None,
            rolling_audio_strip,
            dragging_slot: None,
            config,
            appearance_applied: false,
        };

        if open_component_gallery {
            app.viewer = Some(ViewerSession::ComponentGallery(
                component_gallery::ComponentGalleryState::default(),
            ));
        } else if start_with_binary_diff {
            app.open_binary_diff_viewer();
        } else if start_with_diff {
            app.open_diff_viewer();
        }

        if app.viewer.is_none() {
            app.advance_startup();
        }

        app
    }

    fn advance_startup(&mut self) {
        self.viewer = self.startup_viewers.pop_front();
        if self.viewer.is_none() {
            if let Some(path) = self.startup_midi.take() {
                self.start_midi_load(path);
            }
        }
    }

    fn toggle_appearance(&mut self, ctx: &egui::Context) {
        self.appearance_sync.clear();
        let appearance = self.config.workbench.appearance.toggled();
        self.config.workbench.appearance = appearance;
        self.ui_palette = ui_appearance::palette(appearance);
        ui_appearance::apply_visuals(ctx, &self.ui_palette);
        self.renderer.set_config(self.config.clone());
        self.dirty_mirror = true;
        match AppConfig::save_workbench_appearance_to_file(appearance) {
            Ok(()) => {
                self.appearance_sync.begin();
            }
            Err(err) => {
                self.appearance_sync
                    .set_error(format!("Could not save appearance: {err}"));
            }
        }
        ctx.request_repaint();
    }

    fn render_workbench_header(&mut self, ctx: &egui::Context) {
        // Top Header Panel: Mirror of Trofeo Screen + Tabs
        let header_bg = hex_to_c32(&self.ui_palette.colors.canvas);
        let brand_color = hex_to_c32(&self.ui_palette.left_hand_border);
        let sub_color = hex_to_c32(&self.ui_palette.colors.text_secondary);

        egui::TopBottomPanel::top("header_panel")
            .frame(
                egui::Frame::none()
                    .fill(header_bg)
                    .inner_margin(egui::Margin::symmetric(crate::ui_ext::PANEL_INSET, 10.0)),
            )
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.heading(
                        egui::RichText::new("LITTLE SCREEN CLIP")
                            .color(brand_color)
                            .strong(),
                    );
                    ui.label(
                        egui::RichText::new("— Ambient Dual-Hand Live Mirror").color(sub_color),
                    );

                    let appearance_label = match self.config.workbench.appearance {
                        WorkbenchAppearance::Dark => "Light mode [F4/F6]",
                        WorkbenchAppearance::Light => "Dark mode [F4/F6]",
                    };
                    if ui
                        .add(ui_ext::secondary_button(
                            appearance_label,
                            hex_to_c32(&self.ui_palette.colors.surface_panel),
                            &self.ui_palette,
                        ))
                        .on_hover_text(
                            "Switch the workbench and little screen to the other color mode",
                        )
                        .clicked()
                    {
                        self.toggle_appearance(ctx);
                    }
                    if self.spreadsheet_load_job.is_some() {
                        ui.spinner();
                        ui.label("Opening spreadsheet…");
                    }
                });

                // All workbench sections remain one click away on compact screens.
                ui.horizontal_wrapped(|ui| {
                    let is_auto = self.view == WorkbenchView::AutoActions;
                    let auto_btn = ui_ext::mode_tab_button_icon(
                        Some(Icon::Settings),
                        "Auto-Actions [F3]",
                        is_auto,
                        &self.ui_palette,
                    );
                    if ui.add(auto_btn).clicked() {
                        self.view = if is_auto {
                            WorkbenchView::Palette
                        } else {
                            WorkbenchView::AutoActions
                        };
                    }

                    let is_pal = self.view == WorkbenchView::Palette;
                    let pal_tab = ui_ext::mode_tab_button_icon(
                        Some(Icon::Search),
                        "Quick Palette [F2]",
                        is_pal,
                        &self.ui_palette,
                    );
                    if ui.add(pal_tab).clicked() {
                        self.view = WorkbenchView::Palette;
                    }

                    let is_node = self.view == WorkbenchView::NodeMachine;
                    let node_tab = ui_ext::mode_tab_button_icon(
                        Some(Icon::NodeTree),
                        "Big Node Machine [F1]",
                        is_node,
                        &self.ui_palette,
                    );
                    if ui.add(node_tab).clicked() {
                        self.view = WorkbenchView::NodeMachine;
                    }
                });

                ui.add_space(crate::ui_ext::SPACE_SM);

                // Re-render mirror if dirty
                if self.dirty_mirror || self.mirror_texture.is_none() {
                    self.update_mirror_texture(ctx);
                }

                let mirror_action = ScreenMirrorWidget::new(
                    self.mirror_texture.as_ref(),
                    self.config.slots.left_priors,
                    &mut self.dragging_slot,
                    &self.selected_slots,
                )
                // Node properties need a usable canvas alongside the always-visible mirror.
                // Keep the same fitted aspect/width logic, with a smaller height cap only here.
                .with_height_scale(
                    if self.view == WorkbenchView::NodeMachine
                        && ctx.screen_rect().height() <= 800.0
                    {
                        0.5
                    } else {
                        1.0
                    },
                )
                .show(ui, &self.ui_palette);

                self.handle_mirror_action(mirror_action);
            });
    }

    fn handle_mirror_action(&mut self, action: Option<MirrorAction>) {
        match action {
            Some(MirrorAction::Click(slot) | MirrorAction::DoubleClick(slot)) => {
                if slot != SlotId::Undo {
                    if let Some(index) = self
                        .selected_slots
                        .iter()
                        .position(|&selected| selected == slot)
                    {
                        if self.selected_slots.len() > 1 {
                            self.selected_slots.remove(index);
                        }
                    } else {
                        self.selected_slots.push(slot);
                    }
                    self.dirty_mirror = true;
                    self.selected_action_index = 0;
                }
            }
            Some(MirrorAction::Context(slot)) => {
                if let Some(item) = self.state.get(slot).cloned() {
                    self.open_slot_preview_modal(&item);
                }
            }
            Some(MirrorAction::DropSwap { from, to }) => {
                let _ = send_ipc_request(&IpcRequest::SwapSlots(from, to));
                self.state.swap_slots(from, to);
                self.dirty_mirror = true;
                self.status_msg = Some(format!("Swapped {} and {}", from.label(), to.label()));
            }
            None => {}
        }
    }

    fn open_diff_viewer(&mut self) {
        let (lhs_target, rhs_target) = if self.selected_slots.len() >= 2 {
            let items: Vec<&ClipItem> = self
                .selected_slots
                .iter()
                .filter_map(|s| self.state.get(*s))
                .collect();
            let img_items: Vec<&ClipItem> = items
                .iter()
                .filter(|i| {
                    resolve_diff_target(i)
                        .map(|t| t.is_image())
                        .unwrap_or(false)
                })
                .copied()
                .collect();
            if img_items.len() >= 2 {
                (
                    resolve_diff_target(img_items[0]),
                    resolve_diff_target(img_items[1]),
                )
            } else if let Some((t1, t2)) = items
                .iter()
                .find_map(|i| resolve_diff_pair(i).filter(|(a, b)| a.is_image() && b.is_image()))
            {
                (Some(t1), Some(t2))
            } else if items.len() >= 2 {
                (resolve_diff_target(items[0]), resolve_diff_target(items[1]))
            } else if items.len() == 1 {
                if let Some((t1, t2)) = resolve_diff_pair(items[0]) {
                    (Some(t1), Some(t2))
                } else {
                    (None, None)
                }
            } else {
                (None, None)
            }
        } else if self.selected_slots.len() == 1 {
            let selected_item = self.state.get(self.selected_slots[0]);
            if let Some(item) = selected_item {
                if let Some((t1, t2)) = resolve_diff_pair(item) {
                    (Some(t1), Some(t2))
                } else {
                    let other = if self.selected_slots[0] == SlotId::LeftHand {
                        self.state
                            .right_hand()
                            .or_else(|| self.state.get(SlotId::LeftPrior1))
                            .or_else(|| self.state.get(SlotId::Undo))
                    } else {
                        self.state.left_hand()
                    };
                    (
                        resolve_diff_target(item),
                        other.and_then(resolve_diff_target),
                    )
                }
            } else {
                (None, None)
            }
        } else {
            let left = self.state.left_hand();
            let right = self.state.right_hand();
            if let (Some(l), Some(r)) = (left, right) {
                if let Some((t1, t2)) =
                    resolve_diff_pair(l).filter(|(a, b)| a.is_image() && b.is_image())
                {
                    if !resolve_diff_target(r)
                        .map(|t| t.is_image())
                        .unwrap_or(false)
                    {
                        (Some(t1), Some(t2))
                    } else {
                        (resolve_diff_target(l), resolve_diff_target(r))
                    }
                } else {
                    (resolve_diff_target(l), resolve_diff_target(r))
                }
            } else if let Some(l) = left {
                if let Some((t1, t2)) = resolve_diff_pair(l) {
                    (Some(t1), Some(t2))
                } else {
                    let prior = self
                        .state
                        .get(SlotId::LeftPrior1)
                        .or_else(|| self.state.get(SlotId::Undo))
                        .or_else(|| self.state.get(SlotId::LeftPrior2));
                    (resolve_diff_target(l), prior.and_then(resolve_diff_target))
                }
            } else {
                (None, None)
            }
        };

        match (lhs_target, rhs_target) {
            (Some(lhs), Some(rhs)) => {
                if lhs.is_image() && !rhs.is_image() {
                    self.status_msg = Some(
                        "Comparison target is not an image (both targets must be images or text)"
                            .to_string(),
                    );
                } else if !lhs.is_image() && rhs.is_image() {
                    self.status_msg = Some(
                        "Left target is not an image (both targets must be images or text)"
                            .to_string(),
                    );
                } else {
                    self.replace_viewer(Some(ViewerSession::Diff(DiffViewerState::new(
                        lhs,
                        rhs,
                        DiffDisplayMode::SideBySide,
                    ))));
                }
            }
            (Some(_), None) => {
                self.status_msg = Some(
                    "Diff requires two items. Copy a second item to Right Hand or select 2 slots."
                        .to_string(),
                );
            }
            (None, _) => {
                self.status_msg = Some(
                    "No item selected or slot is empty. Push or copy content first.".to_string(),
                );
            }
        }
    }

    fn open_binary_diff_viewer(&mut self) {
        let selected_items = self.get_selected_items();
        if let Some(((l_lbl, l_bytes), (r_lbl, r_bytes))) =
            resolve_binary_diff_pair(&selected_items, &self.state)
        {
            self.replace_viewer(Some(ViewerSession::BinaryDiff(BinaryDiffViewState::new(
                l_lbl, l_bytes, r_lbl, r_bytes,
            ))));
        } else {
            self.status_msg =
                Some("Binary diff requires 2 slots or 1 slot with 2 files/paths.".to_string());
        }
    }

    fn get_selected_items(&self) -> Vec<&ClipItem> {
        self.selected_slots
            .iter()
            .filter_map(|slot| self.state.get(*slot))
            .collect()
    }

    fn filtered_actions(&self) -> Vec<PaletteAction> {
        let q = self.search_query.to_lowercase();
        let mut actions = Vec::new();

        // 1. Built-in ops
        let items = self.get_selected_items();
        let ops = get_applicable_ops(&items);
        for op in ops {
            if q.is_empty()
                || op.name.to_lowercase().contains(&q)
                || op.label.to_lowercase().contains(&q)
                || op.description.to_lowercase().contains(&q)
                || op.category.to_lowercase().contains(&q)
            {
                actions.push(PaletteAction::Op(op));
            }
        }

        // 2. Discovered Recipes
        for r in &self.recipes {
            if !r.is_applicable(&items) {
                continue;
            }
            if q.is_empty()
                || r.name.to_lowercase().contains(&q)
                || r.description.to_lowercase().contains(&q)
                || r.tags.iter().any(|t| t.to_lowercase().contains(&q))
                || q.starts_with('/')
            {
                actions.push(PaletteAction::Recipe(r.clone()));
            }
        }

        actions
    }

    fn execute_current_action(&mut self, ctx: &egui::Context) {
        self.execute_current_action_with(ctx, |request| {
            send_ipc_request(request).map_err(|error| error.to_string())
        });
    }

    fn execute_current_action_with(
        &mut self,
        ctx: &egui::Context,
        mut send: impl FnMut(&IpcRequest) -> Result<IpcResponse, String>,
    ) {
        let actions = self.filtered_actions();
        if let Some(act) = actions.get(self.selected_action_index) {
            match act {
                PaletteAction::Op(op) => {
                    if op.name == "diff" {
                        self.open_diff_viewer();
                        return;
                    }
                    if op.name == "binary-diff" {
                        self.open_binary_diff_viewer();
                        return;
                    }

                    let req = IpcRequest::ExecuteOp {
                        op_name: op.name.to_string(),
                        secondary_slot: None,
                        selected_slots: Some(self.selected_slots.clone()),
                        separator: None,
                    };
                    self.execute_action_request_with(ctx, &req, true, &mut send);
                }
                PaletteAction::Recipe(r) => {
                    if let Some((msg, def)) = recipe_needs_prompt(r, &self.state) {
                        self.in_app_prompt = Some(InAppPromptState {
                            recipe_name: r.name.clone(),
                            message: msg,
                            input_text: def,
                            first_frame: true,
                        });
                    } else {
                        let req = IpcRequest::ExecuteRecipe {
                            recipe_name: r.name.clone(),
                        };
                        self.execute_action_request_with(ctx, &req, true, &mut send);
                    }
                }
            }
        }
    }

    fn execute_action_request_with(
        &mut self,
        ctx: &egui::Context,
        request: &IpcRequest,
        close_on_success: bool,
        send: &mut impl FnMut(&IpcRequest) -> Result<IpcResponse, String>,
    ) -> bool {
        match send(request) {
            Ok(IpcResponse::OpResult { .. } | IpcResponse::Success { .. }) => {
                if close_on_success {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                true
            }
            Ok(IpcResponse::Error { error }) => {
                self.set_status_message(format!("Error: {error}"));
                false
            }
            Ok(_) => {
                self.set_status_message("Error: unexpected response from daemon");
                false
            }
            Err(error) => {
                self.set_status_message(format!("Error: {error}"));
                false
            }
        }
    }

    fn execute_auto_action_change_with(
        &mut self,
        ctx: &egui::Context,
        previous_enabled: bool,
        enabled: bool,
        send: &mut impl FnMut(&IpcRequest) -> Result<IpcResponse, String>,
    ) {
        let request = IpcRequest::SetAutoAction {
            key: "trim_link_tracking".to_owned(),
            enabled,
        };
        if !self.execute_action_request_with(ctx, &request, false, send) {
            self.config.auto_actions.trim_link_tracking = previous_enabled;
        }
    }

    fn update_mirror_texture(&mut self, ctx: &egui::Context) {
        let mut render_state = self.state.clone();
        render_state.selected = self.selected_slots.clone();

        if let Ok(pixmap) = self.renderer.render_pixmap(&render_state) {
            let pixels: Vec<egui::Color32> = pixmap
                .data()
                .chunks_exact(4)
                .map(|p| egui::Color32::from_rgba_premultiplied(p[0], p[1], p[2], p[3]))
                .collect();

            let color_image = egui::ColorImage {
                size: [SCREEN_WIDTH as usize, SCREEN_HEIGHT as usize],
                pixels,
            };

            self.mirror_texture =
                Some(ctx.load_texture("ambient-mirror", color_image, egui::TextureOptions::LINEAR));
            self.dirty_mirror = false;
        }
    }
}

fn audio_strip_space_pressed(ctx: &egui::Context, audio_region_active: bool) -> bool {
    audio_region_active
        && !ctx.wants_keyboard_input()
        && ctx.input(|i| i.key_pressed(egui::Key::Space))
}

fn recipe_needs_prompt(recipe: &Recipe, state: &SlotManager) -> Option<(String, String)> {
    for node in &recipe.graph.nodes {
        if let NodeKind::PromptOrHandText = node.kind {
            let hand_prop = node
                .props
                .get("hand")
                .map(|s| s.as_str())
                .unwrap_or("right");
            let item = if hand_prop.eq_ignore_ascii_case("left") {
                state.left_hand()
            } else {
                state.right_hand()
            };
            let has_text = item.map_or(false, |it| match &it.payload {
                little_clip_core::ClipPayload::Text(t)
                | little_clip_core::ClipPayload::Markdown(t) => !t.trim().is_empty(),
                little_clip_core::ClipPayload::Url(_) => true,
                _ => false,
            });
            if !has_text {
                let msg = node
                    .props
                    .get("prompt_message")
                    .cloned()
                    .unwrap_or_else(|| format!("Enter text for {}:", recipe.name));
                let def = node.props.get("default_text").cloned().unwrap_or_default();
                return Some((msg, def));
            }
        }
    }
    None
}

impl Drop for WorkbenchApp {
    fn drop(&mut self) {
        if let Some(job) = &self.text_load_job {
            job.ticket.cancel();
        }
        if let Some((ticket, _, _)) = &self.spreadsheet_load_job {
            ticket.cancel();
        }
        if let Some((ticket, _)) = &self.midi_load_job {
            ticket.cancel();
        }
    }
}

impl eframe::App for WorkbenchApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Apply the fixed appearance to egui on the first frame or mode changes.
        if !self.appearance_applied {
            ui_appearance::apply_visuals(ctx, &self.ui_palette);
            self.appearance_applied = true;
        }

        if ctx.input(|input| input.key_pressed(egui::Key::F4) || input.key_pressed(egui::Key::F6)) {
            self.toggle_appearance(ctx);
        }

        // Live state update from background daemon poller
        let mut state_changed = false;
        while let Ok(new_state) = self.state_rx.try_recv() {
            if new_state != self.state {
                self.state = new_state;
                state_changed = true;
            }
        }
        if state_changed {
            self.dirty_mirror = true;
            ctx.request_repaint();
        }

        self.appearance_sync.poll(ctx);
        self.appearance_sync.render(ctx, &self.ui_palette);

        self.poll_rolling_editor_load(ctx);
        self.render_rolling_editor_status(ctx);
        self.poll_image_load(ctx);
        self.poll_text_load(ctx);
        if self.render_modal_viewers(ctx) {
            self.palette_search_needs_focus = true;
            return;
        }

        // Hold table-load jobs/results while another viewer is active. The receiver
        // stays available, but polling waits until the foreground viewer closes.
        self.poll_spreadsheet_load(ctx);
        if self.render_modal_viewers(ctx) {
            self.palette_search_needs_focus = true;
            return;
        }
        self.poll_midi_load(ctx);
        if self.render_modal_viewers(ctx) {
            self.palette_search_needs_focus = true;
            return;
        }

        if self.state.has_active_animation() {
            self.dirty_mirror = true;
            ctx.request_repaint_after(Duration::from_millis(80));
        } else {
            ctx.request_repaint_after(Duration::from_millis(120));
        }

        self.handle_global_shortcuts(ctx);

        self.render_workbench_header(ctx);

        let mut host_action = None;

        if let Some(strip) = &mut self.rolling_audio_strip {
            egui::TopBottomPanel::bottom("rolling_audio_strip_panel")
                .frame(
                    egui::Frame::none()
                        .fill(crate::hex_to_c32(&self.ui_palette.colors.canvas))
                        .inner_margin(egui::Margin::symmetric(
                            crate::ui_ext::PANEL_INSET,
                            crate::ui_ext::FOOTER_INSET_Y,
                        ))
                        .stroke(egui::Stroke::new(
                            1.0_f32,
                            crate::hex_to_c32(&self.ui_palette.colors.surface_panel),
                        )),
                )
                .show(ctx, |ui| {
                    strip.render_ui(ui, ctx, &self.ui_palette, &mut host_action);
                });
        }

        if let Some(host_action) = host_action {
            self.apply_rolling_audio_action(host_action, ctx);
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.visuals_mut().override_text_color = Some(hex_to_c32(&self.ui_palette.colors.text_primary));

            show_workbench_footer(
                ui,
                self.view,
                self.status_msg.as_deref(),
                &self.ui_palette,
            );

            if self.view != WorkbenchView::Palette {
                self.palette_search_needs_focus = true;
            }

            match self.view {
                WorkbenchView::Palette => {
                    let filtered = self.filtered_actions();
                    let text_preview_action = self.check_inspector_text_preview();

                    // Selected slots banner & shortcuts
                    let slot_labels: Vec<&str> = self.selected_slots.iter().map(|s| s.label()).collect();
                    let first_audio_item = self.selected_slots.iter().find_map(|s| {
                        self.state.get(*s).and_then(|it| (!is_large_text_payload(it)).then(|| it.audio_path()).flatten().map(|p| (p, it.title.clone())))
                    });
                    let first_image_slot = self.selected_slots.iter().copied().find(|slot| {
                        self.state
                            .get(*slot)
                            .is_some_and(is_image_preview_action_item)
                    });

                    let mut open_diff_requested = false;
                    let mut open_spreadsheet_requested = None;
                    let mut open_midi_requested = None;

                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            egui::RichText::new(format!("Selected ({}): {}", self.selected_slots.len(), slot_labels.join(" + ")))
                                .color(hex_to_c32(&self.ui_palette.harmonies.base))
                                .strong(),
                        );
                        ui.label(
                                egui::RichText::new(" • [Space] Hands  |  [1] L+L1  |  [2] L+L2  |  [3] 3 Left  |  [0] L only")
                                    .color(hex_to_c32(&self.ui_palette.colors.text_muted))
                                    .size(BODY_FONT_SIZE),
                        );

                        if let Some((path, title)) = first_audio_item {
                            ui.add_space(crate::ui_ext::SPACE_SM);
                            let audio_btn = contextual_palette_action_button(
                                Some(Icon::Audio),
                                "Preview & Slice Audio",
                                &self.ui_palette,
                            );
                            if ui.add(audio_btn).clicked() {
                                self.replace_viewer(Some(ViewerSession::Audio(AudioPreviewState::new(path, title).with_editor(self.config.audio.editor.clone()))));
                            }
                        }

                        if let Some(slot) = first_image_slot {
                            ui.add_space(crate::ui_ext::SPACE_SM);
                            let img_btn = contextual_palette_action_button(
                                Some(Icon::Image),
                                "Preview & Edit Image",
                                &self.ui_palette,
                            );
                            if ui.add(img_btn).clicked() {
                                let source = ImageLoadSource::from_item(self.state.get(slot).unwrap());
                                self.start_image_item_load(source);
                            }
                        }

                        let has_two_images = if self.selected_slots.len() >= 2 {
                            let img_count = self
                                .selected_slots
                                .iter()
                                .filter_map(|s| self.state.get(*s))
                                .filter(|it| !is_large_text_payload(it))
                                .filter(|it| resolve_diff_target(it).map(|t| t.is_image()).unwrap_or(false))
                                .count();
                            img_count >= 2
                                || self.selected_slots.iter().any(|s| {
                                    self.state
                                        .get(*s)
                                        .filter(|it| !is_large_text_payload(it))
                                        .and_then(|it| resolve_diff_pair(it))
                                        .map(|(t1, t2)| t1.is_image() && t2.is_image())
                                        .unwrap_or(false)
                                  })
                        } else if self.selected_slots.len() == 1 {
                            self.state
                                .get(self.selected_slots[0])
                                .filter(|it| !is_large_text_payload(it))
                                .and_then(|it| resolve_diff_pair(it))
                                .map(|(t1, t2)| t1.is_image() && t2.is_image())
                                .unwrap_or(false)
                        } else {
                            false
                        };
                        if has_two_images {
                            ui.add_space(crate::ui_ext::SPACE_SM);
                            let diff_btn = contextual_palette_action_button(
                                Some(Icon::Diff),
                                "Compare Images",
                                &self.ui_palette,
                            );
                            if ui.add(diff_btn).on_hover_text("Open interactive visual diff comparing the 2 images").clicked() {
                                open_diff_requested = true;
                            }
                        }

                    let first_pdf_item = self.selected_slots.iter().find_map(|s| {
                            self.state
                                .get(*s)
                                .filter(|item| is_pdf_clip_item(item))
                                .and_then(bounded_item_file_path)
                        });
                        let first_spreadsheet_slot = self
                            .inspector_text_preview_cache
                            .as_ref()
                            .and_then(|cache| cache.spreadsheet_slot);
                        let first_midi_item = self.selected_slots.iter().find_map(|s| {
                            let it = self.state.get(*s)?;
                            if is_large_text_payload(it) {
                                return None;
                            }
                            if let Some(p) = bounded_item_file_path(it).filter(|p| {
                                p.extension().and_then(|e| e.to_str()).map(|ext| ext.eq_ignore_ascii_case("mid") || ext.eq_ignore_ascii_case("midi")).unwrap_or(false)
                            }) {
                                Some(p)
                            } else if let Some(txt) = it.to_clipboard_text() {
                                let p = std::path::Path::new(txt.trim());
                                if p.is_file() && p.extension().and_then(|e| e.to_str()).map(|ext| ext.eq_ignore_ascii_case("mid") || ext.eq_ignore_ascii_case("midi")).unwrap_or(false) {
                                    Some(p.to_path_buf())
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        });
                        let first_3d_model_item = self.selected_slots.iter().find_map(|s| {
                            let it = self.state.get(*s)?;
                            if is_large_text_payload(it) {
                                return None;
                            }
                            if let Some(p) = bounded_item_file_path(it).filter(|p| is_3d_model_file(p)) {
                                Some(p)
                            } else if let Some(txt) = it.to_clipboard_text() {
                                let p = std::path::Path::new(txt.trim());
                                if p.is_file() && is_3d_model_file(p) {
                                    Some(p.to_path_buf())
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        });

                        if let Some(pdf_path) = first_pdf_item {
                            ui.add_space(crate::ui_ext::SPACE_SM);
                            let pdf_btn = contextual_palette_action_button(
                                Some(Icon::Pdf),
                                "Edit PDF",
                                &self.ui_palette,
                            );
                            if ui.add(pdf_btn).on_hover_text("Open PDF Page Editor (Delete / Rotate pages)").clicked() {
                                self.replace_viewer(Some(ViewerSession::Pdf(PdfEditorState::new(pdf_path))));
                            }
                        }

                        if let Some(slot) = first_spreadsheet_slot {
                            ui.add_space(crate::ui_ext::SPACE_SM);
                            let sheet_btn = contextual_palette_action_button(
                                Some(Icon::Spreadsheet),
                                if self.spreadsheet_load_job.is_some() { "Opening Spreadsheet…" } else { "View Spreadsheet" },
                                &self.ui_palette,
                            );
                            let sheet_response = ui_ext::on_hover_text_with_disabled_reason(
                                ui.add_enabled(self.spreadsheet_load_job.is_none(), sheet_btn),
                                "Open full multi-sheet X/Y scrollable table",
                                "Wait for the current spreadsheet load to finish.",
                            );
                            if sheet_response.clicked() {
                                open_spreadsheet_requested = Some(slot);
                            }
                        }

                        if let Some(midi_path) = first_midi_item {
                            ui.add_space(crate::ui_ext::SPACE_SM);
                            let midi_btn = contextual_palette_action_button(
                                Some(Icon::Midi),
                                if self.midi_load_job.is_some() { "Opening MIDI…" } else { "MIDI Piano Roll" },
                                &self.ui_palette,
                            );
                            let midi_response = ui_ext::on_hover_text_with_disabled_reason(
                                ui.add_enabled(self.midi_load_job.is_none(), midi_btn),
                                "Open interactive MIDI piano roll visualizer & synth audio",
                                "Wait for the current MIDI load to finish.",
                            );
                            if midi_response.clicked() {
                                open_midi_requested = Some(midi_path);
                            }
                        }

                        if let Some(model_path) = first_3d_model_item {
                            ui.add_space(crate::ui_ext::SPACE_SM);
                            let ext = model_path.extension().and_then(|s| s.to_str()).unwrap_or("3D").to_ascii_uppercase();
                            let btn_text = format!("3D {} Model", ext);
                            let model_btn = contextual_palette_action_button(
                                Some(Icon::Model3d),
                                &btn_text,
                                &self.ui_palette,
                            );
                            if ui.add(model_btn).on_hover_text("Open interactive 3D orbit viewer (pan, zoom, rotate)").clicked() {
                                self.replace_viewer(Some(ViewerSession::Model3d(
                                    Model3dViewerState::from_file(model_path)
                                        .with_config(self.config.model3d.clone()),
                                )));
                            }
                        }

                        // Smart text previews: Unit conversions, Network (IP/MAC), and Timestamps
                        let first_text_item = self.selected_slots.iter().find_map(|s| {
                            let item = self.state.get(*s)?;
                            match &item.payload {
                                ClipPayload::Text(text) | ClipPayload::Markdown(text) => {
                                    Some(std::borrow::Cow::Borrowed(text.as_str()))
                                }
                                _ => item.to_clipboard_text().map(std::borrow::Cow::Owned),
                            }
                        });
                        if let Some(ref text) = first_text_item.filter(|text| text.len() <= MAX_INLINE_TEXT_ROUTING_BYTES) {
                            let trimmed = text.trim();
                            if let Some(conv) = detect_unit_conversion(trimmed) {
                                ui.add_space(crate::ui_ext::SPACE_SM);
                                let label = conv.summary_line();
                                let btn = contextual_palette_action_button(
                                    Some(Icon::Swap),
                                    &label,
                                    &self.ui_palette,
                                );
                                if ui.add(btn).on_hover_text("Click to copy converted unit").clicked() {
                                    ui.output_mut(|o| o.copied_text = conv.primary_conversion.clone());
                                    self.status_msg = Some(format!("Copied: {}", conv.primary_conversion));
                                }
                            } else if let Some(net) = detect_network_preview(trimmed) {
                                ui.add_space(crate::ui_ext::SPACE_SM);
                                let (label, net_icon) = match &net {
                                    NetworkPreview::Ip { ip, rfc_class, .. } => (format!("{} ({})", ip, rfc_class), Icon::Link),
                                    NetworkPreview::Mac { formatted, vendor, .. } => (format!("{} ({})", formatted, vendor), Icon::Zap),
                                };
                                let btn = contextual_palette_action_button(
                                    Some(net_icon),
                                    &label,
                                    &self.ui_palette,
                                );
                                if ui.add(btn).on_hover_text("Network address info (Click to copy summary)").clicked() {
                                    let lines = net.summary_lines().join("\n");
                                    ui.output_mut(|o| o.copied_text = lines.clone());
                                    self.status_msg = Some("Copied network info".to_string());
                                }
                            } else if let Some(ts) = detect_timestamp(trimmed) {
                                ui.add_space(crate::ui_ext::SPACE_SM);
                                let label = format!("{} ({})", ts.human_readable, ts.relative);
                                let btn = contextual_palette_action_button(
                                    Some(Icon::Calendar),
                                    &label,
                                    &self.ui_palette,
                                );
                                if ui.add(btn).on_hover_text(format!("UTC: {}\nEpoch: {}\nClick to copy ISO 8601", ts.utc_iso, ts.epoch_sec)).clicked() {
                                    ui.output_mut(|o| o.copied_text = ts.utc_iso.clone());
                                    self.status_msg = Some(format!("Copied ISO 8601: {}", ts.utc_iso));
                                }
                            }
                        }

                        if let Some((slot, file_p, is_md)) = text_preview_action.clone() {
                            ui.add_space(crate::ui_ext::SPACE_SM);
                            let (label, txt_icon) = if is_md {
                                ("Preview Markdown", Icon::Book)
                            } else {
                                ("View Text / Code", Icon::Text)
                            };
                            let txt_btn = contextual_palette_action_button(
                                Some(txt_icon),
                                label,
                                &self.ui_palette,
                            );
                            if ui.add(txt_btn).on_hover_text("Open text or Markdown preview (syntax highlighting, line wrapping, search, and editor export)").clicked() {
                                if let Some(p) = file_p {
                                    self.start_text_file_load(p);
                                } else if let Some(txt) = self.state.get(slot).and_then(ClipItem::to_clipboard_text) {
                                    self.start_text_preview_load(txt, None);
                                }
                            }
                        }

                    });

                    if open_diff_requested {
                        self.open_diff_viewer();
                    }
                    if let Some(slot) = open_spreadsheet_requested {
                        self.open_spreadsheet_item(slot);
                    }
                    if let Some(path) = open_midi_requested {
                        self.start_midi_load(path);
                    }

                    ui.add_space(crate::ui_ext::ITEM_SPACING_Y);

                    // Focus on entry, then leave keyboard focus under the user's control.
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("Action > ")
                                .color(hex_to_c32(&self.ui_palette.harmonies.base))
                                .strong(),
                        );
                        let audio_region_active = self.is_audio_region_active();
                        let mut text_edit = egui::TextEdit::singleline(&mut self.search_query)
                            .hint_text("Type action or /recipe to execute...");
                        if audio_region_active {
                            text_edit = text_edit.interactive(false);
                        }
                        let response = ui.add_sized(
                            [ui.available_width() - 20.0, ui_ext::CONTROL_HEIGHT],
                            text_edit,
                        );
                        let activate_from_audio = response.clicked()
                            || (response.hovered()
                                && ctx.input(|i| i.pointer.primary_clicked()));
                        if audio_region_active && activate_from_audio {
                                if let Some(strip) = &mut self.rolling_audio_strip {
                                    strip.selected_region = None;
                                    strip.stop();
                                }
                        }
                        update_palette_search_focus(
                            ctx,
                            &response,
                            &mut self.palette_search_needs_focus,
                            audio_region_active,
                            activate_from_audio,
                        );
                    });

                    ui.add_space(crate::ui_ext::SPACE_SM);

                    // Filtered Action List filling available vertical height
                    let avail_h = ui.available_height().max(0.0);
                    egui::ScrollArea::vertical().max_height(avail_h).show(ui, |ui| {
                        if filtered.is_empty() {
                            ui.add_space(10.0);
                            ui.label(
                                egui::RichText::new("No matching actions or recipes found.")
                                    .color(hex_to_c32(&self.ui_palette.colors.text_muted))
                                    .italics(),
                            );
                        } else {
                            for (idx, act) in filtered.iter().enumerate() {
                                let is_selected = idx == self.selected_action_index;
                                let bg = if is_selected {
                                    hex_to_c32(&self.ui_palette.colors.surface_panel)
                                } else {
                                    hex_to_c32(&self.ui_palette.colors.surface_recessed)
                                };

                                let frame = egui::Frame::none()
                                    .fill(bg)
                                    .inner_margin(egui::Margin::symmetric(
                                        crate::ui_ext::BUTTON_PADDING_X,
                                        crate::ui_ext::BUTTON_PADDING_Y,
                                    ))
                                    .rounding(crate::ui_ext::CONTROL_CORNER_RADIUS);

                                frame.show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        let preferred_fg = if is_selected {
                                            hex_to_c32(&self.ui_palette.colors.text_primary)
                                        } else {
                                            hex_to_c32(&self.ui_palette.colors.text_secondary)
                                        };
                                        let label_color = contrast_text_c32(bg, Some(preferred_fg), &self.ui_palette);

                                        ui.label(egui::RichText::new(act.label()).color(label_color).strong());
                                        let cat_color = if act.category() == "Recipe" {
                                            hex_to_c32(&self.ui_palette.harmonies.triadic_1)
                                        } else {
                                            hex_to_c32(&self.ui_palette.colors.text_secondary)
                                        };
                                        ui.label(
                                            egui::RichText::new(format!("[{}]", act.category()))
                                                .color(cat_color)
                                                .size(BODY_FONT_SIZE)
                                                .strong(),
                                        );
                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            ui.label(
                                                egui::RichText::new(act.description())
                                                    .color(hex_to_c32(&self.ui_palette.colors.text_muted))
                                                    .size(BODY_FONT_SIZE),
                                            );
                                        });
                                    });
                                });
                                ui.add_space(2.0);
                            }
                        }
                    });

                }
                WorkbenchView::NodeMachine => {
                    let action = self.canvas_state.ui(ui, &self.ui_palette);
                    if let Some(CanvasAction::RunPipeline { yaml }) = action {
                        match send_ipc_request(&IpcRequest::ExecuteGraphYaml { yaml }) {
                            Ok(response) => self.canvas_state.apply_pipeline_response(response),
                            Err(error) => self.canvas_state.apply_pipeline_response(
                                IpcResponse::Error {
                                    error: error.to_string(),
                                },
                            ),
                        }
                        self.dirty_mirror = true;
                        ctx.request_repaint();
                    }
                }
                WorkbenchView::AutoActions => {
                    let previous_enabled = self.config.auto_actions.trim_link_tracking;
                    if let Some(enabled) = render_auto_actions(ui, &mut self.config, &self.ui_palette).0 {
                        self.execute_auto_action_change_with(
                            ctx,
                            previous_enabled,
                            enabled,
                            &mut |request| {
                                send_ipc_request(request).map_err(|error| error.to_string())
                            },
                        );
                    }
                }
            }
        });
    }
}

impl WorkbenchApp {
    pub fn is_audio_region_active(&self) -> bool {
        self.rolling_audio_strip
            .as_ref()
            .map_or(false, |s| s.selected_region.is_some())
    }

    fn render_modal_viewers(&mut self, ctx: &egui::Context) -> bool {
        if self.render_prompt_modal(ctx) {
            return true;
        }

        let Some(viewer) = self.viewer.as_mut() else {
            if self.image_load_job.is_some() || self.text_load_job.is_some() {
                let mut cancel = false;
                show_viewer_panel(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(if self.image_load_job.is_some() {
                            "Loading image…"
                        } else {
                            "Loading text…"
                        });
                        cancel = ui.button("Cancel [Esc]").clicked();
                    });
                });
                if cancel || ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
                    self.image_load_job = None;
                    self.text_load_job = None;
                }
                ctx.request_repaint_after(Duration::from_millis(40));
                return true;
            }
            return false;
        };
        let mut close = false;
        let mut pdf_dirty = false;
        match viewer {
            ViewerSession::Diff(state) => {
                show_viewer_panel(ctx, |ui| {
                    state.render_ui(ui, ctx, &self.ui_palette, &mut close);
                });
            }
            ViewerSession::BinaryDiff(state) => {
                show_viewer_panel(ctx, |ui| {
                    state.render_ui(ui, ctx, &self.ui_palette, &mut close);
                });
            }
            ViewerSession::Audio(state) => {
                show_viewer_panel(ctx, |ui| {
                    state.render_ui(ui, ctx, &self.ui_palette, &mut close);
                });
                if close {
                    state.stop();
                }
            }
            ViewerSession::ImageLoading(_) => {
                show_viewer_panel(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Loading image…");
                        close = ui.button("Cancel [Esc]").clicked();
                    });
                });
                close |= ctx.input(|input| input.key_pressed(egui::Key::Escape));
                ctx.request_repaint_after(Duration::from_millis(40));
            }
            ViewerSession::Image(state) => {
                show_viewer_panel(ctx, |ui| {
                    state.render_ui(ui, ctx, &self.ui_palette, &mut close);
                });
            }
            ViewerSession::Spreadsheet(state) => {
                show_viewer_panel(ctx, |ui| {
                    state.render_ui(ui, ctx, &self.ui_palette, &mut close);
                });
            }
            ViewerSession::Pdf(state) => {
                show_viewer_panel(ctx, |ui| {
                    state.render_ui(ui, ctx, &self.ui_palette, &mut close);
                });
                pdf_dirty = close && state.dirty;
            }
            ViewerSession::Midi(state) => {
                if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
                    if state.selected_region.is_some() {
                        state.selected_region = None;
                        state.play_stop_time = None;
                    } else {
                        close = true;
                    }
                } else {
                    show_viewer_panel(ctx, |ui| {
                        state.render_ui(ui, &self.ui_palette, &mut close);
                    });
                }
                if close {
                    state.stop();
                }
            }
            ViewerSession::Model3d(state) => {
                show_viewer_panel(ctx, |ui| {
                    state.render_ui(ui, &self.ui_palette, &mut close);
                });
            }
            ViewerSession::Text(state) => {
                show_viewer_panel(ctx, |ui| {
                    state.render_ui(ui, ctx, &self.ui_palette, &mut close);
                });
            }
            ViewerSession::ComponentGallery(state) => {
                if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
                    close = true;
                } else {
                    show_viewer_panel(ctx, |ui| {
                        state.render_ui(ui, &self.ui_palette, &mut close);
                    });
                }
            }
        }

        let image_loading = self.image_load_job.is_some();
        let text_loading = self.text_load_job.is_some();
        if image_loading
            || text_loading
            || self.status_msg.as_deref().is_some_and(|message| {
                message.starts_with("Image preview failed:")
                    || message.starts_with("Text preview failed:")
            })
        {
            egui::Area::new(egui::Id::new("image_load_status"))
                .order(egui::Order::Foreground)
                .anchor(egui::Align2::RIGHT_TOP, [-12.0, 12.0])
                .show(ctx, |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            if image_loading || text_loading {
                                ui.spinner();
                                ui.label(if image_loading {
                                    "Loading image…"
                                } else {
                                    "Loading text…"
                                });
                                if ui.button("Cancel").clicked() {
                                    self.image_load_job = None;
                                    self.text_load_job = None;
                                }
                            } else if let Some(message) = &self.status_msg {
                                ui.label(message);
                                if ui.button("Dismiss").clicked() {
                                    self.status_msg = None;
                                }
                            }
                        });
                    });
                });
            if image_loading || text_loading {
                ctx.request_repaint_after(Duration::from_millis(40));
            }
        }
        if close {
            self.image_load_job = None;
            self.text_load_job = None;
            self.advance_startup();
            self.dirty_mirror |= pdf_dirty;
            ctx.request_repaint();
        }
        true
    }

    fn render_prompt_modal(&mut self, ctx: &egui::Context) -> bool {
        self.render_prompt_modal_with(ctx, |request| {
            send_ipc_request(request).map_err(|error| error.to_string())
        })
    }

    fn render_prompt_modal_with(
        &mut self,
        ctx: &egui::Context,
        mut send: impl FnMut(&IpcRequest) -> Result<IpcResponse, String>,
    ) -> bool {
        if let Some(prompt) = &mut self.in_app_prompt {
            let mut close_prompt = false;
            let mut submit_request: Option<IpcRequest> = None;

            let modal_bg = hex_to_c32(&self.ui_palette.colors.surface_recessed);
            let modal_stroke = hex_to_c32(&self.ui_palette.harmonies.base);
            egui::Window::new(format!("Input Required — {}", prompt.recipe_name))
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .fixed_size([500.0, 180.0])
                .frame(
                    egui::Frame::window(&ctx.style())
                        .fill(modal_bg)
                        .stroke(egui::Stroke::new(1.5_f32, modal_stroke))
                        .rounding(crate::ui_ext::CARD_CORNER_RADIUS)
                        .inner_margin(egui::Margin::symmetric(22.0, 18.0)),
                )
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        Icon::Sparkles.ui(
                            ui,
                            18.0,
                            hex_to_c32(&self.ui_palette.harmonies.triadic_1),
                        );
                        ui.heading(
                            egui::RichText::new(&prompt.recipe_name)
                                .color(hex_to_c32(&self.ui_palette.colors.text_primary))
                                .strong(),
                        );
                    });

                    ui.add_space(crate::ui_ext::SPACE_SM);
                    ui.label(
                        egui::RichText::new(&prompt.message)
                            .color(hex_to_c32(&self.ui_palette.colors.text_secondary)),
                    );

                    ui.add_space(10.0);
                    let edit_resp = ui.add_sized(
                        [ui.available_width(), ui_ext::CONTROL_HEIGHT],
                        egui::TextEdit::singleline(&mut prompt.input_text)
                            .hint_text("Type input text here..."),
                    );

                    if prompt.first_frame {
                        edit_resp.request_focus();
                        prompt.first_frame = false;
                    }

                    let enter =
                        edit_resp.lost_focus() && ctx.input(|i| i.key_pressed(egui::Key::Enter));

                    ui.add_space(14.0);
                    ui.horizontal(|ui| {
                        let sub_bg = hex_to_c32(&self.ui_palette.harmonies.base);
                        let sub_fg = contrast_text_c32(
                            sub_bg,
                            Some(hex_to_c32(&self.ui_palette.colors.text_primary)),
                            &self.ui_palette,
                        );
                        let submit_btn = ui.add(
                            egui::Button::new(
                                egui::RichText::new("Execute & Run [Enter]")
                                    .color(sub_fg)
                                    .strong(),
                            )
                            .fill(sub_bg),
                        );

                        let can_bg = hex_to_c32(&self.ui_palette.colors.surface_panel);
                        let can_fg = contrast_text_c32(
                            can_bg,
                            Some(hex_to_c32(&self.ui_palette.colors.text_secondary)),
                            &self.ui_palette,
                        );
                        let cancel_btn = ui.add(
                            egui::Button::new(egui::RichText::new("Cancel [Esc]").color(can_fg))
                                .fill(can_bg),
                        );

                        if submit_btn.clicked() || enter {
                            submit_request = Some(IpcRequest::ExecuteRecipeWithPrompt {
                                recipe_name: prompt.recipe_name.clone(),
                                prompt_text: Some(prompt.input_text.clone()),
                            });
                        }

                        if cancel_btn.clicked() || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                            close_prompt = true;
                        }
                    });
                });

            if let Some(request) = submit_request {
                self.execute_action_request_with(ctx, &request, true, &mut send);
            } else if close_prompt {
                self.in_app_prompt = None;
            }
            return true;
        }
        false
    }

    fn replace_viewer(&mut self, viewer: Option<ViewerSession>) {
        self.image_load_job = None;
        self.text_load_job = None;
        self.viewer = viewer;
    }

    fn start_image_item_load(&mut self, source: Result<ImageLoadSource, String>) {
        match source {
            Ok(source) => self.start_image_load(source),
            Err(error) => {
                self.image_load_job = None;
                self.text_load_job = None;
                self.set_status_message(format!("Image preview failed: {error}"));
            }
        }
    }

    fn start_image_load(&mut self, source: ImageLoadSource) {
        self.image_load_job = None;
        self.text_load_job = None;
        if matches!(self.viewer, Some(ViewerSession::ImageLoading(_))) {
            self.replace_viewer(None);
        }
        if let Some((ticket, _, _)) = self.spreadsheet_load_job.take() {
            ticket.cancel();
        }
        if let Some((ticket, _)) = self.midi_load_job.take() {
            ticket.cancel();
        }
        self.image_load_job = Some(ImageLoadJob::new(source, self.config.clone()));
        self.clear_status_message();
    }

    fn poll_image_load(&mut self, ctx: &egui::Context) {
        // A startup image occupies its FIFO position; jobs behind the active
        // viewer remain queued and cannot publish into the foreground.
        let startup = self.image_load_job.is_none();
        let job = if let Some(job) = &self.image_load_job {
            job
        } else if let Some(ViewerSession::ImageLoading(job)) = &self.viewer {
            job
        } else {
            return;
        };
        let result = match job.receiver.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(40));
                return;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err(JobError::Cancelled),
        };
        self.image_load_job = None;
        match result {
            Ok(Ok(state)) => {
                self.replace_viewer(Some(ViewerSession::Image(state)));
                self.status_msg = None;
            }
            failure => {
                self.status_msg = Some(match failure {
                    Ok(Err(error)) => format!("Image preview failed: {error}"),
                    Err(error) => format!("Image preview failed: {error}"),
                    Ok(Ok(_)) => unreachable!(),
                });
                if startup {
                    self.advance_startup();
                }
            }
        }
        ctx.request_repaint();
    }

    fn start_text_file_load(&mut self, path: PathBuf) {
        self.prepare_text_load();
        self.text_load_job = Some(TextPreviewLoadJob::from_file(path, self.config.clone()));
        self.clear_status_message();
    }

    fn start_text_preview_load(&mut self, content: String, title: Option<String>) {
        self.prepare_text_load();
        self.text_load_job = Some(TextPreviewLoadJob::from_text(
            content,
            title,
            self.config.clone(),
        ));
        self.clear_status_message();
    }

    fn start_shared_text_preview_load(&mut self, content: Arc<String>, title: String) {
        self.prepare_text_load();
        self.text_load_job = Some(TextPreviewLoadJob::from_shared_text(
            content,
            title,
            self.config.clone(),
        ));
        self.clear_status_message();
    }

    fn prepare_text_load(&mut self) {
        self.image_load_job = None;
        self.text_load_job = None;
        if matches!(self.viewer, Some(ViewerSession::ImageLoading(_))) {
            self.replace_viewer(None);
        }
        if let Some((ticket, _, _)) = self.spreadsheet_load_job.take() {
            ticket.cancel();
        }
        if let Some((ticket, _)) = self.midi_load_job.take() {
            ticket.cancel();
        }
    }

    fn poll_text_load(&mut self, ctx: &egui::Context) {
        let Some(job) = &self.text_load_job else {
            return;
        };
        let result = match job.receiver.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(40));
                return;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err(JobError::Cancelled),
        };
        self.text_load_job = None;
        match result {
            Ok(Ok(state)) => {
                self.replace_viewer(Some(ViewerSession::Text(state)));
                self.clear_status_message();
            }
            Ok(Err(error)) => self.set_status_message(format!("Text preview failed: {error}")),
            Err(error) => self.set_status_message(format!("Text preview failed: {error}")),
        }
        ctx.request_repaint();
    }

    fn start_spreadsheet_load(&mut self, source: SpreadsheetLoadSource) {
        self.image_load_job = None;
        self.text_load_job = None;
        if let Some((ticket, _)) = self.midi_load_job.take() {
            ticket.cancel();
        }
        if let Some((ticket, _, _)) = self.spreadsheet_load_job.take() {
            ticket.cancel();
        }
        let fallback = match &source {
            SpreadsheetLoadSource::Text { content, title } => {
                Some((Arc::clone(content), title.clone()))
            }
            SpreadsheetLoadSource::File(_) => None,
        };
        let job = BackgroundJobs::global().submit(Duration::from_secs(30), move || match source {
            SpreadsheetLoadSource::File(path) => {
                load_spreadsheet(&path).map(|data| (data, Some(path)))
            }
            SpreadsheetLoadSource::Text { content, .. } => detect_csv_text(&content)
                .map(|data| (data, None))
                .ok_or_else(|| "Clipboard text is not a valid table".to_string()),
        });
        self.spreadsheet_load_job = Some((job.0, job.1, fallback));
        self.status_msg = None;
    }

    fn poll_spreadsheet_load(&mut self, ctx: &egui::Context) {
        let Some((_, receiver, _)) = &self.spreadsheet_load_job else {
            return;
        };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(80));
                return;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err(JobError::Cancelled),
        };
        let (_, _, fallback) = self.spreadsheet_load_job.take().unwrap();
        match result {
            Ok(Ok((data, path))) => {
                self.replace_viewer(Some(ViewerSession::Spreadsheet(
                    SpreadsheetViewerState::new(data, path),
                )));
                ctx.request_repaint();
            }
            failure => {
                if let Some((content, title)) = fallback {
                    self.start_shared_text_preview_load(content, title);
                } else {
                    self.status_msg = Some(match failure {
                        Ok(Err(error)) => format!("Spreadsheet: {error}"),
                        Err(error) => format!("Spreadsheet load failed: {error}"),
                        Ok(Ok(_)) => unreachable!(),
                    });
                }
            }
        }
    }

    fn start_midi_load(&mut self, path: PathBuf) {
        self.image_load_job = None;
        self.text_load_job = None;
        if let Some((ticket, _, _)) = self.spreadsheet_load_job.take() {
            ticket.cancel();
        }
        if let Some((ticket, _)) = self.midi_load_job.take() {
            ticket.cancel();
        }
        let (ticket, receiver) = BackgroundJobs::global()
            .submit(Duration::from_secs(30), move || {
                MidiData::parse_file(&path).map(|data| (data, path))
            });
        self.midi_load_job = Some((ticket, receiver));
        self.status_msg = None;
    }

    fn poll_midi_load(&mut self, ctx: &egui::Context) {
        let Some((_, receiver)) = &self.midi_load_job else {
            return;
        };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(80));
                return;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err(JobError::Cancelled),
        };
        self.midi_load_job = None;
        match result {
            Ok(Ok((data, path))) => {
                self.replace_viewer(Some(ViewerSession::Midi(
                    MidiViewerState::from_parsed_file(path, data),
                )));
                ctx.request_repaint();
            }
            Ok(Err(error)) => self.status_msg = Some(format!("MIDI load failed: {error}")),
            Err(error) => self.status_msg = Some(format!("MIDI load failed: {error}")),
        }
    }

    fn open_spreadsheet_item(&mut self, slot: SlotId) {
        let source = self.state.get(slot).and_then(|item| {
            if let Some(path) = bounded_item_file_path(item).filter(|p| is_spreadsheet_file(p)) {
                Some(SpreadsheetLoadSource::File(path))
            } else {
                match &item.payload {
                    ClipPayload::Text(text) | ClipPayload::Markdown(text) => {
                        Some(SpreadsheetLoadSource::Text {
                            content: Arc::new(text.clone()),
                            title: item.title.clone(),
                        })
                    }
                    _ => None,
                }
            }
        });
        if let Some(source) = source {
            self.start_spreadsheet_load(source);
        }
    }

    fn apply_rolling_audio_action(&mut self, action: RollingAudioAction, ctx: &egui::Context) {
        match action {
            RollingAudioAction::OpenEditor {
                snapshot_path,
                selected_region,
            } => {
                let Some((start, end)) = selected_region else {
                    let editor =
                        AudioPreviewState::new(snapshot_path, "Rolling Audio Snapshot".to_owned())
                            .with_editor(self.config.audio.editor.clone());
                    self.replace_viewer(Some(ViewerSession::Audio(editor)));
                    return;
                };
                if self.rolling_editor_job.is_some() {
                    self.set_status_message(
                        "A rolling audio editor handoff is already in progress",
                    );
                    return;
                }
                let cancelled = Arc::new(AtomicBool::new(false));
                let worker_cancelled = Arc::clone(&cancelled);
                let source = snapshot_path;
                let (ticket, receiver) =
                    BackgroundJobs::global().submit(Duration::MAX, move || {
                        RollingAudioStripState::materialize_editor_selection(
                            &source,
                            start,
                            end,
                            &worker_cancelled,
                        )
                        .map(|path| {
                            let metadata = little_clip_core::audio::read_audio_metadata(&path);
                            (path, metadata)
                        })
                    });
                self.rolling_editor_job = Some(RollingEditorLoadJob {
                    ticket,
                    cancelled,
                    receiver,
                    editor: self.config.audio.editor.clone(),
                    ready: None,
                });
                self.set_status_message("Preparing selected rolling audio for the editor…");
                ctx.request_repaint();
            }
        }
    }

    fn poll_rolling_editor_load(&mut self, ctx: &egui::Context) {
        let Some(job) = &mut self.rolling_editor_job else {
            return;
        };
        if job.ready.is_none() {
            match job.receiver.try_recv() {
                Ok(Ok(Ok(result))) => job.ready = Some(Ok(result)),
                Ok(Ok(Err(error))) => job.ready = Some(Err(error)),
                Ok(Err(error)) => job.ready = Some(Err(error.to_string())),
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(Duration::from_millis(40));
                    return;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    job.ready = Some(Err("background job was cancelled".to_owned()));
                }
            }
        }
        if self.viewer.is_some() || !self.startup_viewers.is_empty() {
            self.set_status_message(
                "Selected rolling audio is ready; waiting for the active viewer to close",
            );
            ctx.request_repaint_after(Duration::from_millis(80));
            return;
        }
        let Some(mut job) = self.rolling_editor_job.take() else {
            return;
        };
        match job.ready.take().expect("ready result checked above") {
            Ok((path, metadata)) => {
                let editor = AudioPreviewState::new_with_metadata(
                    path,
                    "Rolling Audio Selection".to_owned(),
                    metadata,
                )
                .with_editor(job.editor.clone());
                self.status_msg = None;
                self.replace_viewer(Some(ViewerSession::Audio(editor)));
            }
            Err(error) => {
                self.set_status_message(format!("Could not open rolling audio selection: {error}"))
            }
        }
        ctx.request_repaint();
    }

    fn render_rolling_editor_status(&mut self, ctx: &egui::Context) {
        let active = self.rolling_editor_job.is_some();
        let message = if active {
            self.rolling_editor_job
                .as_ref()
                .filter(|job| job.ready.is_some())
                .map(|_| "Selected audio is ready; waiting for the active viewer to close")
                .unwrap_or("Preparing selected rolling audio for the editor…")
                .to_owned()
        } else {
            match self.status_msg.as_deref() {
                Some(message) if message.starts_with("Could not open rolling audio selection:") => {
                    message.to_owned()
                }
                _ => return,
            }
        };
        let mut dismiss = false;
        egui::Area::new(egui::Id::new("rolling_audio_editor_handoff"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::RIGHT_TOP, [-12.0, 12.0])
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if active {
                            ui.spinner();
                        }
                        ui.label(&message);
                        if active {
                            if ui.button("Cancel").clicked() {
                                self.rolling_editor_job = None;
                                self.status_msg = None;
                            }
                        } else {
                            dismiss = ui.button("Dismiss").clicked();
                        }
                    });
                });
            });
        if dismiss {
            self.status_msg = None;
        }
        if active {
            ctx.request_repaint_after(Duration::from_millis(40));
        }
    }

    fn open_slot_file_preview(&mut self, fp: &std::path::Path, is_image: bool) {
        if fp
            .extension()
            .and_then(|s| s.to_str())
            .map(|e| e.eq_ignore_ascii_case("mid") || e.eq_ignore_ascii_case("midi"))
            .unwrap_or(false)
        {
            self.start_midi_load(fp.to_path_buf());
        } else if is_3d_model_file(fp) {
            self.replace_viewer(Some(ViewerSession::Model3d(
                Model3dViewerState::from_file(fp.to_path_buf())
                    .with_config(self.config.model3d.clone()),
            )));
        } else if little_clip_core::pdf::is_pdf_path(fp) {
            self.replace_viewer(Some(ViewerSession::Pdf(PdfEditorState::new(
                fp.to_path_buf(),
            ))));
        } else if is_spreadsheet_file(fp) {
            self.start_spreadsheet_load(SpreadsheetLoadSource::File(fp.to_path_buf()));
        } else if is_image || is_image_path(fp) {
            self.start_image_load(ImageLoadSource::File(fp.to_path_buf()));
        } else if little_clip_ops::is_text_file(fp) {
            self.start_text_file_load(fp.to_path_buf());
        }
    }

    fn open_slot_text_preview(&mut self, text: String, title: &str) {
        if text.len() <= MAX_FILE_PATH_TEXT_BYTES {
            let p = std::path::Path::new(text.trim());
            if p.is_file() {
                self.open_slot_file_preview(p, false);
                return;
            }
        }
        self.start_spreadsheet_load(SpreadsheetLoadSource::Text {
            content: Arc::new(text),
            title: title.to_string(),
        });
    }

    fn open_slot_preview_modal(&mut self, item: &ClipItem) {
        if is_large_text_payload(item) {
            if let Some(text) = item.to_clipboard_text() {
                self.open_slot_text_preview(text, &item.title);
            }
            return;
        }
        if is_pdf_clip_item(item) {
            if let Some(fp) = item.file_path() {
                self.replace_viewer(Some(ViewerSession::Pdf(PdfEditorState::new(fp))));
            } else {
                self.status_msg = Some("PDF preview requires a local file path".to_owned());
            }
            return;
        }
        if let Some(audio_path) = item.audio_path() {
            self.replace_viewer(Some(ViewerSession::Audio(
                AudioPreviewState::new(audio_path, item.title.clone())
                    .with_editor(self.config.audio.editor.clone()),
            )));
            return;
        }
        if let Some(fp) = bounded_item_file_path(item) {
            self.open_slot_file_preview(&fp, item.is_image());
        } else if item.is_image() {
            self.start_image_item_load(ImageLoadSource::from_item(item));
        } else if let Some(text) = item.to_clipboard_text() {
            self.open_slot_text_preview(text, &item.title);
        }
    }

    fn check_inspector_text_preview(&mut self) -> Option<(SlotId, Option<PathBuf>, bool)> {
        let selected_item_ids = self
            .selected_slots
            .iter()
            .map(|slot| self.state.get(*slot).map(|item| item.id))
            .collect::<Vec<_>>();
        if let Some(cache) = &self.inspector_text_preview_cache
            && cache.selected_slots == self.selected_slots
            && cache.selected_item_ids == selected_item_ids
        {
            return cache.text_preview.clone();
        }

        let spreadsheet_slot = self.selected_slots.iter().copied().find(|slot| {
            let Some(item) = self.state.get(*slot) else {
                return false;
            };
            if bounded_item_file_path(item).is_some_and(|path| is_spreadsheet_file(&path)) {
                return true;
            }
            match &item.payload {
                ClipPayload::Text(text) | ClipPayload::Markdown(text) => {
                    text.len() > MAX_INLINE_TEXT_ROUTING_BYTES || looks_like_csv_text(text)
                }
                _ => false,
            }
        });
        let text_preview = self.selected_slots.iter().find_map(|slot| {
            let item = self.state.get(*slot)?;
            let file_path = if let Some(path) =
                bounded_item_file_path(item).filter(|p| little_clip_ops::is_text_file(p))
            {
                Some(path)
            } else {
                let text = match &item.payload {
                    // Borrow text and Markdown payloads so a large clipboard value is
                    // not copied just to decide which inspector button to show.
                    ClipPayload::Text(text) | ClipPayload::Markdown(text) => {
                        Some(std::borrow::Cow::Borrowed(text.as_str()))
                    }
                    _ => item.to_clipboard_text().map(std::borrow::Cow::Owned),
                };
                text.as_deref().and_then(|text| {
                    if text.len() > MAX_FILE_PATH_TEXT_BYTES {
                        return None;
                    }
                    let path = std::path::Path::new(text.trim());
                    (path.is_file() && little_clip_ops::is_text_file(path))
                        .then(|| path.to_path_buf())
                })
            };
            let (file_path, is_markdown) = if let Some(path) = file_path {
                let is_markdown = path.extension().is_some_and(|ext| {
                    ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown")
                });
                (Some(path), is_markdown)
            } else {
                let is_markdown = match &item.payload {
                    ClipPayload::Markdown(_) => true,
                    ClipPayload::Text(text)
                        if text.len() <= MAX_INLINE_MARKDOWN_CLASSIFICATION_BYTES =>
                    {
                        text_preview::detect_markdown_text(text)
                    }
                    // Preview construction performs full classification on its
                    // worker. This value only chooses the inspector's button label.
                    ClipPayload::Text(_) => false,
                    _ => item
                        .to_clipboard_text()
                        .is_some_and(|text| text_preview::detect_markdown_text(&text)),
                };
                (None, is_markdown)
            };
            Some((*slot, file_path, is_markdown))
        });

        self.inspector_text_preview_cache = Some(InspectorTextPreviewCache {
            selected_slots: self.selected_slots.clone(),
            selected_item_ids,
            text_preview: text_preview.clone(),
            spreadsheet_slot,
        });
        text_preview
    }

    fn handle_global_shortcuts(&mut self, ctx: &egui::Context) {
        let audio_active = self.is_audio_region_active();
        if self.handle_escape_shortcut(ctx, audio_active) {
            return;
        }
        self.handle_audio_strip_shortcuts(ctx, audio_active);
        self.handle_view_switching_shortcuts(ctx);
        self.handle_palette_shortcuts(ctx, audio_active);
    }

    fn handle_escape_shortcut(&mut self, ctx: &egui::Context, audio_region_active: bool) -> bool {
        if !ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            return false;
        }
        match route_workbench_escape(
            &mut self.view,
            &mut self.canvas_state,
            &mut self.palette_search_needs_focus,
            audio_region_active,
        ) {
            WorkbenchEscapeRoute::Handled | WorkbenchEscapeRoute::ReturnToPalette => {}
            WorkbenchEscapeRoute::StopAudio => {
                if let Some(strip) = &mut self.rolling_audio_strip {
                    strip.selected_region = None;
                    strip.stop();
                }
            }
            WorkbenchEscapeRoute::CloseWorkbench => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
        true
    }

    fn handle_audio_strip_shortcuts(&mut self, ctx: &egui::Context, audio_region_active: bool) {
        self.handle_audio_strip_playback(ctx, audio_region_active);
        self.handle_audio_strip_copy(ctx, audio_region_active);
        self.handle_audio_strip_refresh(ctx);
    }

    fn handle_audio_strip_playback(&mut self, ctx: &egui::Context, audio_region_active: bool) {
        if audio_strip_space_pressed(ctx, audio_region_active) {
            if let Some(strip) = &mut self.rolling_audio_strip {
                strip.toggle_playback();
            }
        }
    }

    fn handle_audio_strip_copy(&mut self, ctx: &egui::Context, audio_region_active: bool) {
        if !audio_region_active || ctx.wants_keyboard_input() {
            return;
        }
        let copy_pressed = ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::C));
        if !copy_pressed {
            return;
        }
        if let Some(strip) = &mut self.rolling_audio_strip {
            if strip.selected_region.is_some() {
                let now = ctx.input(|i| i.time);
                if strip.copy_selection(now) {
                    if let Ok(IpcResponse::State(s)) = send_ipc_request(&IpcRequest::GetState) {
                        self.state = s;
                    }
                    self.dirty_mirror = true;
                }
            }
        }
    }

    fn handle_audio_strip_refresh(&mut self, ctx: &egui::Context) {
        let refresh_pressed = ctx.input(|i| {
            i.key_pressed(egui::Key::F5) || (i.modifiers.command && i.key_pressed(egui::Key::R))
        });
        if !refresh_pressed {
            return;
        }
        if let Some(strip) = &mut self.rolling_audio_strip {
            strip.load_snapshot();
        }
    }

    fn handle_view_switching_shortcuts(&mut self, ctx: &egui::Context) {
        if self.view == WorkbenchView::NodeMachine
            && (self.canvas_state.catalog_open || self.canvas_state.save_dialog_open)
        {
            return;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::F1)) {
            self.view = WorkbenchView::NodeMachine;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::F2)) {
            self.view = match self.view {
                WorkbenchView::Palette => WorkbenchView::NodeMachine,
                WorkbenchView::NodeMachine | WorkbenchView::AutoActions => WorkbenchView::Palette,
            };
        }
        if ctx.input(|i| i.key_pressed(egui::Key::F3)) {
            self.view = match self.view {
                WorkbenchView::AutoActions => WorkbenchView::Palette,
                _ => WorkbenchView::AutoActions,
            };
        }
    }

    fn handle_palette_shortcuts(&mut self, ctx: &egui::Context, audio_region_active: bool) {
        if self.view != WorkbenchView::Palette {
            return;
        }

        if self.search_query.is_empty() {
            self.handle_empty_search_shortcuts(ctx, audio_region_active);
        } else {
            self.handle_search_prefix_shortcuts(audio_region_active);
        }

        self.handle_palette_nav_and_execution(ctx);
    }

    fn handle_empty_search_shortcuts(&mut self, ctx: &egui::Context, audio_region_active: bool) {
        self.handle_empty_search_shortcuts_with_swap(
            ctx,
            audio_region_active,
            Self::swap_selected_slots,
        );
    }

    fn handle_empty_search_shortcuts_with_swap(
        &mut self,
        ctx: &egui::Context,
        audio_region_active: bool,
        mut swap_slots: impl FnMut(&mut Self),
    ) {
        if ctx.input(|i| i.key_pressed(egui::Key::Tab)) {
            self.view = WorkbenchView::NodeMachine;
            return;
        }
        // The search field also accepts these keys as one-character commands.
        // Let the text edit own the key now; the prefix handler executes it once
        // on the next frame after the field has updated its text.
        if ctx.wants_keyboard_input() {
            return;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Space)) && !audio_region_active {
            self.apply_quick_slot_selection(vec![SlotId::LeftHand, SlotId::RightHand]);
        } else if ctx.input(|i| i.key_pressed(egui::Key::Num1)) {
            self.apply_quick_slot_selection(vec![SlotId::LeftHand, SlotId::LeftPrior1]);
        } else if ctx.input(|i| i.key_pressed(egui::Key::Num2)) {
            self.apply_quick_slot_selection(vec![SlotId::LeftHand, SlotId::LeftPrior2]);
        } else if ctx.input(|i| i.key_pressed(egui::Key::Num3)) {
            self.apply_quick_slot_selection(vec![
                SlotId::LeftHand,
                SlotId::LeftPrior1,
                SlotId::LeftPrior2,
            ]);
        } else if ctx.input(|i| i.key_pressed(egui::Key::Num0)) {
            self.apply_quick_slot_selection(vec![SlotId::LeftHand]);
        } else if ctx.input(|i| {
            i.modifiers.is_none() && (i.key_pressed(egui::Key::S) || i.key_pressed(egui::Key::X))
        }) && self.selected_slots.len() == 2
        {
            swap_slots(self);
        }
    }

    fn apply_quick_slot_selection(&mut self, slots: Vec<SlotId>) {
        self.selected_slots = slots;
        self.dirty_mirror = true;
        self.selected_action_index = 0;
    }

    fn swap_selected_slots(&mut self) {
        let a = self.selected_slots[0];
        let b = self.selected_slots[1];
        let _ = send_ipc_request(&IpcRequest::SwapSlots(a, b));
        self.state.swap_slots(a, b);
        self.dirty_mirror = true;
        self.status_msg = Some(format!("Swapped {} and {}", a.label(), b.label()));
    }

    fn handle_search_prefix_shortcuts(&mut self, audio_region_active: bool) {
        if self.search_query.starts_with(' ') {
            if !audio_region_active {
                self.apply_quick_slot_selection(vec![SlotId::LeftHand, SlotId::RightHand]);
            }
            self.search_query = self.search_query.trim_start().to_string();
        } else if self.search_query == "1" {
            self.apply_quick_slot_selection(vec![SlotId::LeftHand, SlotId::LeftPrior1]);
            self.search_query.clear();
        } else if self.search_query == "2" {
            self.apply_quick_slot_selection(vec![SlotId::LeftHand, SlotId::LeftPrior2]);
            self.search_query.clear();
        } else if self.search_query == "3" {
            self.apply_quick_slot_selection(vec![
                SlotId::LeftHand,
                SlotId::LeftPrior1,
                SlotId::LeftPrior2,
            ]);
            self.search_query.clear();
        } else if self.search_query == "0" || self.search_query == "`" {
            self.apply_quick_slot_selection(vec![SlotId::LeftHand]);
            self.search_query.clear();
        } else if (self.search_query.trim() == "swap"
            || ((self.search_query == "s" || self.search_query == "x")
                && self.selected_slots.len() == 2))
            && self.selected_slots.len() == 2
        {
            self.swap_selected_slots();
            self.search_query.clear();
        }
    }

    fn handle_palette_nav_and_execution(&mut self, ctx: &egui::Context) {
        let filtered = self.filtered_actions();
        if !filtered.is_empty() && self.selected_action_index >= filtered.len() {
            self.selected_action_index = 0;
        }

        if ctx.input(|i| i.key_pressed(egui::Key::ArrowDown)) && !filtered.is_empty() {
            self.selected_action_index = (self.selected_action_index + 1) % filtered.len();
        }
        if ctx.input(|i| i.key_pressed(egui::Key::ArrowUp)) && !filtered.is_empty() {
            self.selected_action_index = if self.selected_action_index == 0 {
                filtered.len() - 1
            } else {
                self.selected_action_index - 1
            };
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
            self.execute_current_action(ctx);
        }
    }
}

pub(crate) fn send_ipc_request(
    req: &IpcRequest,
) -> Result<IpcResponse, Box<dyn std::error::Error>> {
    send_ipc_request_with_timeout(req, None)
}

fn send_ipc_request_with_timeout(
    req: &IpcRequest,
    timeout: Option<Duration>,
) -> Result<IpcResponse, Box<dyn std::error::Error>> {
    let sock_path = get_socket_path();
    let mut stream = UnixStream::connect(&sock_path)?;
    if let Some(timeout) = timeout {
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout))?;
    }

    let json = serde_json::to_string(req)?;
    stream.write_all(json.as_bytes())?;
    stream.write_all(b"\n")?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;

    let resp: IpcResponse = serde_json::from_str(&line)?;
    Ok(resp)
}

fn get_socket_path() -> PathBuf {
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        PathBuf::from(runtime_dir).join("little-screen-clip.sock")
    } else {
        PathBuf::from("/tmp/little-screen-clip.sock")
    }
}

struct PromptApp {
    title: String,
    message: String,
    input_text: String,
    recipe_name: Option<String>,
    first_frame: bool,
    palette: AppPalette,
}

impl PromptApp {
    pub fn new(
        title: String,
        message: String,
        default_text: String,
        recipe_name: Option<String>,
        palette: AppPalette,
    ) -> Self {
        Self {
            title,
            message,
            input_text: default_text,
            recipe_name,
            first_frame: true,
            palette,
        }
    }
}

impl eframe::App for PromptApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        let p_bg = hex_to_c32(&self.palette.colors.surface_recessed);
        let p_border = hex_to_c32(&self.palette.harmonies.base);
        egui::CentralPanel::default()
            .frame(
                egui::Frame::none()
                    .fill(p_bg)
                    .stroke(egui::Stroke::new(1.0_f32, p_border))
                    .inner_margin(egui::Margin::symmetric(24.0, 20.0)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    Icon::Sparkles.ui(ui, 18.0, hex_to_c32(&self.palette.harmonies.triadic_1));
                    ui.heading(
                        egui::RichText::new(&self.title)
                            .color(hex_to_c32(&self.palette.colors.text_primary))
                            .strong(),
                    );
                });

                ui.add_space(crate::ui_ext::SPACE_SM);
                ui.label(
                    egui::RichText::new(&self.message)
                        .color(hex_to_c32(&self.palette.colors.text_secondary)),
                );

                ui.add_space(crate::ui_ext::SPACE_MD);
                let edit_resp = ui.add_sized(
                    [ui.available_width(), ui_ext::CONTROL_HEIGHT],
                    egui::TextEdit::singleline(&mut self.input_text)
                        .font(egui::TextStyle::Body)
                        .hint_text("Enter input text..."),
                );

                if self.first_frame {
                    edit_resp.request_focus();
                    self.first_frame = false;
                }

                let enter =
                    edit_resp.lost_focus() && ctx.input(|i| i.key_pressed(egui::Key::Enter));

                ui.add_space(16.0);
                ui.horizontal(|ui| {
                    let sub_bg = hex_to_c32(&self.palette.harmonies.base);
                    let submit_btn = ui.add(ui_ext::action_button(
                        "Execute & Run [Enter]",
                        sub_bg,
                        &self.palette,
                    ));

                    let can_bg = hex_to_c32(&self.palette.colors.surface_panel);
                    let cancel_btn = ui.add(ui_ext::secondary_button(
                        "Cancel [Esc]",
                        can_bg,
                        &self.palette,
                    ));

                    if submit_btn.clicked() || enter {
                        let text = self.input_text.clone();
                        if let Some(ref r_name) = self.recipe_name {
                            let _ = send_ipc_request(&IpcRequest::ExecuteRecipeWithPrompt {
                                recipe_name: r_name.clone(),
                                prompt_text: Some(text),
                            });
                        } else {
                            let _ = send_ipc_request(&IpcRequest::ExecuteLastAction {
                                prompt_text: Some(text),
                            });
                        }
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }

                    if cancel_btn.clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
            });
    }
}

fn run_prompt_modal(
    title: String,
    message: String,
    default_text: String,
    recipe_name: Option<String>,
) -> Result<(), eframe::Error> {
    let config = AppConfig::load();
    let palette = ui_appearance::palette(config.workbench.appearance);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([560.0, 240.0])
            .with_resizable(false)
            .with_decorations(true)
            .with_app_id("little-clip-prompt")
            .with_window_type(egui::viewport::X11WindowType::Dialog)
            .with_title(format!("Little Screen Clip — {}", title)),
        ..Default::default()
    };

    eframe::run_native(
        "clip-prompt",
        options,
        Box::new(move |cc| {
            ui_ext::configure_workbench_style(&cc.egui_ctx);
            ui_appearance::apply_visuals(&cc.egui_ctx, &palette);
            egui_extras::install_image_loaders(&cc.egui_ctx);
            Box::new(PromptApp::new(
                title,
                message,
                default_text,
                recipe_name,
                palette,
            ))
        }),
    )
}

fn main() -> Result<(), eframe::Error> {
    let args: Vec<String> = std::env::args().collect();
    if args
        .get(1)
        .is_some_and(|arg| arg == "--internal-barcode-worker")
    {
        let result = match (args.get(2), args.get(3), args.len()) {
            (Some(input), Some(output), 4) => barcode_worker::run_helper(
                std::path::Path::new(input),
                std::path::Path::new(output),
            ),
            _ => Err("invalid barcode worker arguments".into()),
        };
        if let Err(error) = result {
            if let Some(output) = args.get(3) {
                let _ =
                    std::fs::write(std::path::Path::new(output).with_extension("error"), &error);
            }
            eprintln!("barcode worker: {error}");
            std::process::exit(2);
        }
        return Ok(());
    }
    if args
        .get(1)
        .is_some_and(|arg| arg == "--internal-conversion-decode-worker")
    {
        let result = match (args.get(2), args.get(3), args.len()) {
            (Some(input), Some(output), 4) => conversion_decode_worker::run_helper(
                std::path::Path::new(input),
                std::path::Path::new(output),
            ),
            _ => Err("invalid conversion decode worker arguments".into()),
        };
        if let Err(error) = result {
            eprintln!("conversion decode worker: {error}");
            std::process::exit(2);
        }
        return Ok(());
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("Little Screen Clip — Crafting Workbench");
        println!("\nUsage: clip-workbench [OPTIONS]");
        println!("\nOptions:");
        println!("  -d, --diff           Open diff comparison viewer directly");
        println!("  -b, --binary         Open interactive binary diff viewer directly");
        println!("  -a, --audio <PATH>   Open audio waveform preview & slicer directly");
        println!("  -i, --image <PATH>   Open image preview & editor directly");
        println!("      --pdf <PATH>     Open PDF page editor directly");
        println!("      --midi <PATH>    Open MIDI piano roll directly");
        println!("      --component-gallery  Open the UI component styling page directly");
        println!("  -l, --last           Re-execute last action (with modal prompt if needed)");
        println!("  -h, --help           Print help");
        return Ok(());
    }

    let is_last = args.iter().any(|a| a == "--last" || a == "-l");
    let is_diff = args.iter().any(|a| a == "--diff" || a == "-d");
    let is_binary_diff = args
        .iter()
        .any(|a| a == "--binary-diff" || a == "--binary" || a == "-b");
    let initial_audio = args
        .iter()
        .position(|a| a == "--audio" || a == "-a")
        .and_then(|idx| args.get(idx + 1))
        .map(PathBuf::from);
    let initial_image = args
        .iter()
        .position(|a| a == "--image" || a == "-i")
        .and_then(|idx| args.get(idx + 1))
        .map(PathBuf::from);
    let initial_pdf = args
        .iter()
        .position(|a| a == "--pdf")
        .and_then(|idx| args.get(idx + 1))
        .map(PathBuf::from);
    let initial_midi = args
        .iter()
        .position(|a| a == "--midi")
        .and_then(|idx| args.get(idx + 1))
        .map(PathBuf::from);
    let open_component_gallery = args.iter().any(|a| a == "--component-gallery");

    if is_last {
        let state = match send_ipc_request(&IpcRequest::GetState) {
            Ok(IpcResponse::State(s)) => s,
            _ => SlotManager::default(),
        };

        if let Ok(IpcResponse::LastAction(Some(action))) =
            send_ipc_request(&IpcRequest::GetLastAction)
        {
            match action {
                little_clip_core::ipc::LastAction::Recipe { recipe_name } => {
                    if let Some(recipe) = find_recipe_by_name(&recipe_name) {
                        if let Some((msg, def)) = recipe_needs_prompt(&recipe, &state) {
                            return run_prompt_modal(
                                recipe.name.clone(),
                                msg,
                                def,
                                Some(recipe.name),
                            );
                        }
                    }
                    // No prompt needed, execute immediately
                    let _ = send_ipc_request(&IpcRequest::ExecuteLastAction { prompt_text: None });
                    return Ok(());
                }
                little_clip_core::ipc::LastAction::Op { .. } => {
                    let _ = send_ipc_request(&IpcRequest::ExecuteLastAction { prompt_text: None });
                    return Ok(());
                }
            }
        } else {
            eprintln!("No last action available to run.");
            return Ok(());
        }
    }

    let Some(_instance_lock) = popup::claim_instance() else {
        return Ok(());
    };

    let state = match send_ipc_request(&IpcRequest::GetState) {
        Ok(IpcResponse::State(s)) => s,
        _ => SlotManager::default(),
    };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_resizable(false)
            .with_decorations(false)
            .with_app_id("little-clip-workbench")
            .with_window_type(egui::viewport::X11WindowType::Dialog)
            .with_title("Little Screen Clip — Crafting Workbench"),
        window_builder: Some(Box::new(popup::window_builder)),
        persist_window: false,
        ..Default::default()
    };

    popup::center_when_mapped();
    eframe::run_native(
        "clip-workbench",
        options,
        Box::new(move |cc| {
            ui_ext::configure_workbench_style(&cc.egui_ctx);
            egui_extras::install_image_loaders(&cc.egui_ctx);
            Box::new(WorkbenchApp::new(
                state,
                is_diff,
                is_binary_diff,
                initial_audio,
                initial_image,
                initial_pdf,
                initial_midi,
                open_component_gallery,
            ))
        }),
    )
}

#[cfg(test)]
mod workbench_layout_tests {
    use super::*;

    #[test]
    fn contextual_palette_preview_actions_use_neutral_secondary_surface() {
        for appearance in [WorkbenchAppearance::Dark, WorkbenchAppearance::Light] {
            let palette = ui_appearance::palette(appearance);
            let expected_fill = hex_to_c32(&palette.colors.surface_panel);
            let ctx = egui::Context::default();
            let output = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.add(contextual_palette_action_button(
                        Some(Icon::Pdf),
                        "Edit PDF",
                        &palette,
                    ));
                });
            });

            let mut saw_fill = false;
            let mut saw_label = false;
            for shape in &output.shapes {
                match &shape.shape {
                    egui::Shape::Rect(rect) if rect.fill == expected_fill => saw_fill = true,
                    egui::Shape::Text(text) if text.galley.text() == "Edit PDF" => {
                        saw_label = true;
                        for section in &text.galley.job.sections {
                            assert!(
                                little_clip_core::palette::contrast_ratio(
                                    (expected_fill.r(), expected_fill.g(), expected_fill.b()),
                                    (
                                        section.format.color.r(),
                                        section.format.color.g(),
                                        section.format.color.b(),
                                    ),
                                ) >= 4.5,
                                "{appearance:?} contextual action label lacks contrast"
                            );
                        }
                    }
                    _ => {}
                }
            }
            assert!(saw_fill, "{appearance:?} action should use panel fill");
            assert!(saw_label, "{appearance:?} action label should render");
        }
    }

    fn test_workbench_app(
        viewer: Option<ViewerSession>,
        startup_viewers: VecDeque<ViewerSession>,
    ) -> WorkbenchApp {
        let config = AppConfig::default();
        let palette = ui_appearance::palette(config.workbench.appearance);
        let (state_tx, state_rx) = std::sync::mpsc::channel();
        drop(state_tx);
        WorkbenchApp {
            state: SlotManager::default(),
            state_rx,
            view: WorkbenchView::Palette,
            canvas_state: NodeCanvasState::new(),
            recipes: Vec::new(),
            search_query: String::new(),
            palette_search_needs_focus: false,
            selected_action_index: 0,
            selected_slots: vec![SlotId::LeftHand],
            inspector_text_preview_cache: None,
            status_msg: None,
            appearance_sync: AppearanceSync::default(),
            renderer: ScreenRenderer::fallback(config.clone()),
            ui_palette: palette,
            mirror_texture: None,
            dirty_mirror: false,
            in_app_prompt: None,
            viewer,
            startup_viewers,
            startup_midi: None,
            image_load_job: None,
            text_load_job: None,
            spreadsheet_load_job: None,
            midi_load_job: None,
            rolling_editor_job: None,
            rolling_audio_strip: None,
            dragging_slot: None,
            config,
            appearance_applied: true,
        }
    }

    fn viewport_close_command_count(output: &egui::FullOutput) -> usize {
        output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map(|output| {
                output
                    .commands
                    .iter()
                    .filter(|command| matches!(command, egui::ViewportCommand::Close))
                    .count()
            })
            .unwrap_or_default()
    }

    #[test]
    fn inspector_markdown_classification_tracks_clipboard_item_identity() {
        let mut app = test_workbench_app(None, VecDeque::new());
        let slot = SlotId::LeftHand;
        let markdown = ClipItem::from_text("# A heading\n\n- One item");
        let markdown_id = markdown.id;
        *app.state.get_mut(slot).expect("left hand slot") = Some(markdown);

        assert_eq!(app.check_inspector_text_preview(), Some((slot, None, true)));
        assert_eq!(
            app.inspector_text_preview_cache
                .as_ref()
                .and_then(|cache| cache.selected_item_ids.first().copied().flatten()),
            Some(markdown_id)
        );

        let plain_text = ClipItem::from_text("ordinary clipboard text");
        let plain_text_id = plain_text.id;
        *app.state.get_mut(slot).expect("left hand slot") = Some(plain_text);

        assert_eq!(
            app.check_inspector_text_preview(),
            Some((slot, None, false))
        );
        assert_eq!(
            app.inspector_text_preview_cache
                .as_ref()
                .and_then(|cache| cache.selected_item_ids.first().copied().flatten()),
            Some(plain_text_id)
        );
    }

    #[test]
    fn inspector_caches_clipboard_spreadsheet_detection_until_item_changes() {
        let mut app = test_workbench_app(None, VecDeque::new());
        let slot = SlotId::LeftHand;
        *app.state.get_mut(slot).unwrap() = Some(ClipItem::from_text("name,count\nalpha,1"));

        assert_eq!(
            app.check_inspector_text_preview(),
            Some((slot, None, false))
        );
        assert_eq!(
            app.inspector_text_preview_cache
                .as_ref()
                .and_then(|cache| cache.spreadsheet_slot),
            Some(slot)
        );
        assert_eq!(
            app.check_inspector_text_preview(),
            Some((slot, None, false))
        );

        *app.state.get_mut(slot).unwrap() = Some(ClipItem::from_text("plain text"));
        assert_eq!(
            app.check_inspector_text_preview(),
            Some((slot, None, false))
        );
        assert_eq!(
            app.inspector_text_preview_cache
                .as_ref()
                .and_then(|cache| cache.spreadsheet_slot),
            None
        );
    }

    #[test]
    fn oversized_markdown_uses_bounded_inspector_hint_and_full_worker_classification() {
        let content = format!("# Heading\n\n{}", "- a list item\n".repeat(20_000));
        let expected = content.clone();
        let mut item = ClipItem::from_text("placeholder");
        item.payload = ClipPayload::Text(content);
        let mut app = test_workbench_app(None, VecDeque::new());
        *app.state.get_mut(SlotId::LeftHand).unwrap() = Some(item.clone());

        let (_, path, quick_hint) = app.check_inspector_text_preview().unwrap();
        assert!(path.is_none());
        assert!(
            !quick_hint,
            "large Text payload should not be scanned inline"
        );

        app.open_slot_preview_modal(&item);
        wait_for_text_load(&mut app);
        let Some(ViewerSession::Text(state)) = &app.viewer else {
            panic!("missing text viewer")
        };
        assert_eq!(
            state.view_mode,
            text_preview::TextViewMode::RenderedMarkdown
        );
        assert_eq!(state.content.as_str(), expected);
    }

    #[test]
    fn closing_startup_viewers_advances_fifo_then_returns_to_selected_workbench_view() {
        let gallery =
            || ViewerSession::ComponentGallery(component_gallery::ComponentGalleryState::default());
        let mut app = test_workbench_app(
            Some(gallery()),
            VecDeque::from([ViewerSession::Spreadsheet(SpreadsheetViewerState::new(
                SpreadsheetData {
                    title: "Queued spreadsheet".to_owned(),
                    sheets: Vec::new(),
                },
                None,
            ))]),
        );
        app.view = WorkbenchView::NodeMachine;
        let ctx = egui::Context::default();
        let input = |events| egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 700.0),
            )),
            events,
            ..Default::default()
        };
        let escape = || egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };

        let _ = ctx.run(input(vec![escape()]), |ctx| {
            assert!(app.render_modal_viewers(ctx));
        });
        assert!(matches!(app.viewer, Some(ViewerSession::Spreadsheet(_))));
        assert!(app.startup_viewers.is_empty());

        let _ = ctx.run(input(Vec::new()), |ctx| {
            assert!(app.render_modal_viewers(ctx));
        });
        assert!(matches!(app.viewer, Some(ViewerSession::Spreadsheet(_))));

        let _ = ctx.run(input(vec![escape()]), |ctx| {
            assert!(app.render_modal_viewers(ctx));
        });
        assert!(app.viewer.is_none());
        assert_eq!(app.view, WorkbenchView::NodeMachine);
    }

    #[test]
    fn startup_pdf_queue_precedes_background_midi_entry() {
        let queued_pdf = ViewerSession::Pdf(PdfEditorState::new(PathBuf::from("queued.pdf")));
        let mut app = test_workbench_app(None, VecDeque::from([queued_pdf]));
        app.startup_midi = Some(PathBuf::from("dense.mid"));

        app.advance_startup();
        assert!(matches!(app.viewer, Some(ViewerSession::Pdf(_))));
        assert!(app.startup_midi.is_some());
        assert!(app.midi_load_job.is_none());

        app.viewer = None;
        app.advance_startup();
        assert!(app.viewer.is_none());
        assert!(app.startup_midi.is_none());
        assert!(app.midi_load_job.is_some());
    }

    #[test]
    fn closing_dirty_pdf_requests_workbench_mirror_redraw() {
        let mut pdf = PdfEditorState::new(PathBuf::from("edited.pdf"));
        pdf.dirty = true;
        let mut app = test_workbench_app(Some(ViewerSession::Pdf(pdf)), VecDeque::new());
        let ctx = egui::Context::default();
        let escape = egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };

        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 700.0),
                )),
                events: vec![escape],
                ..Default::default()
            },
            |ctx| assert!(app.render_modal_viewers(ctx)),
        );

        assert!(app.viewer.is_none());
        assert!(
            app.dirty_mirror,
            "closing an edited PDF must invalidate the mirror"
        );
        app.update_mirror_texture(&ctx);
        assert!(
            !app.dirty_mirror,
            "the next mirror render should consume invalidation"
        );
        assert!(app.mirror_texture.is_some());
    }

    #[test]
    fn palette_text_diff_opens_and_escape_returns_to_palette() {
        let mut app = test_workbench_app(None, VecDeque::new());
        *app.state.get_mut(SlotId::LeftHand).expect("left slot") =
            Some(ClipItem::from_text("left text\n"));
        *app.state.get_mut(SlotId::RightHand).expect("right slot") =
            Some(ClipItem::from_text("right text\n"));
        app.selected_slots = vec![SlotId::LeftHand, SlotId::RightHand];
        app.search_query = "diff".to_owned();
        let diff_action = app
            .filtered_actions()
            .iter()
            .position(|action| matches!(action, PaletteAction::Op(op) if op.name == "diff"))
            .expect("built-in text Diff action");
        app.selected_action_index = diff_action;

        app.execute_current_action(&egui::Context::default());

        assert!(matches!(app.viewer, Some(ViewerSession::Diff(_))));
        assert_eq!(app.view, WorkbenchView::Palette);

        let ctx = egui::Context::default();
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 700.0),
                )),
                events: vec![egui::Event::Key {
                    key: egui::Key::Escape,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ctx| assert!(app.render_modal_viewers(ctx)),
        );

        assert!(app.viewer.is_none());
        assert_eq!(app.view, WorkbenchView::Palette);
    }

    #[test]
    fn palette_image_diff_opens_and_escape_returns_to_palette() {
        let png_item = |color: [u8; 4], title: &str| {
            let image = image::RgbaImage::from_pixel(2, 2, image::Rgba(color));
            let mut bytes = std::io::Cursor::new(Vec::new());
            image::DynamicImage::ImageRgba8(image)
                .write_to(&mut bytes, image::ImageFormat::Png)
                .expect("encode tiny PNG fixture");
            ClipItem {
                id: uuid::Uuid::new_v4(),
                created_at: 0,
                payload: ClipPayload::Image {
                    bytes: bytes.into_inner(),
                    path: None,
                    width: 2,
                    height: 2,
                    mime: "image/png".to_owned(),
                    thumbnail: None,
                },
                title: title.to_owned(),
                subtitle: "PNG".to_owned(),
                details: vec!["PNG".to_owned()],
                handler_tag: "IMAGE".to_owned(),
            }
        };

        let mut app = test_workbench_app(None, VecDeque::new());
        *app.state.get_mut(SlotId::LeftHand).expect("left slot") =
            Some(png_item([255, 0, 0, 255], "left.png"));
        *app.state.get_mut(SlotId::RightHand).expect("right slot") =
            Some(png_item([0, 0, 255, 255], "right.png"));
        app.selected_slots = vec![SlotId::LeftHand, SlotId::RightHand];
        app.search_query = "diff".to_owned();
        let diff_action = app
            .filtered_actions()
            .iter()
            .position(|action| matches!(action, PaletteAction::Op(op) if op.name == "diff"))
            .expect("built-in image Diff action");
        app.selected_action_index = diff_action;

        app.execute_current_action(&egui::Context::default());

        assert!(matches!(
            app.viewer,
            Some(ViewerSession::Diff(ref state)) if state.is_image()
        ));
        assert_eq!(app.view, WorkbenchView::Palette);

        let ctx = egui::Context::default();
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 700.0),
                )),
                events: vec![egui::Event::Key {
                    key: egui::Key::Escape,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ctx| assert!(app.render_modal_viewers(ctx)),
        );

        assert!(app.viewer.is_none());
        assert_eq!(app.view, WorkbenchView::Palette);
    }

    #[test]
    fn palette_binary_diff_opens_and_escape_returns_to_palette() {
        let mut app = test_workbench_app(None, VecDeque::new());
        *app.state.get_mut(SlotId::LeftHand).expect("left slot") =
            Some(ClipItem::from_text("left binary fixture\n"));
        *app.state.get_mut(SlotId::RightHand).expect("right slot") =
            Some(ClipItem::from_text("right binary fixture\n"));
        app.selected_slots = vec![SlotId::LeftHand, SlotId::RightHand];
        app.search_query = "binary-diff".to_owned();
        let binary_diff_action = app
            .filtered_actions()
            .iter()
            .position(|action| matches!(action, PaletteAction::Op(op) if op.name == "binary-diff"))
            .expect("built-in Binary Diff action");
        app.selected_action_index = binary_diff_action;

        app.execute_current_action(&egui::Context::default());

        assert!(matches!(app.viewer, Some(ViewerSession::BinaryDiff(_))));
        assert_eq!(app.view, WorkbenchView::Palette);

        let ctx = egui::Context::default();
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 700.0),
                )),
                events: vec![egui::Event::Key {
                    key: egui::Key::Escape,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ctx| assert!(app.render_modal_viewers(ctx)),
        );

        assert!(app.viewer.is_none());
        assert_eq!(app.view, WorkbenchView::Palette);
    }

    #[test]
    fn quick_palette_operation_uses_injected_request_sender() {
        let mut app = test_workbench_app(None, VecDeque::new());
        *app.state.get_mut(SlotId::LeftHand).expect("left slot") =
            Some(ClipItem::from_text(" input text "));
        app.selected_slots = vec![SlotId::LeftHand];
        app.search_query = "uppercase".to_owned();
        app.selected_action_index = app
            .filtered_actions()
            .iter()
            .position(|action| matches!(action, PaletteAction::Op(op) if op.name == "uppercase"))
            .expect("uppercase operation is available for text");

        let sent = std::cell::RefCell::new(None);
        app.execute_current_action_with(&egui::Context::default(), |request| {
            *sent.borrow_mut() = Some(request.clone());
            Err("controlled transport failure".to_owned())
        });

        assert!(matches!(
            sent.into_inner(),
            Some(IpcRequest::ExecuteOp {
                op_name,
                selected_slots: Some(slots),
                ..
            }) if op_name == "uppercase" && slots == vec![SlotId::LeftHand]
        ));
        assert_eq!(
            app.status_msg.as_deref(),
            Some("Error: controlled transport failure")
        );
    }

    #[test]
    fn quick_palette_operation_success_closes_popup_once() {
        let mut app = test_workbench_app(None, VecDeque::new());
        *app.state.get_mut(SlotId::LeftHand).expect("left slot") =
            Some(ClipItem::from_text("fixture text"));
        app.selected_slots = vec![SlotId::LeftHand];
        app.search_query = "uppercase".to_owned();
        app.selected_action_index = app
            .filtered_actions()
            .iter()
            .position(|action| matches!(action, PaletteAction::Op(op) if op.name == "uppercase"))
            .expect("uppercase operation is available for text");

        let ctx = egui::Context::default();
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            app.execute_current_action_with(ctx, |_| {
                Ok(IpcResponse::OpResult {
                    item: ClipItem::from_text("FIXTURE TEXT"),
                })
            });
        });

        assert_eq!(viewport_close_command_count(&output), 1);
        assert_eq!(app.view, WorkbenchView::Palette);
    }

    #[test]
    fn quick_palette_operation_error_keeps_popup_open_with_feedback() {
        let mut app = test_workbench_app(None, VecDeque::new());
        *app.state.get_mut(SlotId::LeftHand).expect("left slot") =
            Some(ClipItem::from_text("fixture text"));
        app.selected_slots = vec![SlotId::LeftHand];
        app.search_query = "uppercase".to_owned();
        app.selected_action_index = app
            .filtered_actions()
            .iter()
            .position(|action| matches!(action, PaletteAction::Op(op) if op.name == "uppercase"))
            .expect("uppercase operation is available for text");

        let ctx = egui::Context::default();
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            app.execute_current_action_with(ctx, |_| {
                Ok(IpcResponse::Error {
                    error: "fixture operation failure".to_owned(),
                })
            });
        });

        assert_eq!(viewport_close_command_count(&output), 0);
        assert_eq!(app.view, WorkbenchView::Palette);
        assert_eq!(
            app.status_msg.as_deref(),
            Some("Error: fixture operation failure")
        );
    }

    #[test]
    fn quick_palette_recipe_success_closes_popup_once() {
        let mut app = test_workbench_app(None, VecDeque::new());
        let recipe_name = "No Prompt Success Fixture";
        app.recipes.push(Recipe {
            name: recipe_name.to_owned(),
            description: String::new(),
            input_type: little_clip_ops::pipeline::types::RecipeInputType::None,
            tags: Vec::new(),
            graph: little_clip_ops::pipeline::types::NodeGraph::default(),
        });
        app.search_query = recipe_name.to_owned();
        app.selected_action_index = app
            .filtered_actions()
            .iter()
            .position(|action| matches!(action, PaletteAction::Recipe(recipe) if recipe.name == recipe_name))
            .expect("recipe is available");

        let sent = std::cell::RefCell::new(None);
        let ctx = egui::Context::default();
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            app.execute_current_action_with(ctx, |request| {
                *sent.borrow_mut() = Some(request.clone());
                Ok(IpcResponse::Success {
                    message: "fixture completed".to_owned(),
                })
            });
        });

        assert!(matches!(
            sent.into_inner(),
            Some(IpcRequest::ExecuteRecipe { recipe_name: sent_name }) if sent_name == recipe_name
        ));
        assert_eq!(viewport_close_command_count(&output), 1);
        assert_eq!(app.view, WorkbenchView::Palette);
    }

    #[test]
    fn auto_action_success_sends_once_and_keeps_enabled_config() {
        let mut app = test_workbench_app(None, VecDeque::new());
        app.config.auto_actions.trim_link_tracking = false;
        let ctx = egui::Context::default();
        let mut checkbox_rect = egui::Rect::NOTHING;
        let screen_rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 700.0));
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(screen_rect),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    checkbox_rect = render_auto_actions(ui, &mut app.config, &app.ui_palette)
                        .1
                        .checkbox;
                });
            },
        );
        assert!(checkbox_rect.is_positive());
        let click_pos = checkbox_rect.center();
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(screen_rect),
                events: vec![egui::Event::PointerButton {
                    pos: click_pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    render_auto_actions(ui, &mut app.config, &app.ui_palette);
                });
            },
        );
        let mut changed = None;
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(screen_rect),
                events: vec![egui::Event::PointerButton {
                    pos: click_pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    changed = render_auto_actions(ui, &mut app.config, &app.ui_palette).0;
                });
            },
        );
        assert_eq!(
            changed,
            Some(true),
            "rendered checkbox click should enable it"
        );
        assert!(app.config.auto_actions.trim_link_tracking);
        let sent = std::cell::RefCell::new(Vec::new());

        app.execute_auto_action_change_with(&ctx, false, true, &mut |request| {
            sent.borrow_mut().push(request.clone());
            Ok(IpcResponse::Success {
                message: "updated".to_owned(),
            })
        });

        assert!(matches!(
            sent.borrow().as_slice(),
            [IpcRequest::SetAutoAction { key, enabled: true }]
                if key == "trim_link_tracking"
        ));
        assert!(app.config.auto_actions.trim_link_tracking);
        assert!(app.status_msg.is_none());
    }

    #[test]
    fn auto_action_failure_rolls_back_optimistic_toggle_with_feedback() {
        let mut app = test_workbench_app(None, VecDeque::new());
        app.config.auto_actions.trim_link_tracking = true;
        let sent = std::cell::RefCell::new(Vec::new());

        app.execute_auto_action_change_with(
            &egui::Context::default(),
            false,
            true,
            &mut |request| {
                sent.borrow_mut().push(request.clone());
                Err("controlled daemon failure".to_owned())
            },
        );

        assert!(matches!(
            sent.borrow().as_slice(),
            [IpcRequest::SetAutoAction { key, enabled: true }]
                if key == "trim_link_tracking"
        ));
        assert!(!app.config.auto_actions.trim_link_tracking);
        assert_eq!(
            app.status_msg.as_deref(),
            Some("Error: controlled daemon failure")
        );
    }

    fn space_key_event() -> egui::Event {
        key_press_event(egui::Key::Space)
    }

    fn key_press_event(key: egui::Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    #[test]
    fn quick_palette_space_routes_to_both_hands_only_without_audio_selection() {
        let mut app = test_workbench_app(None, VecDeque::new());
        app.selected_slots = vec![SlotId::LeftPrior1];
        let ctx = egui::Context::default();

        let _ = ctx.run(
            egui::RawInput {
                events: vec![space_key_event()],
                ..Default::default()
            },
            |ctx| app.handle_empty_search_shortcuts(ctx, false),
        );
        assert_eq!(
            app.selected_slots,
            vec![SlotId::LeftHand, SlotId::RightHand]
        );
        assert_eq!(app.selected_action_index, 0);

        app.selected_slots = vec![SlotId::LeftPrior1];
        let _ = ctx.run(
            egui::RawInput {
                events: vec![space_key_event()],
                ..Default::default()
            },
            |ctx| app.handle_empty_search_shortcuts(ctx, true),
        );
        assert_eq!(app.selected_slots, vec![SlotId::LeftPrior1]);
    }

    #[test]
    fn palette_digit_shortcuts_select_exact_slot_sets() {
        let cases = [
            (egui::Key::Num1, vec![SlotId::LeftHand, SlotId::LeftPrior1]),
            (egui::Key::Num2, vec![SlotId::LeftHand, SlotId::LeftPrior2]),
            (
                egui::Key::Num3,
                vec![SlotId::LeftHand, SlotId::LeftPrior1, SlotId::LeftPrior2],
            ),
            (egui::Key::Num0, vec![SlotId::LeftHand]),
        ];

        for (key, expected_slots) in cases {
            let mut app = test_workbench_app(None, VecDeque::new());
            app.selected_slots = vec![SlotId::RightPrior2];
            let _ = egui::Context::default().run(
                egui::RawInput {
                    events: vec![key_press_event(key)],
                    ..Default::default()
                },
                |ctx| app.handle_empty_search_shortcuts(ctx, false),
            );

            assert_eq!(app.selected_slots, expected_slots, "key {key:?}");
            assert!(app.dirty_mirror, "key {key:?} should refresh the mirror");
            assert_eq!(app.selected_action_index, 0, "key {key:?}");
        }
    }

    #[test]
    fn palette_search_prefixes_select_slots_and_keep_remaining_query() {
        let cases = [
            (
                "1",
                vec![SlotId::LeftHand, SlotId::LeftPrior1],
                String::new(),
            ),
            (
                "2",
                vec![SlotId::LeftHand, SlotId::LeftPrior2],
                String::new(),
            ),
            (
                "3",
                vec![SlotId::LeftHand, SlotId::LeftPrior1, SlotId::LeftPrior2],
                String::new(),
            ),
            ("0", vec![SlotId::LeftHand], String::new()),
            ("`", vec![SlotId::LeftHand], String::new()),
            (
                "  uppercase",
                vec![SlotId::LeftHand, SlotId::RightHand],
                "uppercase".to_owned(),
            ),
        ];

        for (query, expected_slots, expected_query) in cases {
            let mut app = test_workbench_app(None, VecDeque::new());
            app.search_query = query.to_owned();
            app.handle_search_prefix_shortcuts(false);

            assert_eq!(app.selected_slots, expected_slots, "query {query:?}");
            assert_eq!(app.search_query, expected_query, "query {query:?}");
            assert!(
                app.dirty_mirror,
                "query {query:?} should refresh the mirror"
            );
        }
    }

    #[test]
    fn palette_swap_prefix_stays_available_without_exactly_two_selected_slots() {
        for selected_slots in [
            vec![SlotId::LeftHand],
            vec![SlotId::LeftHand, SlotId::LeftPrior1, SlotId::RightHand],
        ] {
            let mut app = test_workbench_app(None, VecDeque::new());
            app.selected_slots = selected_slots.clone();
            app.state.left_hand = Some(ClipItem::from_text("left fixture".to_owned()));
            app.state.right_hand = Some(ClipItem::from_text("right fixture".to_owned()));
            let state_before = app.state.clone();
            app.search_query = "swap".to_owned();
            app.dirty_mirror = false;
            app.selected_action_index = 2;

            // With no daemon running, this production parser path must keep the
            // command as a query and avoid entering the IPC-backed swap path.
            app.handle_search_prefix_shortcuts(false);

            assert_eq!(app.search_query, "swap");
            assert_eq!(app.selected_slots, selected_slots);
            assert_eq!(app.state, state_before);
            assert!(!app.dirty_mirror);
            assert_eq!(app.selected_action_index, 2);
            assert!(app.status_msg.is_none());
        }
    }

    #[test]
    fn modified_palette_swap_keys_do_not_swap_slots() {
        let modified_keys = [
            (egui::Key::S, egui::Modifiers::CTRL),
            (egui::Key::S, egui::Modifiers::COMMAND),
            (egui::Key::S, egui::Modifiers::MAC_CMD),
            (egui::Key::S, egui::Modifiers::SHIFT),
            (egui::Key::S, egui::Modifiers::ALT),
            (egui::Key::X, egui::Modifiers::CTRL),
            (egui::Key::X, egui::Modifiers::COMMAND),
            (egui::Key::X, egui::Modifiers::MAC_CMD),
            (egui::Key::X, egui::Modifiers::SHIFT),
            (egui::Key::X, egui::Modifiers::ALT),
        ];

        for (key, modifiers) in modified_keys {
            let mut app = test_workbench_app(None, VecDeque::new());
            app.selected_slots = vec![SlotId::LeftHand, SlotId::RightHand];
            app.state.left_hand = Some(ClipItem::from_text("left fixture".to_owned()));
            app.state.right_hand = Some(ClipItem::from_text("right fixture".to_owned()));
            let state_before = app.state.clone();
            app.dirty_mirror = false;
            app.search_query.clear();
            let mut swap_calls = 0;

            let _ = egui::Context::default().run(
                egui::RawInput {
                    events: vec![egui::Event::Key {
                        key,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers,
                    }],
                    modifiers,
                    ..Default::default()
                },
                |ctx| app.handle_empty_search_shortcuts_with_swap(ctx, false, |_| swap_calls += 1),
            );

            assert_eq!(swap_calls, 0, "key {key:?}, modifiers {modifiers:?}");
            assert_eq!(
                app.selected_slots,
                vec![SlotId::LeftHand, SlotId::RightHand]
            );
            assert_eq!(
                app.state, state_before,
                "key {key:?}, modifiers {modifiers:?}"
            );
            assert!(!app.dirty_mirror, "key {key:?}, modifiers {modifiers:?}");
            assert!(
                app.status_msg.is_none(),
                "key {key:?}, modifiers {modifiers:?}"
            );
        }
    }

    #[test]
    fn unmodified_palette_swap_shortcuts_request_one_slot_swap() {
        for key in [egui::Key::S, egui::Key::X] {
            let mut app = test_workbench_app(None, VecDeque::new());
            app.selected_slots = vec![SlotId::LeftHand, SlotId::RightHand];
            let mut swap_calls = 0;

            let _ = egui::Context::default().run(
                egui::RawInput {
                    events: vec![key_press_event(key)],
                    ..Default::default()
                },
                |ctx| app.handle_empty_search_shortcuts_with_swap(ctx, false, |_| swap_calls += 1),
            );

            assert_eq!(swap_calls, 1, "key {key:?}");
        }
    }

    #[test]
    fn palette_action_navigation_wraps_in_both_directions() {
        let mut app = test_workbench_app(None, VecDeque::new());
        for name in ["Navigation Fixture A", "Navigation Fixture B"] {
            app.recipes.push(Recipe {
                name: name.to_owned(),
                description: String::new(),
                input_type: little_clip_ops::pipeline::types::RecipeInputType::None,
                tags: Vec::new(),
                graph: little_clip_ops::pipeline::types::NodeGraph::default(),
            });
        }
        app.search_query = "Navigation Fixture".to_owned();
        assert_eq!(app.filtered_actions().len(), 2);

        let _ = egui::Context::default().run(
            egui::RawInput {
                events: vec![key_press_event(egui::Key::ArrowDown)],
                ..Default::default()
            },
            |ctx| app.handle_palette_nav_and_execution(ctx),
        );
        assert_eq!(app.selected_action_index, 1, "Down selects the next action");

        let _ = egui::Context::default().run(
            egui::RawInput {
                events: vec![key_press_event(egui::Key::ArrowDown)],
                ..Default::default()
            },
            |ctx| app.handle_palette_nav_and_execution(ctx),
        );
        assert_eq!(
            app.selected_action_index, 0,
            "Down wraps to the first action"
        );

        let _ = egui::Context::default().run(
            egui::RawInput {
                events: vec![key_press_event(egui::Key::ArrowUp)],
                ..Default::default()
            },
            |ctx| app.handle_palette_nav_and_execution(ctx),
        );
        assert_eq!(app.selected_action_index, 1, "Up wraps to the last action");
    }

    #[test]
    fn rolling_audio_space_requires_selection_and_yields_to_focused_text() {
        let mut query = String::new();
        let mut eligible = false;
        let focused_ctx = egui::Context::default();
        let _ = focused_ctx.run(
            egui::RawInput {
                events: vec![space_key_event()],
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.add(egui::TextEdit::singleline(&mut query))
                        .request_focus();
                    assert!(ctx.wants_keyboard_input());
                    assert!(!audio_strip_space_pressed(ctx, true));
                    assert!(!audio_strip_space_pressed(ctx, false));
                });
            },
        );

        let ctx = egui::Context::default();
        let _ = ctx.run(
            egui::RawInput {
                events: vec![space_key_event()],
                ..Default::default()
            },
            |ctx| eligible = audio_strip_space_pressed(ctx, true),
        );
        assert!(
            eligible,
            "an unfocused Space with a selected audio region routes to playback"
        );
    }

    #[test]
    fn auto_actions_chrome_stays_inside_compact_and_wide_hosts() {
        for size in [egui::vec2(1280.0, 720.0), egui::vec2(3440.0, 1440.0)] {
            let ctx = egui::Context::default();
            ui_ext::configure_workbench_style(&ctx);
            let palette = ui_appearance::palette(WorkbenchAppearance::Dark);
            let mut config = AppConfig::default();
            let mut host = egui::Rect::NOTHING;
            let mut rendered = None;

            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                    ..Default::default()
                },
                |ctx| {
                    ui_appearance::apply_visuals(ctx, &palette);
                    egui::CentralPanel::default().show(ctx, |ui| {
                        host = ui.available_rect_before_wrap();
                        rendered = Some(render_auto_actions(ui, &mut config, &palette));
                    });
                },
            );

            let layout = rendered.expect("Auto Actions view should render").1;
            for (name, rect) in [
                ("header", layout.header),
                ("subtitle", layout.subtitle),
                ("card", layout.card),
                ("status", layout.status),
                ("explanation", layout.explanation),
                ("behavior", layout.behavior),
            ] {
                assert!(
                    rect.is_positive(),
                    "{name} was not laid out at {size:?}: {rect:?}"
                );
                assert!(
                    host.contains_rect(rect),
                    "{name} {rect:?} falls outside Auto Actions host {host:?} at {size:?}"
                );
            }
        }
    }

    #[test]
    fn node_machine_substates_each_consume_escape_before_palette_return() {
        let mut view = WorkbenchView::NodeMachine;
        let mut canvas = NodeCanvasState::with_config(AppConfig::default());
        canvas.catalog_open = true;
        canvas.save_dialog_open = true;
        canvas.dragging_from = Some((uuid::Uuid::new_v4(), "Out".to_owned(), true));
        let mut search_needs_focus = false;

        assert_eq!(
            route_workbench_escape(&mut view, &mut canvas, &mut search_needs_focus, false),
            WorkbenchEscapeRoute::Handled
        );
        assert!(!canvas.catalog_open);
        assert!(canvas.save_dialog_open);
        assert!(canvas.dragging_from.is_some());
        assert_eq!(view, WorkbenchView::NodeMachine);

        assert_eq!(
            route_workbench_escape(&mut view, &mut canvas, &mut search_needs_focus, false),
            WorkbenchEscapeRoute::Handled
        );
        assert!(!canvas.save_dialog_open);
        assert!(canvas.dragging_from.is_some());
        assert_eq!(view, WorkbenchView::NodeMachine);

        assert_eq!(
            route_workbench_escape(&mut view, &mut canvas, &mut search_needs_focus, false),
            WorkbenchEscapeRoute::Handled
        );
        assert!(canvas.dragging_from.is_none());
        assert_eq!(view, WorkbenchView::NodeMachine);

        assert_eq!(
            route_workbench_escape(&mut view, &mut canvas, &mut search_needs_focus, false),
            WorkbenchEscapeRoute::ReturnToPalette
        );
        assert_eq!(view, WorkbenchView::Palette);
        assert!(search_needs_focus);
    }

    #[test]
    fn auto_actions_escape_returns_to_palette_and_requests_search_focus() {
        let mut view = WorkbenchView::AutoActions;
        let mut canvas = NodeCanvasState::with_config(AppConfig::default());
        let mut search_needs_focus = false;

        assert_eq!(
            route_workbench_escape(&mut view, &mut canvas, &mut search_needs_focus, false),
            WorkbenchEscapeRoute::ReturnToPalette
        );
        assert_eq!(view, WorkbenchView::Palette);
        assert!(search_needs_focus);
    }

    #[test]
    fn node_machine_catalog_and_save_overlays_hold_f1_f3_view_switches() {
        fn press_shortcut(app: &mut WorkbenchApp, key: egui::Key) {
            let ctx = egui::Context::default();
            let _ = ctx.run(
                egui::RawInput {
                    events: vec![egui::Event::Key {
                        key,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    }],
                    ..Default::default()
                },
                |ctx| app.handle_global_shortcuts(ctx),
            );
        }

        let shortcuts = [egui::Key::F1, egui::Key::F2, egui::Key::F3];
        for catalog_open in [true, false] {
            for shortcut in shortcuts {
                let mut app = test_workbench_app(None, VecDeque::new());
                app.view = WorkbenchView::NodeMachine;
                app.canvas_state.catalog_open = catalog_open;
                app.canvas_state.save_dialog_open = !catalog_open;
                let graph_before = serde_yaml::to_string(&app.canvas_state.graph).unwrap();

                press_shortcut(&mut app, shortcut);

                assert_eq!(app.view, WorkbenchView::NodeMachine);
                assert_eq!(app.canvas_state.catalog_open, catalog_open);
                assert_eq!(app.canvas_state.save_dialog_open, !catalog_open);
                assert_eq!(
                    serde_yaml::to_string(&app.canvas_state.graph).unwrap(),
                    graph_before,
                    "{shortcut:?} must not mutate the graph while an overlay is open"
                );

                app.canvas_state.catalog_open = false;
                app.canvas_state.save_dialog_open = false;
                for (resume_key, expected_view) in [
                    (egui::Key::F2, WorkbenchView::Palette),
                    (egui::Key::F1, WorkbenchView::NodeMachine),
                    (egui::Key::F3, WorkbenchView::AutoActions),
                ] {
                    press_shortcut(&mut app, resume_key);
                    assert_eq!(app.view, expected_view);
                    assert!(!app.canvas_state.catalog_open);
                    assert!(!app.canvas_state.save_dialog_open);
                    assert_eq!(
                        serde_yaml::to_string(&app.canvas_state.graph).unwrap(),
                        graph_before,
                        "{resume_key:?} after dismissal must preserve graph state"
                    );
                }
            }
        }
    }

    #[test]
    fn palette_f1_opens_node_machine_through_global_shortcuts() {
        let mut app = test_workbench_app(None, VecDeque::new());
        assert_eq!(app.view, WorkbenchView::Palette);
        let graph_before = serde_yaml::to_string(&app.canvas_state.graph).unwrap();

        let _ = egui::Context::default().run(
            egui::RawInput {
                events: vec![key_press_event(egui::Key::F1)],
                ..Default::default()
            },
            |ctx| app.handle_global_shortcuts(ctx),
        );

        assert_eq!(app.view, WorkbenchView::NodeMachine);
        assert_eq!(
            serde_yaml::to_string(&app.canvas_state.graph).unwrap(),
            graph_before
        );
    }

    #[test]
    fn palette_f3_opens_auto_actions_through_global_shortcuts() {
        let mut app = test_workbench_app(None, VecDeque::new());
        assert_eq!(app.view, WorkbenchView::Palette);
        let graph_before = serde_yaml::to_string(&app.canvas_state.graph).unwrap();

        let _ = egui::Context::default().run(
            egui::RawInput {
                events: vec![key_press_event(egui::Key::F3)],
                ..Default::default()
            },
            |ctx| app.handle_global_shortcuts(ctx),
        );

        assert_eq!(app.view, WorkbenchView::AutoActions);
        assert_eq!(
            serde_yaml::to_string(&app.canvas_state.graph).unwrap(),
            graph_before
        );
    }

    #[test]
    fn node_canvas_state_persists_across_palette_and_auto_actions_routes() {
        let mut app = test_workbench_app(None, VecDeque::new());
        app.view = WorkbenchView::NodeMachine;
        app.canvas_state.selected_recipe_name = "Persistence fixture".to_owned();
        app.canvas_state.graph.nodes[0].title = "Persistent graph node".to_owned();
        app.canvas_state.selected_node = Some(app.canvas_state.graph.nodes[0].id);
        app.canvas_state
            .undo_stack
            .push(app.canvas_state.graph.clone());
        let graph_before = serde_yaml::to_string(&app.canvas_state.graph).unwrap();
        let undo_before = serde_yaml::to_string(&app.canvas_state.undo_stack).unwrap();
        let selected_node = app.canvas_state.selected_node;
        let recipe_name = app.canvas_state.selected_recipe_name.clone();

        for (key, expected_view) in [
            (egui::Key::F2, WorkbenchView::Palette),
            (egui::Key::F3, WorkbenchView::AutoActions),
            (egui::Key::F2, WorkbenchView::Palette),
            (egui::Key::F1, WorkbenchView::NodeMachine),
        ] {
            let ctx = egui::Context::default();
            let _ = ctx.run(
                egui::RawInput {
                    events: vec![egui::Event::Key {
                        key,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    }],
                    ..Default::default()
                },
                |ctx| app.handle_global_shortcuts(ctx),
            );

            assert_eq!(app.view, expected_view);
            assert_eq!(
                serde_yaml::to_string(&app.canvas_state.graph).unwrap(),
                graph_before
            );
            assert_eq!(app.canvas_state.selected_node, selected_node);
            assert_eq!(app.canvas_state.selected_recipe_name, recipe_name);
            assert_eq!(
                serde_yaml::to_string(&app.canvas_state.undo_stack).unwrap(),
                undo_before
            );
        }
    }

    #[test]
    fn node_machine_prompt_and_viewer_preempt_workbench_shortcuts_until_closed() {
        fn frame_with_key(app: &mut WorkbenchApp, key: egui::Key) -> bool {
            let ctx = egui::Context::default();
            let mut modal_active = false;
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(900.0, 700.0),
                    )),
                    events: vec![egui::Event::Key {
                        key,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    }],
                    ..Default::default()
                },
                |ctx| {
                    modal_active = app.render_modal_viewers(ctx);
                    // Match update(): the prompt/viewer renderer owns the frame and
                    // returns before workbench shortcut dispatch or canvas rendering.
                    if modal_active {
                        return;
                    }
                    app.handle_global_shortcuts(ctx);
                },
            );
            modal_active
        }

        let mut prompt_app = test_workbench_app(None, VecDeque::new());
        prompt_app.view = WorkbenchView::NodeMachine;
        prompt_app.canvas_state.selected_node = prompt_app
            .canvas_state
            .graph
            .nodes
            .first()
            .map(|node| node.id);
        prompt_app.in_app_prompt = Some(InAppPromptState {
            recipe_name: "Modal ownership fixture".to_owned(),
            message: "Enter text".to_owned(),
            input_text: "input".to_owned(),
            first_frame: true,
        });
        let prompt_graph = serde_yaml::to_string(&prompt_app.canvas_state.graph).unwrap();
        assert!(frame_with_key(&mut prompt_app, egui::Key::F2));
        assert!(frame_with_key(&mut prompt_app, egui::Key::Delete));
        assert_eq!(prompt_app.view, WorkbenchView::NodeMachine);
        assert!(prompt_app.in_app_prompt.is_some());
        assert_eq!(
            serde_yaml::to_string(&prompt_app.canvas_state.graph).unwrap(),
            prompt_graph
        );
        assert!(frame_with_key(&mut prompt_app, egui::Key::Escape));
        assert!(prompt_app.in_app_prompt.is_none());
        assert_eq!(prompt_app.view, WorkbenchView::NodeMachine);
        assert!(!frame_with_key(&mut prompt_app, egui::Key::F2));
        assert_eq!(prompt_app.view, WorkbenchView::Palette);

        let mut viewer_app = test_workbench_app(
            Some(ViewerSession::ComponentGallery(
                component_gallery::ComponentGalleryState::default(),
            )),
            VecDeque::new(),
        );
        viewer_app.view = WorkbenchView::NodeMachine;
        viewer_app.canvas_state.selected_node = viewer_app
            .canvas_state
            .graph
            .nodes
            .first()
            .map(|node| node.id);
        let viewer_graph = serde_yaml::to_string(&viewer_app.canvas_state.graph).unwrap();
        assert!(frame_with_key(&mut viewer_app, egui::Key::F2));
        assert!(frame_with_key(&mut viewer_app, egui::Key::Delete));
        assert_eq!(viewer_app.view, WorkbenchView::NodeMachine);
        assert!(viewer_app.viewer.is_some());
        assert_eq!(
            serde_yaml::to_string(&viewer_app.canvas_state.graph).unwrap(),
            viewer_graph
        );
        assert!(frame_with_key(&mut viewer_app, egui::Key::Escape));
        assert!(viewer_app.viewer.is_none());
        assert_eq!(viewer_app.view, WorkbenchView::NodeMachine);
        assert!(!frame_with_key(&mut viewer_app, egui::Key::F2));
        assert_eq!(viewer_app.view, WorkbenchView::Palette);
        assert_eq!(
            serde_yaml::to_string(&viewer_app.canvas_state.graph).unwrap(),
            viewer_graph
        );
    }

    #[test]
    fn spreadsheet_completion_waits_until_modal_viewer_closes() {
        let mut app = test_workbench_app(
            Some(ViewerSession::ComponentGallery(
                component_gallery::ComponentGalleryState::default(),
            )),
            VecDeque::new(),
        );
        let (ticket, _ticket_receiver) =
            BackgroundJobs::global().submit(Duration::from_secs(1), || ());
        let (result_tx, receiver) = std::sync::mpsc::channel::<
            Result<Result<(SpreadsheetData, Option<PathBuf>), String>, JobError>,
        >();
        result_tx.send(Ok(Err("fixture failure".into()))).unwrap();
        app.spreadsheet_load_job = Some((ticket, receiver, None));

        let ctx = egui::Context::default();
        ui_ext::configure_workbench_style(&ctx);
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 700.0),
                )),
                ..Default::default()
            },
            |ctx| assert!(app.render_modal_viewers(ctx)),
        );
        assert!(app.viewer.is_some());
        assert!(app.spreadsheet_load_job.is_some());

        app.viewer = None;
        app.poll_spreadsheet_load(&ctx);
        assert!(app.spreadsheet_load_job.is_none());
        assert_eq!(
            app.status_msg.as_deref(),
            Some("Spreadsheet: fixture failure")
        );
        app.poll_spreadsheet_load(&ctx);
        assert!(matches!(app.viewer, None));
        assert_eq!(
            app.status_msg.as_deref(),
            Some("Spreadsheet: fixture failure")
        );

        app.viewer = Some(ViewerSession::ComponentGallery(
            component_gallery::ComponentGalleryState::default(),
        ));
        let (ticket, _ticket_receiver) =
            BackgroundJobs::global().submit(Duration::from_secs(1), || ());
        let (result_tx, receiver) = std::sync::mpsc::channel();
        result_tx
            .send(Ok(Ok((
                SpreadsheetData {
                    title: "fixture".into(),
                    sheets: Vec::new(),
                },
                None,
            ))))
            .unwrap();
        app.spreadsheet_load_job = Some((ticket, receiver, None));
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            if !app.render_modal_viewers(ctx) {
                app.poll_spreadsheet_load(ctx);
            }
        });
        assert!(app.spreadsheet_load_job.is_some());
        assert!(matches!(
            app.viewer,
            Some(ViewerSession::ComponentGallery(_))
        ));

        app.viewer = None;
        app.poll_spreadsheet_load(&ctx);
        assert!(app.spreadsheet_load_job.is_none());
        assert!(matches!(app.viewer, Some(ViewerSession::Spreadsheet(_))));
        app.poll_spreadsheet_load(&ctx);
        assert!(matches!(app.viewer, Some(ViewerSession::Spreadsheet(_))));
    }

    #[test]
    fn midi_completion_waits_for_foreground_and_startup_viewers() {
        let startup_viewer =
            || ViewerSession::ComponentGallery(component_gallery::ComponentGalleryState::default());
        let mut app =
            test_workbench_app(Some(startup_viewer()), VecDeque::from([startup_viewer()]));
        let (ticket, _ticket_receiver) =
            BackgroundJobs::global().submit(Duration::from_secs(1), || ());
        let (result_tx, receiver) =
            std::sync::mpsc::channel::<Result<Result<(MidiData, PathBuf), String>, JobError>>();
        result_tx
            .send(Ok(Ok((
                MidiData {
                    title: "fixture".into(),
                    track_count: 0,
                    track_names: Vec::new(),
                    total_notes: 0,
                    duration_sec: 0.0,
                    bpm: 120.0,
                    min_key: 0,
                    max_key: 0,
                    notes: Vec::new(),
                },
                PathBuf::from("fixture.mid"),
            ))))
            .unwrap();
        app.midi_load_job = Some((ticket, receiver));

        let ctx = egui::Context::default();
        ui_ext::configure_workbench_style(&ctx);
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 700.0),
                )),
                ..Default::default()
            },
            |ctx| {
                if !app.render_modal_viewers(ctx) {
                    app.poll_midi_load(ctx);
                }
            },
        );
        assert!(app.midi_load_job.is_some());
        assert!(matches!(
            app.viewer,
            Some(ViewerSession::ComponentGallery(_))
        ));

        app.viewer = app.startup_viewers.pop_front();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            if !app.render_modal_viewers(ctx) {
                app.poll_midi_load(ctx);
            }
        });
        assert!(app.midi_load_job.is_some());
        assert!(matches!(
            app.viewer,
            Some(ViewerSession::ComponentGallery(_))
        ));

        app.viewer = app.startup_viewers.pop_front();
        assert!(app.viewer.is_none());
        app.poll_midi_load(&ctx);
        assert!(app.midi_load_job.is_none());
        assert!(matches!(app.viewer, Some(ViewerSession::Midi(_))));
        app.poll_midi_load(&ctx);
        assert!(matches!(app.viewer, Some(ViewerSession::Midi(_))));

        app.viewer = Some(startup_viewer());
        let (ticket, _ticket_receiver) =
            BackgroundJobs::global().submit(Duration::from_secs(1), || ());
        let (result_tx, receiver) = std::sync::mpsc::channel();
        result_tx.send(Ok(Err("fixture failure".into()))).unwrap();
        app.midi_load_job = Some((ticket, receiver));
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            if !app.render_modal_viewers(ctx) {
                app.poll_midi_load(ctx);
            }
        });
        assert!(app.midi_load_job.is_some());
        assert!(matches!(
            app.viewer,
            Some(ViewerSession::ComponentGallery(_))
        ));

        app.viewer = None;
        app.poll_midi_load(&ctx);
        assert!(app.midi_load_job.is_none());
        assert!(app.viewer.is_none());
        assert_eq!(
            app.status_msg.as_deref(),
            Some("MIDI load failed: fixture failure")
        );
        app.poll_midi_load(&ctx);
        assert!(app.viewer.is_none());
        assert_eq!(
            app.status_msg.as_deref(),
            Some("MIDI load failed: fixture failure")
        );
    }

    #[test]
    fn failed_rolling_selection_action_keeps_the_current_viewer_open() {
        let ctx = egui::Context::default();
        let mut app = test_workbench_app(
            Some(ViewerSession::ComponentGallery(
                component_gallery::ComponentGalleryState::default(),
            )),
            VecDeque::new(),
        );

        app.apply_rolling_audio_action(
            RollingAudioAction::OpenEditor {
                snapshot_path: PathBuf::from("snapshot.wav"),
                selected_region: Some((1.0, 2.0)),
            },
            &ctx,
        );

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !app
            .rolling_editor_job
            .as_ref()
            .is_some_and(|job| job.ready.is_some())
        {
            app.poll_rolling_editor_load(&ctx);
            assert!(
                std::time::Instant::now() < deadline,
                "rolling editor job should finish"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        app.poll_rolling_editor_load(&ctx);
        assert!(app.rolling_editor_job.is_some());
        assert!(matches!(
            app.viewer,
            Some(ViewerSession::ComponentGallery(_))
        ));
        app.viewer = None;
        app.poll_rolling_editor_load(&ctx);

        assert!(app.viewer.is_none());
        assert!(
            app.status_msg.as_deref().is_some_and(
                |message| message.starts_with("Could not open rolling audio selection:")
            )
        );
    }

    #[test]
    fn rolling_editor_materialization_waits_for_active_viewer_and_preserves_range_editor() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("rolling-capture.wav");
        let mut buffer = little_clip_core::rolling_audio::RollingAudioBuffer::new(2, 44_100, 2);
        buffer.push_samples(&vec![1000i16; 44_100 * 2 * 2]);
        buffer.write_wav(&source).unwrap();
        let gallery =
            || ViewerSession::ComponentGallery(component_gallery::ComponentGalleryState::default());
        let mut app = test_workbench_app(Some(gallery()), VecDeque::new());
        app.config.audio.editor = "custom-audio-editor".to_owned();
        let ctx = egui::Context::default();
        app.apply_rolling_audio_action(
            RollingAudioAction::OpenEditor {
                snapshot_path: source,
                selected_region: Some((1.25, 0.5)),
            },
            &ctx,
        );

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !app
            .rolling_editor_job
            .as_ref()
            .is_some_and(|job| job.ready.is_some())
        {
            app.poll_rolling_editor_load(&ctx);
            assert!(
                std::time::Instant::now() < deadline,
                "rolling editor materialization should finish"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        app.poll_rolling_editor_load(&ctx);
        assert!(matches!(
            app.viewer,
            Some(ViewerSession::ComponentGallery(_))
        ));
        assert!(app.rolling_editor_job.is_some());
        app.viewer = None;
        app.poll_rolling_editor_load(&ctx);

        let Some(ViewerSession::Audio(editor)) = app.viewer.as_ref() else {
            panic!(
                "rolling selection should open the audio editor after the current viewer closes"
            );
        };
        assert_eq!(editor.title, "Rolling Audio Selection");
        assert_eq!(editor.audio_editor, "custom-audio-editor");
        assert!(
            (0.5..=0.8).contains(&editor.duration_sec),
            "selected range should remain bounded after optional edge trimming: {}",
            editor.duration_sec
        );
    }

    fn wait_for_image_load(app: &mut WorkbenchApp) {
        let ctx = egui::Context::default();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while app.image_load_job.is_some()
            || matches!(app.viewer, Some(ViewerSession::ImageLoading(_)))
        {
            app.poll_image_load(&ctx);
            assert!(
                std::time::Instant::now() < deadline,
                "image load did not finish"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn wait_for_text_load(app: &mut WorkbenchApp) {
        let ctx = egui::Context::default();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while app.text_load_job.is_some() || app.spreadsheet_load_job.is_some() {
            app.poll_spreadsheet_load(&ctx);
            app.poll_text_load(&ctx);
            assert!(
                std::time::Instant::now() < deadline,
                "text load did not finish"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn write_test_image(path: &std::path::Path, width: u32) {
        image::RgbImage::from_pixel(width, 2, image::Rgb([24, 120, 180]))
            .save(path)
            .unwrap();
    }

    fn gallery_viewer() -> ViewerSession {
        ViewerSession::ComponentGallery(component_gallery::ComponentGalleryState::default())
    }

    #[test]
    fn model3d_toolbar_clicks_change_only_their_view_controls() {
        fn frame(
            ctx: &egui::Context,
            app: &mut WorkbenchApp,
            events: Vec<egui::Event>,
        ) -> egui::FullOutput {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(900.0, 700.0),
                    )),
                    time: Some(1.0),
                    events,
                    ..Default::default()
                },
                |ctx| assert!(app.render_modal_viewers(ctx)),
            )
        }

        fn label_rect(output: &egui::FullOutput, label: &str) -> egui::Rect {
            output
                .shapes
                .iter()
                .find_map(|shape| {
                    let egui::epaint::Shape::Text(text) = &shape.shape else {
                        return None;
                    };
                    (text.galley.job.text == label)
                        .then(|| text.galley.rect.translate(text.pos.to_vec2()))
                })
                .unwrap_or_else(|| panic!("toolbar label {label:?} was not rendered"))
        }

        fn click_label(ctx: &egui::Context, app: &mut WorkbenchApp, label: &str) {
            let rect = label_rect(&frame(ctx, app, Vec::new()), label);
            let pos = rect.center();
            let pointer = egui::Event::PointerMoved(pos);
            let button = |pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            let _ = frame(ctx, app, vec![pointer.clone(), button(true)]);
            let _ = frame(ctx, app, vec![pointer, button(false)]);
        }

        #[derive(Debug, PartialEq)]
        struct ViewerSnapshot {
            route: WorkbenchView,
            path: Option<PathBuf>,
            title: String,
            status: model3d_view::ModelLoadingStatus,
            model: Option<(usize, String, String, usize)>,
            pitch: f32,
            yaw: f32,
            zoom: f32,
            pan_offset: egui::Vec2,
            auto_rotate: bool,
            wireframe: bool,
            up_axis: model3d_view::UpAxis,
        }

        #[derive(Clone, Copy)]
        enum ChangedControl {
            AutoRotate,
            Wireframe,
            UpAxis,
            Camera,
        }

        fn snapshot(app: &WorkbenchApp) -> ViewerSnapshot {
            let Some(ViewerSession::Model3d(state)) = &app.viewer else {
                panic!("3D viewer should remain active")
            };
            ViewerSnapshot {
                route: app.view,
                path: state.path.clone(),
                title: state.title.clone(),
                status: state.status.clone(),
                model: state.mesh.as_ref().map(|mesh| {
                    (
                        mesh as *const _ as usize,
                        mesh.name.clone(),
                        mesh.format.badge_label().to_owned(),
                        mesh.triangles.len(),
                    )
                }),
                pitch: state.pitch,
                yaw: state.yaw,
                zoom: state.zoom,
                pan_offset: state.pan_offset,
                auto_rotate: state.auto_rotate,
                wireframe: state.wireframe,
                up_axis: state.up_axis,
            }
        }

        fn assert_unchanged_except(
            before: &ViewerSnapshot,
            after: &ViewerSnapshot,
            changed: ChangedControl,
        ) {
            assert_eq!(
                (
                    &before.route,
                    &before.path,
                    &before.title,
                    &before.status,
                    &before.model
                ),
                (
                    &after.route,
                    &after.path,
                    &after.title,
                    &after.status,
                    &after.model
                ),
                "route, load status, and model session remain unchanged"
            );
            match changed {
                ChangedControl::AutoRotate => assert_eq!(
                    (
                        before.pitch,
                        before.yaw,
                        before.zoom,
                        before.pan_offset,
                        before.wireframe,
                        before.up_axis
                    ),
                    (
                        after.pitch,
                        after.yaw,
                        after.zoom,
                        after.pan_offset,
                        after.wireframe,
                        after.up_axis
                    )
                ),
                ChangedControl::Wireframe => assert_eq!(
                    (
                        before.pitch,
                        before.yaw,
                        before.zoom,
                        before.pan_offset,
                        before.auto_rotate,
                        before.up_axis
                    ),
                    (
                        after.pitch,
                        after.yaw,
                        after.zoom,
                        after.pan_offset,
                        after.auto_rotate,
                        after.up_axis
                    )
                ),
                ChangedControl::UpAxis => assert_eq!(
                    (
                        before.pitch,
                        before.yaw,
                        before.zoom,
                        before.pan_offset,
                        before.auto_rotate,
                        before.wireframe
                    ),
                    (
                        after.pitch,
                        after.yaw,
                        after.zoom,
                        after.pan_offset,
                        after.auto_rotate,
                        after.wireframe
                    )
                ),
                ChangedControl::Camera => assert_eq!(
                    (before.auto_rotate, before.wireframe, before.up_axis),
                    (after.auto_rotate, after.wireframe, after.up_axis)
                ),
            }
        }

        fn click_and_snapshot(
            ctx: &egui::Context,
            app: &mut WorkbenchApp,
            label: &str,
            changed: ChangedControl,
        ) -> (ViewerSnapshot, ViewerSnapshot) {
            let before = snapshot(app);
            click_label(ctx, app, label);
            let after = snapshot(app);
            assert_unchanged_except(&before, &after, changed);
            (before, after)
        }

        let model = little_clip_core::presentation::model3d::Model3d {
            name: "synthetic.obj".to_owned(),
            format: little_clip_core::presentation::model3d::ModelFormat::Obj,
            triangles: Vec::new(),
            thumbnail_triangles: Vec::new(),
            min_bounds: little_clip_core::presentation::model3d::Vec3::ZERO,
            max_bounds: little_clip_core::presentation::model3d::Vec3::ZERO,
            center: little_clip_core::presentation::model3d::Vec3::ZERO,
            size: little_clip_core::presentation::model3d::Vec3::ZERO,
            max_dimension: 1.0,
        };
        let mut app = test_workbench_app(None, VecDeque::new());
        app.view = WorkbenchView::NodeMachine;
        app.viewer = Some(ViewerSession::Model3d(Model3dViewerState::from_model(
            model,
            Some(PathBuf::from("synthetic.obj")),
        )));
        let ctx = egui::Context::default();
        crate::ui_ext::configure_workbench_style(&ctx);

        // The initial frame both measures the real button bounds and starts
        // the egui clock. Reusing its fixed timestamp avoids animation drift
        // while exercising successive clicks.
        let _ = frame(&ctx, &mut app, Vec::new());

        let (before, after) =
            click_and_snapshot(&ctx, &mut app, "Pause Rotation", ChangedControl::AutoRotate);
        assert!(
            before.auto_rotate && !after.auto_rotate,
            "Pause Rotation toggles auto-rotate off"
        );

        let (before, after) =
            click_and_snapshot(&ctx, &mut app, "Auto-Rotate", ChangedControl::AutoRotate);
        assert!(
            !before.auto_rotate && after.auto_rotate,
            "Auto-Rotate resumes"
        );

        let (before, after) =
            click_and_snapshot(&ctx, &mut app, "Wireframe", ChangedControl::Wireframe);
        assert!(!before.wireframe && after.wireframe);

        let (before, after) =
            click_and_snapshot(&ctx, &mut app, "Shaded", ChangedControl::Wireframe);
        assert!(before.wireframe && !after.wireframe, "Shaded restores fill");

        let (before, after) =
            click_and_snapshot(&ctx, &mut app, "Axis: Y-Up", ChangedControl::UpAxis);
        assert_eq!(before.up_axis, model3d_view::UpAxis::YUp);
        assert_eq!(after.up_axis, model3d_view::UpAxis::ZUp);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        let Some(ViewerSession::Model3d(state)) = &app.viewer else {
            panic!("3D viewer should remain active after axis toggle");
        };
        assert_eq!(
            state.status_toast.message(now),
            Some("Orientation set to Z-Up (CAD / 3D printing)")
        );

        let (before, after) =
            click_and_snapshot(&ctx, &mut app, "Axis: Z-Up", ChangedControl::UpAxis);
        assert_eq!(before.up_axis, model3d_view::UpAxis::ZUp);
        assert_eq!(after.up_axis, model3d_view::UpAxis::YUp);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        let Some(ViewerSession::Model3d(state)) = &app.viewer else {
            panic!("3D viewer should remain active after axis toggle");
        };
        assert_eq!(
            state.status_toast.message(now),
            Some("Orientation set to Y-Up (3D Graphics)")
        );

        if let Some(ViewerSession::Model3d(state)) = &mut app.viewer {
            state.auto_rotate = false;
            state.pitch = 1.1;
            state.yaw = 2.2;
            state.zoom = 3.5;
            state.pan_offset = egui::vec2(17.0, -9.0);
        }
        let (before, after) =
            click_and_snapshot(&ctx, &mut app, "Reset View", ChangedControl::Camera);
        assert_eq!(
            (after.pitch, after.yaw, after.zoom, after.pan_offset),
            (0.4, 0.6, 1.0, egui::Vec2::ZERO)
        );
        assert_ne!(before.pitch, after.pitch);
    }

    #[test]
    fn file_backed_3d_viewer_escape_returns_to_workbench_and_drops_pending_session() {
        let temp_dir = tempfile::tempdir().unwrap();
        let model_path = temp_dir.path().join("fixture.obj");
        std::fs::write(
            &model_path,
            "v 0 0 0\nv 1 0 0\nv 0 1 0\nv 0 0 1\nf 1 2 3\nf 1 2 4\nf 1 3 4\nf 2 3 4\n",
        )
        .unwrap();
        let mut app = test_workbench_app(None, VecDeque::new());
        app.view = WorkbenchView::Palette;

        app.open_slot_file_preview(&model_path, false);

        let Some(ViewerSession::Model3d(state)) = app.viewer.as_ref() else {
            panic!("OBJ route should open the file-backed 3D viewer");
        };
        assert_eq!(state.path.as_deref(), Some(model_path.as_path()));
        assert!(state.rx.is_some(), "file parsing should remain async");
        assert_eq!(app.view, WorkbenchView::Palette);

        let ctx = egui::Context::default();
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 700.0),
                )),
                events: vec![egui::Event::Key {
                    key: egui::Key::Escape,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ctx| assert!(app.render_modal_viewers(ctx)),
        );
        assert!(
            app.viewer.is_none(),
            "Escape should drop the 3D viewer session"
        );
        assert_eq!(app.view, WorkbenchView::Palette);

        // A subsequent modal poll must not let a completed parse publish into
        // or replace the workbench after the viewer has been closed.
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            assert!(!app.render_modal_viewers(ctx));
        });
        assert!(app.viewer.is_none());
        assert_eq!(app.view, WorkbenchView::Palette);
    }

    #[test]
    fn malformed_image_preview_reports_cause_and_valid_preview_recovers() {
        let temp_dir = tempfile::tempdir().unwrap();
        let malformed_path = temp_dir.path().join("broken.png");
        std::fs::write(&malformed_path, b"not a PNG").unwrap();
        let valid_path = temp_dir.path().join("valid.png");
        write_test_image(&valid_path, 2);
        let mut app = test_workbench_app(Some(gallery_viewer()), VecDeque::new());

        app.open_slot_file_preview(&malformed_path, true);
        assert!(matches!(
            app.viewer,
            Some(ViewerSession::ComponentGallery(_))
        ));
        assert!(app.image_load_job.is_some());
        wait_for_image_load(&mut app);
        assert!(matches!(
            app.viewer,
            Some(ViewerSession::ComponentGallery(_))
        ));
        assert!(app.status_msg.as_deref().is_some_and(|message| {
            message.starts_with("Image preview failed: Failed to decode image bytes:")
        }));

        app.open_slot_file_preview(&valid_path, true);
        assert!(matches!(
            app.viewer,
            Some(ViewerSession::ComponentGallery(_))
        ));
        wait_for_image_load(&mut app);
        assert!(matches!(app.viewer, Some(ViewerSession::Image(_))));
        assert!(app.status_msg.is_none());
    }

    #[test]
    fn mirror_context_preview_routes_pdf_image_payload_to_pdf_editor() {
        let temp_dir = tempfile::tempdir().unwrap();
        let pdf_path = temp_dir.path().join("document.data");
        std::fs::write(&pdf_path, b"%PDF-1.4\n").unwrap();
        let item = ClipItem {
            id: uuid::Uuid::new_v4(),
            created_at: 0,
            payload: ClipPayload::Image {
                bytes: Vec::new(),
                path: Some(pdf_path),
                width: 800,
                height: 600,
                mime: "application/pdf".to_owned(),
                thumbnail: None,
            },
            title: "document.pdf".to_owned(),
            subtitle: "PDF (1 pg)".to_owned(),
            details: vec!["PDF".to_owned()],
            handler_tag: "PDF (DOCUMENT)".to_owned(),
        };
        assert!(item.is_image(), "PDF slot previews are image-backed");
        assert!(
            !is_image_preview_action_item(&item),
            "PDF MIME should use the PDF editor even when its backing path has no .pdf extension"
        );

        let mut app = test_workbench_app(None, VecDeque::new());
        app.state.push_left(item);
        app.handle_mirror_action(Some(MirrorAction::Context(SlotId::LeftHand)));

        assert!(matches!(app.viewer, Some(ViewerSession::Pdf(_))));
        assert!(app.image_load_job.is_none());
    }

    #[test]
    fn mirror_context_preview_never_sends_pathless_pdf_bytes_to_image_decoder() {
        let item = ClipItem {
            id: uuid::Uuid::new_v4(),
            created_at: 0,
            payload: ClipPayload::Image {
                bytes: b"%PDF-1.4".to_vec(),
                path: None,
                width: 800,
                height: 600,
                mime: "application/pdf".to_owned(),
                thumbnail: None,
            },
            title: "document.pdf".to_owned(),
            subtitle: "PDF".to_owned(),
            details: vec!["PDF".to_owned()],
            handler_tag: "PDF (DOCUMENT)".to_owned(),
        };
        let mut app = test_workbench_app(None, VecDeque::new());
        app.state.push_left(item);

        app.handle_mirror_action(Some(MirrorAction::Context(SlotId::LeftHand)));

        assert!(app.viewer.is_none());
        assert!(app.image_load_job.is_none());
        assert_eq!(
            app.status_msg.as_deref(),
            Some("PDF preview requires a local file path")
        );
    }

    #[test]
    fn clipboard_image_preview_decodes_owned_bytes_on_worker() {
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            3,
            2,
            image::Rgba([12, 34, 56, 255]),
        ))
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
        let mut item = ClipItem::from_image_bytes(bytes, "image/png".to_string()).unwrap();
        if let ClipPayload::Image { path, .. } = &mut item.payload {
            *path = None;
        }
        let mut app = test_workbench_app(None, VecDeque::new());
        app.open_slot_preview_modal(&item);
        assert!(app.viewer.is_none());
        assert!(app.image_load_job.is_some());
        wait_for_image_load(&mut app);
        let Some(ViewerSession::Image(state)) = &app.viewer else {
            panic!("missing image viewer")
        };
        assert_eq!(state.current_rgba.dimensions(), (3, 2));
        assert_eq!(state.title, item.title);
        assert!(matches!(&item.payload, ClipPayload::Image { bytes, .. } if !bytes.is_empty()));
    }

    #[test]
    fn text_file_preview_loads_on_worker_and_reports_read_errors() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("sample.rs");
        std::fs::write(&path, "fn sample() {\n    42\n}\n").unwrap();
        let mut app = test_workbench_app(Some(gallery_viewer()), VecDeque::new());

        app.open_slot_file_preview(&path, false);
        assert!(app.text_load_job.is_some());
        assert!(matches!(
            app.viewer,
            Some(ViewerSession::ComponentGallery(_))
        ));
        wait_for_text_load(&mut app);
        let Some(ViewerSession::Text(state)) = &app.viewer else {
            panic!("missing text viewer")
        };
        assert_eq!(state.path.as_deref(), Some(path.as_path()));
        assert!(state.content.contains("fn sample()"));
        assert_eq!(state.language, "rs");
        assert!(app.status_msg.is_none());

        app.start_text_file_load(temp_dir.path().join("missing.rs"));
        wait_for_text_load(&mut app);
        assert!(matches!(app.viewer, Some(ViewerSession::Text(_))));
        assert!(app.status_msg.as_deref().is_some_and(|message| {
            message.starts_with("Text preview failed: Could not read or prepare text file")
        }));
    }

    #[test]
    fn clipboard_text_preview_prepares_on_worker_and_preserves_full_searchable_text() {
        let text = format!("{}\nTARGET_AT_END", "clipboard row\n".repeat(80_000));
        let expected = text.clone();
        let mut app = test_workbench_app(None, VecDeque::new());
        let mut item = ClipItem::from_text(text);
        item.title = "large clipboard".to_owned();

        app.open_slot_preview_modal(&item);
        assert!(app.spreadsheet_load_job.is_some());
        wait_for_text_load(&mut app);

        let Some(ViewerSession::Text(state)) = &mut app.viewer else {
            panic!("missing text viewer")
        };
        assert_eq!(state.title, "large clipboard");
        assert_eq!(state.content.as_str(), expected);
        state.find_query = "TARGET_AT_END".to_owned();
        text_preview::update_search_matches_and_wait(state);
        assert_eq!(state.match_count(), 1);
        let matched = state.search_match(0).unwrap();
        assert_eq!(
            &state.content[matched.start_byte..matched.end_byte],
            "TARGET_AT_END"
        );
    }

    #[test]
    fn oversized_csv_remains_available_in_inspector_and_generic_preview() {
        let content = format!("{}name,count\nalpha,1\nbeta,2", " ".repeat(100_000));
        let mut app = test_workbench_app(None, VecDeque::new());
        let slot = SlotId::LeftHand;
        let item = ClipItem::from_text(content);
        *app.state.get_mut(slot).unwrap() = Some(item.clone());

        assert_eq!(
            app.check_inspector_text_preview(),
            Some((slot, None, false))
        );
        assert_eq!(
            app.inspector_text_preview_cache
                .as_ref()
                .and_then(|cache| cache.spreadsheet_slot),
            Some(slot)
        );

        app.open_slot_preview_modal(&item);
        assert!(app.spreadsheet_load_job.is_some());
        let ctx = egui::Context::default();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while app.spreadsheet_load_job.is_some() {
            app.poll_spreadsheet_load(&ctx);
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(matches!(app.viewer, Some(ViewerSession::Spreadsheet(_))));
    }

    #[test]
    fn oversized_malformed_csv_falls_back_to_full_text_preview() {
        let content = format!("{}name,count\nalpha,1,extra", " ".repeat(100_000));
        let mut app = test_workbench_app(None, VecDeque::new());
        let slot = SlotId::LeftHand;
        *app.state.get_mut(slot).unwrap() = Some(ClipItem::from_text(content.clone()));

        assert_eq!(
            app.check_inspector_text_preview(),
            Some((slot, None, false))
        );
        assert_eq!(
            app.inspector_text_preview_cache
                .as_ref()
                .and_then(|cache| cache.spreadsheet_slot),
            Some(slot)
        );

        app.open_spreadsheet_item(slot);
        assert!(app.spreadsheet_load_job.is_some());
        wait_for_text_load(&mut app);
        let Some(ViewerSession::Text(state)) = &app.viewer else {
            panic!("malformed CSV should open the text viewer")
        };
        assert_eq!(state.content.as_str(), content);
    }

    #[test]
    fn replacing_text_file_request_cancels_stale_completion() {
        let temp_dir = tempfile::tempdir().unwrap();
        let first = temp_dir.path().join("large.txt");
        let second = temp_dir.path().join("newer.txt");
        std::fs::write(&first, "older text\n".repeat(50_000)).unwrap();
        std::fs::write(&second, "newer text\n").unwrap();
        let mut app = test_workbench_app(Some(gallery_viewer()), VecDeque::new());

        app.start_text_file_load(first.clone());
        app.start_text_file_load(second.clone());
        wait_for_text_load(&mut app);
        let Some(ViewerSession::Text(state)) = &app.viewer else {
            panic!("missing text viewer")
        };
        assert_eq!(state.path.as_deref(), Some(second.as_path()));
        assert!(state.content.contains("newer text"));

        app.start_text_file_load(first);
        app.replace_viewer(Some(gallery_viewer()));
        assert!(app.text_load_job.is_none());
        app.poll_text_load(&egui::Context::default());
        assert!(matches!(
            app.viewer,
            Some(ViewerSession::ComponentGallery(_))
        ));
    }

    #[test]
    fn replaced_image_request_and_replaced_viewer_discard_stale_completion() {
        let temp_dir = tempfile::tempdir().unwrap();
        let first = temp_dir.path().join("first.png");
        let second = temp_dir.path().join("second.png");
        write_test_image(&first, 3);
        write_test_image(&second, 5);
        let mut app = test_workbench_app(Some(gallery_viewer()), VecDeque::new());
        app.open_slot_file_preview(&first, true);
        app.open_slot_file_preview(&second, true);
        wait_for_image_load(&mut app);
        let Some(ViewerSession::Image(state)) = &app.viewer else {
            panic!("missing image viewer")
        };
        assert_eq!(state.current_rgba.width(), 5);

        app.open_slot_file_preview(&first, true);
        app.replace_viewer(Some(gallery_viewer()));
        assert!(app.image_load_job.is_none());
        app.poll_image_load(&egui::Context::default());
        assert!(matches!(
            app.viewer,
            Some(ViewerSession::ComponentGallery(_))
        ));
    }

    #[test]
    fn replacing_startup_image_request_cancels_its_placeholder() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("image.png");
        write_test_image(&path, 3);
        let loading = ViewerSession::ImageLoading(ImageLoadJob::new(
            ImageLoadSource::File(path),
            AppConfig::default(),
        ));
        let mut app = test_workbench_app(Some(loading), VecDeque::new());
        app.start_image_load(ImageLoadSource::File(temp_dir.path().join("missing.png")));
        assert!(app.viewer.is_none());
        wait_for_image_load(&mut app);
        app.poll_image_load(&egui::Context::default());
        assert!(app.viewer.is_none());
        assert!(app.status_msg.as_deref().is_some_and(|message| {
            message.starts_with("Image preview failed: Failed to read file")
        }));
    }

    #[test]
    fn closing_viewer_cancels_pending_image_replacement() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("image.png");
        write_test_image(&path, 3);
        let mut app = test_workbench_app(Some(gallery_viewer()), VecDeque::new());
        app.open_slot_file_preview(&path, true);
        let ctx = egui::Context::default();
        let _ = ctx.run(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: egui::Key::Escape,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ctx| {
                app.render_modal_viewers(ctx);
            },
        );
        assert!(app.viewer.is_none());
        assert!(app.image_load_job.is_none());
        app.poll_image_load(&ctx);
        assert!(app.viewer.is_none());
    }

    #[test]
    fn startup_image_loading_keeps_fifo_position_and_waits_for_foreground() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("image.png");
        write_test_image(&path, 4);
        let loading = ViewerSession::ImageLoading(ImageLoadJob::new(
            ImageLoadSource::File(path),
            AppConfig::default(),
        ));
        let mut app = test_workbench_app(
            Some(gallery_viewer()),
            VecDeque::from([loading, gallery_viewer()]),
        );
        app.poll_image_load(&egui::Context::default());
        assert!(matches!(
            app.viewer,
            Some(ViewerSession::ComponentGallery(_))
        ));
        assert_eq!(app.startup_viewers.len(), 2);
        app.viewer = app.startup_viewers.pop_front();
        wait_for_image_load(&mut app);
        assert!(matches!(app.viewer, Some(ViewerSession::Image(_))));
        assert_eq!(app.startup_viewers.len(), 1);
    }

    #[test]
    fn palette_prompt_escape_cancels_only_the_overlay() {
        let mut app = test_workbench_app(None, VecDeque::new());
        app.search_query = "preserve this Palette query".to_owned();
        app.selected_action_index = 3;
        app.in_app_prompt = Some(InAppPromptState {
            recipe_name: "Prompt Escape Fixture".to_owned(),
            message: "Enter test text".to_owned(),
            input_text: "unchanged input".to_owned(),
            first_frame: true,
        });
        let ctx = egui::Context::default();
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 700.0),
                )),
                events: vec![egui::Event::Key {
                    key: egui::Key::Escape,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ctx| {
                let modal_active = app.render_modal_viewers(ctx);
                assert!(modal_active, "the prompt owns the Escape frame");
                if !modal_active {
                    app.handle_global_shortcuts(ctx);
                }
            },
        );

        let close_commands = output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map(|output| {
                output
                    .commands
                    .iter()
                    .filter(|command| matches!(command, egui::ViewportCommand::Close))
                    .count()
            })
            .unwrap_or_default();
        assert_eq!(close_commands, 0, "Escape cancels only the prompt overlay");
        assert!(app.in_app_prompt.is_none());
        assert_eq!(app.view, WorkbenchView::Palette);
        assert_eq!(app.search_query, "preserve this Palette query");
        assert_eq!(app.selected_action_index, 3);
    }

    #[test]
    fn quick_palette_enter_executes_prompt_recipe_while_search_is_focused() {
        let mut app = test_workbench_app(None, VecDeque::new());
        let recipe_name = "Enter Prompt Fixture";
        let node = little_clip_ops::pipeline::types::NodeInstance {
            id: uuid::Uuid::new_v4(),
            kind: NodeKind::PromptOrHandText,
            title: "Prompt".to_owned(),
            pos: [0.0, 0.0],
            props: [
                ("hand".to_owned(), "right".to_owned()),
                ("prompt_message".to_owned(), "Enter test text".to_owned()),
            ]
            .into_iter()
            .collect(),
            inputs: Vec::new(),
            outputs: Vec::new(),
        };
        app.recipes.push(Recipe {
            name: recipe_name.to_owned(),
            description: String::new(),
            input_type: little_clip_ops::pipeline::types::RecipeInputType::None,
            tags: Vec::new(),
            graph: little_clip_ops::pipeline::types::NodeGraph {
                nodes: vec![node],
                ..Default::default()
            },
        });
        app.search_query = recipe_name.to_owned();

        let ctx = egui::Context::default();
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 700.0),
                )),
                events: vec![egui::Event::Key {
                    key: egui::Key::Enter,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let response = ui.add(egui::TextEdit::singleline(&mut app.search_query));
                    response.request_focus();
                    assert!(ctx.wants_keyboard_input());
                    app.handle_palette_nav_and_execution(ctx);
                });
            },
        );

        let prompt = app
            .in_app_prompt
            .as_ref()
            .expect("Enter executes the selected recipe");
        assert_eq!(prompt.recipe_name, recipe_name);
        assert_eq!(prompt.message, "Enter test text");
    }

    #[test]
    fn in_app_prompt_enter_submits_from_focused_editor_once() {
        let mut app = test_workbench_app(None, VecDeque::new());
        let recipe_name = "Prompt Enter Fixture";
        app.in_app_prompt = Some(InAppPromptState {
            recipe_name: recipe_name.to_owned(),
            message: "Enter test text".to_owned(),
            input_text: "submitted value".to_owned(),
            first_frame: true,
        });
        let ctx = egui::Context::default();
        let sent = std::cell::RefCell::new(Vec::new());
        let frame = |app: &mut WorkbenchApp, events| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(900.0, 700.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    assert!(app.render_prompt_modal_with(ctx, |request| {
                        sent.borrow_mut().push(request.clone());
                        Ok(IpcResponse::Success {
                            message: "fixture complete".to_owned(),
                        })
                    }));
                },
            )
        };

        frame(&mut app, Vec::new());
        assert!(
            ctx.wants_keyboard_input(),
            "prompt editor should receive focus"
        );
        assert!(
            app.in_app_prompt.is_some(),
            "opening the prompt must not close it"
        );

        let submitted = frame(
            &mut app,
            vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        let close_commands = submitted
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map(|output| {
                output
                    .commands
                    .iter()
                    .filter(|command| matches!(command, egui::ViewportCommand::Close))
                    .count()
            })
            .unwrap_or_default();
        assert_eq!(
            close_commands, 1,
            "Enter submits and requests one window close"
        );
        assert!(
            matches!(
                sent.borrow().as_slice(),
                [IpcRequest::ExecuteRecipeWithPrompt {
                    recipe_name: sent_recipe,
                    prompt_text: Some(text),
                }] if sent_recipe == recipe_name && text == "submitted value"
            ),
            "Enter must send the captured prompt exactly once"
        );
        assert!(
            app.in_app_prompt.is_some(),
            "submission should close the popup through its viewport command, not also dismiss prompt state"
        );
        assert_eq!(
            app.view,
            WorkbenchView::Palette,
            "successful prompt submission must retain its Palette route"
        );
    }

    #[test]
    fn shared_workbench_status_is_visible_in_every_host_view() {
        let palette = ui_appearance::palette(WorkbenchAppearance::Dark);
        for view in [
            WorkbenchView::Palette,
            WorkbenchView::NodeMachine,
            WorkbenchView::AutoActions,
        ] {
            let ctx = egui::Context::default();
            let mut status_height = 0.0;
            let mut hint_height = 0.0;
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(900.0, 700.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        status_height = show_workbench_footer(
                            ui,
                            view,
                            Some("Image preview failed: invalid PNG header"),
                            &palette,
                        )
                        .height();
                    });
                },
            );
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    hint_height = show_workbench_footer(ui, view, None, &palette).height();
                });
            });
            assert!(
                status_height > hint_height,
                "status was not reserved in {view:?}"
            );
        }
    }

    #[test]
    fn palette_search_focus_yields_to_audio_and_returns_on_activation() {
        fn render_field(
            ctx: &egui::Context,
            value: &mut String,
            needs_focus: &mut bool,
            audio_active: bool,
            activate_from_audio: bool,
        ) -> egui::Id {
            let mut field_id = None;
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(500.0, 180.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let mut field = egui::TextEdit::singleline(value);
                        if audio_active {
                            field = field.interactive(false);
                        }
                        let response = ui.add(field.desired_width(300.0));
                        field_id = Some(response.id);
                        update_palette_search_focus(
                            ctx,
                            &response,
                            needs_focus,
                            audio_active,
                            activate_from_audio,
                        );
                    });
                },
            );
            field_id.unwrap()
        }

        let ctx = egui::Context::default();
        ui_ext::configure_workbench_style(&ctx);
        let mut value = String::new();
        let mut needs_focus = true;
        let id = render_field(&ctx, &mut value, &mut needs_focus, false, false);
        assert_eq!(ctx.memory(|memory| memory.focused()), Some(id));

        render_field(&ctx, &mut value, &mut needs_focus, true, false);
        assert_eq!(ctx.memory(|memory| memory.focused()), None);

        render_field(&ctx, &mut value, &mut needs_focus, true, true);
        assert_eq!(ctx.memory(|memory| memory.focused()), Some(id));

        render_field(&ctx, &mut value, &mut needs_focus, false, false);
        assert_eq!(ctx.memory(|memory| memory.focused()), Some(id));
    }

    #[test]
    fn compact_palette_footer_reserves_wrapped_status_above_scrollable_results() {
        let ctx = egui::Context::default();
        ui_ext::configure_workbench_style(&ctx);
        let palette = ui_appearance::palette(WorkbenchAppearance::Dark);
        let status = "Action failed: a long diagnostic message that wraps across the compact footer while preserving the palette hint and the full error status.";
        let mut remaining_height = 0.0;
        let mut footer_rect = egui::Rect::NOTHING;
        let mut body_rect = egui::Rect::NOTHING;
        let mut results_content_height = 0.0;
        let mut results_viewport_height = 0.0;

        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1177.0, 662.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.allocate_space(egui::vec2(ui.available_width(), 480.0));
                    body_rect = ui.available_rect_before_wrap();
                    footer_rect =
                        show_workbench_footer(ui, WorkbenchView::Palette, Some(status), &palette);
                    remaining_height = ui.available_height();
                    let results = egui::ScrollArea::vertical()
                        .max_height(ui.available_height().max(0.0))
                        .show(ui, |ui| {
                            for result in 0..40 {
                                ui.label(format!("Result {result}: representative operation"));
                            }
                        });
                    results_content_height = results.content_size.y;
                    results_viewport_height = results.inner_rect.height();
                });
            },
        );

        assert!(remaining_height > 0.0);
        assert!(footer_rect.min.y > 0.0);
        assert!(
            footer_rect.max.y <= body_rect.max.y,
            "footer {footer_rect:?} exceeds body {body_rect:?}"
        );
        assert!(results_viewport_height > 0.0);
        assert!(results_content_height > results_viewport_height);
    }
}
