use super::{Channel, Format};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ToneMode {
    #[default]
    Unknown,
    Carrier,
    TransmitTone,
    ToneSquelch,
    Dtcs,
    ReverseTone,
    ReverseDtcs,
    Cross,
}

impl ToneMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unknown => "—",
            Self::Carrier => "Carrier",
            Self::TransmitTone => "Tone",
            Self::ToneSquelch => "TSQL",
            Self::Dtcs => "DTCS",
            Self::ReverseTone => "TSQL-R",
            Self::ReverseDtcs => "DTCS-R",
            Self::Cross => "Cross",
        }
    }
}

/// A configured squelch tone or transmitted code in one direction.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tone {
    #[default]
    Unknown,
    Plain,
    Disabled,
    Ctcss {
        value: ToneValue,
        reject: bool,
    },
    Dtcs {
        value: ToneValue,
        polarity: Option<char>,
        reject: bool,
    },
}

/// CHIRP stores two values for some standardized modes. A target radio
/// chooses the applicable field according to its advertised features.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToneValue {
    Missing,
    One(u16),
    DeviceSpecific(u16, u16),
}

impl ToneValue {
    fn new(primary: Option<u16>, secondary: Option<u16>) -> Self {
        match (primary, secondary) {
            (Some(a), Some(b)) if a != b => Self::DeviceSpecific(a, b),
            (Some(value), _) | (_, Some(value)) => Self::One(value),
            _ => Self::Missing,
        }
    }

    fn display(self, display: impl Fn(u16) -> String) -> String {
        match self {
            Self::Missing => "—".into(),
            Self::One(value) => display(value),
            Self::DeviceSpecific(a, b) => format!("{} / {}", display(a), display(b)),
        }
    }
}

impl Tone {
    pub fn device_specific(self) -> bool {
        matches!(
            self,
            Self::Ctcss {
                value: ToneValue::DeviceSpecific(..),
                ..
            } | Self::Dtcs {
                value: ToneValue::DeviceSpecific(..),
                ..
            }
        )
    }

