//! Synchronized GStreamer playback with a bounded, clocked application video sink.
use clip_media_protocol::playback::{Control, Options, Settings, Track, Tracks, Update};
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

struct Source {
    file: File,
    start_ms: u64,
    software: bool,
    gpu_scale: bool,
}
impl Source {
    fn open(mut file: File, settings: &Settings) -> Result<Self> {
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
        {
            return Err(
                "Playback requires an ordinary video within the source pixel budget".into(),
            );
        }
        file.rewind().map_err(message)?;
        let number = |value: &serde_json::Value| {
            value
                .as_str()
                .and_then(|value| value.parse::<f64>().ok())
                .filter(|value| value.is_finite() && value.abs() < 9.0e9)
        };
        let container_start = number(&metadata["format"]["start_time"]).unwrap_or(0.0);
        let start_ms =
            ((number(&stream["start_time"]).unwrap_or(container_start) - container_start).max(0.0)
                * 1000.0) as u64;
        let cuda_codec = matches!(stream["codec_name"].as_str(), Some("h264" | "hevc"));
        let cuda_format = stream["cuda_420"].as_bool().unwrap_or(false);
        let small_h264 = stream["codec_name"] == "h264" && width * height <= 1920 * 1080;
        let software = small_h264 || (cuda_codec && !cuda_format);
        let gpu_scale = cuda_codec && cuda_format && !software;
        Ok(Self {
            file,
            start_ms,
            software,
            gpu_scale,
        })
    }
}

