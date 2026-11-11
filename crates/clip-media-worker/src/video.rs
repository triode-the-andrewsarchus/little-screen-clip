//! Bounded video decoding with actual source clocks and display orientation.
mod preview;
use clip_media_protocol::{VideoFrame, VideoFrameInfo, VideoStream};
use ffmpeg::{Rescale, codec, filter, format, frame, media};
use ffmpeg_next as ffmpeg;
pub(super) use preview::render as preview;
use std::{fs::File, io::Write};

type Result<T> = std::result::Result<T, String>;
const MAX_IMAGE_BYTES: usize = 32 * 1024 * 1024;

pub(super) fn snapshot(file: File, job: &VideoFrame, output: &mut impl Write) -> Result<()> {
    validate(job)?;
    let mut source = VideoSource::open(file, job)?;
    while let Some(decoded) = source.next_frame()? {
        if job.accurate && decoded.seconds + 0.000001 < job.seconds {
            continue;
        }
        let spec = scale_spec(decoded.matrix.as_ref(), job, "rgba")?;
        let mut graph = FrameFilter::new(&decoded.image, source.time_base, &spec)?;
        graph.source.source().add(&decoded.image).map_err(message)?;
        let mut filtered = frame::Video::empty();
        graph.sink.sink().frame(&mut filtered).map_err(message)?;
        filtered.remove_side_data(ffmpeg::util::frame::side_data::Type::DisplayMatrix);
        return write_snapshot(&filtered, decoded.seconds, output);
    }
    // Empty success permits the caller's preceding-seek-point retry for sparse
    // clips; it must never label a guessed frame with the requested timestamp.
    Ok(())
}

fn write_snapshot(image: &frame::Video, seconds: f64, output: &mut impl Write) -> Result<()> {
    let png = encode_png(image)?;
    let info = VideoFrameInfo {
        seconds,
        width: image.width(),
        height: image.height(),
        encoded_bytes: png.len() as u32,
    };
    let header = serde_json::to_vec(&info).map_err(message)?;
    output
        .write_all(&(header.len() as u32).to_le_bytes())
        .map_err(message)?;
    output.write_all(&header).map_err(message)?;
    output.write_all(&png).map_err(message)?;
    output.flush().map_err(message)
}

fn validate(job: &VideoFrame) -> Result<()> {
    if job.version != 1
        || !job.seconds.is_finite()
        || !(0.0..9.0e9).contains(&job.seconds)
        || !(1..=4096).contains(&job.width)
        || !(1..=4096).contains(&job.height)
        || !(1..=8192 * 8192).contains(&job.max_source_pixels)
        || job.formats.is_empty()
        || job.formats.contains('\0')
    {
        return Err("Invalid video frame settings".into());
    }
    Ok(())
}

struct VideoSource {
    input: format::context::Input,
    decoder: codec::decoder::Video,
    stream: usize,
    time_base: ffmpeg::Rational,
    origin: i64,
    matrix: Option<[i32; 9]>,
    attached: bool,
    eof_sent: bool,
    max_source_pixels: u64,
    default_frame_duration: i64,
    last_end: Option<i64>,
}

struct DecodedVideoFrame {
    image: frame::Video,
    seconds: f64,
    matrix: Option<[i32; 9]>,
}
impl VideoSource {
    fn open(file: File, job: &VideoFrame) -> Result<Self> {
        let mut input = crate::probe::open_input(file, &job.formats)?;
        // SAFETY: this dedicated process has finished its one-time library init.
        // Still/video frames may need larger planes than audio/probing jobs.
        unsafe {
            ffmpeg::ffi::av_max_alloc(256 * 1024 * 1024);
        }
        let stream = match job.stream {
            VideoStream::Index(index) => input.stream(index),
            VideoStream::Ordinal(index) => input
                .streams()
                .filter(|stream| stream.parameters().medium() == media::Type::Video)
                .nth(index),
            VideoStream::FirstNonCover => input.streams().find(|stream| {
                stream.parameters().medium() == media::Type::Video
                    && !stream
                        .disposition()
                        .contains(format::stream::Disposition::ATTACHED_PIC)
            }),
        }
        .filter(|stream| stream.parameters().medium() == media::Type::Video)
        .ok_or("No selected video stream")?;
        let index = stream.index();
        let time_base = stream.time_base();
        let rate = [stream.avg_frame_rate(), stream.rate()]
            .into_iter()
            .find(|rate| rate.numerator() > 0 && rate.denominator() > 0)
            .unwrap_or(ffmpeg::Rational(25, 1));
        let default_frame_duration = 1_i64
            .rescale((rate.denominator(), rate.numerator()), time_base)
            .max(1);
        let attached = stream
            .disposition()
            .contains(format::stream::Disposition::ATTACHED_PIC);
        let matrix = stream
            .side_data()
            .find(|side| side.kind() == ffmpeg::packet::side_data::Type::DisplayMatrix)
            .map(|side| display_matrix(side.data()))
            .transpose()?;
        let parameters = stream.parameters();
        // SAFETY: parameters owns the live immutable codec-parameter allocation.
        let (width, height) =
            unsafe { ((*parameters.as_ptr()).width, (*parameters.as_ptr()).height) };
        check_dimensions(
            u32::try_from(width).map_err(message)?,
            u32::try_from(height).map_err(message)?,
            job.max_source_pixels,
        )?;
        let mut context = codec::Context::from_parameters(parameters).map_err(message)?;
        context.set_threading(codec::threading::Config {
            count: 2,
            ..Default::default()
        });
        let mut decoder = context.decoder();
        decoder.set_packet_time_base(time_base);
        let decoder = decoder.video().map_err(message)?;
        check_dimensions(decoder.width(), decoder.height(), job.max_source_pixels)?;
        // SAFETY: input owns this live format context throughout the read.
        let origin = unsafe { (*input.as_ptr()).start_time };
        let origin = if origin == ffmpeg::ffi::AV_NOPTS_VALUE {
            0
        } else {
            origin
        };
        if job.seconds > 0.0 && !attached {
            let target = (job.seconds * 1_000_000.0).round() as i64;
            let target = target.saturating_add(origin);
            input.seek(target, ..target).map_err(message)?;
        }
        Ok(Self {
            input,
            decoder,
            stream: index,
            time_base,
            origin,
            matrix,
            attached,
            eof_sent: false,
            max_source_pixels: job.max_source_pixels,
            default_frame_duration,
            last_end: None,
        })
    }

