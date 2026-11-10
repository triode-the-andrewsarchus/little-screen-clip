//! Read-only source layer exploration over a document canvas.
use super::*;

pub(super) struct Document {
    summary: little_clip_core::image_document::Summary,
    preview: little_clip_core::ImageThumbnail,
    stored_composite: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(
        ctx: &egui::Context,
        state: &mut ImagePreviewState,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 700.0))),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.set_max_width(240.0);
                    state.render_view_toolbar(ui, ctx, &AppPalette::default());
                });
            },
        )
    }

    #[test]
    fn selecting_source_layers_highlights_bounds_without_editing_pixels_or_history() {
        for (bytes, has_bounds) in [
            (
                include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../little-clip-core/tests/fixtures/layers.psd"
                ))
                .as_slice(),
                true,
            ),
            (
                include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../little-clip-core/tests/fixtures/layered.kra"
                ))
                .as_slice(),
                false,
            ),
        ] {
            let mut state = ImagePreviewState::from_bytes_with_meta(
                bytes.to_vec(),
                "image/vnd.adobe.photoshop".into(),
                None,
                "layers.psd".into(),
                &AppConfig::default(),
            )
            .unwrap();
            assert!(state.has_view_metadata());
            let preview = &state.layer_inspector.document.as_ref().unwrap().preview;
            assert_eq!((preview.width, preview.height), (64, 32));
            let pixels = state.current_rgba.clone();
            let stamp = state.input_stamp();
            let ctx = egui::Context::default();
            frame(&ctx, &mut state, Vec::new());
            let output = frame(&ctx, &mut state, Vec::new());
            let position = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.text() == "Top patch" => {
                        Some(text.pos + text.galley.size() * 0.5)
                    }
                    _ => None,
                })
                .expect("source layer list");
            for pressed in [true, false] {
                let event = egui::Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                };
                frame(
                    &ctx,
                    &mut state,
                    vec![egui::Event::PointerMoved(position), event],
                );
            }
            assert_eq!(state.layer_inspector.selected, Some(1));
            let output = frame(&ctx, &mut state, Vec::new());
            let detail = if has_bounds {
                "Raster mask"
            } else {
                "Source offset (32, 0)"
            };
            assert!(output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text().contains(detail))));
            if has_bounds {
                assert!(output.shapes.iter().any(
            |shape| matches!(&shape.shape, egui::Shape::Rect(rect) if rect.stroke.width == 2.0)
        ));
            }
            state.layer_inspector.selected = Some(0);
            let output = frame(&ctx, &mut state, Vec::new());
            assert!(output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text() == "Layer group")));
            assert_eq!(state.input_stamp(), stamp);
            assert!(Arc::ptr_eq(&pixels, &state.current_rgba));
            assert!(state.undo_stack.is_empty());
        }
    }

    #[test]
    fn xcf_source_inspection_preserves_pixels_and_editor_history() {
        let path = std::path::PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../little-clip-core/tests/fixtures/layered.xcf"
        ));
        let mut state = ImagePreviewState::from_path(path, &AppConfig::default()).unwrap();
        let pixels = state.current_rgba.clone();
        let stamp = state.input_stamp();
        state.layer_inspector.selected = Some(2);
        let ctx = egui::Context::default();
        frame(&ctx, &mut state, Vec::new());
        let output = frame(&ctx, &mut state, Vec::new());
        assert!(output.shapes.iter().any(|shape|matches!(&shape.shape,egui::Shape::Text(text) if text.galley.text()=="Rendered canvas")));
        assert!(output.shapes.iter().any(|shape|matches!(&shape.shape,egui::Shape::Text(text) if text.galley.text().contains("Raster mask"))));
        assert_eq!(state.input_stamp(), stamp);
        assert!(Arc::ptr_eq(&pixels, &state.current_rgba));
        assert!(state.undo_stack.is_empty());
    }
    #[test]
    fn composite_only_document_has_no_layer_selection_prompt() {
        let path = std::path::PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../little-clip-core/tests/fixtures/composite-16-3.psd"
        ));
        let mut state = ImagePreviewState::from_path(path, &AppConfig::default()).unwrap();
        let ctx = egui::Context::default();
        frame(&ctx, &mut state, Vec::new());
        let output = frame(&ctx, &mut state, Vec::new());
        assert!(output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text() == "No stored layers")));
        assert!(!output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text().contains("Select a layer"))));
    }
}
impl Document {
    pub(super) fn from_bytes(bytes: &[u8], image: &RgbaImage) -> Result<Option<Arc<Self>>, String> {
        let summary = if bytes.starts_with(b"8BPS") {
            little_clip_core::psd_image::inspect(bytes)?
        } else if bytes.starts_with(b"gimp xcf ") {
            little_clip_core::xcf_image::inspect(bytes)?
        } else if bytes.starts_with(b"PK\x03\x04") {
            little_clip_core::kra_image::inspect(bytes)?
        } else {
            return Ok(None);
        };
        let scale = (240.0 / image.width() as f32)
            .min(160.0 / image.height() as f32)
            .min(1.0);
        if summary.dimensions != image.dimensions() {
            return Err("Saved composite dimensions differ from the source canvas".into());
        }
        let width = (image.width() as f32 * scale).round().max(1.0) as u32;
        let height = (image.height() as f32 * scale).round().max(1.0) as u32;
        let preview = little_clip_core::ImageThumbnail::from_rgba(image::imageops::thumbnail(
            image, width, height,
        ));
        Ok(Some(Arc::new(Self {
            summary,
            preview,
            stored_composite: !bytes.starts_with(b"gimp xcf "),
        })))
    }
}

