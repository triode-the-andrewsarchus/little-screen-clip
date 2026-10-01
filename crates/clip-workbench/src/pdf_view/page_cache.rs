use super::*;

impl PdfEditorState {
    pub(super) fn page_render_width(&self, page_num: usize) -> u32 {
        if self.viewport.page_num == Some(page_num) {
            self.viewport.render_width
        } else {
            PAGE_RASTER_MAX_DIM
        }
    }

    /// Upload on demand and evict least recently used pages. The selected page
    /// and any texture already referenced by this frame's paint shapes are
    /// protected, so eviction cannot leave a dangling image in the draw list.
    pub(super) fn ensure_page_texture(
        &mut self,
        page_num: usize,
        ctx: &egui::Context,
    ) -> Option<egui::TextureId> {
        let requested_width = self.page_render_width(page_num);
        let index = page_num.checked_sub(1)?;
        let page = self.pages.get_mut(index)?;
        debug_assert!(
            page.raster
                .as_ref()
                .is_none_or(|raster| raster.page_num() == page_num),
            "page raster metadata must match its owning page"
        );
        let upgrade = self.selected_page == Some(page_num)
            && page
                .raster
                .as_ref()
                .is_some_and(|raster| raster.needs_render(requested_width));
        let ready = self.decode_job.as_ref().is_some_and(|job| {
            job.raster.page_num() == page_num && !job.cancelled && job.ready.is_some()
        });
        if let Some(texture) = &page.texture {
            if (!upgrade || page.error.is_some()) && !ready {
                page.texture_last_used = self.texture_frame;
                return Some(texture.id());
            }
        }
        if page.raster.is_none() || page.error.is_some() {
            return None;
        }
        if !self.decode_requests.contains(&page_num) {
            if self.decode_requests.len() == PAGE_DECODE_REQUEST_LIMIT
                && self.selected_page == Some(page_num)
            {
                self.decode_requests.pop();
            }
            if self.decode_requests.len() < PAGE_DECODE_REQUEST_LIMIT {
                self.decode_requests.push(page_num);
            }
        }
        let page = &mut self.pages[index];
        if page.texture.is_some() {
            page.texture_last_used = self.texture_frame;
        }
        let current_texture = page.texture.as_ref().map(|texture| texture.id());
        let Some(job) = self.decode_job.as_ref().filter(|job| {
            job.raster.page_num() == page_num && !job.cancelled && job.ready.is_some()
        }) else {
            return current_texture;
        };
        let bytes = job.ready.as_ref()?.pixels.len() * 4;
        if !self.make_texture_room(bytes, page_num) {
            ctx.request_repaint();
            return current_texture;
        }
        let job = self.decode_job.take()?;
        let color_image = job.ready?;
        self.pages[index].raster = Some(job.raster);
        let page = &mut self.pages[index];
        let texture = ctx.load_texture(
            format!("pdf_page_{page_num}"),
            color_image,
            egui::TextureOptions::LINEAR,
        );
        let id = texture.id();
        page.texture = Some(texture);
        page.texture_last_used = self.texture_frame;
        Some(id)
    }

    pub(super) fn poll_decode_worker(&mut self, ctx: &egui::Context) {
        let Some(job) = &mut self.decode_job else {
            return;
        };
        let page_num = job.raster.page_num();
        match job.poll(ctx) {
            PdfDecodePoll::Pending => return,
            PdfDecodePoll::Ready => {}
            PdfDecodePoll::AlreadyReady => return,
            PdfDecodePoll::QueueFull | PdfDecodePoll::Cancelled => self.decode_job = None,
            PdfDecodePoll::Failed(error) => {
                if let Some(page) = self.pages.get_mut(page_num - 1) {
                    page.error = Some(format!("PDF preview decode failed: {error}"));
                }
                self.decode_job = None;
            }
        }
        ctx.request_repaint();
    }

    // Called after painting, when the complete visible demand is known. No
    // pixel work runs here; one job is submitted, selected page first.
    pub(super) fn schedule_page_decode(&mut self, ctx: &egui::Context) {
        let selected_width = self.selected_page.map(|page| self.page_render_width(page));
        if let Some(job) = &mut self.decode_job {
            let selected_waiting = self.selected_page.is_some_and(|selected| {
                selected != job.raster.page_num()
                    && self.decode_requests.contains(&selected)
                    && self.pages.get(selected - 1).is_some_and(|page| {
                        page.error.is_none()
                            && page.raster.is_some()
                            && (page.texture.is_none()
                                || page.raster.as_ref().is_some_and(|raster| {
                                    raster
                                        .needs_render(selected_width.unwrap_or(PAGE_RASTER_MAX_DIM))
                                }))
                    })
            });
            if !self.decode_requests.contains(&job.raster.page_num()) || selected_waiting {
                if job.ready.is_some() {
                    self.decode_job = None;
                } else {
                    job.cancel();
                }
            }
        }
        if self.decode_job.is_none() {
            let candidate = self
                .selected_page
                .into_iter()
                .chain(self.decode_requests.iter().copied())
                .find(|page_num| {
                    self.decode_requests.contains(page_num)
                        && self.pages.get(page_num - 1).is_some_and(|page| {
                            page.error.is_none()
                                && page.raster.is_some()
                                && (page.texture.is_none()
                                    || (self.selected_page == Some(*page_num)
                                        && page.raster.as_ref().is_some_and(|raster| {
                                            raster.needs_render(self.page_render_width(*page_num))
                                        })))
                        })
                });
            if let Some(page_num) = candidate {
                let raster = self.pages[page_num - 1]
                    .raster
                    .as_ref()
                    .expect("decode candidates have cached rasters")
                    .clone();
                let requested_width = self.page_render_width(page_num);
                let source = (self.selected_page == Some(page_num)
                    && raster.needs_render(requested_width))
                .then(|| (self.path.clone(), requested_width));
                self.decode_job = Some(PdfPageDecodeSlot::start(raster, source, ctx));
            }
        }
        if self.decode_job.is_some() {
            ctx.request_repaint_after(Duration::from_millis(20));
        }
    }

    pub(super) fn make_texture_room(
        &mut self,
        additional_bytes: usize,
        replacing_page: usize,
    ) -> bool {
        let (mut count, mut bytes) = self
            .pages
            .iter()
            .filter(|page| page.page_num != replacing_page)
            .filter_map(|page| page.texture.as_ref())
            .fold((0, 0), |(count, bytes), texture| {
                (count + 1, bytes + texture.byte_size())
            });
        while count >= PAGE_TEXTURE_LIMIT || bytes + additional_bytes > PAGE_TEXTURE_BYTE_LIMIT {
            let candidate = self
                .pages
                .iter_mut()
                .filter(|page| {
                    page.texture.is_some()
                        && Some(page.page_num) != self.selected_page
                        && page.texture_last_used != self.texture_frame
                })
                .min_by_key(|page| page.texture_last_used);
            let Some(page) = candidate else {
                return false;
            };
            let texture = page.texture.take().expect("eviction candidate has texture");
            count -= 1;
            bytes -= texture.byte_size();
        }
        true
    }
}