pub(crate) fn run(file: File, settings: Settings) -> Result<()> {
    validate(&settings)?;
    let mut source = Source::open(file, &settings)?;
    gst::init().map_err(message)?;
    let mut software = source.software;
    let mut engine = Engine::new(&source, &settings, software).or_else(|error| {
        if software {
            Err(error)
        } else {
            software = true;
            Engine::new(&source, &settings, true)
        }
    })?;
    let mut output = std::io::BufWriter::new(std::io::stdout().lock());
    let mut result = engine.pump(&settings, &mut output);
    if result.is_err() && !software && engine.frames_sent == 0 {
        // A hardware decoder that cannot preroll must not prevent CPU playback.
        // No control commands are consumed until the first frame is delivered.
        drop(engine);
        source.file.rewind().map_err(message)?;
        engine = Engine::new(&source, &settings, true)?;
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
    if !settings.options.valid()
        || settings.width == 0
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
    options: Options,
    tracks_pending: bool,
    streams: Vec<gst::Stream>,
    selected: Vec<String>,
    video_stream: u32,
    silent: bool,
}

impl Engine {
    fn new(source: &Source, settings: &Settings, software: bool) -> Result<Self> {
        if software {
            // Each disposable worker owns one pipeline. Decodebin3 does not
            // implement playbin2's force-sw flag; keep hardware decoders out of
            // this process's candidate list before any decoder is constructed.
            for factory in gst::ElementFactory::factories_with_type(
                gst::ElementFactoryType::DECODER,
                gst::Rank::NONE,
            ) {
                if factory
                    .metadata("klass")
                    .is_some_and(|class| class.split('/').any(|part| part == "Hardware"))
                {
                    factory.set_rank(gst::Rank::NONE);
                }
            }
        }
        let (video, sink) = video_sink(settings, source.gpu_scale && !software)?;
        let player = gst::ElementFactory::make("playbin3")
            .build()
            .map_err(message)?;
        // filesrc supports pull-mode indexing needed by MXF and other seek-heavy
        // demuxers. The descriptor-backed view still names this retained inode,
        // never the original pathname; fdsrc only supplies push-mode reads.
        player.set_property(
            "uri",
            format!("file:///proc/self/fd/{}", source.file.as_raw_fd()),
        );
        player.set_property("video-sink", &video);
        let mut flags = if settings.silent {
            "video+deinterlace".to_owned()
        } else {
            "video+audio+soft-volume+deinterlace".to_owned()
        };
        if settings.options.subtitles_enabled {
            flags.push_str("+text");
        }
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
        if let Ok(tempo) = gst::ElementFactory::make("scaletempo").build() {
            player.set_property("audio-filter", tempo);
        }
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
            stream_start_ms: source.start_ms,
            options: settings.options.clone(),
            tracks_pending: true,
            streams: Vec::new(),
            selected: Vec::new(),
            video_stream: settings.stream,
            silent: settings.silent,
        })
    }

    fn pump(&mut self, settings: &Settings, output: &mut impl Write) -> Result<()> {
        self.start(settings, output)?;
        let mut commands = Commands::new()?;
        let mut last_status = Instant::now();
        loop {
            match commands.poll()? {
                Input::Closed => return Ok(()),
                Input::Control(control) => self.control(control, output)?,
                Input::Idle => {}
            }
            self.events(output)?;
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

    fn start(&mut self, settings: &Settings, output: &mut impl Write) -> Result<()> {
        self.player.set_state(gst::State::Paused).map_err(message)?;
        self.wait_ready()?;
        let first = self
            .sink
            .try_pull_preroll(gst::ClockTime::from_seconds(5))
            .ok_or("No initial video preroll")?;
        self.origin_ms = stream_time(&first).ok_or("Initial video timestamp unavailable")? as i64
            - self.stream_start_ms as i64;
        self.events(output)?;
        self.apply_tracks()?;
        self.seek(settings.start_ms, output)?;
        self.player
            .set_state(gst::State::Playing)
            .map_err(message)?;
        self.playing = true;
        Ok(())
    }
    fn events(&mut self, output: &mut impl Write) -> Result<()> {
        let bus = self.player.bus().ok_or("Playback bus unavailable")?;
        for event in bus.iter() {
            self.observe_tracks(&event);
            match event.view() {
                gst::MessageView::Error(error) => {
                    return Err(format!("{} ({:?})", error.error(), error.debug()));
                }
                gst::MessageView::Eos(_) if self.options.looping => {
                    self.seek(0, output)?;
                    self.player
                        .set_state(gst::State::Playing)
                        .map_err(message)?;
                    self.playing = true;
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
        Ok(())
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
        // Flush the old segment before waiting for new preroll. Waiting here
        // can wait on buffers/clock state from the previous playback rate.
        self.player
            .seek(
                self.options.rate,
                gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
                gst::SeekType::Set,
                gst::ClockTime::from_mseconds(position),
                gst::SeekType::None,
                gst::ClockTime::NONE,
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
        if !control.options.valid()
            || !control.volume.is_finite()
            || !(0.0..=1.0).contains(&control.volume)
            || control.seek_ms.is_some_and(|ms| ms > 9_000_000_000_000)
        {
            return Err("Invalid playback control".into());
        }
        let changed = self.options != control.options;
        let tracks_changed = self.options.audio_track != control.options.audio_track
            || self.options.subtitle_track != control.options.subtitle_track
            || self.options.subtitles_enabled != control.options.subtitles_enabled;
        self.options = control.options;
        if tracks_changed {
            // Stream selection needs data flow even when the viewer is paused.
            // Mute this internal preroll, then restore the exact requested frame
            // and playing state with the flushing seek below.
            self.player.set_property("mute", true);
            self.player
                .set_state(gst::State::Playing)
                .map_err(message)?;
            self.apply_tracks()?;
            self.tracks_pending = true;
        }
        if let Some(position) = control
            .seek_ms
            .or_else(|| changed.then_some(self.position_ms))
        {
            self.epoch = control.epoch;
            self.seek(position, output)?;
        }
        self.player.set_property("volume", control.volume);
        self.player.set_property("mute", false);
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

    fn update(&mut self, time: Option<u64>, ended: bool) -> Update {
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
            tracks: if std::mem::take(&mut self.tracks_pending) {
                Some(self.tracks())
            } else {
                None
            },
        }
    }

    fn streams_of(&self, kind: gst::StreamType) -> impl Iterator<Item = &gst::Stream> {
        self.streams
            .iter()
            .filter(move |s| s.stream_type().contains(kind))
    }

    fn observe_tracks(&mut self, event: &gst::MessageRef) {
        match event.view() {
            gst::MessageView::StreamCollection(collection) => {
                self.streams = collection.stream_collection().iter().take(192).collect();
                self.tracks_pending = true;
            }
            gst::MessageView::StreamsSelected(selected) => {
                self.selected = selected
                    .streams()
                    .filter_map(|s| s.stream_id().map(|s| s.to_string()))
                    .take(192)
                    .collect();
                self.tracks_pending = true;
            }
            _ => {}
        }
    }

    fn desired_streams(&self) -> Result<Vec<String>> {
        let mut desired = Vec::new();
        for (kind, requested, enabled) in [
            (gst::StreamType::VIDEO, Some(self.video_stream), true),
            (
                gst::StreamType::AUDIO,
                self.options.audio_track,
                !self.silent,
            ),
            (
                gst::StreamType::TEXT,
                self.options.subtitle_track,
                self.options.subtitles_enabled,
            ),
        ] {
            if !enabled {
                continue;
            }
            let streams = self.streams_of(kind).take(64).collect::<Vec<_>>();
            let index = requested.map(|n| n as usize).unwrap_or_else(|| {
                streams
                    .iter()
                    .position(|s| {
                        s.stream_id().is_some_and(|id| {
                            self.selected.iter().any(|selected| selected == id.as_str())
                        })
                    })
                    .unwrap_or(0)
            });
            if let Some(stream) = streams.get(index) {
                desired.push(
                    stream
                        .stream_id()
                        .ok_or("Missing playback stream identity")?
                        .to_string(),
                );
            } else if requested.is_some() {
                return Err("Selected playback track unavailable".into());
            }
        }
        Ok(desired)
    }

    fn apply_tracks(&mut self) -> Result<()> {
        let desired = self.desired_streams()?;
        let flags = self.player.property_value("flags");
        let class =
            gst::glib::FlagsClass::with_type(flags.type_()).ok_or("Missing playback flags")?;
        let flags = if self.options.subtitles_enabled {
            class.set_by_nick(flags, "text")
        } else {
            class.unset_by_nick(flags, "text")
        }
        .map_err(|_| "Missing subtitle playback flag")?;
        self.player.set_property("flags", flags);
        if !self.player.send_event(gst::event::SelectStreams::new(
            desired.iter().map(String::as_str),
        )) {
            return Err("Playback stream selection failed".into());
        }
        let bus = self.player.bus().ok_or("Playback bus unavailable")?;
        let deadline = Instant::now() + Duration::from_secs(5);
        while !desired.iter().all(|id| self.selected.contains(id)) {
            if Instant::now() >= deadline {
                return Err("Playback track selection timed out".into());
            }
            if let Some(event) = bus.timed_pop(gst::ClockTime::from_mseconds(100)) {
                self.observe_tracks(&event);
                if let gst::MessageView::Error(error) = event.view() {
                    return Err(message(error.error()));
                }
            }
        }
        self.tracks_pending = true;
        Ok(())
    }

    fn tracks(&self) -> Tracks {
        let collect = |kind| {
            self.streams_of(kind)
                .take(64)
                .enumerate()
                .map(|(index, stream)| {
                    let label = stream
                        .tags()
                        .map(|tags| {
                            [
                                tags.get::<gst::tags::Title>().map(|v| v.get().to_owned()),
                                tags.get::<gst::tags::LanguageCode>()
                                    .map(|v| v.get().to_owned()),
                            ]
                            .into_iter()
                            .flatten()
                            .collect::<Vec<_>>()
                            .join(" · ")
                        })
                        .filter(|label| !label.is_empty())
                        .unwrap_or_else(|| format!("Track {}", index + 1));
                    Track {
                        index: index as u32,
                        label: label
                            .chars()
                            .filter(|c| !c.is_control())
                            .take(100)
                            .collect(),
                    }
                })
                .collect()
        };
        let current = |kind| {
            self.streams_of(kind)
                .position(|s| {
                    s.stream_id().is_some_and(|id| {
                        self.selected.iter().any(|selected| selected == id.as_str())
                    })
                })
                .map(|n| n as u32)
        };
        Tracks {
            audio: collect(gst::StreamType::AUDIO),
            subtitles: collect(gst::StreamType::TEXT),
            current_audio: current(gst::StreamType::AUDIO),
            current_subtitle: current(gst::StreamType::TEXT),
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

fn video_sink(settings: &Settings, gpu: bool) -> Result<(gst::Bin, gstreamer_app::AppSink)> {
    let sink = gstreamer_app::AppSink::builder()
        .caps(&output_caps(settings, false))
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
    let mut pipeline = gpu_filters(settings, gpu)?;
    pipeline.extend(filters);
    video.add_many(&pipeline).map_err(message)?;
    video.add(&sink).map_err(message)?;
    gst::Element::link_many(&pipeline).map_err(message)?;
    pipeline
        .last()
        .ok_or("Missing video filters")?
        .link(&sink)
        .map_err(message)?;
    let pad = gst::GhostPad::with_target(
        &pipeline[0]
            .static_pad("sink")
            .ok_or("Missing video sink pad")?,
    )
    .map_err(message)?;
    video.add_pad(&pad).map_err(message)?;
    Ok((video, sink))
}

fn gpu_filters(settings: &Settings, enabled: bool) -> Result<Vec<gst::Element>> {
    let factories = ["cudaupload", "cudaconvertscale", "cudadownload"];
    if !enabled
        || !factories
            .iter()
            .all(|name| gst::ElementFactory::find(name).is_some())
    {
        return Ok(Vec::new());
    }
    let mut filters = factories
        .map(|name| gst::ElementFactory::make(name).build().map_err(message))
        .into_iter()
        .collect::<Result<Vec<_>>>()?;
    let caps = output_caps(settings, true);
    let cap = gst::ElementFactory::make("capsfilter")
        .property("caps", caps)
        .build()
        .map_err(message)?;
    filters.insert(2, cap);
    Ok(filters)
}

fn output_caps(settings: &Settings, gpu: bool) -> gst::Caps {
    let caps = gst::Caps::builder("video/x-raw")
        .field("format", "RGBA")
        .field("width", gst::IntRange::<i32>::new(1, settings.width as i32))
        .field(
            "height",
            gst::IntRange::<i32>::new(1, settings.height as i32),
        )
        .field("pixel-aspect-ratio", gst::Fraction::new(1, 1));
    if gpu {
        caps.features(["memory:CUDAMemory"]).build()
    } else {
        caps.field(
            "framerate",
            gst::FractionRange::new(
                gst::Fraction::new(0, 1),
                gst::Fraction::new(settings.fps as i32, 1),
            ),
        )
        .build()
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
        let stdin = std::io::stdin();
        let flags = rustix::fs::fcntl_getfl(&stdin).map_err(message)?;
        rustix::fs::fcntl_setfl(&stdin, flags | rustix::fs::OFlags::NONBLOCK).map_err(message)?;
        Ok(Self {
            stdin,
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
