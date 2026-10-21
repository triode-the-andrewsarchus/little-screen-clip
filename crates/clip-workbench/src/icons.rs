//! Vector SVG icon system for `clip-workbench`.
//!
//! Provides resolution-independent vector icons embedded directly into the binary
//! that adapt to the fixed Light/Dark palette and contrast requirements via
//! `egui_extras` SVG loader and `egui::Image::tint`.

use eframe::egui::{self, Color32, Response, Ui, Vec2};

/// Enumeration of all vector SVG icons used across the workbench.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Icon {
    Audio,
    AudioWav,
    AudioFlac,
    AudioAiff,
    AudioAac,
    AudioOgg,
    AudioOga,
    AudioM4a,
    AudioWma,
    ClefBass,
    ClefAlto,
    ClefTreble,
    Image,
    Diff,
    BinaryDiff,
    Text,
    Pdf,
    Spreadsheet,
    Midi,
    Model3d,
    Microphone,
    NodeTree,
    Palette,
    Settings,
    Search,
    Book,
    Code,
    Play,
    Pause,
    Stop,
    Repeat,
    Volume,
    Save,
    Trash,
    Undo,
    Redo,
    Copy,
    Scissors,
    Crop,
    Pencil,
    Eye,
    Barcode,
    Wand,
    Sparkles,
    Film,
    Droplet,
    Shield,
    Brush,
    Eyedropper,
    Camera,
    Zap,
    Crosshair,
    Layers,
    Waves,
    ZoomIn,
    ZoomOut,
    ZoomFit,
    RotateCw,
    RotateCcw,
    FlipHorizontal,
    FlipVertical,
    Swap,
    ArrowUp,
    ArrowDown,
    Plus,
    Minus,
    Check,
    Close,
    Alert,
    Info,
    Folder,
    Lock,
    Unlock,
    Wrap,
    Link,
    Calendar,
    Pin,
    Map,
    Tv,
    Star,
    Grid,
    Flame,
    Resize,
    ArrowRight,
    ShapeCircle,
    ShapeRect,
    ShapeLine,
    Highlight,
    Badge,
    ArrowLeft,
    Bat,
    Teapot,
    Plug,
    Clipboard,
    Chart,
    Contrast,
    Tag,
    Dice,
    Tape,
    Snowflake,
    Signal,
    Clock,
    Globe,
    Mouse,
    Hourglass,
    Drive,
    Eject,
    Usb,
}

