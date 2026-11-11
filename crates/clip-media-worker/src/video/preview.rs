//! One decoder/filter/encoder session for a fixed-rate JPEG preview.
use super::*;
use clip_media_protocol::VideoPreview;

pub(crate) fn render(file: File, job: &VideoPreview, output: &mut impl Write) -> Result<()> {
    validate_preview(job)?;
    let mut source = VideoSource::open(file, &job.frame)?;
    let Some(first) = source.next_frame()? else {
        return Err("No video preview frames".into());
    };
    let end = job.duration.map(|duration| job.frame.seconds + duration);
    let mut graph = preview_filter(job, &source, &first.image, first.matrix.as_ref(), end)?;
    let mut writer = JpegWriter::default();
    let mut current = Some(first);
    while let Some(decoded) = current {
        if end.is_some_and(|end| decoded.seconds >= end) {
            break;
        }
        match graph.source.source().add(&decoded.image) {
            Ok(()) => {}
            Err(ffmpeg::Error::Eof) => break,
            Err(error) => return Err(message(error)),
        }
        writer.drain_filter(&mut graph, output, job.frames)?;
        if writer.frames >= job.frames {
            break;
        }
        current = source.next_frame()?;
    }
    if writer.frames < job.frames {
        // close() supplies the observed end in the input clock, so fps can
        // retain a low-rate final image without padding past the source's end.
        let observed = source.last_end.ok_or("Video preview end unavailable")?;
        let close = end
            .map(|end| source.timestamp(end))
            .map_or(observed, |end| end.min(observed));
        match graph.source.source().close(close) {
            Ok(()) | Err(ffmpeg::Error::Eof) => {}
            Err(error) => return Err(message(error)),
        }
        writer.drain_filter(&mut graph, output, job.frames)?;
    }
    writer.finish(output)
}

fn validate_preview(job: &VideoPreview) -> Result<()> {
    validate(&job.frame)?;
    if !(1..=60).contains(&job.fps)
        || !(1..=512).contains(&job.frames)
        || job
            .duration
            .is_some_and(|duration| !duration.is_finite() || duration <= 0.0 || duration > 60.0)
    {
        return Err("Invalid video preview interval or frame budget".into());
    }
    Ok(())
}

fn preview_filter(
    job: &VideoPreview,
    source: &VideoSource,
    first: &frame::Video,
    matrix: Option<&[i32; 9]>,
    end: Option<f64>,
) -> Result<FrameFilter> {
    let origin = source.origin as f64 / 1_000_000.0;
    let absolute_start = origin + job.frame.seconds;
    let trim = match end {
        Some(end) => format!("trim=start={absolute_start:.9}:end={:.9}", origin + end),
        None => format!("trim=start={absolute_start:.9}"),
    };
    let spec = format!(
        "{trim},setpts=PTS-{absolute_start:.9}/TB,fps={},{}",
        job.fps,
        scale_spec(matrix, &job.frame, "yuvj444p")?
    );
    FrameFilter::new(first, source.time_base, &spec)
}

#[derive(Default)]
struct JpegWriter {
    encoder: Option<codec::encoder::video::Encoder>,
    frames: u32,
    bytes: usize,
}
const QUALITY: i32 = 6 * ffmpeg::ffi::FF_QP2LAMBDA;

impl JpegWriter {
    fn drain_filter(
        &mut self,
        graph: &mut FrameFilter,
        output: &mut impl Write,
        limit: u32,
    ) -> Result<()> {
        while self.frames < limit {
            let mut image = frame::Video::empty();
            match graph.sink.sink().frame(&mut image) {
                Ok(()) => {}
                Err(ffmpeg::Error::Eof) => break,
                Err(ffmpeg::Error::Other { errno }) if errno == libc::EAGAIN => break,
                Err(error) => return Err(message(error)),
            }
            image.remove_side_data(ffmpeg::util::frame::side_data::Type::DisplayMatrix);
            if self.encoder.is_none() {
                self.encoder = Some(image_encoder(&image, codec::Id::MJPEG, Some(QUALITY))?);
            }
            let encoder = self.encoder.as_mut().ok_or("JPEG encoder unavailable")?;
            if encoder.width() != image.width()
                || encoder.height() != image.height()
                || encoder.format() != image.format()
            {
                return Err("Video preview output format changed".into());
            }
            // SAFETY: image owns this frame, exclusively borrowed here. This
            // sets encoder quality metadata, without touching shared pixels.
            unsafe {
                (*image.as_mut_ptr()).quality = QUALITY;
            }
            encoder.send_frame(&image).map_err(message)?;
            self.frames += 1;
            self.drain_packets(output)?;
        }
        Ok(())
    }

    fn drain_packets(&mut self, output: &mut impl Write) -> Result<()> {
        let encoder = self.encoder.as_mut().ok_or("JPEG encoder unavailable")?;
        loop {
            let mut packet = ffmpeg::Packet::empty();
            match encoder.receive_packet(&mut packet) {
                Ok(()) => {}
                Err(ffmpeg::Error::Eof) => return Ok(()),
                Err(ffmpeg::Error::Other { errno }) if errno == libc::EAGAIN => return Ok(()),
                Err(error) => return Err(message(error)),
            }
            let data = packet.data().ok_or("Empty encoded JPEG frame")?;
            self.bytes = self
                .bytes
                .checked_add(data.len())
                .filter(|bytes| *bytes <= MAX_IMAGE_BYTES)
                .ok_or("Video preview exceeds the 32 MiB output budget")?;
            output.write_all(data).map_err(message)?;
        }
    }

    fn finish(mut self, output: &mut impl Write) -> Result<()> {
        self.encoder
            .as_mut()
            .ok_or("No sampled video preview frames")?
            .send_eof()
            .map_err(message)?;
        self.drain_packets(output)?;
        output.flush().map_err(message)
    }
}