    fn next_frame(&mut self) -> Result<Option<DecodedVideoFrame>> {
        loop {
            let mut image = frame::Video::empty();
            match self.decoder.receive_frame(&mut image) {
                Ok(()) => return self.describe_frame(image).map(Some),
                Err(ffmpeg::Error::Eof) => return Ok(None),
                Err(ffmpeg::Error::Other { errno }) if errno == libc::EAGAIN => {}
                Err(error) => return Err(message(error)),
            }
            if self.eof_sent {
                return Err("Video decoder requested packets after EOF".into());
            }
            match self.next_packet()? {
                Some(packet) => self.decoder.send_packet(&packet).map_err(message)?,
                None => {
                    self.decoder.send_eof().map_err(message)?;
                    self.eof_sent = true;
                }
            }
        }
    }

    fn next_packet(&mut self) -> Result<Option<ffmpeg::Packet>> {
        loop {
            let mut packet = ffmpeg::Packet::empty();
            match packet.read(&mut self.input) {
                Ok(()) if packet.stream() == self.stream => return Ok(Some(packet)),
                Ok(()) => {}
                Err(ffmpeg::Error::Eof) => return Ok(None),
                Err(error) => return Err(message(error)),
            }
        }
    }

    fn describe_frame(&mut self, mut image: frame::Video) -> Result<DecodedVideoFrame> {
        check_dimensions(image.width(), image.height(), self.max_source_pixels)?;
        let timestamp = if self.attached {
            self.origin
                .rescale(ffmpeg::rescale::TIME_BASE, self.time_base)
        } else {
            image
                .timestamp()
                .ok_or("Decoded video timestamp unavailable")?
        };
        image.set_pts(Some(timestamp));
        let duration = image.packet().duration;
        self.last_end = Some(timestamp.saturating_add(if duration > 0 {
            duration
        } else {
            self.default_frame_duration
        }));
        let seconds = if self.attached {
            0.0
        } else {
            timestamp
                .rescale(self.time_base, ffmpeg::rescale::TIME_BASE)
                .saturating_sub(self.origin) as f64
                / 1_000_000.0
        };
        let matrix = image
            .side_data(ffmpeg::util::frame::side_data::Type::DisplayMatrix)
            .map(|side| display_matrix(side.data()))
            .transpose()?
            .or(self.matrix);
        Ok(DecodedVideoFrame {
            image,
            seconds,
            matrix,
        })
    }

    fn timestamp(&self, seconds: f64) -> i64 {
        ((seconds * 1_000_000.0).round() as i64)
            .saturating_add(self.origin)
            .rescale(ffmpeg::rescale::TIME_BASE, self.time_base)
    }
}

fn check_dimensions(width: u32, height: u32, budget: u64) -> Result<()> {
    if width == 0
        || height == 0
        || width > 16384
        || height > 16384
        || u64::from(width) * u64::from(height) > budget
    {
        Err("Video source dimensions exceed the frame budget".into())
    } else {
        Ok(())
    }
}

fn display_matrix(bytes: &[u8]) -> Result<[i32; 9]> {
    let bytes = bytes.get(..36).ok_or("Invalid video display matrix")?;
    let mut matrix = [0; 9];
    for (value, bytes) in matrix.iter_mut().zip(bytes.as_chunks::<4>().0) {
        *value = i32::from_ne_bytes(*bytes);
    }
    Ok(matrix)
}