    pub fn label(self, receive: bool) -> String {
        let prefix = |reject| if reject { "Reject " } else { "" };
        match self {
            Self::Unknown => "Tone unavailable".into(),
            Self::Disabled => "Disabled".into(),
            Self::Plain if receive => "Carrier squelch".into(),
            Self::Plain => "No tone".into(),
            Self::Ctcss { value, reject } => {
                let hz = value.display(|value| format!("{}.{:01}", value / 10, value % 10));
                format!("{}CTCSS {hz} Hz", prefix(reject))
            }
            Self::Dtcs {
                value,
                polarity,
                reject,
            } => {
                let code = value.display(|value| format!("{value:03}"));
                let polarity = match polarity {
                    Some('N') => " · normal",
                    Some('R') => " · inverted",
                    _ => "",
                };
                format!("{}DCS {code}{polarity}", prefix(reject))
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tones {
    pub mode: ToneMode,
    pub transmit: Tone,
    pub receive: Tone,
}

impl Tones {
    pub fn for_channel(channel: &Channel, format: Format) -> Self {
        let mut tones = match format {
            Format::ChirpCsv => chirp_csv(channel),
            Format::ChirpXml => chirp_xml(channel),
            Format::Icom => icom(channel),
        };
        if channel.duplex == "off" {
            tones.transmit = Tone::Disabled;
        }
        tones
    }
}

#[derive(Default)]
struct Codes<'a> {
    tone: [Option<u16>; 2],
    digital: [Option<u16>; 2],
    polarity: Option<&'a str>,
}

impl Codes<'_> {
    fn shared(&self, kind: &str, receive: bool, reject: bool) -> Tone {
        match kind {
            "Tone" => ctcss(self.tone, reject),
            "DTCS" => self.dtcs(self.digital, receive, reject),
            _ => Tone::Unknown,
        }
    }

    fn dtcs(&self, values: [Option<u16>; 2], receive: bool, reject: bool) -> Tone {
        let polarity = self
            .polarity
            .and_then(|polarity| polarity.as_bytes().get(usize::from(receive)))
            .map(|byte| char::from(*byte))
            .filter(|polarity| matches!(polarity, 'N' | 'R'));
        Tone::Dtcs {
            value: ToneValue::new(values[0], values[1]),
            polarity,
            reject,
        }
    }

    fn direction(&self, kind: &str, receive: bool, shared: bool) -> Tone {
        let side = usize::from(receive);
        match kind {
            "" => Tone::Plain,
            "Tone" if !receive || !shared => ctcss([self.tone[side], None], false),
            "DTCS" if !receive || !shared => self.dtcs([self.digital[side], None], receive, false),
            "Tone" | "DTCS" => self.shared(kind, receive, false),
            _ => Tone::Unknown,
        }
    }
}

fn chirp_csv(channel: &Channel) -> Tones {
    let Some(mode) = channel.field("Tone") else {
        return Tones::default();
    };
    let codes = Codes {
        tone: [channel.field("rToneFreq"), channel.field("cToneFreq")].map(tone_hz),
        digital: [channel.field("DtcsCode"), channel.field("RxDtcsCode")].map(dcs),
        polarity: channel.field("DtcsPolarity"),
    };
    classify(mode, channel.field("CrossMode"), codes)
}

fn chirp_xml(channel: &Channel) -> Tones {
    let Some(setting) = channel.field("squelchSetting") else {
        return Tones::default();
    };
    let mode = match setting {
        "rtone" => "Tone",
        "ctone" => "TSQL",
        "dtcs" => "DTCS",
        mode => mode,
    };
    let mut digital = channel.field("dtcs").unwrap_or_default().split_whitespace();
    let codes = Codes {
        tone: [channel.field("rtone"), channel.field("ctone")].map(tone_hz),
        digital: [dcs(digital.next()), None],
        polarity: digital.next(),
    };
    classify(mode, None, codes)
}

fn classify(mode: &str, cross: Option<&str>, codes: Codes<'_>) -> Tones {
    use ToneMode::*;
    let (mode, transmit, receive) = match mode.trim() {
        "" | "None" | "Carrier" => (Carrier, Tone::Plain, Tone::Plain),
        "Tone" => (
            TransmitTone,
            codes.direction("Tone", false, false),
            Tone::Plain,
        ),
        "TSQL" => (
            ToneSquelch,
            codes.shared("Tone", false, false),
            codes.shared("Tone", true, false),
        ),
        "DTCS" => (
            Dtcs,
            codes.shared("DTCS", false, false),
            codes.shared("DTCS", true, false),
        ),
        "TSQL-R" => (ReverseTone, Tone::Unknown, codes.shared("Tone", true, true)),
        "DTCS-R" => (ReverseDtcs, Tone::Plain, codes.shared("DTCS", true, true)),
        "Cross" => {
            let Some((tx, rx)) = cross.and_then(|mode| mode.split_once("->")) else {
                return Tones {
                    mode: Cross,
                    ..Tones::default()
                };
            };
            let (tx, rx) = (tx.trim(), rx.trim());
            (
                Cross,
                codes.direction(tx, false, false),
                codes.direction(rx, true, tx != rx),
            )
        }
        _ => return Tones::default(),
    };
    Tones {
        mode,
        transmit,
        receive,
    }
}

fn icom(channel: &Channel) -> Tones {
    let Some(mode) = channel.field("Tone mode") else {
        return Tones::default();
    };
    let codes = Codes {
        tone: [
            channel.field("Transmit tone"),
            channel.field("Receive tone"),
        ]
        .map(tone_hz),
        digital: [dcs(channel.field("DTCS code")), None],
        polarity: channel.field("DTCS polarity"),
    };
    if mode == "TSQL" {
        return Tones {
            mode: ToneMode::ToneSquelch,
            transmit: codes.shared("Tone", false, false),
            receive: codes.direction("Tone", true, false),
        };
    }
    if mode == "TSQL-R" {
        return Tones {
            mode: ToneMode::ReverseTone,
            transmit: Tone::Unknown,
            receive: ctcss([codes.tone[1], None], true),
        };
    }
    classify(mode, None, codes)
}

fn ctcss([a, b]: [Option<u16>; 2], reject: bool) -> Tone {
    Tone::Ctcss {
        value: ToneValue::new(a, b),
        reject,
    }
}

fn tone_hz(text: Option<&str>) -> Option<u16> {
    let text = text?.trim().trim_end_matches(" Hz");
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    let digits = fraction.trim_end_matches('0');
    if digits.len() > 1 {
        return None;
    }
    let tenths = if digits.is_empty() {
        0
    } else {
        digits.parse::<u16>().ok()?
    };
    whole
        .parse::<u16>()
        .ok()?
        .checked_mul(10)?
        .checked_add(tenths)
        .filter(|hz| *hz != 0)
}

fn dcs(text: Option<&str>) -> Option<u16> {
    let text = text?.trim();
    (text.len() <= 3 && text.bytes().all(|byte| matches!(byte, b'0'..=b'7')))
        .then(|| text.parse().ok())
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shows_the_stored_tone_options_when_chirp_lacks_a_radio_model() {
        let row = channel(&[
            ("Tone", "TSQL"),
            ("rToneFreq", "131.8"),
            ("cToneFreq", "151.4"),
        ]);
        let tones = Tones::for_channel(&row, Format::ChirpCsv);
        assert_eq!(tones.mode, ToneMode::ToneSquelch);
        assert_eq!(tones.receive.label(true), "CTCSS 131.8 / 151.4 Hz");
        assert!(tones.receive.device_specific());
        assert_eq!(tones.transmit, tones.receive);
    }

    #[test]
    fn cross_mode_reports_separate_transmit_and_receive_settings() {
        let mut row = channel(&[
            ("Tone", "Cross"),
            ("CrossMode", "DTCS->DTCS"),
            ("DtcsCode", "023"),
            ("RxDtcsCode", "125"),
            ("DtcsPolarity", "NR"),
        ]);
        let tones = Tones::for_channel(&row, Format::ChirpCsv);
        assert_eq!(tones.mode, ToneMode::Cross);
        assert_eq!(tones.transmit.label(false), "DCS 023 · normal");
        assert_eq!(tones.receive.label(true), "DCS 125 · inverted");
        row.duplex = "off".into();
        assert_eq!(
            Tones::for_channel(&row, Format::ChirpCsv).transmit,
            Tone::Disabled
        );
    }

    #[test]
    fn legacy_xml_and_icom_keep_their_directional_code_metadata() {
        let xml = channel(&[("squelchSetting", "dtcs"), ("dtcs", "054 RN")]);
        assert_eq!(
            Tones::for_channel(&xml, Format::ChirpXml)
                .transmit
                .label(false),
            "DCS 054 · inverted"
        );
        let icom = channel(&[("Tone mode", "TSQL-R"), ("Receive tone", "67.0 Hz")]);
        assert_eq!(
            Tones::for_channel(&icom, Format::Icom).receive.label(true),
            "Reject CTCSS 67.0 Hz"
        );
        assert_eq!(tone_hz(Some("67.a")), None);
    }

    fn channel(fields: &[(&str, &str)]) -> Channel {
        Channel {
            number: 1,
            name: String::new(),
            receive_hz: 145_550_000,
            duplex: String::new(),
            offset_hz: None,
            mode: "FM".into(),
            fields: fields
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect(),
        }
    }
}
