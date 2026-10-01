use super::*;

impl ImagePreviewState {
    // --- Transform & Crop Actions ---

    pub fn undo(&mut self) {
        if let Some(prev) = self.undo_stack.pop() {
            self.redo_stack.push(self.current_rgba.clone());
            self.current_rgba = prev;
            self.mark_pixels_changed();
            self.dirty_texture = true;
            self.crop_state.normalized_rect =
                Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0));
            self.dcs_state.stretched_rgba = None;
            self.show_toast("⟲ Undone");
        } else {
            self.show_toast("Nothing to undo");
        }
    }

    pub fn redo(&mut self) {
        if let Some(next) = self.redo_stack.pop() {
            self.undo_stack.push(self.current_rgba.clone());
            self.current_rgba = next;
            self.mark_pixels_changed();
            self.dirty_texture = true;
            self.crop_state.normalized_rect =
                Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0));
            self.dcs_state.stretched_rgba = None;
            self.show_toast("⟳ Redone");
        }
    }

    pub fn revert_to_original(&mut self) {
        if !self.undo_stack.is_empty() {
            self.undo_stack.push(self.current_rgba.clone());
            self.current_rgba = self.original_rgba.clone();
            self.mark_pixels_changed();
            self.dirty_texture = true;
            self.crop_state.normalized_rect =
                Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0));
            self.dcs_state.stretched_rgba = None;
            self.show_toast("↺ Reverted to original image");
        }
    }
}
