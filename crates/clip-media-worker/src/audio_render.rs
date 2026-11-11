//! Streaming decode/filter/encode jobs over inherited ordinary file handles.
use clip_media_protocol::{AudioFilter, AudioInput, AudioRender};
use ffmpeg::{Rescale, codec, filter, format, frame, media};
use ffmpeg_next as ffmpeg;
use std::{fs::File, io::Seek};

type Result<T> = std::result::Result<T, String>;

pub(super) fn render(job: &AudioRender, files: Vec<File>, output: File) -> Result<()> {
    validate(job, files.len())?;
    let mut inputs = files
        .into_iter()
        .zip(&job.inputs)
        .map(|(file, options)| Input::open(file, options, job.range))
        .collect::<Result<Vec<_>>>()?;
    let (mut output, codec) = open_output(job, output)?;
    let mut graph = RenderGraph::new(job, &inputs, &codec)?;
    let mut encoder = make_encoder(&mut output, &graph, codec)?;
    output.set_metadata(inputs[0].context.metadata().to_owned());
    output.write_header().map_err(message)?;
    let output_base = output
        .stream(0)
        .ok_or("Missing encoded audio stream")?
        .time_base();
    let mut frames = 0_u64;
    loop {
        frames += drain_filter(&mut graph, &mut encoder, &mut output, output_base)?;
        let Some(index) = graph.requested_input(&inputs) else {
            break;
        };
        inputs[index].feed(&mut graph.sources[index])?;
    }
    frames += drain_filter(&mut graph, &mut encoder, &mut output, output_base)?;
    if frames == 0 {
        return Err("No audio samples in the selected range".into());
    }
    encoder.send_eof().map_err(message)?;
    drain_encoder(&mut encoder, &mut output, output_base)?;
    output.write_trailer().map_err(message)
}

fn open_output(
    job: &AudioRender,
    mut output: File,
) -> Result<(format::context::Output, ffmpeg::codec::Audio)> {
    if !output.metadata().map_err(message)?.is_file() {
        return Err("Audio output must be an owned seekable staging file".into());
    }
    output.set_len(0).map_err(message)?;
    output.rewind().map_err(message)?;
    let io = format::context::StreamIo::from_write_seek(output).map_err(message)?;
    let output = format::output_to_stream(io, None, Some(&job.format)).map_err(message)?;
    let codec = match &job.codec {
        Some(name) => ffmpeg::encoder::find_by_name(name),
        None => ffmpeg::encoder::find(output.format().codec("", media::Type::Audio)),
    }
    .ok_or("Audio encoder unavailable")?
    .audio()
    .map_err(message)?;
    Ok((output, codec))
}

fn validate(job: &AudioRender, inputs: usize) -> Result<()> {
    if job.version != 1 || inputs == 0 || inputs > 8 || inputs != job.inputs.len() {
        return Err("Invalid audio render protocol or input count".into());
    }
    if job.range.is_some_and(|(start, end)| {
        inputs != 1
            || !start.is_finite()
            || !end.is_finite()
            || start < 0.0
            || end <= start
            || end > (i64::MAX / 4) as f64 / 1_000_000.0
    }) || job
        .sample_rate
        .is_some_and(|rate| !(1..=384_000).contains(&rate))
        || job
            .channels
            .is_some_and(|channels| !(1..=8).contains(&channels))
    {
        return Err("Invalid audio render range, rate or channel count".into());
    }
    if job.codec.as_ref().is_some_and(|name| {
        name.is_empty()
            || name.len() > 64
            || !name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
    }) || job
        .inputs
        .iter()
        .any(|input| input.formats.is_empty() || input.formats.contains('\0'))
    {
        return Err("Invalid audio codec or container whitelist".into());
    }
    // Only ordinary single-stream audio muxers may write to custom staging I/O.
    if ![
        "wav", "flac", "mp2", "mp3", "ipod", "mp4", "adts", "ogg", "opus", "aiff", "asf",
        "matroska", "webm", "caf", "au", "s16le", "f32le", "ac3", "eac3", "spdif", "wv", "tta",
    ]
    .contains(&job.format.as_str())
    {
        return Err("Unsupported audio output container".into());
    }
    Ok(())
}

