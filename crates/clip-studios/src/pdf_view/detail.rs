//! Bounded detail for the selected page. Sidebar previews use their own cache.
use super::*;
use egui::{ColorImage, Rect, TextureHandle, pos2, vec2};
use little_clip_core::{jobs::task::Task, notebook::RetainedPage, page_region::PageRegion};
use std::sync::LazyLock;
use std::time::Instant;

const SETTLE: Duration = Duration::from_millis(75);
const MARGIN_PX: f32 = 64.0;
static JOBS: LazyLock<BackgroundJobs> = LazyLock::new(|| BackgroundJobs::new(1, 4));

type DetailResult = Result<(Arc<RetainedPage>, ColorImage), String>;

#[derive(Default)]
pub(super) struct PageDetail {
    selected: Option<usize>,
    scene: Option<Arc<RetainedPage>>,
    desired: Option<(PageRegion, Instant)>,
    job: Option<(PageRegion, Task<DetailResult>)>,
    texture: Option<(PageRegion, TextureHandle)>,
    failed: bool,
}

impl PageDetail {
    pub fn select(&mut self, page: Option<usize>) {
        if self.selected != page {
            self.selected = page;
            self.cancel_request();
            self.scene = None;
            self.texture = None;
            self.failed = false;
        }
    }

    fn cancel_request(&mut self) {
        self.desired = None;
        if let Some((_, task)) = &self.job {
            // Keep the occupied slot until the renderer has returned and reaped
            // its child. Fast wheel movement cannot accumulate detail renders.
            task.token.store(true, Ordering::Release);
        }
    }

    pub fn show(
        &mut self,
        ui: &egui::Ui,
        canvas: Rect,
        page: Rect,
        base: [usize; 2],
        path: &Path,
        check: &crate::file_source::ReadCheck,
    ) {
        let ctx = ui.ctx();
        self.poll(ctx);
        let ppp = ctx.pixels_per_point();
        let visible = page.intersect(canvas).intersect(ui.clip_rect());
        if self.failed || !visible.is_positive() || page.width() * ppp <= base[0] as f32 {
            self.cancel_request();
            return;
        }
        let limit = ui.input(|input| input.max_texture_side);
        let (visible, crop) = regions(page, visible, ppp, limit);
        if let Some((region, texture)) = &self.texture {
            ui.painter_at(canvas).image(
                texture.id(),
                region_rect(*region, page),
                Rect::from_min_max(egui::Pos2::ZERO, pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
            if covers(*region, visible) {
                self.cancel_request();
                return;
            }
        }
        if self
            .desired
            .is_none_or(|(region, _)| !covers(region, visible))
        {
            self.cancel_request();
            self.desired = Some((crop, Instant::now()));
        }
        self.schedule(ctx, path, check);
    }

    fn poll(&mut self, ctx: &egui::Context) {
        let Some((region, task)) = &self.job else {
            return;
        };
        let Some(result) = task.poll() else {
            ctx.request_repaint_after(Duration::from_millis(20));
            return;
        };
        let region = *region;
        let cancelled = task.token.load(Ordering::Acquire);
        self.job = None;
        if cancelled {
            return;
        }
        match result {
            Ok(Ok((scene, image))) => {
                self.scene = Some(scene);
                let texture =
                    ctx.load_texture("document-detail", image, egui::TextureOptions::LINEAR);
                self.texture = Some((region, texture));
            }
            Err(JobError::QueueFull | JobError::Cancelled) => {
                ctx.request_repaint_after(Duration::from_millis(20));
            }
            failure => {
                let error = failure
                    .map_err(|error| error.to_string())
                    .and_then(|result| result)
                    .err()
                    .unwrap_or_default();
                tracing::debug!(%error, "Document detail unavailable");
                self.failed = true;
            }
        }
    }

    fn schedule(
        &mut self,
        ctx: &egui::Context,
        path: &Path,
        check: &crate::file_source::ReadCheck,
    ) {
        let (Some((region, since)), Some(page_num)) = (self.desired, self.selected) else {
            return;
        };
        if self.job.is_some() {
            return;
        }
        let remaining = SETTLE.saturating_sub(since.elapsed());
        if !remaining.is_zero() {
            ctx.request_repaint_after(remaining);
            return;
        }
        let path = path.to_owned();
        let check = check.clone();
        let scene = self.scene.clone();
        self.job = Some((
            region,
            Task::start(&JOBS, Duration::from_secs(25), move |cancel| {
                check.read(cancel, || {
                    let scene = match scene {
                        Some(scene) => scene,
                        None => Arc::new(little_clip_core::notebook::retain_xournal_page(
                            &path, page_num, cancel,
                        )?),
                    };
                    let raster = scene.render_region(region, cancel)?;
                    let image = ColorImage::from_rgba_premultiplied(
                        region.size.map(|side| side as usize),
                        raster.data(),
                    );
                    Ok((scene, image))
                })
            }),
        ));
        ctx.request_repaint_after(Duration::from_millis(20));
    }
}

fn regions(
    page: Rect,
    visible: Rect,
    ppp: f32,
    max_texture_side: usize,
) -> (PageRegion, PageRegion) {
    let limit = max_texture_side.min(PageRegion::MAX_SIDE as usize) as f32;
    let padded = (visible.size() * ppp + Vec2::splat(MARGIN_PX * 2.0)).min(page.size() * ppp);
    let ratio = (limit / padded.max_elem())
        .min((PageRegion::MAX_PIXELS as f32 / (padded.x * padded.y)).sqrt())
        .min(PageRegion::MAX_PAGE_SIDE as f32 / (page.size().max_elem() * ppp))
        .min(1.0);
    // Leave two pixels for outward integer rounding at the crop edges.
    let density = (ppp * ratio * if ratio < 1.0 { 0.998 } else { 1.0 }).max(f32::EPSILON);
    let size = page.size() * density;
    let full = [
        size.x.round().max(1.0) as u32,
        size.y.round().max(1.0) as u32,
    ];
    let visible = Rect::from_min_max(
        ((visible.min - page.min) / page.size() * vec2(full[0] as f32, full[1] as f32)).to_pos2(),
        ((visible.max - page.min) / page.size() * vec2(full[0] as f32, full[1] as f32)).to_pos2(),
    );
    let to_region = |rect: Rect| {
        let origin = [rect.min.x.floor() as u32, rect.min.y.floor() as u32];
        let end = [rect.max.x.ceil() as u32, rect.max.y.ceil() as u32];
        PageRegion {
            page: full,
            origin,
            size: std::array::from_fn(|axis| end[axis].min(full[axis]) - origin[axis]),
        }
    };
    (
        to_region(visible),
        to_region(visible.expand(MARGIN_PX * ratio)),
    )
}

fn covers(outer: PageRegion, inner: PageRegion) -> bool {
    outer.page == inner.page
        && (0..2).all(|axis| {
            outer.origin[axis] <= inner.origin[axis]
                && outer.origin[axis] + outer.size[axis] >= inner.origin[axis] + inner.size[axis]
        })
}

fn region_rect(region: PageRegion, page: Rect) -> Rect {
    let scale = page.size() / vec2(region.page[0] as f32, region.page[1] as f32);
    let min = page.min + vec2(region.origin[0] as f32, region.origin[1] as f32) * scale;
    Rect::from_min_size(
        min,
        vec2(region.size[0] as f32, region.size[1] as f32) * scale,
    )
}
