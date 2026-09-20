//! Audio Studio tools. The inspect editor is shared by standalone inspection and stem editing.
mod digital_decode;
mod inspect;
mod stem_separation;
pub(super) use digital_decode::DigitalDecodeState;
pub(super) use stem_separation::StemSeparationState;
#[cfg(test)]
pub(super) use stem_separation::{
    audacity_lof_contents, editor_stem_switch, parse_stem_manifest,
    prepare_isolated_audacity_command, stem_is_audible, waveform_scrub_time,
};