impl Icon {
    /// Returns the canonical asset filename (without .svg extension).
    #[inline]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Audio => "audio",
            Self::AudioWav => "audio_wav",
            Self::AudioFlac => "audio_flac",
            Self::AudioAiff => "audio_aiff",
            Self::AudioAac => "audio_aac",
            Self::AudioOgg => "audio_ogg",
            Self::AudioOga => "audio_oga",
            Self::AudioM4a => "audio_m4a",
            Self::AudioWma => "audio_wma",
            Self::ClefBass => "clef_bass",
            Self::ClefAlto => "clef_alto",
            Self::ClefTreble => "clef_treble",
            Self::Image => "image",
            Self::Diff => "diff",
            Self::BinaryDiff => "binary",
            Self::Text => "text",
            Self::Pdf => "pdf",
            Self::Spreadsheet => "spreadsheet",
            Self::Midi => "midi",
            Self::Model3d => "cube",
            Self::Microphone => "mic",
            Self::NodeTree => "node_tree",
            Self::Palette => "palette",
            Self::Settings => "settings",
            Self::Search => "search",
            Self::Book => "book",
            Self::Code => "code",
            Self::Play => "play",
            Self::Pause => "pause",
            Self::Stop => "stop",
            Self::Repeat => "repeat",
            Self::Volume => "volume",
            Self::Save => "save",
            Self::Trash => "trash",
            Self::Undo => "undo",
            Self::Redo => "redo",
            Self::Copy => "copy",
            Self::Scissors => "scissors",
            Self::Crop => "crop",
            Self::Pencil => "pencil",
            Self::Eye => "eye",
            Self::Barcode => "barcode",
            Self::Wand => "wand",
            Self::Sparkles => "sparkles",
            Self::Film => "film",
            Self::Droplet => "droplet",
            Self::Shield => "shield",
            Self::Brush => "brush",
            Self::Eyedropper => "eyedropper",
            Self::Camera => "camera",
            Self::Zap => "zap",
            Self::Crosshair => "crosshair",
            Self::Layers => "layers",
            Self::Waves => "waves",
            Self::ZoomIn => "zoom_in",
            Self::ZoomOut => "zoom_out",
            Self::ZoomFit => "zoom_fit",
            Self::RotateCw => "rotate_cw",
            Self::RotateCcw => "rotate_ccw",
            Self::FlipHorizontal => "flip_h",
            Self::FlipVertical => "flip_v",
            Self::Swap => "swap",
            Self::ArrowUp => "arrow_up",
            Self::ArrowDown => "arrow_down",
            Self::Plus => "plus",
            Self::Minus => "minus",
            Self::Check => "check",
            Self::Close => "x",
            Self::Alert => "alert",
            Self::Info => "info",
            Self::Folder => "folder",
            Self::Lock => "lock",
            Self::Unlock => "unlock",
            Self::Wrap => "wrap",
            Self::Link => "link",
            Self::Calendar => "calendar",
            Self::Pin => "pin",
            Self::Map => "map",
            Self::Tv => "tv",
            Self::Star => "sparkle_star",
            Self::Grid => "grid",
            Self::Flame => "flame",
            Self::Resize => "resize",
            Self::ArrowRight => "arrow_right",
            Self::ShapeCircle => "circle",
            Self::ShapeRect => "rect",
            Self::ShapeLine => "line",
            Self::Highlight => "highlight",
            Self::Badge => "badge",
            Self::ArrowLeft => "arrow_left",
            Self::Bat => "bat",
            Self::Teapot => "teapot",
            Self::Plug => "plug",
            Self::Clipboard => "clipboard",
            Self::Chart => "chart",
            Self::Contrast => "contrast",
            Self::Tag => "tag",
            Self::Dice => "dice",
            Self::Tape => "tape",
            Self::Snowflake => "snowflake",
            Self::Signal => "signal",
            Self::Clock => "clock",
            Self::Globe => "globe",
            Self::Mouse => "mouse",
            Self::Hourglass => "hourglass",
            Self::Drive => "drive",
            Self::Eject => "eject",
            Self::Usb => "usb",
        }
    }

    /// Returns the static bytes URI identifying this SVG resource in egui.
    #[inline]
    pub const fn uri(&self) -> &'static str {
        match self {
            Self::Audio => "bytes://icons/audio.svg",
            Self::AudioWav => "bytes://icons/audio_wav.svg",
            Self::AudioFlac => "bytes://icons/audio_flac.svg",
            Self::AudioAiff => "bytes://icons/audio_aiff.svg",
            Self::AudioAac => "bytes://icons/audio_aac.svg",
            Self::AudioOgg => "bytes://icons/audio_ogg.svg",
            Self::AudioOga => "bytes://icons/audio_oga.svg",
            Self::AudioM4a => "bytes://icons/audio_m4a.svg",
            Self::AudioWma => "bytes://icons/audio_wma.svg",
            Self::ClefBass => "bytes://icons/clef_bass.svg",
            Self::ClefAlto => "bytes://icons/clef_alto.svg",
            Self::ClefTreble => "bytes://icons/clef_treble.svg",
            Self::Image => "bytes://icons/image.svg",
            Self::Diff => "bytes://icons/diff.svg",
            Self::BinaryDiff => "bytes://icons/binary.svg",
            Self::Text => "bytes://icons/text.svg",
            Self::Pdf => "bytes://icons/pdf.svg",
            Self::Spreadsheet => "bytes://icons/spreadsheet.svg",
            Self::Midi => "bytes://icons/midi.svg",
            Self::Model3d => "bytes://icons/cube.svg",
            Self::Microphone => "bytes://icons/mic.svg",
            Self::NodeTree => "bytes://icons/node_tree.svg",
            Self::Palette => "bytes://icons/palette.svg",
            Self::Settings => "bytes://icons/settings.svg",
            Self::Search => "bytes://icons/search.svg",
            Self::Book => "bytes://icons/book.svg",
            Self::Code => "bytes://icons/code.svg",
            Self::Play => "bytes://icons/play.svg",
            Self::Pause => "bytes://icons/pause.svg",
            Self::Stop => "bytes://icons/stop.svg",
            Self::Repeat => "bytes://icons/repeat.svg",
            Self::Volume => "bytes://icons/volume.svg",
            Self::Save => "bytes://icons/save.svg",
            Self::Trash => "bytes://icons/trash.svg",
            Self::Undo => "bytes://icons/undo.svg",
            Self::Redo => "bytes://icons/redo.svg",
            Self::Copy => "bytes://icons/copy.svg",
            Self::Scissors => "bytes://icons/scissors.svg",
            Self::Crop => "bytes://icons/crop.svg",
            Self::Pencil => "bytes://icons/pencil.svg",
            Self::Eye => "bytes://icons/eye.svg",
            Self::Barcode => "bytes://icons/barcode.svg",
            Self::Wand => "bytes://icons/wand.svg",
            Self::Sparkles => "bytes://icons/sparkles.svg",
            Self::Film => "bytes://icons/film.svg",
            Self::Droplet => "bytes://icons/droplet.svg",
            Self::Shield => "bytes://icons/shield.svg",
            Self::Brush => "bytes://icons/brush.svg",
            Self::Eyedropper => "bytes://icons/eyedropper.svg",
            Self::Camera => "bytes://icons/camera.svg",
            Self::Zap => "bytes://icons/zap.svg",
            Self::Crosshair => "bytes://icons/crosshair.svg",
            Self::Layers => "bytes://icons/layers.svg",
            Self::Waves => "bytes://icons/waves.svg",
            Self::ZoomIn => "bytes://icons/zoom_in.svg",
            Self::ZoomOut => "bytes://icons/zoom_out.svg",
            Self::ZoomFit => "bytes://icons/zoom_fit.svg",
            Self::RotateCw => "bytes://icons/rotate_cw.svg",
            Self::RotateCcw => "bytes://icons/rotate_ccw.svg",
            Self::FlipHorizontal => "bytes://icons/flip_h.svg",
            Self::FlipVertical => "bytes://icons/flip_v.svg",
            Self::Swap => "bytes://icons/swap.svg",
            Self::ArrowUp => "bytes://icons/arrow_up.svg",
            Self::ArrowDown => "bytes://icons/arrow_down.svg",
            Self::Plus => "bytes://icons/plus.svg",
            Self::Minus => "bytes://icons/minus.svg",
            Self::Check => "bytes://icons/check.svg",
            Self::Close => "bytes://icons/x.svg",
            Self::Alert => "bytes://icons/alert.svg",
            Self::Info => "bytes://icons/info.svg",
            Self::Folder => "bytes://icons/folder.svg",
            Self::Lock => "bytes://icons/lock.svg",
            Self::Unlock => "bytes://icons/unlock.svg",
            Self::Wrap => "bytes://icons/wrap.svg",
            Self::Link => "bytes://icons/link.svg",
            Self::Calendar => "bytes://icons/calendar.svg",
            Self::Pin => "bytes://icons/pin.svg",
            Self::Map => "bytes://icons/map.svg",
            Self::Tv => "bytes://icons/tv.svg",
            Self::Star => "bytes://icons/sparkle_star.svg",
            Self::Grid => "bytes://icons/grid.svg",
            Self::Flame => "bytes://icons/flame.svg",
            Self::Resize => "bytes://icons/resize.svg",
            Self::ArrowRight => "bytes://icons/arrow_right.svg",
            Self::ShapeCircle => "bytes://icons/circle.svg",
            Self::ShapeRect => "bytes://icons/rect.svg",
            Self::ShapeLine => "bytes://icons/line.svg",
            Self::Highlight => "bytes://icons/highlight.svg",
            Self::Badge => "bytes://icons/badge.svg",
            Self::ArrowLeft => "bytes://icons/arrow_left.svg",
            Self::Bat => "bytes://icons/bat.svg",
            Self::Teapot => "bytes://icons/teapot.svg",
            Self::Plug => "bytes://icons/plug.svg",
            Self::Clipboard => "bytes://icons/clipboard.svg",
            Self::Chart => "bytes://icons/chart.svg",
            Self::Contrast => "bytes://icons/contrast.svg",
            Self::Tag => "bytes://icons/tag.svg",
            Self::Dice => "bytes://icons/dice.svg",
            Self::Tape => "bytes://icons/tape.svg",
            Self::Snowflake => "bytes://icons/snowflake.svg",
            Self::Signal => "bytes://icons/signal.svg",
            Self::Clock => "bytes://icons/clock.svg",
            Self::Globe => "bytes://icons/globe.svg",
            Self::Mouse => "bytes://icons/mouse.svg",
            Self::Hourglass => "bytes://icons/hourglass.svg",
            Self::Drive => "bytes://icons/drive.svg",
            Self::Eject => "bytes://icons/eject.svg",
            Self::Usb => "bytes://icons/usb.svg",
        }
    }

    /// Returns the raw embedded SVG file bytes.
    #[inline]
    pub const fn svg_bytes(&self) -> &'static [u8] {
        match self {
            Self::Audio => include_bytes!("../assets/icons/audio.svg"),
            Self::AudioWav => include_bytes!("../assets/icons/audio_wav.svg"),
            Self::AudioFlac => include_bytes!("../assets/icons/audio_flac.svg"),
            Self::AudioAiff => include_bytes!("../assets/icons/audio_aiff.svg"),
            Self::AudioAac => include_bytes!("../assets/icons/audio_aac.svg"),
            Self::AudioOgg => include_bytes!("../assets/icons/audio_ogg.svg"),
            Self::AudioOga => include_bytes!("../assets/icons/audio_oga.svg"),
            Self::AudioM4a => include_bytes!("../assets/icons/audio_m4a.svg"),
            Self::AudioWma => include_bytes!("../assets/icons/audio_wma.svg"),
            Self::ClefBass => include_bytes!("../assets/icons/clef_bass.svg"),
            Self::ClefAlto => include_bytes!("../assets/icons/clef_alto.svg"),
            Self::ClefTreble => include_bytes!("../assets/icons/clef_treble.svg"),
            Self::Image => include_bytes!("../assets/icons/image.svg"),
            Self::Diff => include_bytes!("../assets/icons/diff.svg"),
            Self::BinaryDiff => include_bytes!("../assets/icons/binary.svg"),
            Self::Text => include_bytes!("../assets/icons/text.svg"),
            Self::Pdf => include_bytes!("../assets/icons/pdf.svg"),
            Self::Spreadsheet => include_bytes!("../assets/icons/spreadsheet.svg"),
            Self::Midi => include_bytes!("../assets/icons/midi.svg"),
            Self::Model3d => include_bytes!("../assets/icons/cube.svg"),
            Self::Microphone => include_bytes!("../assets/icons/mic.svg"),
            Self::NodeTree => include_bytes!("../assets/icons/node_tree.svg"),
            Self::Palette => include_bytes!("../assets/icons/palette.svg"),
            Self::Settings => include_bytes!("../assets/icons/settings.svg"),
            Self::Search => include_bytes!("../assets/icons/search.svg"),
            Self::Book => include_bytes!("../assets/icons/book.svg"),
            Self::Code => include_bytes!("../assets/icons/code.svg"),
            Self::Play => include_bytes!("../../little-clip-render/assets/icons/play.svg"),
            Self::Pause => include_bytes!("../assets/icons/pause.svg"),
            Self::Stop => include_bytes!("../assets/icons/stop.svg"),
            Self::Repeat => include_bytes!("../assets/icons/repeat.svg"),
            Self::Volume => include_bytes!("../assets/icons/volume.svg"),
            Self::Save => include_bytes!("../assets/icons/save.svg"),
            Self::Trash => include_bytes!("../assets/icons/trash.svg"),
            Self::Undo => include_bytes!("../assets/icons/undo.svg"),
            Self::Redo => include_bytes!("../assets/icons/redo.svg"),
            Self::Copy => include_bytes!("../assets/icons/copy.svg"),
            Self::Scissors => include_bytes!("../assets/icons/scissors.svg"),
            Self::Crop => include_bytes!("../assets/icons/crop.svg"),
            Self::Pencil => include_bytes!("../assets/icons/pencil.svg"),
            Self::Eye => include_bytes!("../assets/icons/eye.svg"),
            Self::Barcode => include_bytes!("../assets/icons/barcode.svg"),
            Self::Wand => include_bytes!("../assets/icons/wand.svg"),
            Self::Sparkles => include_bytes!("../assets/icons/sparkles.svg"),
            Self::Film => include_bytes!("../assets/icons/film.svg"),
            Self::Droplet => include_bytes!("../assets/icons/droplet.svg"),
            Self::Shield => include_bytes!("../assets/icons/shield.svg"),
            Self::Brush => include_bytes!("../assets/icons/brush.svg"),
            Self::Eyedropper => include_bytes!("../assets/icons/eyedropper.svg"),
            Self::Camera => include_bytes!("../assets/icons/camera.svg"),
            Self::Zap => include_bytes!("../assets/icons/zap.svg"),
            Self::Crosshair => include_bytes!("../assets/icons/crosshair.svg"),
            Self::Layers => include_bytes!("../assets/icons/layers.svg"),
            Self::Waves => include_bytes!("../assets/icons/waves.svg"),
            Self::ZoomIn => include_bytes!("../assets/icons/zoom_in.svg"),
            Self::ZoomOut => include_bytes!("../assets/icons/zoom_out.svg"),
            Self::ZoomFit => include_bytes!("../assets/icons/zoom_fit.svg"),
            Self::RotateCw => include_bytes!("../assets/icons/rotate_cw.svg"),
            Self::RotateCcw => include_bytes!("../assets/icons/rotate_ccw.svg"),
            Self::FlipHorizontal => include_bytes!("../assets/icons/flip_h.svg"),
            Self::FlipVertical => include_bytes!("../assets/icons/flip_v.svg"),
            Self::Swap => include_bytes!("../assets/icons/swap.svg"),
            Self::ArrowUp => include_bytes!("../assets/icons/arrow_up.svg"),
            Self::ArrowDown => include_bytes!("../assets/icons/arrow_down.svg"),
            Self::Plus => include_bytes!("../assets/icons/plus.svg"),
            Self::Minus => include_bytes!("../assets/icons/minus.svg"),
            Self::Check => include_bytes!("../assets/icons/check.svg"),
            Self::Close => include_bytes!("../assets/icons/x.svg"),
            Self::Alert => include_bytes!("../assets/icons/alert.svg"),
            Self::Info => include_bytes!("../assets/icons/info.svg"),
            Self::Folder => include_bytes!("../assets/icons/folder.svg"),
            Self::Lock => include_bytes!("../assets/icons/lock.svg"),
            Self::Unlock => include_bytes!("../assets/icons/unlock.svg"),
            Self::Wrap => include_bytes!("../assets/icons/wrap.svg"),
            Self::Link => include_bytes!("../assets/icons/link.svg"),
            Self::Calendar => include_bytes!("../assets/icons/calendar.svg"),
            Self::Pin => include_bytes!("../assets/icons/pin.svg"),
            Self::Map => include_bytes!("../assets/icons/map.svg"),
            Self::Tv => include_bytes!("../assets/icons/tv.svg"),
            Self::Star => include_bytes!("../assets/icons/sparkle_star.svg"),
            Self::Grid => include_bytes!("../assets/icons/grid.svg"),
            Self::Flame => include_bytes!("../assets/icons/flame.svg"),
            Self::Resize => include_bytes!("../assets/icons/resize.svg"),
            Self::ArrowRight => include_bytes!("../assets/icons/arrow_right.svg"),
            Self::ShapeCircle => include_bytes!("../assets/icons/circle.svg"),
            Self::ShapeRect => include_bytes!("../assets/icons/rect.svg"),
            Self::ShapeLine => include_bytes!("../assets/icons/line.svg"),
            Self::Highlight => include_bytes!("../assets/icons/highlight.svg"),
            Self::Badge => include_bytes!("../assets/icons/badge.svg"),
            Self::ArrowLeft => include_bytes!("../assets/icons/arrow_left.svg"),
            Self::Bat => include_bytes!("../assets/icons/bat.svg"),
            Self::Teapot => include_bytes!("../assets/icons/teapot.svg"),
            Self::Plug => include_bytes!("../assets/icons/plug.svg"),
            Self::Clipboard => include_bytes!("../assets/icons/clipboard.svg"),
            Self::Chart => include_bytes!("../assets/icons/chart.svg"),
            Self::Contrast => include_bytes!("../assets/icons/contrast.svg"),
            Self::Tag => include_bytes!("../assets/icons/tag.svg"),
            Self::Dice => include_bytes!("../assets/icons/dice.svg"),
            Self::Tape => include_bytes!("../assets/icons/tape.svg"),
            Self::Snowflake => include_bytes!("../assets/icons/snowflake.svg"),
            Self::Signal => include_bytes!("../assets/icons/signal.svg"),
            Self::Clock => include_bytes!("../assets/icons/clock.svg"),
            Self::Globe => include_bytes!("../assets/icons/globe.svg"),
            Self::Mouse => include_bytes!("../assets/icons/mouse.svg"),
            Self::Hourglass => include_bytes!("../assets/icons/hourglass.svg"),
            Self::Drive => include_bytes!("../assets/icons/drive.svg"),
            Self::Eject => include_bytes!("../assets/icons/eject.svg"),
            Self::Usb => include_bytes!("../assets/icons/usb.svg"),
        }
    }

    /// Creates an `egui::Image` with the exact square dimension.
    pub fn image(&self, size: f32) -> egui::Image<'static> {
        egui::Image::from_bytes(self.uri(), self.svg_bytes()).fit_to_exact_size(Vec2::splat(size))
    }

    /// Creates an `egui::Image` with exact square dimension and color tint.
    pub fn image_tinted(&self, size: f32, tint: Color32) -> egui::Image<'static> {
        self.image(size).tint(tint)
    }

    /// Directly renders the tinted icon onto `ui`.
    pub fn ui(&self, ui: &mut Ui, size: f32, tint: Color32) -> Response {
        ui.add(self.image_tinted(size, tint))
    }

    /// Paints an SVG into an existing canvas without creating a widget.
    pub fn paint(self, painter: &egui::Painter, rect: egui::Rect, color: Color32) {
        // SVGs are rasterized by egui_extras before the renderer scales the
        // texture. Request physical pixels here so high-DPI displays do not
        // enlarge a logical-point-sized raster and soften the icon edges.
        let pixels_per_point = painter.ctx().pixels_per_point();
        let raster_size = rect.size() * pixels_per_point;
        match self
            .image(rect.width() * pixels_per_point)
            .load_for_size(painter.ctx(), raster_size)
        {
            Ok(egui::load::TexturePoll::Ready { texture }) => {
                painter.image(
                    texture.id,
                    rect,
                    egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                    color,
                );
            }
            Ok(egui::load::TexturePoll::Pending { .. }) => painter.ctx().request_repaint(),
            Err(error) => tracing::warn!(?self, %error, "Unable to load SVG icon"),
        }
    }

    /// Paints an SVG beside canvas text, aligning the complete group at `pos`.
    #[allow(clippy::too_many_arguments)]
    pub fn paint_text(
        self,
        painter: &egui::Painter,
        pos: egui::Pos2,
        align: egui::Align2,
        text: impl ToString,
        font: egui::FontId,
        color: Color32,
    ) {
        let size = font.size;
        let galley = painter.layout_no_wrap(text.to_string(), font, color);
        let gap = crate::ui_ext::SPACE_XS;
        let bounds = align.anchor_rect(egui::Rect::from_min_size(
            pos,
            Vec2::new(size + gap + galley.size().x, size.max(galley.size().y)),
        ));
        self.paint(
            painter,
            egui::Rect::from_center_size(
                egui::pos2(bounds.min.x + size * 0.5, bounds.center().y),
                Vec2::splat(size),
            ),
            color,
        );
        painter.galley(
            egui::pos2(
                bounds.min.x + size + gap,
                bounds.center().y - galley.size().y * 0.5,
            ),
            galley,
            color,
        );
    }

    /// A label with a vector icon, keeping the caller's text styling.
    pub fn label(self, text: impl Into<egui::WidgetText>) -> impl egui::Widget {
        let text = text.into();
        move |ui: &mut Ui| {
            ui.horizontal(|ui| {
                let job = text.clone().into_layout_job(
                    ui.style(),
                    egui::TextStyle::Body.into(),
                    egui::Align::Center,
                );
                let tint = job
                    .sections
                    .first()
                    .map(|section| section.format.color)
                    .filter(|color| *color != Color32::PLACEHOLDER)
                    .unwrap_or_else(|| ui.visuals().text_color());
                self.ui(ui, crate::ui_ext::CONTROL_ICON_SIZE, tint);
                ui.label(text)
            })
            .inner
        }
    }

    /// Slice of all available icon variants for testing and validation.
    pub const ALL: &[Icon] = &[
        Self::Audio,
        Self::Image,
        Self::Diff,
        Self::BinaryDiff,
        Self::Text,
        Self::Pdf,
        Self::Spreadsheet,
        Self::Midi,
        Self::Model3d,
        Self::Microphone,
        Self::NodeTree,
        Self::Palette,
        Self::Settings,
        Self::Search,
        Self::Book,
        Self::Code,
        Self::Play,
        Self::Pause,
        Self::Stop,
        Self::Repeat,
        Self::Volume,
        Self::Save,
        Self::Trash,
        Self::Undo,
        Self::Redo,
        Self::Copy,
        Self::Scissors,
        Self::Crop,
        Self::Pencil,
        Self::Eye,
        Self::Barcode,
        Self::Wand,
        Self::Sparkles,
        Self::Film,
        Self::Droplet,
        Self::Shield,
        Self::Brush,
        Self::Eyedropper,
        Self::Camera,
        Self::Zap,
        Self::Crosshair,
        Self::Layers,
        Self::Waves,
        Self::ZoomIn,
        Self::ZoomOut,
        Self::ZoomFit,
        Self::RotateCw,
        Self::RotateCcw,
        Self::FlipHorizontal,
        Self::FlipVertical,
        Self::Swap,
        Self::ArrowUp,
        Self::ArrowDown,
        Self::Plus,
        Self::Minus,
        Self::Check,
        Self::Close,
        Self::Alert,
        Self::Info,
        Self::Folder,
        Self::Lock,
        Self::Unlock,
        Self::Wrap,
        Self::Link,
        Self::Calendar,
        Self::Pin,
        Self::Map,
        Self::Tv,
        Self::Star,
        Self::Grid,
        Self::Flame,
        Self::Resize,
        Self::ArrowRight,
        Self::ShapeCircle,
        Self::ShapeRect,
        Self::ShapeLine,
        Self::Highlight,
        Self::Badge,
        Self::ArrowLeft,
        Self::Bat,
        Self::Teapot,
        Self::Plug,
        Self::Clipboard,
        Self::Chart,
        Self::Contrast,
        Self::Tag,
        Self::Dice,
        Self::Tape,
        Self::Snowflake,
        Self::Signal,
        Self::Clock,
        Self::Globe,
        Self::Mouse,
        Self::Hourglass,
        Self::Drive,
        Self::Eject,
        Self::Usb,
    ];
}

