//! Lazy stereo spectra: retain the displayed textures until a current worker result is ready.
use super::*;
use little_clip_ops::hf_audio::render_hf_viewport_spectrogram;

#[derive(Clone)]
struct SpectrumRequest {
    samples: [Arc<[f32]>; 4],
    sample_rate: u32,
    viewport: (u32, u32, usize),
}

impl SpectrumRequest {
    fn matches(&self, other: &Self) -> bool {
        self.sample_rate == other.sample_rate
            && self.viewport == other.viewport
            && self
                .samples
                .iter()
                .zip(&other.samples)
                .all(|(a, b)| Arc::ptr_eq(a, b))
    }
}

struct SpectrumJob {
    ticket: JobTicket,
    cancelled: Arc<AtomicBool>,
    receiver: Receiver<Result<Option<Vec<egui::ColorImage>>, JobError>>,
}

impl Drop for SpectrumJob {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        self.ticket.cancel();
    }
}

#[derive(Default)]
pub(super) struct ChannelSpectra {
    pub(super) textures: [Option<egui::TextureHandle>; 4],
    requested: Option<SpectrumRequest>,
    job: Option<SpectrumJob>,
}

impl ChannelSpectra {
    pub(super) fn update(
        &mut self,
        ctx: &egui::Context,
        samples: [Arc<[f32]>; 4],
        sample_rate: u32,
        start: f32,
        duration: f32,
        width: f32,
    ) {
        let request = SpectrumRequest {
            samples,
            sample_rate,
            viewport: (
                (start * 1000.0).max(0.0) as u32,
                (duration * 1000.0).max(1.0) as u32,
                (width as usize).clamp(200, 2048),
            ),
        };
        if !self
            .requested
            .as_ref()
            .is_some_and(|old| old.matches(&request))
        {
            self.job = None;
            let worker_request = request.clone();
            let cancelled = Arc::new(AtomicBool::new(false));
            let worker_cancelled = cancelled.clone();
            let (ticket, receiver) = BackgroundJobs::global().submit(Duration::MAX, move || {
                let mut images = Vec::with_capacity(4);
                for samples in &worker_request.samples {
                    if worker_cancelled.load(Ordering::Acquire) {
                        return None;
                    }
                    let (w, h, rgba) = render_hf_viewport_spectrogram(
                        samples,
                        worker_request.sample_rate,
                        worker_request.viewport.0 as f32 / 1000.0,
                        worker_request.viewport.1 as f32 / 1000.0,
                        worker_request.viewport.2,
                        HfSpectrogramConfig {
                            fft_size: 1024,
                            gain_db: 0.0,
                            range_db: 80.0,
                            contrast: 1.0,
                        },
                    );
                    images.push(egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba));
                }
                Some(images)
            });
            self.requested = Some(request);
            self.job = Some(SpectrumJob {
                ticket,
                cancelled,
                receiver,
            });
        }
        if let Some(job) = self.job.take() {
            match job.receiver.try_recv() {
                Ok(Ok(Some(images))) => {
                    for (index, image) in images.into_iter().enumerate() {
                        self.textures[index] = Some(ctx.load_texture(
                            format!("channel-spectrum-{index}"),
                            image,
                            egui::TextureOptions::LINEAR,
                        ));
                    }
                }
                Err(TryRecvError::Empty) => self.job = Some(job),
                _ => self.requested = None,
            }
            ctx.request_repaint_after(Duration::from_millis(30));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_spectra_keep_displayed_textures_until_complete_and_keys_track_revisions() {
        let ctx = egui::Context::default();
        let samples = std::array::from_fn(|_| Arc::<[f32]>::from(vec![0.25; 80]));
        let request = SpectrumRequest {
            samples,
            sample_rate: 8000,
            viewport: (0, 10, 200),
        };
        let old_texture = ctx.load_texture(
            "old-spectrum",
            egui::ColorImage::new([2, 2], Color32::GREEN),
            egui::TextureOptions::LINEAR,
        );
        let mut spectra = ChannelSpectra {
            textures: std::array::from_fn(|_| Some(old_texture.clone())),
            requested: Some(request.clone()),
            job: None,
        };
        let (ticket, _) = BackgroundJobs::global().submit(Duration::MAX, || {});
        let (sender, receiver) = channel();
        spectra.job = Some(SpectrumJob {
            ticket,
            receiver,
            cancelled: Arc::new(AtomicBool::new(false)),
        });
        spectra.update(&ctx, request.samples.clone(), 8000, 0.0, 0.01, 200.0);
        assert!(spectra.job.is_some());
        assert!(
            spectra
                .textures
                .iter()
                .all(|texture| texture.as_ref().unwrap().id() == old_texture.id())
        );
        sender
            .send(Ok(Some(vec![
                egui::ColorImage::new([2, 2], Color32::BLUE);
                4
            ])))
            .unwrap();
        spectra.update(&ctx, request.samples.clone(), 8000, 0.0, 0.01, 200.0);
        assert!(spectra.job.is_none());
        assert!(
            spectra
                .textures
                .iter()
                .all(|texture| texture.as_ref().unwrap().id() != old_texture.id())
        );
        let mut revision = request.clone();
        revision.samples[0] = vec![0.25; 80].into();
        assert!(
            !request.matches(&revision),
            "equal samples in a new revision still require fresh spectra"
        );
        revision = request.clone();
        revision.viewport.0 = 1;
        assert!(!request.matches(&revision));
    }
}
