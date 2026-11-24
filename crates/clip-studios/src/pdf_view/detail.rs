//! Bounded detail for the selected page. Sidebar previews use their own cache.
use super::*;
use egui::{ColorImage, Rect, TextureHandle, pos2, vec2};
use little_clip_core::{jobs::task::Task, notebook::RetainedPage, page_region::PageRegion};
use std::sync::LazyLock;
use std::time::Instant;

const SETTLE: Duration = Duration::from_millis(75);
const MARGIN_PX: f32 = 64.0;
static JOBS: LazyLock<BackgroundJobs> = LazyLock::new(|| BackgroundJobs::new(1, 4));

enum DetailOutput {
    Notebook(Arc<RetainedPage>),
    Crop(PageRegion, ColorImage),
}

struct DetailJob {
    task: Task<Result<DetailOutput, String>>,
    preparing_notebook: bool,
}

pub(super) struct DetailSource<'a> {
    pub path: &'a Path,
    pub check: &'a crate::file_source::ReadCheck,
    pub backend: PageSource,
}

#[derive(Default)]
pub(super) struct PageDetail {
    selected: Option<usize>,
    scene: Option<Arc<RetainedPage>>,
    desired: Option<(PageRegion, Instant)>,
    job: Option<DetailJob>,
    texture: Option<(PageRegion, TextureHandle)>,
    failed: bool,
}

impl PageDetail {
    pub fn select(&mut self, page: Option<usize>) {
        if self.selected != page {
            self.selected = page;
            self.desired = None;
            if let Some(job) = &self.job {
                job.task.token.store(true, Ordering::Release);
            }
            self.scene = None;
            self.texture = None;
            self.failed = false;
        }
    }

    fn cancel_crop(&mut self) {
        self.desired = None;
        if let Some(job) = &self.job
            && !job.preparing_notebook
        {
            // Keep the occupied slot until the renderer has returned and reaped
            // its child. Fast wheel movement cannot accumulate detail renders.
            job.task.token.store(true, Ordering::Release);
        }
    }

