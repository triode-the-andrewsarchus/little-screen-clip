use super::*;

#[cfg(test)]
pub fn optimize_to_target_size(
    base_img: &RgbaImage,
    target_bytes: usize,
    format: TargetFormat,
) -> FileSizeOptimizationResult {
    optimize_to_target_size_with_progress(base_img, target_bytes, format, None)
}

#[cfg(test)]
pub fn optimize_to_target_size_with_progress(
    base_img: &RgbaImage,
    target_bytes: usize,
    format: TargetFormat,
    progress: Option<&dyn Fn(String)>,
) -> FileSizeOptimizationResult {
    optimize_to_target_size_cancellable(base_img, target_bytes, format, progress, &|| false)
        .expect("synchronous test optimization cannot be cancelled")
}

pub(super) fn optimize_to_target_size_cancellable(
    base_img: &RgbaImage,
    target_bytes: usize,
    format: TargetFormat,
    progress: Option<&dyn Fn(String)>,
    cancelled: &dyn Fn() -> bool,
) -> Option<FileSizeOptimizationResult> {
    let search = TargetSizeSearch {
        image: base_img,
        target_bytes,
        target_str: format_bytes_size(target_bytes),
        progress,
        cancelled,
    };
    search.check().ok()?;
    match format {
        TargetFormat::Jpeg => search.optimize_jpeg().ok(),
        TargetFormat::Png => search.optimize_png().ok(),
    }
}

struct OptimizationCancelled;

type SearchResult<T> = Result<T, OptimizationCancelled>;

struct TargetSizeSearch<'a> {
    image: &'a RgbaImage,
    target_bytes: usize,
    target_str: String,
    progress: Option<&'a dyn Fn(String)>,
    cancelled: &'a dyn Fn() -> bool,
}

struct OptimizedCandidate {
    image: RgbaImage,
    bytes: Vec<u8>,
    scale: f32,
    quality: u8,
}

