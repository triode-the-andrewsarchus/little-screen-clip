//! Bounded sequential timeline sampling.
use super::*;
use clip_media_protocol::VideoTimeline;

pub(crate) fn render(file: File, job: &VideoTimeline, output: &mut impl Write) -> Result<()> {
    let (frame, discard) = sampling(job)?;
    let mut source = VideoSource::open(file, &frame, discard, std::time::Duration::from_secs(240))?;
    let Some(first) = source.next_frame()? else {
        return Err("No timeline frames".into());
    };
    let duration = job.duration_ms as f64 / 1000.0;
    let origin = source.origin as f64 / 1_000_000.0 + frame.seconds;
    // A localized seek can deliver frames before its start. Align its first
    // output column with the requested origin and hold the last sample at EOF.
    let alignment = if job.start_ms.is_some() {
        ":start_time=0"
    } else {
        ""
    };
    let spec = format!(
        "setpts=PTS-{origin:.9}/TB,{}scale=1:{}:flags=area,tpad=stop_mode=clone:stop_duration={duration:.9},fps={}000/{}{alignment},tile={}x1,format=pix_fmts=yuvj444p",
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

fn sampling(job: &VideoTimeline) -> Result<(VideoFrame, ffmpeg::Discard)> {
    validate(&job.frame)?;
    let start = job.start_ms.unwrap_or(0);
    if job.frame.seconds != 0.0
        || !(1..=16384).contains(&job.columns)
        || job.duration_ms == 0
        || start.saturating_add(job.duration_ms) > 9_000_000_000_000
    {
        return Err("Invalid timeline settings".into());
    }
    let mut frame = job.frame.clone();
    frame.seconds = start as f64 / 1000.0;
    let discard = if job.start_ms.is_some() && job.duration_ms <= 120_000 {
        ffmpeg::Discard::Default
    } else {
        ffmpeg::Discard::NonKey
    };
    Ok((frame, discard))
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