#[derive(Default)]
pub(super) struct Inspector {
    document: Option<Arc<Document>>,
    selected: Option<usize>,
    texture: Option<egui::TextureHandle>,
}
impl Inspector {
    pub(super) fn new(document: Option<Arc<Document>>) -> Self {
        Self {
            document,
            ..Default::default()
        }
    }
    pub(super) fn has_document(&self) -> bool {
        self.document.is_some()
    }
    pub(super) fn ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, palette: &AppPalette) {
        let Some(document) = self.document.clone() else {
            return;
        };
        let summary = &document.summary;
        ui.label(
            egui::RichText::new(if document.stored_composite {
                "Saved composite"
            } else {
                "Rendered canvas"
            })
            .strong(),
        );
        ui.label(format!(
            "{} × {} · {}{}",
            summary.dimensions.0,
            summary.dimensions.1,
            summary
                .depth
                .map(|bits| format!("{bits} bit · "))
                .unwrap_or_default(),
            summary.color_mode.to_uppercase()
        ))
        .on_hover_text("Original source color space. Editing and raster export use 8-bit RGBA.");
        if let Some(profile) = &summary.profile {
            ui.add(egui::Label::new(profile).truncate(true))
                .on_hover_text(profile);
        }
        self.overview(ui, ctx, palette, &document);
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), 64.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_min_height(64.0);
                self.properties(ui, summary);
            },
        );
        ui.label(egui::RichText::new(format!("Source layers · {}", summary.layers.len())).strong());
        ui.scope(|ui| {
            ui.spacing_mut().interact_size.y = 22.0;
            ui.spacing_mut().button_padding.y = 2.0;
            ui.spacing_mut().item_spacing.y = 2.0;
            if ui
                .selectable_label(self.selected.is_none(), "Document overview")
                .clicked()
            {
                self.selected = None;
            }
            egui::ScrollArea::vertical()
                .id_source("source_layers")
                .max_height(110.0)
                .min_scrolled_height(0.0)
                .show_rows(ui, 22.0, summary.layers.len(), |ui, range| {
                    ui.style_mut().wrap = Some(false);
                    for index in range {
                        let layer = &summary.layers[index];
                        ui.horizontal(|ui| {
                            ui.add_space(layer.depth.min(10) as f32 * 12.0);
                            let mut label = egui::RichText::new(
                                layer.name.chars().take(100).collect::<String>(),
                            );
                            if layer.group {
                                label = label.strong();
                            }
                            if layer.visible == Some(false) {
                                label = label.weak();
                            }
                            if ui
                                .selectable_label(self.selected == Some(index), label)
                                .on_hover_text(layer.name.chars().take(1024).collect::<String>())
                                .clicked()
                            {
                                self.selected = Some(index);
                            }
                        });
                    }
                });
        });
        if summary.paths > 0 {
            ui.label(format!("Saved paths: {}", summary.paths));
        }
        ui.separator();
    }

    fn properties(&self, ui: &mut egui::Ui, summary: &little_clip_core::image_document::Summary) {
        if summary.layers.is_empty() {
            ui.label("No stored layers");
            return;
        }
        let Some(layer) = self.selected.and_then(|i| summary.layers.get(i)) else {
            ui.label("Select a layer for source properties.");
            return;
        };
        ui.label(format!(
            "{} · {} · {}",
            match layer.visible {
                Some(true) => "Visible",
                Some(false) => "Hidden",
                None => "Visibility unknown",
            },
            layer
                .opacity
                .map(|opacity| format!("{:.0}%", f32::from(opacity) * 100.0 / 255.0))
                .unwrap_or_else(|| "Opacity unknown".into()),
            layer.blend_name()
        ));
        if layer.group {
            ui.label("Layer group");
        } else if let Some([top, left, bottom, right]) = layer.bounds {
            ui.label(format!(
                "{} × {} at ({left}, {top})",
                i64::from(right) - i64::from(left),
                i64::from(bottom) - i64::from(top)
            ))
            .on_hover_text("Bounds in pixels in the original document");
        } else if let Some((x, y)) = layer.offset {
            ui.label(format!("Source offset ({x}, {y})"));
        }
        if !layer.group
            && let Some(kind) = &layer.kind
        {
            ui.label(kind);
        }
        let masks = [
            (layer.mask, "Raster mask"),
            (layer.vector_mask, "Vector mask"),
        ]
        .into_iter()
        .filter_map(|(present, name)| present.then_some(name))
        .collect::<Vec<_>>();
        if !masks.is_empty() {
            ui.label(masks.join(" · "));
        }
    }

    fn overview(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &AppPalette,
        document: &Document,
    ) {
        let preview = &document.preview;
        let texture = self.texture.get_or_insert_with(|| {
            ctx.load_texture(
                "saved_layer_composite",
                egui::ColorImage::from_rgba_premultiplied(
                    [preview.width as usize, preview.height as usize],
                    &preview.premultiplied_rgba,
                ),
                egui::TextureOptions::LINEAR,
            )
        });
        let native = texture.size_vec2();
        let preview_height = if document.summary.profile.is_some() {
            40.0
        } else {
            64.0
        };
        let size = native
            * (ui.available_width() / native.x)
                .min(preview_height / native.y)
                .min(1.0);
        let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
        draw_checkerboard(
            ui.painter(),
            rect,
            1.0,
            hex_to_c32(&palette.colors.surface_panel),
            hex_to_c32(&palette.colors.surface_raised),
        );
        ui.painter().image(
            texture.id(),
            rect,
            Rect::from_min_max(Pos2::ZERO, egui::pos2(1.0, 1.0)),
            Color32::WHITE,
        );
        if let Some([top, left, bottom, right]) = self
            .selected
            .and_then(|i| document.summary.layers.get(i))
            .and_then(|layer| layer.bounds)
        {
            let (width, height) = document.summary.dimensions;
            let point = |x: i32, y: i32| {
                rect.min
                    + egui::vec2(
                        x as f32 / width as f32 * rect.width(),
                        y as f32 / height as f32 * rect.height(),
                    )
            };
            let bounds = Rect::from_min_max(point(left, top), point(right, bottom)).intersect(rect);
            if bounds.is_positive() {
                ui.painter().with_clip_rect(rect).rect_stroke(
                    bounds,
                    0.0,
                    Stroke::new(2.0_f32, hex_to_c32(&palette.harmonies.base)),
                );
            }
        }
        response
            .on_hover_text("Original saved composite; source offsets do not imply layer extents");
    }
}
