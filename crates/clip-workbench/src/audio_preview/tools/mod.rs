//! Audio Studio tools. The inspect editor is shared by standalone inspection and stem editing.
mod digital_decode;
pub(crate) mod export_image;
pub(crate) mod hf_analysis;
mod inspect;
pub(crate) mod spectrogram_view;
mod stem_separation;
pub(super) use digital_decode::DigitalDecodeState;
pub(super) use export_image::{render_spectrum_image, render_waveform_image};
#[allow(unused_imports)]
pub(super) use hf_analysis::{HfAnalysisState, HfPlaybackMode};
#[allow(unused_imports)]
pub(super) use spectrogram_view::{
    ViewportSpectrogramCache, format_time_label, frequency_ticks, paint_frequency_axis_and_grid,
    paint_spectrogram_surface, render_and_paint_spectrogram, time_ruler_interval,
};
pub(super) use stem_separation::StemSeparationState;
#[cfg(test)]
pub(super) use stem_separation::{
    audacity_lof_contents, editor_stem_switch, parse_stem_manifest,
    prepare_isolated_audacity_command, stem_is_audible, waveform_scrub_time,
};

mod channel_tools;
pub(super) use channel_tools::{ChannelMonitorSource, ChannelToolsState};