struct Input {
    context: format::context::Input,
    decoder: codec::decoder::Audio,
    stream: usize,
    time_base: ffmpeg::Rational,
    origin: i64,
    stop: Option<i64>,
    eof: bool,
    next_pts: i64,
}

impl Input {
    fn open(file: File, options: &AudioInput, range: Option<(f64, f64)>) -> Result<Self> {
        // Accepted writes have no read-preview deadline. Their owner retains
        // cancellation and child admission until the process actually exits.
        let mut context = crate::probe::open_input_with_deadline(file, &options.formats, None)?;
        let stream = match options.stream {
            Some(index) => context.stream(index),
            None => context.streams().best(media::Type::Audio),
        }
        .filter(|stream| stream.parameters().medium() == media::Type::Audio)
        .ok_or("No selected audio stream")?;
        let index = stream.index();
        let time_base = stream.time_base();
        let start = stream.start_time();
        let mut codec_context =
            codec::Context::from_parameters(stream.parameters()).map_err(message)?;
        codec_context.set_threading(codec::threading::Config {
            count: 1,
            ..Default::default()
        });
        codec_context.set_time_base(time_base);
        let mut decoder = codec_context.decoder();
        decoder.set_packet_time_base(time_base);
        let mut decoder = decoder.audio().map_err(message)?;
        if !(1..=8).contains(&decoder.channels()) || !(1..=384_000).contains(&decoder.rate()) {
            return Err("Unsupported audio channel count or sample rate".into());
        }
        if decoder.channel_layout().is_empty() {
            decoder.set_channel_layout(ffmpeg::ChannelLayout::default(i32::from(
                decoder.channels(),
            )));
        }
        // FFmpeg timestamps are relative to the container start for normal
        // inputs, falling back to stream start when the container has none.
        // SAFETY: the immutable read uses the live context owned above.
        let container_start = unsafe { (*context.as_ptr()).start_time };
        let origin = if container_start != ffmpeg::ffi::AV_NOPTS_VALUE {
            container_start.rescale(ffmpeg::rescale::TIME_BASE, time_base)
        } else if start != ffmpeg::ffi::AV_NOPTS_VALUE {
            start
        } else {
            0
        };
        if let Some((first, _)) = range.filter(|(first, _)| *first > 1.0) {
            let target = (((first - 1.0) * 1_000_000.0).round() as i64)
                .saturating_add(origin.rescale(time_base, ffmpeg::rescale::TIME_BASE));
            context
                .seek(target, ..target)
                .map_err(|error| format!("Audio seek: {error}"))?;
            decoder.flush();
        }
        Ok(Self {
            context,
            decoder,
            stream: index,
            time_base,
            origin,
            stop: range.map(|(_, end)| ((end + 1.0) * 1_000_000.0).round() as i64),
            eof: false,
            next_pts: 0,
        })
    }

    fn feed(&mut self, source: &mut filter::Context) -> Result<()> {
        loop {
            let mut packet = ffmpeg::Packet::empty();
            match packet.read(&mut self.context) {
                Ok(()) if packet.stream() != self.stream => continue,
                Ok(()) => {
                    if self.past_end(&packet) {
                        break;
                    }
                    self.decoder.send_packet(&packet).map_err(message)?;
                    self.drain(source)?;
                    return Ok(());
                }
                Err(ffmpeg::Error::Eof) => break,
                Err(error) => return Err(message(error)),
            }
        }
        self.decoder.send_eof().map_err(message)?;
        self.drain(source)?;
        source.source().flush().map_err(message)?;
        self.eof = true;
        Ok(())
    }

