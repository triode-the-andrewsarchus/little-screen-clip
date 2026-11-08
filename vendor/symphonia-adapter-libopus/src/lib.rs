#![warn(missing_docs, missing_debug_implementations)]
#![forbid(clippy::unwrap_used)]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc = include_str!("../README.md")]

use std::fmt;

use symphonia_core::audio::{
    AsAudioBufferRef, AudioBuffer, AudioBufferRef, Channels, Layout, Signal, SignalSpec,
};
use symphonia_core::codecs::{
    self, CODEC_TYPE_OPUS, CodecDescriptor, CodecParameters, DecoderOptions, FinalizeResult,
};
use symphonia_core::errors::{Result, unsupported_error};
use symphonia_core::formats::Packet;
use symphonia_core::io::{BufReader, ReadBytes};
use symphonia_core::support_codec;

use crate::decoder::Decoder;

mod decoder;

/// Maximum sampling rate is 48 kHz for normal opus, and 96 kHz for Opus HD in the 1.6 spec.
const MAX_SAMPLE_RATE: usize = 48000;
const DEFAULT_SAMPLE_RATE: usize = 48000;
/// Assuming 48 kHz sample rate with the default 20 ms frames.
const DEFAULT_SAMPLES_PER_CHANNEL: usize = DEFAULT_SAMPLE_RATE * 20 / 1000;
/// Opus maximum frame size is 60 ms, with worst case being 120 ms when combining frames per packet.
const MAX_SAMPLES_PER_CHANNEL: usize = MAX_SAMPLE_RATE * 120 / 1000;

/// Symphonia-compatible wrapper for the libopus decoder.
pub struct OpusDecoder {
    params: CodecParameters,
    decoder: Decoder,
    buf: AudioBuffer<f32>,
    pcm: [f32; MAX_SAMPLES_PER_CHANNEL * 2],
    samples_per_channel: usize,
    sample_rate: u32,
    num_channels: usize,
    pre_skip: usize,
    initial_pre_skip: usize,
    reset_pending: bool,
}

impl fmt::Debug for OpusDecoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpusDecoder")
            .field("params", &self.params)
            .field("decoder", &self.decoder)
            .field("buf", &"<buf>")
            .field("pcm", &self.pcm)
            .field("samples_per_channel", &self.samples_per_channel)
            .field("sample_rate", &self.sample_rate)
            .field("num_channels", &self.num_channels)
            .field("pre_skip", &self.pre_skip)
            .field("initial_pre_skip", &self.initial_pre_skip)
            .field("reset_pending", &self.reset_pending)
            .finish()
    }
}

// This should probably be handled in the Ogg demuxer, but we'll include it here for now.
fn parse_pre_skip(buf: &[u8]) -> Result<usize> {
    // See https://wiki.xiph.org/OggOpus

    let mut reader = BufReader::new(buf);

    // Header - "OpusHead"
    let mut header = [0; 8];
    reader.read_buf_exact(&mut header)?;

    // Version - 1 is the only valid version currently
    reader.read_byte()?;

    // Number of channels (same as what we get from the CodecParameters)
    reader.read_byte()?;

    // Pre-skip - number of samples (at 48 kHz) to discard from the start of the stream
    let pre_skip = reader.read_u16()?;

    Ok(pre_skip as usize)
}

impl codecs::Decoder for OpusDecoder {
    fn try_new(params: &CodecParameters, _opts: &DecoderOptions) -> Result<Self>
    where
        Self: Sized,
    {
        let channels = params
            .channel_layout
            .map(|c| c.into_channels())
            .or(params.channels);

        let num_channels = if let Some(channels) = &channels {
            channels.count()
        } else {
            return unsupported_error("opus: channels or channel layout is required");
        };
        let sample_rate = if let Some(sample_rate) = params.sample_rate {
            sample_rate
        } else {
            return unsupported_error("opus: sample rate required");
        };

        if !(1..=2).contains(&num_channels) {
            return unsupported_error("opus: unsupported number of channels");
        }

        let pre_skip = if let Some(extra_data) = &params.extra_data {
            parse_pre_skip(extra_data).unwrap_or_default()
        } else {
            0
        };

        Ok(Self {
            params: params.to_owned(),
            decoder: Decoder::new(sample_rate, num_channels as u32)?,
            buf: audio_buffer(
                sample_rate,
                DEFAULT_SAMPLES_PER_CHANNEL as u64,
                num_channels,
            ),
            pcm: [0.0; _],
            samples_per_channel: DEFAULT_SAMPLES_PER_CHANNEL,
            sample_rate,
            num_channels,
            pre_skip,
            initial_pre_skip: pre_skip,
            reset_pending: false,
        })
    }

    fn supported_codecs() -> &'static [CodecDescriptor]
    where
        Self: Sized,
    {
        &[support_codec!(CODEC_TYPE_OPUS, "opus", "Opus")]
    }

    fn reset(&mut self) {
        self.decoder.reset();
        self.reset_pending = true;
    }

