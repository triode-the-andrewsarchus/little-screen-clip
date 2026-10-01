use super::super::*;

impl ImagePreviewState {
    pub(crate) fn suspend_pending_work(&mut self) {
        self.cancel_pixel_resize();
        if let Some(ticket) = self.palette_ticket.take() {
            ticket.cancel();
        }
        self.palette_rx = None;
        self.film_sim_state.cancel_render();
        self.creative_fx_state.cancel_render();
        self.cancel_optimizer();
        self.cancel_optipng_process();
        self.cancel_ocr();
        self.cancel_barcode_scan();
        self.cancel_decorrelation_stretch();
        self.cancel_image_tool_process();
        self.film_sim_state.cancel_thumbnail_batch();
        self.cancel_creative_fx_thumbnail_batch();
        if let Some(control) = self.creative_fx_state.depth_process_control.take() {
            control.cancel();
        }
        if let Some(ticket) = self.creative_fx_state.depth_job_ticket.take() {
            ticket.cancel();
        }
        if let Some(control) = self.creative_fx_state.alpha_process_control.take() {
            control.cancel();
        }
        if let Some(ticket) = self.creative_fx_state.alpha_job_ticket.take() {
            ticket.cancel();
        }
    }
}

impl Drop for ImagePreviewState {
    fn drop(&mut self) {
        self.suspend_pending_work();
    }
}