/// A parameter label with an optional SVG icon.
pub struct IconLabel {
    icon: Option<Icon>,
    text: String,
}

impl IconLabel {
    pub fn show(self, ui: &mut Ui, color: Color32) {
        let text = egui::RichText::new(self.text).color(color);
        if let Some(icon) = self.icon {
            ui.add(icon.label(text));
        } else {
            ui.label(text);
        }
    }
}

impl From<&str> for IconLabel {
    fn from(text: &str) -> Self {
        Self {
            icon: None,
            text: text.into(),
        }
    }
}

impl From<String> for IconLabel {
    fn from(text: String) -> Self {
        Self { icon: None, text }
    }
}

impl From<&String> for IconLabel {
    fn from(text: &String) -> Self {
        Self::from(text.as_str())
    }
}

impl<T: Into<String>> From<(Icon, T)> for IconLabel {
    fn from((icon, text): (Icon, T)) -> Self {
        Self {
            icon: Some(icon),
            text: text.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_icons_load_valid_svg_bytes() {
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);

        for icon in Icon::ALL {
            let bytes = icon.svg_bytes();
            assert!(!bytes.is_empty(), "Icon {:?} has empty bytes", icon);
            assert!(
                bytes.starts_with(b"<svg"),
                "Icon {:?} does not start with <svg tag",
                icon
            );

            let img = icon.image_tinted(16.0, Color32::WHITE);
            let res = img.load_for_size(&ctx, Vec2::splat(16.0));
            assert!(
                res.is_ok(),
                "Failed to rasterize icon {:?}: {:?}",
                icon,
                res.err()
            );
        }
    }
}
