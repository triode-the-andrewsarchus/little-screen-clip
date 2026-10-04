//! Little Screen Clip desktop GUI workbench (`clip-workbench`).
//!
//! An egui-based control center providing multi-slot visual inspection, operation command palette,
//! visual node graph builder, audio/image/video/3D/PDF/MIDI inspection windows, and live screen mirror.

pub(crate) use clip_workbench::{hex_to_c32, hex_to_c32_alpha};
use eframe::egui;
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

mod workbench;
use workbench::{ClipboardSelection, MirrorJob, PaletteOpsJob};

mod ansi;
mod appearance_sync;
mod audio_preview;
mod barcode_worker;
mod binary_diff_view;
mod canvas;
mod component_gallery;
mod conversion_decode_worker;
mod diff_view;
pub use clip_workbench::icons;
mod image_preview;
mod midi_view;
mod model3d_view;
mod pdf_view;
mod popup;
mod rolling_audio_strip;
mod script_editor;
pub mod shell;
mod splat_view;
mod spreadsheet_view;
mod state_sync;
mod text_preview;
use clip_workbench::ui_appearance;
pub use clip_workbench::{ui_ext, ui_tabs};

use appearance_sync::AppearanceSync;
use icons::Icon;

use audio_preview::AudioPreviewState;
use binary_diff_view::BinaryDiffViewState;
use canvas::*;
use diff_view::DiffViewerState;
use image_preview::{DecodedImageData, ImagePreviewState};
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
use text_preview::{TextPreviewData, TextPreviewState};
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
    Splat(splat_view::SplatViewerState),
    Text(TextPreviewState),
    ComponentGallery(component_gallery::ComponentGalleryState),
}

fn scene_file_viewer(path: PathBuf, config: &AppConfig) -> ViewerSession {
    if little_clip_scene::is_splat_file(&path) {
        ViewerSession::Splat(
            splat_view::SplatViewerState::from_file(path).with_config(config.model3d.clone()),
        )
    } else {
        ViewerSession::Model3d(
            Model3dViewerState::from_file(path).with_config(config.model3d.clone()),
        )
    }
}

