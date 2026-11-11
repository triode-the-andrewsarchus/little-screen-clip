//! Synchronized GStreamer playback with a bounded, clocked application video sink.
use clip_media_protocol::playback::{Control, Settings, Update};
use gstreamer::{self as gst, prelude::*};
use gstreamer_video::VideoFrameExt;
use std::{
    fs::File,
    io::{Read, Seek, Write},
    os::fd::AsRawFd,
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, String>;
fn message(error: impl std::fmt::Display) -> String {
    error.to_string()
}

pub(crate) fn run(mut file: File, settings: Settings) -> Result<()> {
    validate(&settings)?;
    if !file.metadata().map_err(message)?.is_file() {
        return Err("Playback requires an ordinary seekable file".into());
    }
    let metadata = crate::probe::probe(
        file.try_clone().map_err(message)?,
        clip_media_protocol::VIDEO_FORMATS,
    )?;
    let stream = metadata["streams"]
        .as_array()
        .ok_or("Missing video metadata")?
        .iter()
        .filter(|stream| {
            stream["codec_type"] == "video"
                && stream["disposition"]["attached_pic"].as_u64() != Some(1)
        })
        .nth(settings.stream as usize)
        .ok_or("Video stream unavailable")?;
    let width = stream["width"].as_u64().unwrap_or(0);
    let height = stream["height"].as_u64().unwrap_or(0);
    if width == 0
        || height == 0
        || width > 4096
        || height > 4096
        || width * height > 4096 * 2160
        || stream["disposition"]["attached_pic"].as_u64() == Some(1)
    {
        return Err("Playback requires an ordinary video within the source pixel budget".into());
    }
    file.rewind().map_err(message)?;
    gst::init().map_err(message)?;
    let number = |value: &serde_json::Value| {
        value
            .as_str()
            .and_then(|value| value.parse::<f64>().ok())
            .filter(|value| value.is_finite() && value.abs() < 9.0e9)
    };
    let container_start = number(&metadata["format"]["start_time"]).unwrap_or(0.0);
    let start_ms = ((number(&stream["start_time"]).unwrap_or(container_start) - container_start)
        .max(0.0)
        * 1000.0) as u64;
    let software = stream["codec_name"] == "h264" && width * height <= 1920 * 1080;
    let mut engine = Engine::new(&file, &settings, software, start_ms)?;
    let mut output = std::io::BufWriter::new(std::io::stdout().lock());
    let mut result = engine.pump(&settings, &mut output);
    if result.is_err() && !software && engine.frames_sent == 0 {
        // A hardware decoder that cannot preroll must not prevent CPU playback.
        // No control commands are consumed until the first frame is delivered.
        drop(engine);
        file.rewind().map_err(message)?;
        engine = Engine::new(&file, &settings, true, start_ms)?;
        result = engine.pump(&settings, &mut output);
    }
    if let Err(error) = &result {
        let mut update = engine.update(None, false);
        update.error = Some(error.chars().take(1000).collect());
        let _ = update.write(&[], &mut output);
    }
    result
}

fn validate(settings: &Settings) -> Result<()> {
    if settings.width == 0
        || settings.height == 0
        || settings.width > 1920
        || settings.height > 1920
        || u64::from(settings.width) * u64::from(settings.height) > 1920 * 1080
        || !(1..=60).contains(&settings.fps)
        || settings.stream > 64
        || settings.start_ms > 9_000_000_000_000
        || settings.duration_ms > 9_000_000_000_000
    {
        return Err("Invalid playback settings".into());
    }
    Ok(())
}

struct Engine {
    player: gst::Element,
    sink: gstreamer_app::AppSink,
    epoch: u64,
    playing: bool,
    ended: bool,
    frames_sent: u64,
    origin_ms: i64,
    position_ms: u64,
    duration_ms: u64,
    stream_start_ms: u64,
}

impl Engine {
    fn new(file: &File, settings: &Settings, software: bool, stream_start_ms: u64) -> Result<Self> {
        let sink = gstreamer_app::AppSink::builder()
            .caps(
                &gst::Caps::builder("video/x-raw")
                    .field("format", "RGBA")
                    .field("width", gst::IntRange::<i32>::new(1, settings.width as i32))
                    .field(
                        "height",
                        gst::IntRange::<i32>::new(1, settings.height as i32),
                    )
                    .field("pixel-aspect-ratio", gst::Fraction::new(1, 1))
                    .field(
                        "framerate",
                        gst::FractionRange::new(
                            gst::Fraction::new(0, 1),
                            gst::Fraction::new(settings.fps as i32, 1),
                        ),
                    )
                    .build(),
            )
            .max_buffers(1)
            .drop(true)
            .sync(true)
            .wait_on_eos(false)
            .build();
        let video = gst::Bin::new();
        let filters = ["videoflip", "videoconvert", "videoscale", "videorate"]
            .map(|name| gst::ElementFactory::make(name).build().map_err(message))
            .into_iter()
            .collect::<Result<Vec<_>>>()?;
        filters[0].set_property_from_str("video-direction", "auto");
        filters[3].set_property("drop-only", true);
        video.add_many(&filters).map_err(message)?;
        video.add(&sink).map_err(message)?;
        gst::Element::link_many(&filters).map_err(message)?;
        filters[3].link(&sink).map_err(message)?;
        let pad = gst::GhostPad::with_target(
            &filters[0]
                .static_pad("sink")
                .ok_or("Missing video sink pad")?,
        )
        .map_err(message)?;
        video.add_pad(&pad).map_err(message)?;
        let player = gst::ElementFactory::make("playbin")
            .build()
            .map_err(message)?;
        player.set_property("uri", format!("fd://{}", file.as_raw_fd()));
        player.set_property("video-sink", &video);
        let mut flags = if settings.silent {
            "video+deinterlace".to_owned()
        } else {
            "video+audio+soft-volume+deinterlace".to_owned()
        };
        if software {
            flags.push_str("+force-sw-decoders");
        }
        player.set_property_from_str("flags", &flags);
        player.connect("element-setup", false, |values| {
            if let Ok(element) = values[1].get::<gst::Element>()
                && element.find_property("max-threads").is_some()
            {
                element.set_property_from_str("max-threads", "2");
            }
            None
        });
        if settings.silent {
            let audio = gst::ElementFactory::make("fakesink")
                .property("sync", true)
                .build()
                .map_err(message)?;
            player.set_property("audio-sink", audio);
        }
        Ok(Self {
            player,
            sink,
            epoch: 0,
            playing: false,
            ended: false,
            frames_sent: 0,
            origin_ms: 0,
            position_ms: settings.start_ms,
            duration_ms: settings.duration_ms,
            stream_start_ms,
        })
    }

    fn pump(&mut self, settings: &Settings, output: &mut impl Write) -> Result<()> {
        self.player.set_state(gst::State::Paused).map_err(message)?;
        self.wait_ready()?;
        let first = self
            .sink
            .try_pull_preroll(gst::ClockTime::from_seconds(5))
            .ok_or("No initial video preroll")?;
        self.origin_ms = stream_time(&first).ok_or("Initial video timestamp unavailable")? as i64
            - self.stream_start_ms as i64;
        if settings.stream as i32 >= self.player.property::<i32>("n-video") {
            return Err("Selected video stream unavailable".into());
        }
        self.player
            .set_property("current-video", settings.stream as i32);
        self.seek(settings.start_ms, output)?;
        self.player
            .set_state(gst::State::Playing)
            .map_err(message)?;
        self.playing = true;
        let mut commands = Commands::new()?;
        let mut last_status = Instant::now();
        loop {
            match commands.poll()? {
                Input::Closed => return Ok(()),
                Input::Control(control) => self.control(control, output)?,
                Input::Idle => {}
            }
            let bus = self.player.bus().ok_or("Playback bus unavailable")?;
            for event in bus.iter() {
                match event.view() {
                    gst::MessageView::Error(error) => {
                        return Err(format!("{} ({:?})", error.error(), error.debug()));
                    }
                    gst::MessageView::Eos(_) => {
                        self.playing = false;
                        self.ended = true;
                        self.player.set_state(gst::State::Paused).map_err(message)?;
                        self.update(None, true)
                            .write(&[], output)
                            .map_err(message)?;
                    }
                    _ => {}
                }
            }
            if self.playing {
                if let Some(sample) = self.sink.try_pull_sample(gst::ClockTime::from_mseconds(10)) {
                    self.frame(sample, output)?;
                }
            } else {
                std::thread::sleep(Duration::from_millis(10));
            }
            if last_status.elapsed() >= Duration::from_millis(250) {
                self.update(None, self.ended)
                    .write(&[], output)
                    .map_err(message)?;
                last_status = Instant::now();
            }
        }
    }

    fn wait_ready(&self) -> Result<()> {
        let (result, state, _) = self.player.state(gst::ClockTime::from_seconds(10));
        result.map_err(message)?;
        if state != gst::State::Paused {
            return Err("Playback preroll timed out".into());
        }
        Ok(())
    }

    fn seek(&mut self, position: u64, output: &mut impl Write) -> Result<()> {
        self.player.set_state(gst::State::Paused).map_err(message)?;
        self.wait_ready()?;
        self.player
            .seek_simple(
                gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
                gst::ClockTime::from_mseconds(position),
            )
            .map_err(message)?;
        self.wait_ready()?;
        let sample = self
            .sink
            .try_pull_preroll(gst::ClockTime::from_seconds(5))
            .ok_or("No frame at the requested position")?;
        self.ended = false;
        self.frame(sample, output)
    }

    fn control(&mut self, control: Control, output: &mut impl Write) -> Result<()> {
        if !control.volume.is_finite()
            || !(0.0..=1.0).contains(&control.volume)
            || control.seek_ms.is_some_and(|ms| ms > 9_000_000_000_000)
        {
            return Err("Invalid playback control".into());
        }
        if let Some(position) = control.seek_ms {
            self.epoch = control.epoch;
            self.seek(position, output)?;
        }
        self.player.set_property("volume", control.volume);
        self.player
            .set_state(if control.playing {
                gst::State::Playing
            } else {
                gst::State::Paused
            })
            .map_err(message)?;
        self.playing = control.playing;
        self.update(None, self.ended)
            .write(&[], output)
            .map_err(message)
    }

    fn update(&self, time: Option<u64>, ended: bool) -> Update {
        Update {
            epoch: self.epoch,
            position_ms: time.unwrap_or(self.position_ms),
            duration_ms: self.duration_ms,
            playing: self.playing,
            ended,
            width: 0,
            height: 0,
            bytes: 0,
            error: None,
        }
    }

    fn frame(&mut self, sample: gst::Sample, output: &mut impl Write) -> Result<()> {
        let buffer = sample.buffer().ok_or("Missing video buffer")?;
        let info =
            gstreamer_video::VideoInfo::from_caps(sample.caps().ok_or("Missing video caps")?)
                .map_err(message)?;
        let frame = gstreamer_video::VideoFrameRef::from_buffer_ref_readable(buffer, &info)
            .map_err(message)?;
        let width = info.width();
        let height = info.height();
        let size = width as usize * height as usize * 4;
        if size > clip_media_protocol::playback::MAX_FRAME_BYTES {
            return Err("Playback frame exceeds budget".into());
        }
        let stride = usize::try_from(frame.plane_stride()[0]).map_err(message)?;
        let plane = frame.plane_data(0).map_err(message)?;
        let mut pixels = Vec::with_capacity(size);
        for row in 0..height as usize {
            pixels.extend_from_slice(
                plane
                    .get(row * stride..row * stride + width as usize * 4)
                    .ok_or("Incomplete video plane")?,
            );
        }
        let time = stream_time(&sample).map(|time| (time as i64 - self.origin_ms).max(0) as u64);
        if let Some(time) = time {
            self.position_ms = time;
        }
        let mut update = self.update(time, false);
        update.width = width;
        update.height = height;
        update.bytes = pixels.len();
        update.write(&pixels, output).map_err(message)?;
        self.frames_sent += 1;
        Ok(())
    }
}

fn stream_time(sample: &gst::Sample) -> Option<u64> {
    let pts = sample.buffer()?.pts()?;
    sample
        .segment()?
        .downcast_ref::<gst::ClockTime>()?
        .to_stream_time(pts)
        .map(|time| time.mseconds())
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.player.set_state(gst::State::Null);
    }
}