    fn codec_params(&self) -> &CodecParameters {
        &self.params
    }

    fn decode(&mut self, packet: &Packet) -> Result<AudioBufferRef<'_>> {
        let samples_per_channel = self.decoder.decode(&packet.data, &mut self.pcm)?;
        if self.reset_pending {
            self.pre_skip = if packet.ts() == 0 {
                self.initial_pre_skip
            } else {
                0
            };
            self.reset_pending = false;
        }

        if samples_per_channel != self.samples_per_channel {
            self.buf = audio_buffer(
                self.sample_rate,
                samples_per_channel as u64,
                self.num_channels,
            );
            self.samples_per_channel = samples_per_channel;
        }

        let samples = samples_per_channel * self.num_channels;
        let pcm = &self.pcm[..samples];

        self.buf.clear();
        self.buf.render_reserved(None);
        match self.num_channels {
            1 => {
                self.buf.chan_mut(0).copy_from_slice(pcm);
            }
            2 => {
                let (l, r) = self.buf.chan_pair_mut(0, 1);
                for (i, j) in (0..samples).step_by(2).enumerate() {
                    l[i] = pcm[j];
                    r[i] = pcm[j + 1];
                }
            }
            _ => {}
        }

        // Header delay and demuxer trimming describe the same leading frames.
        let header_trim = (self.pre_skip * self.sample_rate as usize) / DEFAULT_SAMPLE_RATE;
        self.buf.trim(
            (packet.trim_start() as usize)
                .max(header_trim)
                .min(samples_per_channel),
            packet.trim_end() as usize,
        );
        self.pre_skip = self
            .pre_skip
            .saturating_sub(samples_per_channel * DEFAULT_SAMPLE_RATE / self.sample_rate as usize);
        Ok(self.buf.as_audio_buffer_ref())
    }

    fn finalize(&mut self) -> FinalizeResult {
        FinalizeResult::default()
    }

    fn last_decoded(&self) -> AudioBufferRef<'_> {
        self.buf.as_audio_buffer_ref()
    }
}

fn map_to_channels(num_channels: usize) -> Option<Channels> {
    let channels = match num_channels {
        1 => Layout::Mono.into_channels(),
        2 => Layout::Stereo.into_channels(),
        _ => return None,
    };

    Some(channels)
}

fn audio_buffer(
    sample_rate: u32,
    samples_per_channel: u64,
    num_channels: usize,
) -> AudioBuffer<f32> {
    let channels = map_to_channels(num_channels).expect("invalid channels");
    let spec = SignalSpec::new(sample_rate, channels);
    AudioBuffer::new(samples_per_channel, spec)
}

#[cfg(test)]
mod local_tests {
    use super::*;
    use symphonia_core::codecs::Decoder as _;

    fn decoder(pre_skip: u16) -> Result<OpusDecoder> {
        let mut params = CodecParameters::new();
        let mut header = b"OpusHead\x01\x02".to_vec();
        header.extend(pre_skip.to_le_bytes());
        header.extend(48000u32.to_le_bytes());
        header.extend([0; 3]);
        params
            .for_codec(CODEC_TYPE_OPUS)
            .with_sample_rate(48000)
            .with_channels(Layout::Stereo.into_channels())
            .with_extra_data(header.into_boxed_slice());
        OpusDecoder::try_new(&params, &DecoderOptions::default())
    }

    fn silence(ts: u64) -> Packet {
        // A 20 ms Opus silence packet. Timestamps use the 48 kHz Opus clock.
        Packet::new_from_slice(0, ts, 960, &[0xf8, 0xff, 0xfe])
    }

    #[test]
    fn rewind_restores_header_delay_but_later_seeks_do_not() -> Result<()> {
        let mut decoder = decoder(312)?;
        assert_eq!(decoder.decode(&silence(0))?.frames(), 648);
        decoder.reset();
        assert_eq!(decoder.decode(&silence(0))?.frames(), 648);
        decoder.reset();
        assert_eq!(decoder.decode(&silence(960))?.frames(), 960);
        Ok(())
    }

    #[test]
    fn demuxer_and_header_delay_are_not_counted_twice() -> Result<()> {
        let mut decoder = decoder(312)?;
        let mut packet = silence(0);
        packet.trim_start = 312;
        assert_eq!(decoder.decode(&packet)?.frames(), 648);
        let mut last = silence(960);
        last.trim_end = 120;
        assert_eq!(decoder.decode(&last)?.frames(), 840);
        Ok(())
    }

    #[test]
    fn header_delay_spanning_multiple_packets_is_fully_removed() -> Result<()> {
        let mut decoder = decoder(2000)?;
        assert_eq!(decoder.decode(&silence(0))?.frames(), 0);
        assert_eq!(decoder.decode(&silence(960))?.frames(), 0);
        assert_eq!(decoder.decode(&silence(1920))?.frames(), 880);
        decoder.reset();
        assert_eq!(decoder.decode(&silence(1920))?.frames(), 960);
        Ok(())
    }
}