fn gallery_request_to_viewer_session(
    request: component_gallery::GalleryOpenRequest,
    config: &AppConfig,
) -> ViewerSession {
    match request {
        component_gallery::GalleryOpenRequest::Image(source) => {
            ViewerSession::Image(ImagePreviewState::from_decoded(
                source.as_ref().clone(),
                None,
                "Gallery image".to_owned(),
                config,
            ))
        }
        component_gallery::GalleryOpenRequest::Text(source) => ViewerSession::Text(
            TextPreviewState::from_data(source, Some("Gallery text".to_owned()), config),
        ),
        component_gallery::GalleryOpenRequest::Spreadsheet(source) => {
            ViewerSession::Spreadsheet(SpreadsheetViewerState::from_shared(source, None))
        }
        component_gallery::GalleryOpenRequest::Midi(source) => ViewerSession::Midi(
            MidiViewerState::from_shared(source, None, "Gallery MIDI".to_owned()),
        ),
        component_gallery::GalleryOpenRequest::Model3d(source) => {
            ViewerSession::Model3d(Model3dViewerState::from_shared_model(source, None))
        }
        component_gallery::GalleryOpenRequest::Audio(source, path, file_guard) => {
            ViewerSession::Audio(AudioPreviewState::from_completed_source_with_file_guard(
                path,
                "Gallery audio".to_owned(),
                None,
                source,
                file_guard,
            ))
        }
        component_gallery::GalleryOpenRequest::Pdf(path, raster) => {
            ViewerSession::Pdf(PdfEditorState::from_shared_page_raster(path, raster))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ImageSessionKey {
    ClipboardItem(uuid::Uuid),
    File {
        path: PathBuf,
        len: u64,
        modified_ns: u128,
    },
}

impl ImageSessionKey {
    fn file(path: &std::path::Path) -> Option<Self> {
        let metadata = std::fs::metadata(path).ok()?;
        let modified_ns = metadata
            .modified()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_nanos();
        Some(Self::File {
            path: path.to_path_buf(),
            len: metadata.len(),
            modified_ns,
        })
    }
}

fn show_viewer_panel(ctx: &egui::Context, contents: impl FnOnce(&mut egui::Ui)) {
    egui::CentralPanel::default()
        .frame(egui::Frame::none().fill(ctx.style().visuals.panel_fill))
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

    fn decode(self) -> Result<ImageLoadResult, String> {
        match self {
            Self::File(path) => {
                let title = image_title_for_path(&path);
                let data = DecodedImageData::from_path(&path)?;
                Ok(ImageLoadResult {
                    data,
                    path: Some(path),
                    title,
                })
            }
            Self::ItemFile { path, mime, title } => {
                let bytes = std::fs::read(&path)
                    .map_err(|error| format!("Failed to read file {}: {error}", path.display()))?;
                let data = DecodedImageData::from_bytes(bytes, mime, Some(path.clone()))?;
                Ok(ImageLoadResult {
                    data,
                    path: Some(path),
                    title,
                })
            }
            Self::Bytes {
                bytes,
                mime,
                path,
                title,
            } => {
                let data = DecodedImageData::from_bytes(bytes, mime, path.clone())?;
                Ok(ImageLoadResult { data, path, title })
            }
        }
    }
}

struct ImageLoadResult {
    data: DecodedImageData,
    path: Option<PathBuf>,
    title: String,
}

fn image_title_for_path(path: &std::path::Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Image Preview".to_string())
}

struct ImageLoadJob {
    key: Option<ImageSessionKey>,
    ticket: JobTicket,
    receiver: Receiver<Result<Result<ImageLoadResult, String>, JobError>>,
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
    fn new(source: ImageLoadSource) -> Self {
        let key = source.session_key();
        Self::new_with_key(source, key)
    }

    fn new_with_key(source: ImageLoadSource, key: Option<ImageSessionKey>) -> Self {
        let (ticket, receiver) =
            BackgroundJobs::global().submit(Duration::from_secs(30), move || source.decode());
        Self {
            key,
            ticket,
            receiver,
        }
    }
}

impl ImageLoadSource {
    fn session_key(&self) -> Option<ImageSessionKey> {
        match self {
            Self::File(path) | Self::ItemFile { path, .. } => ImageSessionKey::file(path),
            Self::Bytes { path, .. } => path.as_deref().and_then(ImageSessionKey::file),
        }
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
                let data = TextPreviewData::from_text(content);
                Ok(TextPreviewState::from_data(data, Some(title), &config))
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
    state: Arc<SlotManager>,
    state_rx: Receiver<Result<Arc<SlotManager>, String>>,
    state_loaded: bool,
    startup_comparison: Option<bool>,
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
    renderer: Option<ScreenRenderer>,
    mirror_job: Option<MirrorJob>,
    mirror_error: Option<String>,
    mirror_animated: bool,
    palette_ops: Option<(ClipboardSelection, Vec<OpDescriptor>)>,
    palette_ops_job: Option<PaletteOpsJob>,
    ui_palette: AppPalette,
    mirror_texture: Option<egui::TextureHandle>,
    dirty_mirror: bool,
    in_app_prompt: Option<InAppPromptState>,
    viewer: Option<ViewerSession>,
    active_image_key: Option<ImageSessionKey>,
    retained_image_session: Option<(ImageSessionKey, ImagePreviewState)>,
    pending_image_close: bool,
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
) -> crate::ui_ext::StyledButton<'a> {
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
        let card_bg = hex_to_c32(&palette.colors.surface_raised);
        let card_stroke = hex_to_c32(&palette.chrome_border);
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
        println!("      --scene <PATH>   Open a read-only 3D scene viewer directly");
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
    let initial_scene = args
        .iter()
        .position(|argument| argument == "--scene")
        .and_then(|index| args.get(index + 1))
        .map(PathBuf::from);

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
                cc.egui_ctx.clone(),
                is_diff,
                is_binary_diff,
                initial_audio,
                initial_image,
                initial_pdf,
                initial_midi,
                initial_scene,
                open_component_gallery,
            ))
        }),
    )
}

#[cfg(test)]
#[path = "workbench/tests/mod.rs"]
mod workbench_layout_tests;