    fn past_end(&self, packet: &ffmpeg::Packet) -> bool {
        match (self.stop, packet.pts()) {
            (Some(stop), Some(pts)) => {
                pts.saturating_sub(self.origin)
                    .rescale(self.time_base, ffmpeg::rescale::TIME_BASE)
                    >= stop
            }
            _ => false,
        }
    }

    fn drain(&mut self, source: &mut filter::Context) -> Result<()> {
        loop {
            let mut decoded = frame::Audio::empty();
            match self.decoder.receive_frame(&mut decoded) {
                Ok(()) => {}
                Err(error) if exhausted(error) => return Ok(()),
                Err(error) => return Err(message(error)),
            }
            if decoded.rate() != self.decoder.rate()
                || decoded.channels() != self.decoder.channels()
            {
                return Err("Audio format changed during rendering".into());
            }
            if decoded.channel_layout().is_empty() {
                decoded.set_channel_layout(self.decoder.channel_layout());
            }
            let pts = decoded
                .timestamp()
                .map_or(self.next_pts, |pts| pts.saturating_sub(self.origin));
            self.next_pts = pts.saturating_add(
                (decoded.samples() as i64).rescale((1, decoded.rate() as i32), self.time_base),
            );
            decoded.set_pts(Some(pts));
            match source.source().add(&decoded) {
                Ok(()) => {}
                Err(ffmpeg::Error::Eof) => {
                    // A bounded trim can finish before its source file ends.
                    self.eof = true;
                    return Ok(());
                }
                Err(error) => return Err(format!("Audio filter input: {error}")),
            }
        }
    }
}

struct RenderGraph {
    // Context handles below borrow this graph's owned native filter instances.
    _graph: filter::Graph,
    sources: Vec<filter::Context>,
    sink: filter::Context,
}

impl RenderGraph {
    fn new(job: &AudioRender, inputs: &[Input], codec: &ffmpeg::codec::Audio) -> Result<Self> {
        let mut graph = filter::Graph::new();
        // SAFETY: exclusively configure the newly allocated, unconfigured graph.
        unsafe {
            (*graph.as_mut_ptr()).nb_threads = 1;
        }
        let mut sources = Vec::new();
        for (index, input) in inputs.iter().enumerate() {
            let decoder = &input.decoder;
            let args = format!(
                "time_base={}:sample_rate={}:sample_fmt={}:channel_layout=0x{:x}",
                input.time_base,
                decoder.rate(),
                decoder.format().name(),
                decoder.channel_layout().bits()
            );
            sources.push(
                graph
                    .add(
                        &filter::find("abuffer").ok_or("Audio buffer filter unavailable")?,
                        &format!("{index}:a"),
                        &args,
                    )
                    .map_err(message)?,
            );
        }
        let (spec, output) = graph_spec(job);
        crate::audio_filter::validate(&mut graph, &spec)?;
        if output.is_empty()
            || !output
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_')
        {
            return Err("Invalid audio graph output label".into());
        }
        let mut conversion = graph
            .add(
                &filter::find("aformat").ok_or("Audio format filter unavailable")?,
                &output,
                &conversion_options(job, codec)?,
            )
            .map_err(message)?;
        let mut sink = graph
            .add(
                &filter::find("abuffersink").ok_or("Audio sink filter unavailable")?,
                "encoded_sink",
                "",
            )
            .map_err(message)?;
        conversion.link(0, &mut sink, 0);
        let mut parser = filter::graph::Parser::new(&mut graph)
            .input(&output, 0)
            .map_err(message)?;
        for index in 0..inputs.len() {
            parser = parser.output(&format!("{index}:a"), 0).map_err(message)?;
        }
        parser.parse(&spec).map_err(message)?;
        graph.validate().map_err(message)?;
        Ok(Self {
            _graph: graph,
            sources,
            sink,
        })
    }

