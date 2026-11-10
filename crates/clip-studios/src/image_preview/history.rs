use super::*;

impl ImagePreviewState {
    // Restore one retained canvas revision without recording a new edit. Keep
    // dependent controls and gesture state consistent in every history path.
    fn restore_history_pixels(&mut self, pixels: Arc<RgbaImage>) {
        self.current_rgba = pixels;
        self.mark_pixels_changed();
        self.dirty_texture = true;
        self.crop_state.normalized_rect =
            Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0));
        self.crop_state.active_drag = None;
        self.resize_state.sync_from_image(&self.current_rgba);
    }

    pub fn undo(&mut self) {
        if let Some(prev) = self.undo_stack.pop() {
            self.redo_stack.push(self.current_rgba.clone());
            self.restore_history_pixels(prev);
            self.show_toast("⟲ Undone");
        } else {
            self.show_toast("Nothing to undo");
        }
    }

    pub fn redo(&mut self) {
        if let Some(next) = self.redo_stack.pop() {
            self.undo_stack.push(self.current_rgba.clone());
            self.restore_history_pixels(next);
            self.show_toast("⟳ Redone");
        }
    }

    pub fn revert_to_original(&mut self) {
        if !self.undo_stack.is_empty() {
            self.undo_stack.push(self.current_rgba.clone());
            self.redo_stack.clear();
            self.restore_history_pixels(Arc::clone(&self.original_rgba));
            self.show_toast(crate::ui_ext::StatusMessage::new(
                crate::icons::Icon::Undo,
                "Reverted to original image",
            ));
        }
    }
}
