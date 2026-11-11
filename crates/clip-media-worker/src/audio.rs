//! Packet-sized float PCM preparation for codecs without a native decoder.

use ffmpeg_next as ffmpeg;
use std::{fs::File, io::Write};

const MAX_PCM: usize = 256 * 1024 * 1024;

pub(super) fn decode(file: File, formats: &str, output: &mut impl Write) -> Result<(), String> {
    let mut input = crate::probe::open_input(file, formats)?;
    let stream = input
        .streams()
        .find(|stream| stream.parameters().medium() == ffmpeg::media::Type::Audio)
        .ok_or("No audio stream")?;
    let index = stream.index();
    let mut context = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
        .map_err(|error| error.to_string())?;
    context.set_threading(ffmpeg::codec::threading::Config {
        count: 1,
        ..Default::default()
    });
    let mut decoder = context.decoder();
    decoder.set_packet_time_base(stream.time_base());
    let mut decoder = decoder.audio().map_err(|error| error.to_string())?;
    if !(1..=8).contains(&decoder.channels()) || !(1..=384_000).contains(&decoder.rate()) {
        return Err("Unsupported audio channel count or sample rate".into());
    }
    let layout = if decoder.channel_layout().bits() == 0 {
        ffmpeg::ChannelLayout::default(i32::from(decoder.channels()))
    } else {
        decoder.channel_layout()
    };
    decoder.set_channel_layout(layout);
    let mut converter = ffmpeg::software::resampling::Context::get(
        decoder.format(),
        layout,
        decoder.rate(),
        ffmpeg::format::Sample::F32(ffmpeg::format::sample::Type::Packed),
        layout,
        decoder.rate(),
    )
    .map_err(|error| error.to_string())?;
    let mut written = 0;
    loop {
        let mut packet = ffmpeg::Packet::empty();
        match packet.read(&mut input) {
            Ok(()) => {}
            Err(ffmpeg::Error::Eof) => break,
            Err(error) => return Err(error.to_string()),
        }
        if packet.stream() == index {
            decoder
                .send_packet(&packet)
                .map_err(|error| error.to_string())?;
            drain(&mut decoder, &mut converter, output, &mut written)?;
        }
    }
    decoder.send_eof().map_err(|error| error.to_string())?;
    drain(&mut decoder, &mut converter, output, &mut written)?;
    // Same-rate format conversion normally has no delay; preserve buffered
    // samples if a decoder/converter combination does retain a tail.
    while converter.delay().is_some() {
        let mut tail = ffmpeg::frame::Audio::new(
            converter.output().format,
            4096,
            converter.output().channel_layout,
        );
        converter
            .flush(&mut tail)
            .map_err(|error| error.to_string())?;
        if tail.samples() == 0 && converter.delay().is_some() {
            return Err("Audio converter could not drain its remaining samples".into());
        }
        write_frame(&tail, output, &mut written)?;
    }
    if written == 0 {
        return Err("No decoded audio samples".into());
    }
    output.flush().map_err(|error| error.to_string())
}

fn drain(
    decoder: &mut ffmpeg::codec::decoder::Audio,
    converter: &mut ffmpeg::software::resampling::Context,
    output: &mut impl Write,
    written: &mut usize,
) -> Result<(), String> {
    loop {
        let mut decoded = ffmpeg::frame::Audio::empty();
        match decoder.receive_frame(&mut decoded) {
            Ok(()) => {}
            Err(ffmpeg::Error::Eof) => return Ok(()),
            Err(ffmpeg::Error::Other { errno }) if errno == libc::EAGAIN => return Ok(()),
            Err(error) => return Err(error.to_string()),
        }
        if decoded.rate() != converter.input().rate || decoded.channels() != decoder.channels() {
            return Err("Audio format changed during decoding".into());
        }
        if decoded.channel_layout().bits() == 0 {
            decoded.set_channel_layout(converter.input().channel_layout);
        }
        let mut converted = ffmpeg::frame::Audio::empty();
        converter
            .run(&decoded, &mut converted)
            .map_err(|error| error.to_string())?;
        write_frame(&converted, output, written)?;
    }
}

fn write_frame(
    frame: &ffmpeg::frame::Audio,
    output: &mut impl Write,
    written: &mut usize,
) -> Result<(), String> {
    if frame.samples() == 0 {
        return Ok(());
    }
    let length = frame
        .samples()
        .checked_mul(usize::from(frame.channels()))
        .and_then(|samples| samples.checked_mul(4))
        .ok_or("Audio size overflow")?;
    *written = written
        .checked_add(length)
        .filter(|size| *size <= MAX_PCM)
        .ok_or("Decoded audio exceeds the 256 MiB source budget")?;
    let bytes = frame
        .data(0)
        .get(..length)
        .ok_or("Invalid float PCM frame")?;
    #[cfg(target_endian = "little")]
    output.write_all(bytes).map_err(|error| error.to_string())?;
    #[cfg(target_endian = "big")]
    for sample in bytes.chunks_exact(4) {
        let value = f32::from_ne_bytes(sample.try_into().map_err(|_| "Invalid float sample")?);
        output
            .write_all(&value.to_le_bytes())
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compressed_audio_decodes_into_interleaved_finite_float_pcm() {
        let mut input = tempfile::tempfile().unwrap();
        input
            .write_all(include_bytes!("../tests/fixtures/silence.aac"))
            .unwrap();
        let mut output = Vec::new();
        decode(input, "aac", &mut output).unwrap();
        assert!(!output.is_empty());
        assert_eq!(output.len() % 8, 0);
        assert!(
            output
                .chunks_exact(4)
                .all(|sample| f32::from_le_bytes(sample.try_into().unwrap()).is_finite())
        );
    }
}
