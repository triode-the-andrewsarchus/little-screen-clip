//! Text-only decoding of the same embedded subtitle ordinals used by playback.
mod ebml;
mod matroska;

use super::{Result, message, stream_time};
use clip_media_protocol::{
    VIDEO_FORMATS,
    playback::{MAX_SUBTITLE_BYTES, SUBTITLE_READ_TIMEOUT, SubtitleCue},
};
use gstreamer::{self as gst, prelude::*};
use std::{
    fs::File,
    io::{Seek, Write},
    os::{fd::AsRawFd, unix::fs::MetadataExt},
    sync::Mutex,
    time::{Duration, Instant},
};

const MAX_CUE_BYTES: usize = 16 * 1024;

pub(crate) fn read(
    mut file: File,
    track: u32,
    output: &mut impl Write,
    progress: Option<File>,
) -> Result<()> {
    if track >= 64 || !file.metadata().map_err(message)?.is_file() {
        return Err("Invalid embedded subtitle source or track".into());
    }
    let progress = progress.map(Progress::new).transpose()?;
    // Use playback's ordinary media admission before handing the pinned URI to GStreamer.
    let metadata = crate::probe::probe(file.try_clone().map_err(message)?, VIDEO_FORMATS)?;
    if metadata["format"]["start_time"].as_str() == Some("0")
        && let Some(cues) = matroska::try_read(
            &file,
            track,
            Instant::now() + SUBTITLE_READ_TIMEOUT,
            progress.as_ref(),
        )?
    {
        return write_transcript(cues, output);
    }
    file.rewind().map_err(message)?;
    gst::init().map_err(message)?;
    let reader = Reader::new(&file, track)?;
    reader.write_cues(output)
}

fn write_transcript(cues: Vec<SubtitleCue>, output: &mut impl Write) -> Result<()> {
    output.write_all(b"[").map_err(message)?;
    let mut total = 2;
    for cue in cues {
        write_subtitle(&cue, output, &mut total)?;
    }
    output.write_all(b"]").map_err(message)?;
    output.flush().map_err(message)
}

struct Progress(Mutex<(File, usize)>);

impl Progress {
    fn new(file: File) -> Result<Self> {
        let metadata = file.metadata().map_err(message)?;
        if !metadata.is_file() || metadata.nlink() != 0 || metadata.len() != 0 {
            return Err("Subtitle progress requires an empty private output".into());
        }
        Ok(Self(Mutex::new((file, 0))))
    }

    fn publish(&self, cues: &[SubtitleCue]) -> ebml::Result<()> {
        if cues.is_empty() {
            return Ok(());
        }
        let mut batch = Vec::new();
        for cue in cues {
            serde_json::to_writer(&mut batch, cue).map_err(|_| ebml::Error::Unsupported)?;
            batch.push(b'\n');
            if batch.len() > MAX_SUBTITLE_BYTES {
                return Err(ebml::Error::Limit);
            }
        }
        let mut state = self.0.lock().map_err(|_| ebml::Error::Unsupported)?;
        let (file, written) = &mut *state;
        if *written + batch.len() > MAX_SUBTITLE_BYTES {
            return Err(ebml::Error::Limit);
        }
        file.write_all(&batch)?;
        *written += batch.len();
        Ok(())
    }
}

struct Reader {
    pipeline: gst::Pipeline,
    sink: gstreamer_app::AppSink,
}

impl Reader {
    fn new(file: &File, track: u32) -> Result<Self> {
        let pipeline = gst::Pipeline::new();
        // Sparse subtitles can stall demuxing if video has nowhere to go.
        // Drain parsed video packets without opening a video decoder.
        let source = gst::ElementFactory::make("uridecodebin3")
            .property("uri", format!("file:///proc/self/fd/{}", file.as_raw_fd()))
            .property("caps", gst::Caps::new_any())
            .build()
            .map_err(message)?;
        let drain = gst::ElementFactory::make("fakesink")
            .property("sync", false)
            .property("async", false)
            .build()
            .map_err(message)?;
        let text_caps = "text/x-raw;application/x-ass;application/x-ssa"
            .parse::<gst::Caps>()
            .map_err(message)?;
        let sink = gstreamer_app::AppSink::builder()
            .caps(&text_caps)
            .sync(false)
            .async_(false)
            .max_buffers(8)
            .build();
        pipeline
            .add_many([&source, &drain, sink.upcast_ref()])
            .map_err(message)?;
        source.connect("select-stream", false, move |values| {
            Some(i32::from(select_stream(values, track).unwrap_or(false)).to_value())
        });
        let target = sink.static_pad("sink").ok_or("Subtitle sink unavailable")?;
        let discard = drain.static_pad("sink").ok_or("Video drain unavailable")?;
        source.connect_pad_added(move |element, pad| {
            if pad.name().starts_with("text_") {
                if pad.link(&target).is_err() {
                    gst::element_error!(
                        element,
                        gst::StreamError::Format,
                        ("Subtitle track has no searchable text")
                    );
                }
            } else if pad.name().starts_with("video_") {
                let _ = pad.link(&discard);
            }
        });
        Ok(Self { pipeline, sink })
    }