fn orientation(matrix: Option<&[i32; 9]>) -> Result<String> {
    let Some(matrix) = matrix else {
        return Ok(String::new());
    };
    // SAFETY: the owned, aligned array contains the nine native matrix entries.
    let angle = unsafe { ffmpeg::ffi::av_display_rotation_get(matrix.as_ptr()) };
    if !angle.is_finite() {
        return Err("Singular video display matrix".into());
    }
    // libav's reported angle is counterclockwise; display transforms use its
    // inverse. Reflection signs distinguish the two possible transpositions.
    let clockwise = (-(angle.round() as i32)).rem_euclid(360);
    let filters = match clockwise {
        90 if matrix[3] > 0 => "transpose=cclock_flip,".into(),
        90 => "transpose=clock,".into(),
        180 => {
            let mut filters = String::new();
            if matrix[0] < 0 {
                filters.push_str("hflip,");
            }
            if matrix[4] < 0 {
                filters.push_str("vflip,");
            }
            filters
        }
        270 if matrix[3] < 0 => "transpose=clock_flip,".into(),
        270 => "transpose=cclock,".into(),
        0 if matrix[4] < 0 => "vflip,".into(),
        0 => String::new(),
        _ => format!("rotate={clockwise}*PI/180,"),
    };
    Ok(filters)
}

fn scale_spec(matrix: Option<&[i32; 9]>, job: &VideoFrame, pixel: &str) -> Result<String> {
    Ok(format!(
        "{}scale={}:{}:force_original_aspect_ratio=decrease,format=pix_fmts={pixel}",
        orientation(matrix)?,
        job.width,
        job.height
    ))
}

struct FrameFilter {
    _graph: filter::Graph,
    source: filter::Context,
    sink: filter::Context,
}
impl FrameFilter {
    fn new(decoded: &frame::Video, time_base: ffmpeg::Rational, spec: &str) -> Result<Self> {
        let mut graph = filter::Graph::new();
        // SAFETY: this new graph is exclusively owned and not yet configured.
        unsafe {
            (*graph.as_mut_ptr()).nb_threads = 1;
        }
        let aspect = decoded.aspect_ratio();
        let aspect = if aspect.numerator() > 0 && aspect.denominator() > 0 {
            aspect
        } else {
            ffmpeg::Rational(1, 1)
        };
        let args = format!(
            "video_size={}x{}:pix_fmt={}:time_base={}:pixel_aspect={}",
            decoded.width(),
            decoded.height(),
            decoded.format() as i32,
            time_base,
            aspect
        );
        let source = graph
            .add(
                &filter::find("buffer").ok_or("Video buffer unavailable")?,
                "in",
                &args,
            )
            .map_err(message)?;
        let sink = graph
            .add(
                &filter::find("buffersink").ok_or("Video sink unavailable")?,
                "out",
                "",
            )
            .map_err(message)?;
        graph
            .output("in", 0)
            .map_err(message)?
            .input("out", 0)
            .map_err(message)?
            .parse(spec)
            .map_err(message)?;
        graph.validate().map_err(message)?;
        Ok(Self {
            _graph: graph,
            source,
            sink,
        })
    }
}

fn image_encoder(
    image: &frame::Video,
    id: codec::Id,
    quality: Option<i32>,
) -> Result<codec::encoder::video::Encoder> {
    let codec = ffmpeg::encoder::find(id).ok_or("Image encoder unavailable")?;
    let mut encoder = codec::Context::new_with_codec(codec)
        .encoder()
        .video()
        .map_err(message)?;
    encoder.set_width(image.width());
    encoder.set_height(image.height());
    encoder.set_format(image.format());
    encoder.set_time_base((1, 1000));
    if let Some(quality) = quality {
        encoder.set_flags(codec::Flags::QSCALE);
        encoder.set_global_quality(quality);
    }
    encoder.open_as(codec).map_err(message)
}

fn encode_png(image: &frame::Video) -> Result<Vec<u8>> {
    let mut encoder = image_encoder(image, codec::Id::PNG, None)?;
    encoder.send_frame(image).map_err(message)?;
    encoder.send_eof().map_err(message)?;
    let mut bytes = Vec::new();
    loop {
        let mut packet = ffmpeg::Packet::empty();
        match encoder.receive_packet(&mut packet) {
            Ok(()) => {}
            Err(ffmpeg::Error::Eof) => break,
            Err(error) => return Err(message(error)),
        }
        let data = packet.data().ok_or("Empty encoded PNG")?;
        if bytes.len().saturating_add(data.len()) > MAX_IMAGE_BYTES {
            return Err("PNG frame exceeds the output budget".into());
        }
        bytes.extend_from_slice(data);
    }
    if bytes.is_empty() {
        return Err("PNG encoder produced no image".into());
    }
    Ok(bytes)
}

fn message(error: impl std::fmt::Display) -> String {
    error.to_string()
}
