//! Separate bounded metadata and layout executors; visible chunks are admitted first.
use super::*;
use little_clip_core::{BackgroundJobs, JobError, JobTicket};
use std::{
    sync::{
        LazyLock,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

pub(super) struct Pending<T> {
    ticket: JobTicket,
    cancelled: Arc<AtomicBool>,
    receiver: mpsc::Receiver<Result<T, JobError>>,
    generation: u64,
}
impl<T> Drop for Pending<T> {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
        self.ticket.cancel();
    }
}
fn workers() -> usize {
    static COUNT: LazyLock<usize> = LazyLock::new(|| {
        std::env::var("LITTLE_CLIP_GALLERY_METADATA_WORKERS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(4)
            .clamp(1, 8)
    });
    *COUNT
}
fn metadata_jobs() -> &'static BackgroundJobs {
    static JOBS: LazyLock<BackgroundJobs> = LazyLock::new(|| BackgroundJobs::new(workers(), 8));
    &JOBS
}
fn layout_jobs() -> &'static BackgroundJobs {
    static JOBS: LazyLock<BackgroundJobs> = LazyLock::new(|| BackgroundJobs::new(2, 8));
    &JOBS
}
impl Gallery {
    pub(super) fn poll(&mut self, ctx: &egui::Context) {
        for chunk in &mut self.chunks {
            if let Some(result) = chunk
                .probe
                .as_ref()
                .and_then(|job| job.receiver.try_recv().ok())
            {
                chunk.probe = None;
                if let Ok(values) = result {
                    self.metadata_cache
                        .extend(chunk.keys.iter().cloned().zip(values.iter().copied()));
                    chunk.metadata = Some(values.into());
                }
            }
            if let Some(result) = chunk
                .packing
                .as_ref()
                .and_then(|job| job.receiver.try_recv().ok())
            {
                let current = chunk
                    .packing
                    .as_ref()
                    .is_some_and(|job| job.generation == chunk.generation);
                chunk.packing = None;
                if let Ok(layout) = result
                    && current
                {
                    chunk.layout = Some(Arc::new(layout));
                }
            }
        }
        if self
            .chunks
            .iter()
            .any(|chunk| chunk.probe.is_some() || chunk.packing.is_some())
        {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
    }
    pub(super) fn schedule(&mut self) {
        if self.chunks.is_empty() {
            return;
        }
        let first = self.chunk_at(self.viewport.0);
        let last = self.chunk_at(self.viewport.1);
        let mut order: Vec<_> = (0..self.chunks.len()).collect();
        order.sort_by_key(|index| {
            let distance = first.saturating_sub(*index).max(index.saturating_sub(last));
            (
                distance,
                if self.direction {
                    *index < first
                } else {
                    *index > last
                },
            )
        });
        // Cancel queued speculative work after a jump. Already completed metadata
        // and layouts remain reusable; admission stays bounded on every frame.
        if self
            .scheduled_focus
            .is_some_and(|old| old.abs_diff(first) > 2)
        {
            for (index, chunk) in self.chunks.iter_mut().enumerate() {
                if first.saturating_sub(index).max(index.saturating_sub(last)) > 2 {
                    chunk.probe = None;
                    chunk.packing = None;
                }
            }
        }
        self.scheduled_focus = Some(first);
        let mut probes = self.chunks.iter().filter(|c| c.probe.is_some()).count();
        let mut layouts = self.chunks.iter().filter(|c| c.packing.is_some()).count();
        for index in order {
            let chunk = &mut self.chunks[index];
            if chunk.metadata.is_none() && chunk.probe.is_none() && probes < workers() {
                chunk.probe = Some(start_probe(chunk, &self.metadata_cache, &self.context));
                probes += 1;
            }
            if chunk.metadata.is_some()
                && chunk.layout.is_none()
                && chunk.packing.is_none()
                && layouts < 2
            {
                chunk.packing = Some(start_layout(chunk, self.width));
                layouts += 1;
            }
        }
    }
}
fn start_probe(
    chunk: &Chunk,
    cache: &BTreeMap<Key, Metadata>,
    context: &super::super::search::PreviewContext,
) -> Pending<Vec<Metadata>> {
    let context = context.clone();
    let keys = chunk.keys.clone();
    let known: Vec<_> = keys.iter().map(|key| cache.get(key).copied()).collect();
    let cancelled = Arc::new(AtomicBool::new(false));
    let token = cancelled.clone();
    let (ticket, receiver) = metadata_jobs().submit(Duration::from_secs(60), move || {
        keys.iter()
            .zip(known)
            .map_while(|(key, cached)| {
                if token.load(Ordering::Relaxed) {
                    return None;
                }
                Some(
                    cached
                        .or_else(|| {
                            key.photo
                                .then(|| {
                                    context
                                        .read(&key.path, &token, || {
                                            metadata::probe(&key.path, key.revision)
                                        })
                                        .ok()
                                })
                                .flatten()
                        })
                        .unwrap_or(Metadata {
                            width: 1,
                            height: 1,
                            orientation: 1,
                        }),
                )
            })
            .collect()
    });
    Pending {
        ticket,
        receiver,
        cancelled,
        generation: chunk.generation,
    }
}
fn start_layout(chunk: &Chunk, width: usize) -> Pending<Layout> {
    let metadata = chunk.metadata.as_ref().unwrap().clone();
    let final_chunk = chunk.final_chunk;
    let cancelled = Arc::new(AtomicBool::new(false));
    let (ticket, receiver) = layout_jobs().submit(Duration::from_secs(10), move || {
        packing::pack_chunk(
            &metadata
                .iter()
                .map(|value| value.aspect())
                .collect::<Vec<_>>(),
            width,
            final_chunk,
        )
    });
    Pending {
        ticket,
        receiver,
        cancelled,
        generation: chunk.generation,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pending<T>(generation: u64) -> (Pending<T>, mpsc::Sender<Result<T, JobError>>) {
        let (ticket, _) = BackgroundJobs::global().submit(Duration::from_secs(1), || ());
        let (sender, receiver) = mpsc::channel();
        (
            Pending {
                ticket,
                receiver,
                cancelled: Arc::new(AtomicBool::new(false)),
                generation,
            },
            sender,
        )
    }
    #[test]
    fn out_of_order_metadata_and_stale_layout_results_are_guarded() {
        let directory = super::super::tests::synthetic_directory(100);
        let mut gallery = Gallery::default();
        gallery.sync(&directory, &(0..100).collect::<Vec<_>>(), false, 8);
        let (job0, send0) = pending(gallery.generation);
        let (job1, send1) = pending(gallery.generation);
        gallery.chunks[0].probe = Some(job0);
        gallery.chunks[1].probe = Some(job1);
        let metadata = Metadata {
            width: 160,
            height: 90,
            orientation: 1,
        };
        send1.send(Ok(vec![metadata; 48])).unwrap();
        gallery.poll(&egui::Context::default());
        assert!(gallery.chunks[0].metadata.is_none());
        assert_eq!(gallery.chunks[1].metadata.as_ref().unwrap()[0], metadata);
        send0.send(Ok(vec![metadata; 48])).unwrap();
        gallery.poll(&egui::Context::default());
        assert_eq!(gallery.chunks[0].metadata.as_ref().unwrap()[0], metadata);
        let (stale, send_stale) = pending(gallery.generation.wrapping_sub(1));
        gallery.chunks[1].packing = Some(stale);
        send_stale.send(Ok(packing::pack(&[1.5; 48], 8))).unwrap();
        gallery.poll(&egui::Context::default());
        assert!(gallery.chunks[1].layout.is_none());
    }
    #[test]
    fn rapid_jump_cancels_speculative_jobs_and_admits_the_visible_chunk_first() {
        let directory = super::super::tests::synthetic_directory(10_000);
        let mut gallery = Gallery::default();
        gallery.sync(&directory, &(0..10_000).collect::<Vec<_>>(), false, 8);
        for chunk in &gallery.chunks {
            gallery
                .metadata_cache
                .extend(chunk.keys.iter().cloned().map(|key| {
                    (
                        key,
                        Metadata {
                            width: 120,
                            height: 80,
                            orientation: 1,
                        },
                    )
                }));
        }
        gallery.set_viewport(0.0, 8.0);
        gallery.schedule();
        let token = gallery.chunks[0].probe.as_ref().unwrap().cancelled.clone();
        let target = gallery.offsets[100];
        gallery.set_viewport(target, target + 8.0);
        gallery.schedule();
        assert!(token.load(Ordering::Relaxed));
        assert!(gallery.chunks[0].probe.is_none());
        assert!(gallery.chunks[100].probe.is_some());
        assert!(
            gallery
                .chunks
                .iter()
                .filter(|chunk| chunk.probe.is_some())
                .count()
                <= workers()
        );
    }
}
