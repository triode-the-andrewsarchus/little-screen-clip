use super::{
    super::{Block, State},
    BlockRates, RateMeaning, RateSetting, StreamRate,
    numbers::Numbers,
};

#[derive(Clone, Copy)]
pub(super) enum Rule {
    Unknown,
    Source(Option<RateSetting>),
    Pacing(Option<RateSetting>),
    Sink(Option<RateSetting>),
    Transfer(f64),
}

impl Rule {
    pub(super) fn read(numbers: &Numbers<'_>, block: &Block) -> Self {
        if block.state != State::Enabled
            || (block.setting("vlen").is_some() && numbers.setting(block, "vlen") != Some(1.0))
        {
            return Self::Unknown;
        }
        let declared = |key, meaning| {
            positive(numbers.setting(block, key)?).map(|rate| RateSetting { rate, key, meaning })
        };
        match block.kind.as_str() {
            "analog_sig_source_x" => Self::Source(declared("samp_rate", RateMeaning::Signal)),
            "uhd_usrp_source" | "audio_source" => {
                Self::Source(declared("samp_rate", RateMeaning::Hardware))
            }
            "osmosdr_source" => Self::Source(declared("sample_rate", RateMeaning::Hardware)),
            "blocks_throttle" | "blocks_throttle2" => {
                Self::Pacing(declared("samples_per_second", RateMeaning::Pacing))
            }
            "qtgui_time_sink_x" => Self::Sink(declared("srate", RateMeaning::Display)),
            "qtgui_freq_sink_x" | "qtgui_waterfall_sink_x" | "qtgui_sink_x" => {
                Self::Sink(declared("bw", RateMeaning::Display))
            }
            "uhd_usrp_sink" | "audio_sink" => {
                Self::Sink(declared("samp_rate", RateMeaning::Hardware))
            }
            _ => ratio(numbers, block).map_or(Self::Unknown, Self::Transfer),
        }
    }

    pub(super) fn initial(self) -> BlockRates {
        let (setting, ratio) = match self {
            Self::Source(value) | Self::Pacing(value) | Self::Sink(value) => (value, None),
            Self::Transfer(ratio) if ratio != 1.0 => (None, Some(ratio)),
            Self::Unknown | Self::Transfer(_) => (None, None),
        };
        BlockRates {
            setting,
            ratio,
            ..BlockRates::default()
        }
    }

    pub(super) fn output(self, block: usize, input: StreamRate) -> StreamRate {
        match (self, input) {
            (Self::Source(Some(rate)), _) | (Self::Pacing(Some(rate)), StreamRate::Unknown) => {
                StreamRate::anchored(block, rate)
            }
            (Self::Pacing(_), input) => input,
            (Self::Transfer(ratio), StreamRate::Nominal(mut nominal)) => {
                let Some(rate) = positive(nominal.rate * ratio) else {
                    return StreamRate::Unknown;
                };
                nominal.rate = rate;
                StreamRate::Nominal(nominal)
            }
            (Self::Transfer(_), StreamRate::Conflicting) => StreamRate::Conflicting,
            _ => StreamRate::Unknown,
        }
    }
}

fn ratio(numbers: &Numbers<'_>, block: &Block) -> Option<f64> {
    let natural = |name| {
        let value = positive(numbers.setting(block, name)?)?;
        (value.fract() == 0.0).then_some(value)
    };
    let factor = match block.kind.as_str() {
        "rational_resampler_xxx" => natural("interp")? / natural("decim")?,
        "pfb_arb_resampler_xxx" => numbers.setting(block, "rrate")?,
        "fir_filter_xxx"
        | "freq_xlating_fir_filter_xxx"
        | "low_pass_filter"
        | "high_pass_filter"
        | "band_pass_filter"
        | "band_reject_filter" => natural("decim")?.recip(),
        "interp_fir_filter_xxx" | "blocks_repeat" => natural("interp")?,
        "blocks_keep_one_in_n" => natural("n")?.recip(),
        "blocks_complex_to_mag"
        | "blocks_complex_to_mag_squared"
        | "blocks_complex_to_real"
        | "blocks_complex_to_imag"
        | "blocks_float_to_complex"
        | "blocks_multiply_xx"
        | "blocks_add_xx"
        | "blocks_add_const_vxx"
        | "blocks_multiply_const_vxx"
        | "blocks_abs_xx"
        | "blocks_nlog10_ff"
        | "analog_frequency_modulator_fc"
        | "analog_quadrature_demod_cf"
        | "single_pole_iir_filter_xx" => 1.0,
        _ => return None,
    };
    positive(factor)
}

fn positive(value: f64) -> Option<f64> {
    (value.is_finite() && value > 0.0).then_some(value)
}
