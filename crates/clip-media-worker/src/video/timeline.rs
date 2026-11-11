//! Sequential keyframe-column fallback, preserving the existing sampling policy.
use super::*;
use clip_media_protocol::VideoTimeline;

pub(crate) fn render(file: File, job: &VideoTimeline, output: &mut impl Write) -> Result<()> {
    validate(&job.frame)?;
    if job.frame.seconds != 0.0
        || !(1..=16384).contains(&job.columns)
        || job.duration_ms == 0
        || job.duration_ms > 9_000_000_000_000
    {
        return Err("Invalid keyframe timeline settings".into());
    }
    let mut source = VideoSource::open(
        file,
        &job.frame,
        ffmpeg::Discard::NonKey,
        std::time::Duration::from_secs(240),
    )?;
    let Some(first) = source.next_frame()? else {
        return Err("No timeline keyframes".into());
    };
    let duration = job.duration_ms as f64 / 1000.0;
    let origin = source.origin as f64 / 1_000_000.0;
    // Each decoded keyframe is reduced once. fps holds that column until the
    // next keyframe; tpad supplies the final hold before tile consumes exactly
    // the requested number of physical output columns.
    let spec = format!(
        "setpts=PTS-{origin:.9}/TB,{}scale=1:{}:flags=area,tpad=stop_mode=clone:stop_duration={duration:.9},fps={}000/{},tile={}x1,format=pix_fmts=yuvj444p",
        orientation(first.matrix.as_ref())?,
        job.frame.height,
        job.columns,
        job.duration_ms,
        job.columns
    );
    let mut graph = FrameFilter::new(&first.image, &source, &spec)?;
    let mut current = Some(first);
    while let Some(decoded) = current {
        graph.source.source().add(&decoded.image).map_err(message)?;
        if let Some(image) = drain(&mut graph)? {
            return write(image, output);
        }
        current = source.next_frame()?;
    }
    let end = source.last_end.ok_or("Timeline source end unavailable")?;
    graph.source.source().close(end).map_err(message)?;
    write(
        drain(&mut graph)?.ok_or("No sampled timeline image")?,
        output,
    )
}

fn drain(graph: &mut FrameFilter) -> Result<Option<frame::Video>> {
    let mut image = frame::Video::empty();
    match graph.sink.sink().frame(&mut image) {
        Ok(()) => Ok(Some(image)),
        Err(ffmpeg::Error::Eof) => Ok(None),
        Err(ffmpeg::Error::Other { errno }) if errno == libc::EAGAIN => Ok(None),
        Err(error) => Err(message(error)),
    }
}

fn write(mut image: frame::Video, output: &mut impl Write) -> Result<()> {
    image.remove_side_data(ffmpeg::util::frame::side_data::Type::DisplayMatrix);
    let bytes = encode_image(
        &mut image,
        codec::Id::MJPEG,
        Some(2 * ffmpeg::ffi::FF_QP2LAMBDA),
    )?;
    output.write_all(&bytes).map_err(message)?;
    output.flush().map_err(message)
}