impl TargetSizeSearch<'_> {
    fn check(&self) -> SearchResult<()> {
        if (self.cancelled)() {
            Err(OptimizationCancelled)
        } else {
            Ok(())
        }
    }

    fn notify(&self, message: String) {
        if let Some(progress) = self.progress {
            progress(message);
        }
    }

    fn finish(
        &self,
        candidate: OptimizedCandidate,
        format: TargetFormat,
    ) -> FileSizeOptimizationResult {
        FileSizeOptimizationResult {
            target_bytes: self.target_bytes,
            achieved_bytes: candidate.bytes.len(),
            width: candidate.image.width(),
            height: candidate.image.height(),
            scale: candidate.scale,
            quality: candidate.quality,
            format,
            encoded_data: Arc::new(candidate.bytes),
            result_rgba: candidate.image,
        }
    }

    fn optimize_jpeg(&self) -> SearchResult<FileSizeOptimizationResult> {
        self.notify(format!(
            "Testing full resolution ({}×{}) at JPEG quality 95%...",
            self.image.width(),
            self.image.height()
        ));
        if let Some(candidate) = self.jpeg_full_resolution()? {
            return Ok(self.finish(candidate, TargetFormat::Jpeg));
        }

        self.notify(
            "Exceeds budget at 100% resolution. Scaling down with Lanczos3 filter...".into(),
        );
        let mut best = self.jpeg_scale_search()?;
        if best.bytes.is_empty() {
            self.jpeg_minimal_fallback(&mut best)?;
        } else {
            self.jpeg_tune_quality(&mut best)?;
        }
        self.check()?;
        self.notify(format!(
            "Complete: {} achieved at {}×{} (Quality {}%)",
            format_bytes_size(best.bytes.len()),
            best.image.width(),
            best.image.height(),
            best.quality
        ));
        Ok(self.finish(best, TargetFormat::Jpeg))
    }

    fn jpeg_full_resolution(&self) -> SearchResult<Option<OptimizedCandidate>> {
        if let Ok(bytes) = encode_jpeg_with_quality(self.image, 95) {
            self.check()?;
            if bytes.len() <= self.target_bytes {
                self.notify(format!(
                    "Full resolution fits budget: {} <= {}",
                    format_bytes_size(bytes.len()),
                    self.target_str
                ));
                return Ok(Some(OptimizedCandidate {
                    image: self.image.clone(),
                    bytes,
                    scale: 1.0,
                    quality: 95,
                }));
            }
            self.notify(format!(
                "Quality 95% is {} (budget: {}). Searching JPEG quality [60..95]...",
                format_bytes_size(bytes.len()),
                self.target_str
            ));
        }

        let Some((quality, bytes)) = self.jpeg_quality_search()? else {
            return Ok(None);
        };
        self.check()?;
        self.notify(format!(
            "Found optimal quality {}% at 100% resolution ({})",
            quality,
            format_bytes_size(bytes.len())
        ));
        Ok(Some(OptimizedCandidate {
            image: self.image.clone(),
            bytes,
            scale: 1.0,
            quality,
        }))
    }

    fn jpeg_quality_search(&self) -> SearchResult<Option<(u8, Vec<u8>)>> {
        let mut best = None;
        let mut low_q = 60u8;
        let mut high_q = 95u8;
        while low_q <= high_q {
            self.check()?;
            let mid_q = low_q + (high_q - low_q) / 2;
            self.notify(format!(
                "Trying quality {}% at 100% resolution ({}×{})...",
                mid_q,
                self.image.width(),
                self.image.height()
            ));
            let Ok(bytes) = encode_jpeg_with_quality(self.image, mid_q) else {
                break;
            };
            self.check()?;
            let len = bytes.len();
            if len <= self.target_bytes {
                self.notify(format!(
                    "Quality {}% fits budget: {} <= {}",
                    mid_q,
                    format_bytes_size(len),
                    self.target_str
                ));
                best = Some((mid_q, bytes));
                low_q = mid_q + 1;
            } else {
                self.notify(format!(
                    "Quality {}% exceeds budget: {} > {}",
                    mid_q,
                    format_bytes_size(len),
                    self.target_str
                ));
                if mid_q == 0 {
                    break;
                }
                high_q = mid_q - 1;
            }
        }
        Ok(best)
    }

    fn jpeg_scale_search(&self) -> SearchResult<OptimizedCandidate> {
        let mut best = OptimizedCandidate {
            image: self.image.clone(),
            bytes: Vec::new(),
            scale: 0.5,
            quality: 80,
        };
        let mut low_s = 0.05f32;
        let mut high_s = 1.0f32;
        for iter in 1..=10 {
            self.check()?;
            let mid_s = (low_s + high_s) / 2.0;
            let width = ((self.image.width() as f32) * mid_s).round().max(16.0) as u32;
            let height = ((self.image.height() as f32) * mid_s).round().max(16.0) as u32;
            self.notify(format!(
                "[Step {}/10] Resampling to {:.1}% ({}×{}) with Lanczos3...",
                iter,
                mid_s * 100.0,
                width,
                height
            ));
            let scaled = image::imageops::resize(
                self.image,
                width,
                height,
                image::imageops::FilterType::Lanczos3,
            );
            self.check()?;
            let Ok(bytes) = encode_jpeg_with_quality(&scaled, 80) else {
                break;
            };
            self.check()?;
            let len = bytes.len();
            if len <= self.target_bytes {
                self.notify(format!(
                    "Scale {:.1}% ({}×{}) fits: {} <= {}",
                    mid_s * 100.0,
                    width,
                    height,
                    format_bytes_size(len),
                    self.target_str
                ));
                best = OptimizedCandidate {
                    image: scaled,
                    bytes,
                    scale: mid_s,
                    quality: 80,
                };
                low_s = mid_s;
            } else {
                self.notify(format!(
                    "Scale {:.1}% ({}×{}) exceeds: {} > {}",
                    mid_s * 100.0,
                    width,
                    height,
                    format_bytes_size(len),
                    self.target_str
                ));
                high_s = mid_s;
            }
        }
        Ok(best)
    }

    fn jpeg_tune_quality(&self, best: &mut OptimizedCandidate) -> SearchResult<()> {
        self.notify(format!(
            "Fine-tuning JPEG quality on {}×{}...",
            best.image.width(),
            best.image.height()
        ));
        for quality in (81..=95).step_by(2) {
            self.check()?;
            if let Ok(bytes) = encode_jpeg_with_quality(&best.image, quality) {
                if bytes.len() > self.target_bytes {
                    break;
                }
                self.notify(format!(
                    "Tuned quality to {}%: {} <= {}",
                    quality,
                    format_bytes_size(bytes.len()),
                    self.target_str
                ));
                best.quality = quality;
                best.bytes = bytes;
            }
        }
        Ok(())
    }

    fn jpeg_minimal_fallback(&self, best: &mut OptimizedCandidate) -> SearchResult<()> {
        self.notify("Applying minimal scale fallback...".into());
        let width = ((self.image.width() as f32) * 0.1).round().max(16.0) as u32;
        let height = ((self.image.height() as f32) * 0.1).round().max(16.0) as u32;
        best.image = image::imageops::resize(
            self.image,
            width,
            height,
            image::imageops::FilterType::Triangle,
        );
        self.check()?;
        best.scale = 0.1;
        best.quality = 40;
        if let Ok(bytes) = encode_jpeg_with_quality(&best.image, 40) {
            best.bytes = bytes;
        }
        Ok(())
    }

    fn optimize_png(&self) -> SearchResult<FileSizeOptimizationResult> {
        self.notify(format!(
            "Testing full resolution ({}×{}) as lossless PNG...",
            self.image.width(),
            self.image.height()
        ));
        if let Some(candidate) = self.png_full_resolution()? {
            return Ok(self.finish(candidate, TargetFormat::Png));
        }
        let best = self.png_scale_search()?;
        self.check()?;
        self.notify(format!(
            "Complete: {} achieved at {}×{}",
            format_bytes_size(best.bytes.len()),
            best.image.width(),
            best.image.height()
        ));
        Ok(self.finish(best, TargetFormat::Png))
    }

    fn png_full_resolution(&self) -> SearchResult<Option<OptimizedCandidate>> {
        let mut cursor = std::io::Cursor::new(Vec::new());
        if self
            .image
            .write_to(&mut cursor, image::ImageFormat::Png)
            .is_ok()
        {
            self.check()?;
            let bytes = cursor.into_inner();
            if bytes.len() <= self.target_bytes {
                self.notify(format!(
                    "Full resolution PNG fits budget: {} <= {}",
                    format_bytes_size(bytes.len()),
                    self.target_str
                ));
                return Ok(Some(OptimizedCandidate {
                    image: self.image.clone(),
                    bytes,
                    scale: 1.0,
                    quality: 100,
                }));
            }
            self.notify(format!(
                "PNG at 100% is {} > {}. Searching resolution scale...",
                format_bytes_size(bytes.len()),
                self.target_str
            ));
        }
        Ok(None)
    }

    fn png_scale_search(&self) -> SearchResult<OptimizedCandidate> {
        let mut best = OptimizedCandidate {
            image: self.image.clone(),
            bytes: Vec::new(),
            scale: 1.0,
            quality: 100,
        };
        let mut low_s = 0.05f32;
        let mut high_s = 1.0f32;
        for iter in 1..=10 {
            self.check()?;
            let mid_s = (low_s + high_s) / 2.0;
            let width = ((self.image.width() as f32) * mid_s).round().max(16.0) as u32;
            let height = ((self.image.height() as f32) * mid_s).round().max(16.0) as u32;
            self.notify(format!(
                "[Step {}/10] Resampling to {:.1}% ({}×{})...",
                iter,
                mid_s * 100.0,
                width,
                height
            ));
            let scaled = image::imageops::resize(
                self.image,
                width,
                height,
                image::imageops::FilterType::Lanczos3,
            );
            self.check()?;
            let mut cursor = std::io::Cursor::new(Vec::new());
            if scaled
                .write_to(&mut cursor, image::ImageFormat::Png)
                .is_ok()
            {
                self.check()?;
                let bytes = cursor.into_inner();
                let len = bytes.len();
                if len <= self.target_bytes {
                    self.notify(format!(
                        "Scale {:.1}% fits: {} <= {}",
                        mid_s * 100.0,
                        format_bytes_size(len),
                        self.target_str
                    ));
                    best = OptimizedCandidate {
                        image: scaled,
                        bytes,
                        scale: mid_s,
                        quality: 100,
                    };
                    low_s = mid_s;
                } else {
                    self.notify(format!(
                        "Scale {:.1}% exceeds: {} > {}",
                        mid_s * 100.0,
                        format_bytes_size(len),
                        self.target_str
                    ));
                    high_s = mid_s;
                }
            }
        }
        Ok(best)
    }
}