enum Input {
    Idle,
    Closed,
    Control(Control),
}
struct Commands {
    stdin: std::io::Stdin,
    bytes: Vec<u8>,
}
impl Commands {
    fn new() -> Result<Self> {
        // SAFETY: stdin is this disposable worker's owned control pipe.
        let flags = unsafe { libc::fcntl(0, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(0, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err(message(std::io::Error::last_os_error()));
        }
        Ok(Self {
            stdin: std::io::stdin(),
            bytes: Vec::new(),
        })
    }
    fn poll(&mut self) -> Result<Input> {
        let mut newest = None;
        let mut buffer = [0; 1024];
        loop {
            match self.stdin.read(&mut buffer) {
                Ok(0) => return Ok(Input::Closed),
                Ok(count) => self.bytes.extend_from_slice(&buffer[..count]),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(message(error)),
            }
            while let Some(end) = self.bytes.iter().position(|byte| *byte == b'\n') {
                if end > 1024 {
                    return Err("Playback command exceeds budget".into());
                }
                let control: Control =
                    serde_json::from_slice(&self.bytes[..end]).map_err(message)?;
                // A later volume/pause update must retain an earlier pending seek.
                let control = if let Some(previous) = newest.take() {
                    merge(previous, control)
                } else {
                    control
                };
                newest = Some(control);
                self.bytes.drain(..=end);
            }
            if self.bytes.len() > 1024 {
                return Err("Playback command exceeds budget".into());
            }
        }
        Ok(newest.map_or(Input::Idle, Input::Control))
    }
}
fn merge(previous: Control, mut next: Control) -> Control {
    if next.seek_ms.is_none() && next.epoch == previous.epoch {
        next.seek_ms = previous.seek_ms;
    }
    next
}