    fn write_cues(&self, output: &mut impl Write) -> Result<()> {
        let bus = self
            .pipeline
            .bus()
            .ok_or("Subtitle decoder bus unavailable")?;
        let deadline = Instant::now() + Duration::from_secs(30);
        self.pipeline
            .set_state(gst::State::Playing)
            .map_err(message)?;
        output.write_all(b"[").map_err(message)?;
        let mut total = 2;
        while Instant::now() < deadline {
            if let Some(sample) = self.sink.try_pull_sample(gst::ClockTime::from_mseconds(20)) {
                write_cue(&sample, output, &mut total)?;
            }
            if let Some(event) = bus.pop_filtered(&[gst::MessageType::Error, gst::MessageType::Eos])
            {
                match event.view() {
                    gst::MessageView::Error(error) => {
                        return Err(message(error.error()));
                    }
                    gst::MessageView::Eos(_) => {
                        output.write_all(b"]").map_err(message)?;
                        return output.flush().map_err(message);
                    }
                    _ => {}
                }
            }
        }
        Err("Reading the subtitle track timed out".into())
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

fn select_stream(values: &[gst::glib::Value], track: u32) -> Option<bool> {
    let collection = values.get(1)?.get::<gst::StreamCollection>().ok()?;
    let stream = values.get(2)?.get::<gst::Stream>().ok()?;
    let (kind, index) = if stream.stream_type().contains(gst::StreamType::VIDEO) {
        (gst::StreamType::VIDEO, 0)
    } else {
        (gst::StreamType::TEXT, track as usize)
    };
    let selected = collection
        .iter()
        .filter(|item| item.stream_type().contains(kind))
        .nth(index)?;
    Some(selected == stream)
}

fn write_cue(sample: &gst::Sample, output: &mut impl Write, total: &mut usize) -> Result<()> {
    let start_ms = stream_time(sample).ok_or("Subtitle timestamp unavailable")?;
    let buffer = sample.buffer().ok_or("Missing subtitle text")?;
    if buffer.size() > MAX_CUE_BYTES {
        return Err("A subtitle cue exceeds the search limit".into());
    }
    let map = buffer.map_readable().map_err(message)?;
    let text = std::str::from_utf8(map.as_slice())
        .map_err(message)?
        .trim_matches('\0');
    if text.trim().is_empty() {
        return Ok(());
    }
    let caps = sample.caps().and_then(|caps| caps.structure(0));
    let markup = caps.and_then(|caps| caps.get::<&str>("format").ok()) == Some("pango-markup");
    let text = if caps.is_some_and(|caps| {
        matches!(
            caps.name().as_str(),
            "application/x-ass" | "application/x-ssa"
        )
    }) {
        ass_dialogue(text)?
    } else {
        text.into()
    };
    write_subtitle(
        &SubtitleCue {
            start_ms,
            text,
            markup,
        },
        output,
        total,
    )
}

fn write_subtitle(cue: &SubtitleCue, output: &mut impl Write, total: &mut usize) -> Result<()> {
    let cue = serde_json::to_vec(cue).map_err(message)?;
    let separator = usize::from(*total > 2);
    if *total + cue.len() + separator > MAX_SUBTITLE_BYTES {
        return Err("The subtitle track exceeds the search limit".into());
    }
    if separator != 0 {
        output.write_all(b",").map_err(message)?;
    }
    output.write_all(&cue).map_err(message)?;
    *total += cue.len() + separator;
    Ok(())
}

fn ass_dialogue(packet: &str) -> Result<String> {
    // Matroska transfers the eight dialogue fields before the caption itself.
    let mut remainder = packet
        .splitn(9, ',')
        .nth(8)
        .ok_or("Invalid ASS dialogue packet")?;
    let mut text = String::new();
    while let Some((before, tag)) = remainder.split_once("{\\")
        && let Some((_, after)) = tag.split_once('}')
    {
        text.push_str(before);
        remainder = after;
    }
    text.push_str(remainder);
    Ok(text
        .replace("\\N", " ")
        .replace("\\n", " ")
        .replace("\\h", " "))
}