    fn requested_input(&self, inputs: &[Input]) -> Option<usize> {
        inputs
            .iter()
            .enumerate()
            .filter(|(_, input)| !input.eof)
            .max_by_key(|(index, _)| {
                // SAFETY: each source belongs to the graph retained by this object.
                unsafe {
                    ffmpeg::ffi::av_buffersrc_get_nb_failed_requests(
                        self.sources[*index].as_ptr() as *mut _
                    )
                }
            })
            .map(|(index, _)| index)
    }
}

fn graph_spec(job: &AudioRender) -> (String, String) {
    let (mut spec, output) = match &job.filter {
        None => ("[0:a]anull[out]".to_owned(), "out".to_owned()),
        Some(AudioFilter::Chain(spec)) => (format!("[0:a]{spec}[out]"), "out".to_owned()),
        Some(AudioFilter::Graph { spec, output }) => (spec.clone(), output.clone()),
    };
    if let Some((start, end)) = job.range {
        spec = format!(
            "[0:a]atrim=start={start:.9}:end={end:.9},asetpts=PTS-STARTPTS[range];{}",
            spec.replace("[0:a]", "[range]")
        );
    }
    (spec, output)
}

fn conversion_options(job: &AudioRender, codec: &ffmpeg::codec::Audio) -> Result<String> {
    let sample = codec
        .formats()
        .and_then(|mut formats| formats.next())
        .ok_or("Unknown encoder sample formats")?;
    let mut options = format!("sample_fmts={}", sample.name());
    let rates: Vec<_> = match job.sample_rate {
        Some(rate) => vec![rate as i32],
        None => codec.rates().map(Iterator::collect).unwrap_or_default(),
    };
    if !rates.is_empty() {
        let rates = rates
            .iter()
            .map(i32::to_string)
            .collect::<Vec<_>>()
            .join("|");
        options.push_str(&format!(":sample_rates={rates}"));
    }
    let layouts: Vec<_> = if let Some(channels) = job.channels {
        vec![ffmpeg::ChannelLayout::default(i32::from(channels))]
    } else {
        codec
            .channel_layouts()
            .map(Iterator::collect)
            .unwrap_or_default()
    };
    if !layouts.is_empty() {
        let layouts = layouts
            .into_iter()
            .filter(|layout| !layout.is_empty())
            .map(|layout| format!("0x{:x}", layout.bits()))
            .collect::<Vec<_>>()
            .join("|");
        options.push_str(&format!(":channel_layouts={layouts}"));
    }
    Ok(options)
}

fn make_encoder(
    output: &mut format::context::Output,
    graph: &RenderGraph,
    codec: ffmpeg::codec::Audio,
) -> Result<codec::encoder::audio::Encoder> {
    // SAFETY: sink is configured, live and immutable during these getter calls.
    let (rate, sample, layout) = unsafe {
        let rate = ffmpeg::ffi::av_buffersink_get_sample_rate(graph.sink.as_ptr());
        let sample = ffmpeg::ffi::av_buffersink_get_format(graph.sink.as_ptr());
        let mut layout: ffmpeg::ffi::AVChannelLayout = std::mem::zeroed();
        let result = ffmpeg::ffi::av_buffersink_get_ch_layout(graph.sink.as_ptr(), &mut layout);
        if result < 0 {
            return Err(message(ffmpeg::Error::from(result)));
        }
        // Only native masks have value semantics in ffmpeg-next's Copy layout
        // wrapper; do not leak or shallow-copy custom allocated channel maps.
        if layout.order != ffmpeg::ffi::AVChannelOrder::AV_CHANNEL_ORDER_NATIVE {
            ffmpeg::ffi::av_channel_layout_uninit(&mut layout);
            return Err("Custom encoded channel maps are unsupported".into());
        }
        (rate, sample, ffmpeg::ChannelLayout(layout))
    };
    let mut encoder = codec::Context::new_with_codec(*codec)
        .encoder()
        .audio()
        .map_err(message)?;
    encoder.set_rate(rate);
    encoder.set_format(format::Sample::from(ffmpeg_sample_format(sample)?));
    encoder.set_channel_layout(layout);
    encoder.set_time_base((1, rate));
    if output
        .format()
        .flags()
        .contains(format::flag::Flags::GLOBAL_HEADER)
    {
        encoder.set_flags(codec::flag::Flags::GLOBAL_HEADER);
    }
    let encoder = encoder.open_as(codec).map_err(message)?;
    let mut stream = output.add_stream(codec).map_err(message)?;
    stream.set_time_base((1, rate));
    stream.set_parameters(&encoder);
    Ok(encoder)
}

