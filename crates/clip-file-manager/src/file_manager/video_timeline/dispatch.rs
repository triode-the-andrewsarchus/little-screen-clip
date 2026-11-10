//! Divide a bounded decoder budget across visible video jobs after probing.
use super::super::media::VideoMetadata;
use std::sync::LazyLock;

pub(super) static FILE_LIMIT: LazyLock<usize> = LazyLock::new(|| {
    std::thread::available_parallelism()
        .map_or(4, usize::from)
        .clamp(1, 4)
});

#[derive(Clone, Copy)]
pub(super) struct Policy {
    pub prefer_gpu: bool,
    cpu_workers: usize,
    gpu_workers: usize,
}
impl Policy {
    pub fn for_video(video: &VideoMetadata, peers: usize) -> Self {
        let cores = std::thread::available_parallelism().map_or(4, usize::from);
        Self::choose(
            &video.codec,
            video.dimensions,
            video.size_bytes,
            video.duration_ms,
            peers,
            cores,
        )
    }
    fn choose(
        codec: &str,
        dimensions: (u32, u32),
        bytes: u64,
        duration: u64,
        peers: usize,
        cores: usize,
    ) -> Self {
        let peers = peers.clamp(1, 4).min(cores.max(1));
        let pixels = u64::from(dimensions.0) * u64::from(dimensions.1);
        let budget = cores.clamp(1, 16);
        let heavy = bytes >= 512 * 1024 * 1024 || duration >= 1_800_000 || pixels >= 2_000_000;
        let cpu_workers = (budget / peers).clamp(1, if heavy { 8 } else { 4 });
        // Large HEVC includes cropped/portrait 4K pictures, not just width >=3840.
        let prefer_gpu =
            (codec == "h264" && pixels >= 2_000_000) || (codec == "hevc" && pixels >= 4_000_000);
        let gpu_workers = if peers == 1 { 4 } else { 2 }.min(cpu_workers);
        Self {
            prefer_gpu,
            cpu_workers,
            gpu_workers,
        }
    }
    pub fn workers(self, gpu: bool) -> usize {
        if gpu {
            self.gpu_workers
        } else {
            self.cpu_workers
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn single_cropped_4k_uses_four_gpu_workers_and_eight_cpu_fallback_workers() {
        let policy = Policy::choose("hevc", (2862, 2158), 4_400_000_000, 5_500_000, 1, 32);
        assert!(policy.prefer_gpu);
        assert_eq!(policy.workers(true), 4);
        assert_eq!(policy.workers(false), 8);
    }
    #[test]
    fn batches_divide_cpu_budget_and_limit_gpu_contexts() {
        for peers in 1..=4 {
            let policy = Policy::choose("hevc", (1920, 1080), 1_600_000_000, 1_500_000, peers, 16);
            assert!(!policy.prefer_gpu);
            assert!(policy.workers(false) * peers <= 16);
        }
        let gpu = Policy::choose("h264", (1920, 1080), 2_000_000_000, 3_000_000, 3, 32);
        assert!(gpu.prefer_gpu);
        assert_eq!(gpu.workers(true), 2);
    }
    #[test]
    fn small_files_and_low_core_machines_have_bounded_dispatch() {
        let light = Policy::choose("av1", (640, 360), 10_000_000, 700_000, 1, 32);
        let long = Policy::choose("av1", (640, 360), 10_000_000, 2_000_000, 1, 32);
        assert_eq!(light.workers(false), 4);
        assert_eq!(long.workers(false), 8);
        let large = Policy::choose("hevc", (1920, 1080), 1_000_000_000, 700_000, 1, 32);
        assert_eq!(large.workers(false), 8);
        let limited = Policy::choose("h264", (1920, 1080), 2_000_000_000, 3_000_000, 4, 2);
        assert_eq!(limited.workers(true), 1);
        assert_eq!(limited.workers(false), 1);
    }
}
