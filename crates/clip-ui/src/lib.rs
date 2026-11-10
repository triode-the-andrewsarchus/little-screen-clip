//! Shared desktop widgets, SVG icons, tabs, appearance and font setup.
//!
//! Application navigation, document sessions, native dialogs and audio-device
//! ownership belong to their hosts. This crate does not depend on either app
//! or on the raster/scene renderers.
pub mod buttons;
pub mod icons;
pub mod sliders;
pub mod ui_appearance;
pub mod ui_ext;
pub mod ui_tabs;

pub fn hex_to_c32(hex: &str) -> egui::Color32 {
    let (r, g, b, a) = little_clip_core::color::parse_hex_color_rgba_or_white(hex);
    egui::Color32::from_rgba_unmultiplied(r, g, b, a)
}

pub fn hex_to_c32_alpha(hex: &str, alpha: u8) -> egui::Color32 {
    let (r, g, b, _) = little_clip_core::color::parse_hex_color_rgba_or_white(hex);
    egui::Color32::from_rgba_unmultiplied(r, g, b, alpha)
}