    pub fn show(
        &mut self,
        ui: &egui::Ui,
        canvas: Rect,
        page: Rect,
        base: [usize; 2],
        source: DetailSource<'_>,
    ) {
        let ctx = ui.ctx();
        self.poll(ctx);
        let ppp = ctx.pixels_per_point();
        let visible = page.intersect(canvas).intersect(ui.clip_rect());
        if self.failed || !visible.is_positive() || page.width() * ppp <= base[0] as f32 {
            self.cancel_crop();
            return;
        }
        let limit = ui.input(|input| input.max_texture_side);
        let (visible, crop) = regions(page, visible, ppp, limit);
        if visible.page[0] as usize <= base[0] {
            self.cancel_crop();
            return;
        }
        if let Some((region, texture)) = &self.texture
            && region.page[0] as usize > base[0]
        {
            ui.painter_at(canvas).image(
                texture.id(),
                region_rect(*region, page),
                Rect::from_min_max(egui::Pos2::ZERO, pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
            if covers(*region, visible) {
                self.cancel_crop();
                return;
            }
        }
        if self
            .desired
            .is_none_or(|(region, _)| !covers(region, visible))
        {
            self.cancel_crop();
            self.desired = Some((crop, Instant::now()));
        }
        self.schedule(ctx, source);
    }

    fn poll(&mut self, ctx: &egui::Context) {
        let Some(job) = &self.job else {
            return;
        };
        let cancelled = job.task.token.load(Ordering::Acquire);
        let Some(result) = job.task.poll() else {
            ctx.request_repaint_after(Duration::from_millis(20));
            return;
        };
        self.job = None;
        if cancelled {
            return;
        }
        match result {
            Ok(Ok(DetailOutput::Notebook(scene))) => self.scene = Some(scene),
            Ok(Ok(DetailOutput::Crop(region, image))) => {
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

    fn schedule(&mut self, ctx: &egui::Context, source: DetailSource<'_>) {
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
        let path = source.path.to_owned();
        let check = source.check.clone();
        let backend = source.backend;
        let scene = self.scene.clone();
        let preparing_notebook = backend == PageSource::Xournal && scene.is_none();
        self.job = Some(DetailJob {
            preparing_notebook,
            task: Task::start(&JOBS, Duration::from_secs(25), move |cancel| {
                check.read(cancel, || {
                    if preparing_notebook {
                        let scene = little_clip_core::notebook::retain_xournal_page(
                            &path, page_num, cancel,
                        )?;
                        Ok(DetailOutput::Notebook(Arc::new(scene)))
                    } else {
                        render(backend, &path, page_num, scene.as_deref(), region, cancel)
                            .map(|image| DetailOutput::Crop(region, image))
                    }
                })
            }),
        });
        ctx.request_repaint_after(Duration::from_millis(20));
    }
}

fn render(
    backend: PageSource,
    path: &Path,
    page_num: usize,
    scene: Option<&RetainedPage>,
    region: PageRegion,
    cancel: &AtomicBool,
) -> Result<ColorImage, String> {
    match backend {
        PageSource::Pdf => {
            let png =
                little_clip_core::pdf::render_pdf_page_region(path, page_num, region, cancel)?;
            decode_page_png(png, PageRegion::MAX_SIDE)
        }
        PageSource::Xournal => {
            let scene = scene.ok_or("Xournal page preparation is unavailable")?;
            let raster = scene.render_region(region, cancel)?;
            Ok(ColorImage::from_rgba_premultiplied(
                region.size.map(|side| side as usize),
                raster.data(),
            ))
        }
        PageSource::Notebook => Err("Notebook format uses whole-page previews".into()),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    #[test]
    fn deep_zoom_uses_physical_pixels_and_reuses_margin_while_panning() {
        let page = Rect::from_min_size(pos2(-3000.0, -5000.0), vec2(8000.0, 12_000.0));
        let canvas = Rect::from_min_size(pos2(200.0, 100.0), vec2(1100.0, 760.0));
        let (visible, crop) = regions(page, canvas, 1.5, 4096);
        assert!((1650..=1652).contains(&visible.size[0]));
        assert!((1140..=1142).contains(&visible.size[1]));
        assert_eq!(crop.page, [12_000, 18_000]);
        assert!(region_rect(crop, page).contains_rect(canvas));
        let (near, _) = regions(page.translate(vec2(35.0, 0.0)), canvas, 1.5, 4096);
        let (far, _) = regions(page.translate(vec2(350.0, 0.0)), canvas, 1.5, 4096);
        assert!(covers(crop, near));
        assert!(!covers(crop, far));

        let big_canvas = Rect::from_min_size(pos2(20.0, 20.0), vec2(4000.0, 3800.0));
        let (visible, crop) = regions(page, big_canvas, 2.0, 3072);
        assert!(covers(crop, visible));
        assert!(crop.size.into_iter().all(|side| side <= 3072));
        assert!(crop.size.into_iter().product::<u32>() <= PageRegion::MAX_PIXELS);
        assert!(region_rect(crop, page).contains_rect(big_canvas));
    }

    #[test]
    fn page_change_retires_an_in_flight_crop_until_actual_completion() {
        let ctx = egui::Context::default();
        let mut detail = PageDetail::default();
        detail.select(Some(1));
        let (running_tx, running_rx) = channel();
        let (release_tx, release_rx) = channel();
        let region = PageRegion {
            page: [2000, 3000],
            origin: [700, 1100],
            size: [32, 32],
        };
        let jobs = BackgroundJobs::new(1, 1);
        detail.job = Some(DetailJob {
            preparing_notebook: false,
            task: Task::start(&jobs, Duration::from_secs(5), move |_| {
                running_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                Ok(DetailOutput::Crop(
                    region,
                    ColorImage::filled([32, 32], egui::Color32::BLUE),
                ))
            }),
        });
        running_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        detail.select(Some(2));
        detail.poll(&ctx);
        assert!(
            detail
                .job
                .as_ref()
                .unwrap()
                .task
                .token
                .load(Ordering::Acquire)
        );
        assert!(detail.texture.is_none());
        release_tx.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while detail.job.is_some() && Instant::now() < deadline {
            detail.poll(&ctx);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(detail.job.is_none());
        assert!(detail.texture.is_none());
        assert!(!detail.failed);
    }
}