fn ffmpeg_sample_format(value: i32) -> Result<ffmpeg::ffi::AVSampleFormat> {
    // Avoid transmuting an arbitrary integer from a foreign API into an enum.
    const FORMATS: &[ffmpeg::ffi::AVSampleFormat] = &[
        ffmpeg::ffi::AVSampleFormat::AV_SAMPLE_FMT_U8,
        ffmpeg::ffi::AVSampleFormat::AV_SAMPLE_FMT_S16,
        ffmpeg::ffi::AVSampleFormat::AV_SAMPLE_FMT_S32,
        ffmpeg::ffi::AVSampleFormat::AV_SAMPLE_FMT_FLT,
        ffmpeg::ffi::AVSampleFormat::AV_SAMPLE_FMT_DBL,
        ffmpeg::ffi::AVSampleFormat::AV_SAMPLE_FMT_U8P,
        ffmpeg::ffi::AVSampleFormat::AV_SAMPLE_FMT_S16P,
        ffmpeg::ffi::AVSampleFormat::AV_SAMPLE_FMT_S32P,
        ffmpeg::ffi::AVSampleFormat::AV_SAMPLE_FMT_FLTP,
        ffmpeg::ffi::AVSampleFormat::AV_SAMPLE_FMT_DBLP,
        ffmpeg::ffi::AVSampleFormat::AV_SAMPLE_FMT_S64,
        ffmpeg::ffi::AVSampleFormat::AV_SAMPLE_FMT_S64P,
    ];
    FORMATS
        .iter()
        .copied()
        .find(|format| *format as i32 == value)
        .ok_or("Invalid negotiated audio format".into())
}

fn drain_filter(
    graph: &mut RenderGraph,
    encoder: &mut codec::encoder::audio::Encoder,
    output: &mut format::context::Output,
    output_base: ffmpeg::Rational,
) -> Result<u64> {
    if encoder.frame_size() > 0 {
        graph.sink.sink().set_frame_size(encoder.frame_size());
    }
    let time_base = graph.sink.sink().time_base();
    let mut count = 0;
    loop {
        let mut filtered = frame::Audio::empty();
        match graph.sink.sink().frame(&mut filtered) {
            Ok(()) => {}
            Err(error) if exhausted(error) => return Ok(count),
            Err(error) => return Err(message(error)),
        }
        count += filtered.samples() as u64;
        let pts = filtered
            .pts()
            .map(|pts| pts.rescale(time_base, encoder.time_base()));
        filtered.set_pts(pts);
        encoder.send_frame(&filtered).map_err(message)?;
        drain_encoder(encoder, output, output_base)?;
    }
}

fn drain_encoder(
    encoder: &mut codec::encoder::audio::Encoder,
    output: &mut format::context::Output,
    output_base: ffmpeg::Rational,
) -> Result<()> {
    loop {
        let mut packet = ffmpeg::Packet::empty();
        match encoder.receive_packet(&mut packet) {
            Ok(()) => {}
            Err(error) if exhausted(error) => return Ok(()),
            Err(error) => return Err(message(error)),
        }
        packet.set_stream(0);
        packet.rescale_ts(encoder.time_base(), output_base);
        packet.write_interleaved(output).map_err(message)?;
    }
}

fn exhausted(error: ffmpeg::Error) -> bool {
    error == ffmpeg::Error::Eof
        || error
            == (ffmpeg::Error::Other {
                errno: libc::EAGAIN,
            })
}
fn message(error: impl std::fmt::Display) -> String {
    error.to_string()
}
