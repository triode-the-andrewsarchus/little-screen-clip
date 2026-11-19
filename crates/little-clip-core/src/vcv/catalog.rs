//! Module catalog, role classification, port metadata, and semantic label resolution
//! ported from `ModularPatchTools.Core.Catalog` and `ModularPatchTools.Diagram.Analysis`.

use super::archive::PatchModule;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::sync::OnceLock;

static EMBEDDED_CATALOG_ZST: &[u8] = include_bytes!("catalog_data.json.zst");

/// Functional role of a Eurorack module in a VCV Rack patch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
pub enum Role {
    Time,
    Pitch,
    Controller,
    Source,
    Modifier,
    Effect,
    Mixer,
    Io,
    Performance,
    Monitor,
    Router,
}

impl Role {
    pub fn label(self) -> &'static str {
        match self {
            Self::Time => "time",
            Self::Pitch => "pitch",
            Self::Controller => "controller",
            Self::Source => "source",
            Self::Modifier => "modifier",
            Self::Effect => "effect",
            Self::Mixer => "mixer",
            Self::Io => "I/O",
            Self::Performance => "performance",
            Self::Monitor => "monitor",
            Self::Router => "router",
        }
    }
}

/// Confidence source of a module's resolved role, ordered from highest certainty to fallback.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RoleSource {
    Explicit,
    Tags,
    Description,
    Name,
    Default,
}

/// Detailed functional classification used for icons, panel previews, and optional function titles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ModuleFunction {
    Midi,
    Clock,
    Sequencer,
    Logic,
    Quantizer,
    PitchUtility,
    Oscillator,
    SoundSource,
    Drum,
    Sampler,
    Filter,
    Vca,
    Envelope,
    Lfo,
    Random,
    Slew,
    Modulator,
    Waveshaper,
    Dynamics,
    Panning,
    Modifier,
    Delay,
    Reverb,
    Chorus,
    Effect,
    Mixer,
    Audio,
    Controller,
    Scope,
    Router,
    Utility,
}

impl ModuleFunction {
    pub fn label(self) -> &'static str {
        match self {
            Self::Midi => "MIDI",
            Self::Clock => "Clock",
            Self::Sequencer => "Sequencer",
            Self::Logic => "Logic",
            Self::Quantizer => "Quantizer",
            Self::PitchUtility => "Pitch",
            Self::Oscillator => "Oscillator",
            Self::SoundSource => "Source",
            Self::Drum => "Drum",
            Self::Sampler => "Sampler",
            Self::Filter => "Filter",
            Self::Vca => "VCA",
            Self::Envelope => "Envelope",
            Self::Lfo => "LFO",
            Self::Random => "Random",
            Self::Slew => "Slew",
            Self::Modulator => "Modulator",
            Self::Waveshaper => "Shaper",
            Self::Dynamics => "Dynamics",
            Self::Panning => "Pan",
            Self::Modifier => "Modifier",
            Self::Delay => "Delay",
            Self::Reverb => "Reverb",
            Self::Chorus => "Chorus",
            Self::Effect => "Effect",
            Self::Mixer => "Mixer",
            Self::Audio => "Audio",
            Self::Controller => "Control",
            Self::Scope => "Scope",
            Self::Router => "Switch",
            Self::Utility => "Utility",
        }
    }

    pub fn is_generic_name(self) -> bool {
        matches!(self, Self::Midi | Self::Audio | Self::Utility)
    }

    /// Resolves the specific module function from role, Rack tags, model slug, and name.
    pub fn classify(role: Role, tags: &[String], model: &str, name: Option<&str>) -> Self {
        let canonical = canonical_tags(tags);
        let has_tag = |tag: &str| canonical.iter().any(|t| t.eq_ignore_ascii_case(tag));
        let text = format!("{} {}", model, name.unwrap_or(""));
        let has_word = |needle: &str| contains_ci(&text, needle);

        match role {
            Role::Io => {
                if has_tag("MIDI") || has_word("midi") {
                    Self::Midi
                } else {
                    Self::Audio
                }
            }
            Role::Performance => {
                if has_tag("MIDI") || has_word("midi") {
                    Self::Midi
                } else {
                    Self::Controller
                }
            }
            Role::Monitor => Self::Scope,
            Role::Router => Self::Router,
            Role::Mixer => Self::Mixer,
            Role::Time => {
                if has_tag("Sequencer") || has_tag("Arpeggiator") || has_word("seq") {
                    Self::Sequencer
                } else if has_tag("Logic") || has_word("logic") {
                    Self::Logic
                } else {
                    Self::Clock
                }
            }
            Role::Pitch => {
                if has_tag("Quantizer") || has_word("quant") {
                    Self::Quantizer
                } else {
                    Self::PitchUtility
                }
            }
            Role::Source => {
                if has_tag("Drum") || has_word("drum") || has_word("kick") || has_word("snare") {
                    Self::Drum
                } else if has_tag("Sampler") || has_word("sampl") {
                    Self::Sampler
                } else if has_tag("Oscillator")
                    || has_tag("Voltage-controlled oscillator")
                    || has_tag("Synth voice")
                    || has_word("vco")
                    || has_word("osc")
                {
                    Self::Oscillator
                } else {
                    Self::SoundSource
                }
            }
            Role::Modifier => {
                if has_tag("Filter")
                    || has_tag("Low-pass gate")
                    || has_word("vcf")
                    || has_word("filter")
                {
                    Self::Filter
                } else if has_tag("Voltage-controlled amplifier") || has_word("vca") {
                    Self::Vca
                } else if has_tag("Waveshaper")
                    || has_tag("Distortion")
                    || has_word("shap")
                    || has_word("fold")
                {
                    Self::Waveshaper
                } else if has_tag("Compressor")
                    || has_tag("Limiter")
                    || has_tag("Dynamics")
                    || has_word("comp")
                {
                    Self::Dynamics
                } else if has_tag("Panning") || has_word("pan") {
                    Self::Panning
                } else {
                    Self::Modifier
                }
            }
            Role::Effect => {
                if has_tag("Reverb") || has_word("reverb") {
                    Self::Reverb
                } else if has_tag("Delay") || has_word("delay") || has_word("echo") {
                    Self::Delay
                } else if has_tag("Chorus") || has_tag("Flanger") || has_tag("Phaser") {
                    Self::Chorus
                } else {
                    Self::Effect
                }
            }
            Role::Controller => {
                if has_tag("Envelope generator")
                    || has_tag("Envelope follower")
                    || has_tag("Function generator")
                    || has_word("adsr")
                    || has_word("env")
                {
                    Self::Envelope
                } else if has_tag("Low-frequency oscillator") || has_word("lfo") {
                    Self::Lfo
                } else if has_tag("Random") || has_tag("Sample and hold") || has_word("rand") {
                    Self::Random
                } else if has_tag("Slew limiter") || has_word("slew") {
                    Self::Slew
                } else if has_tag("Controller") {
                    Self::Controller
                } else {
                    Self::Modulator
                }
            }
        }
    }
}

/// Surge XT style programmable modulation matrix description.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModMatrix {
    pub first_input: i32,
    pub input_count: i32,
    pub first_depth_param: i32,
    #[serde(default)]
    pub targets: Vec<String>,
}

/// Active modulation matrix link discovered on a patch module.
#[derive(Clone, Debug)]
pub struct ModRoute {
    pub target: String,
    pub depth: f64,
}

impl std::fmt::Display for ModRoute {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let pct = (self.depth * 100.0).round() as i64;
        if pct >= 0 {
            write!(f, "{} +{pct}%", self.target)
        } else {
            write!(f, "{} {pct}%", self.target)
        }
    }
}

/// Module metadata and port names from the embedded catalog.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModuleInfo {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub inputs: Vec<String>,
    #[serde(default)]
    pub outputs: Vec<String>,
    #[serde(default)]
    pub modulation: Option<ModMatrix>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BundleJson {
    roles: HashMap<String, Role>,
    library_roles: HashMap<String, Role>,
    library_plugins: HashSet<String>,
    modules: HashMap<String, ModuleInfo>,
}

/// Shared catalog instance decompressed once from the embedded zstd JSON bundle.
pub struct VcvCatalog {
    roles: HashMap<String, Role>,
    library_roles: HashMap<String, Role>,
    library_plugins: HashSet<String>,
    modules: HashMap<String, ModuleInfo>,
}

static CATALOG: OnceLock<VcvCatalog> = OnceLock::new();

impl VcvCatalog {
    /// Returns the shared embedded catalog, decompressing and parsing it once on demand.
    pub fn embedded() -> &'static Self {
        CATALOG.get_or_init(|| {
            let mut decoder =
                zstd::stream::Decoder::new(EMBEDDED_CATALOG_ZST).expect("valid embedded zst");
            let mut raw = Vec::with_capacity(1_024 * 1024);
            decoder
                .read_to_end(&mut raw)
                .expect("decompress embedded vcv catalog");
            let parsed: BundleJson =
                serde_json::from_slice(&raw).expect("parse embedded vcv catalog JSON");
            Self {
                roles: parsed.roles,
                library_roles: parsed.library_roles,
                library_plugins: parsed.library_plugins,
                modules: parsed.modules,
            }
        })
    }

    pub fn module_info(&self, plugin: &str, model: &str) -> Option<&ModuleInfo> {
        self.modules.get(&format!("{plugin}/{model}"))
    }

    pub fn input_name(&self, plugin: &str, model: &str, index: i32) -> String {
        let idx = usize::try_from(index).ok();
        if let Some(info) = self.module_info(plugin, model)
            && let Some(i) = idx
            && let Some(name) = info.inputs.get(i)
            && !name.is_empty()
        {
            return name.clone();
        }
        format!("in#{index}")
    }

    pub fn output_name(&self, plugin: &str, model: &str, index: i32) -> String {
        let idx = usize::try_from(index).ok();
        if let Some(info) = self.module_info(plugin, model)
            && let Some(i) = idx
            && let Some(name) = info.outputs.get(i)
            && !name.is_empty()
        {
            return name.clone();
        }
        format!("out#{index}")
    }

    pub fn is_listed_in_library(&self, plugin: &str) -> bool {
        !plugin.is_empty()
            && plugin != "Cardinal"
            && (self.library_plugins.is_empty() || self.library_plugins.contains(plugin))
    }

    pub fn resolve_role(
        &self,
        plugin: &str,
        model: &str,
        info: Option<&ModuleInfo>,
    ) -> (Role, RoleSource) {
        let key = format!("{plugin}/{model}");
        if let Some(&r) = self.roles.get(&key) {
            return (r, RoleSource::Explicit);
        }
        if let Some(info) = info
            && let Some(r) = role_from_tags(&info.tags)
        {
            return (r, RoleSource::Tags);
        }
        if let Some(&r) = self.library_roles.get(&key) {
            return (r, RoleSource::Description);
        }

        let bare_model = strip_plugin_prefix(model, plugin);
        for &(keyword, role) in SLUG_ROLES {
            if name_has(bare_model, keyword) {
                return (role, RoleSource::Name);
            }
        }
        (Role::Controller, RoleSource::Default)
    }
}

static RACK_TAGS: &[&str] = &[
    "Arpeggiator",
    "Attenuator",
    "Blank",
    "Chorus",
    "Clock generator",
    "Clock modulator",
    "Compressor",
    "Controller",
    "Delay",
    "Digital",
    "Distortion",
    "Drum",
    "Dual",
    "Dynamics",
    "Effect",
    "Envelope follower",
    "Envelope generator",
    "Equalizer",
    "Expander",
    "External",
    "Filter",
    "Flanger",
    "Function generator",
    "Granular",
    "Hardware clone",
    "Limiter",
    "Logic",
    "Low-frequency oscillator",
    "Low-pass gate",
    "MIDI",
    "Mixer",
    "Multiple",
    "Noise",
    "Oscillator",
    "Panning",
    "Phaser",
    "Physical modeling",
    "Polyphonic",
    "Quad",
    "Quantizer",
    "Random",
    "Recording",
    "Reverb",
    "Ring modulator",
    "Sample and hold",
    "Sampler",
    "Sequencer",
    "Slew limiter",
    "Speech",
    "Switch",
    "Synth voice",
    "Tuner",
    "Utility",
    "Visual",
    "Vocoder",
    "Voltage-controlled amplifier",
    "Waveshaper",
];

static TAG_ALIASES: &[(&str, &str)] = &[
    ("Clock", "Clock generator"),
    ("Drums", "Drum"),
    ("Percussion", "Drum"),
    ("EQ", "Equalizer"),
    ("VCF", "Filter"),
    ("Voltage controlled filter", "Filter"),
    ("Hardware", "Hardware clone"),
    ("LFO", "Low-frequency oscillator"),
    ("Low frequency oscillator", "Low-frequency oscillator"),
    ("Low pass gate", "Low-pass gate"),
    ("Lowpass gate", "Low-pass gate"),
    ("VCO", "Oscillator"),
    ("Voltage controlled oscillator", "Oscillator"),
    ("Pan", "Panning"),
    ("Poly", "Polyphonic"),
    ("S&H", "Sample and hold"),
    ("Sample & hold", "Sample and hold"),
    ("Amplifier", "Voltage-controlled amplifier"),
    ("VCA", "Voltage-controlled amplifier"),
    (
        "Voltage controlled amplifier",
        "Voltage-controlled amplifier",
    ),
];

pub fn canonical_tags(tags: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for entry in tags {
        for part in entry.split(',') {
            let trimmed = part.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Some(&(_, canonical)) = TAG_ALIASES
                .iter()
                .find(|&&(alias, _)| alias.eq_ignore_ascii_case(trimmed))
            {
                out.push(canonical.to_string());
            } else if let Some(&tag) = RACK_TAGS
                .iter()
                .find(|&&known| known.eq_ignore_ascii_case(trimmed))
            {
                out.push(tag.to_string());
            } else {
                out.push(trimmed.to_string());
            }
        }
    }
    out
}

static TAG_ROLES: &[(&str, Role)] = &[
    ("Synth voice", Role::Source),
    ("Visual", Role::Monitor),
    ("Quantizer", Role::Pitch),
    ("Arpeggiator", Role::Time),
    ("Sequencer", Role::Time),
    ("Clock generator", Role::Time),
    ("Clock modulator", Role::Time),
    ("Logic", Role::Time),
    ("Envelope generator", Role::Controller),
    ("Low-frequency oscillator", Role::Controller),
    ("Random", Role::Controller),
    ("Sample and hold", Role::Controller),
    ("Slew limiter", Role::Controller),
    ("Envelope follower", Role::Controller),
    ("Controller", Role::Controller),
    ("Switch", Role::Router),
    ("Voltage-controlled oscillator", Role::Source),
    ("Oscillator", Role::Source),
    ("Noise", Role::Source),
    ("Sampler", Role::Source),
    ("Drum", Role::Source),
    ("Physical modeling", Role::Source),
    ("Reverb", Role::Effect),
    ("Delay", Role::Effect),
    ("Chorus", Role::Effect),
    ("Flanger", Role::Effect),
    ("Phaser", Role::Effect),
    ("Mixer", Role::Mixer),
    ("Effect", Role::Effect),
    ("Filter", Role::Modifier),
    ("Voltage-controlled amplifier", Role::Modifier),
    ("Waveshaper", Role::Modifier),
    ("Distortion", Role::Modifier),
    ("Equalizer", Role::Modifier),
    ("Compressor", Role::Modifier),
    ("Limiter", Role::Modifier),
    ("Dynamics", Role::Modifier),
    ("Low-pass gate", Role::Modifier),
    ("External", Role::Io),
    ("MIDI", Role::Io),
    ("Recording", Role::Io),
    ("Tuner", Role::Monitor),
    ("Ring modulator", Role::Modifier),
    ("Panning", Role::Modifier),
    ("Function generator", Role::Controller),
    ("Multiple", Role::Router),
    ("Attenuator", Role::Controller),
];

static SLUG_ROLES: &[(&str, Role)] = &[
    ("Host", Role::Io),
    ("Audio", Role::Io),
    ("MIDI", Role::Io),
    ("Seq", Role::Time),
    ("Clock", Role::Time),
    ("Clk", Role::Time),
    ("Quant", Role::Pitch),
    ("Chord", Role::Pitch),
    ("Octave", Role::Pitch),
    ("Harmon", Role::Pitch),
    ("Transpos", Role::Pitch),
    ("Merge", Role::Router),
    ("Split", Role::Router),
    ("Splirge", Role::Router),
    ("Sort", Role::Router),
    ("Mux", Role::Router),
    ("FlipFlop", Role::Time),
    ("Switch", Role::Router),
    ("Flip", Role::Router),
    ("Mute", Role::Router),
    ("Count", Role::Time),
    ("Divid", Role::Time),
    ("ShiftReg", Role::Time),
    ("Trig", Role::Time),
    ("Drum", Role::Source),
    ("Kick", Role::Source),
    ("Snare", Role::Source),
    ("Perc", Role::Source),
    ("Clap", Role::Source),
    ("Cymbal", Role::Source),
    ("LFO", Role::Controller),
    ("ADSR", Role::Controller),
    ("Env", Role::Controller),
    ("Random", Role::Controller),
    ("Slew", Role::Controller),
    ("VCO", Role::Source),
    ("Osc", Role::Source),
    ("Noise", Role::Source),
    ("Reverb", Role::Effect),
    ("Delay", Role::Effect),
    ("Mix", Role::Mixer),
    ("VCF", Role::Modifier),
    ("Filter", Role::Modifier),
    ("VCA", Role::Modifier),
    ("Shaper", Role::Modifier),
    ("EQ", Role::Modifier),
    ("Gain", Role::Modifier),
    ("MidSide", Role::Modifier),
    ("Atten", Role::Controller),
    ("Offset", Role::Controller),
    ("Const", Role::Controller),
    ("Scale", Role::Controller),
    ("Mult", Role::Router),
    ("Pan", Role::Modifier),
];

pub fn role_from_tags(tags: &[String]) -> Option<Role> {
    let canonical = canonical_tags(tags);
    for &(tag, role) in TAG_ROLES {
        if canonical.iter().any(|t| t.eq_ignore_ascii_case(tag)) {
            return Some(role);
        }
    }
    None
}

fn strip_plugin_prefix<'a>(model: &'a str, plugin: &str) -> &'a str {
    let trimmed = if model
        .get(..plugin.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(plugin))
    {
        &model[plugin.len()..]
    } else {
        model
    };
    trimmed.trim_matches(|c: char| c == '-' || c == '_' || c == ' ')
}

fn contains_ci(haystack: &str, needle: &str) -> bool {
    let h = haystack.to_ascii_lowercase();
    let n = needle.to_ascii_lowercase();
    h.contains(&n)
}

fn name_has(name: &str, keyword: &str) -> bool {
    if keyword.len() >= 5 {
        return contains_ci(name, keyword);
    }
    let lower_name = name.to_ascii_lowercase();
    let lower_kw = keyword.to_ascii_lowercase();
    let name_bytes = name.as_bytes();
    let mut start = 0;
    while let Some(pos) = lower_name[start..].find(&lower_kw) {
        let at = start + pos;
        let word_start = at == 0
            || !(name_bytes[at - 1] as char).is_ascii_alphabetic()
            || ((name_bytes[at] as char).is_ascii_uppercase()
                && (name_bytes[at - 1] as char).is_ascii_lowercase());
        let multi = keyword == "Mult"
            && at + 4 < name_bytes.len()
            && (name_bytes[at + 4] as char).eq_ignore_ascii_case(&'i');
        if word_start && !multi {
            return true;
        }
        start = at + 1;
    }
    false
}

/// Resolves custom typed labels on Aluminium panels (`Fader`, `Faders2`, `Faders4`, `Buttons`, `PadX`).
pub fn resolve_port_label(module: &PatchModule, index: i32, port: String, input: bool) -> String {
    if module.plugin != "Aluminium" {
        return port;
    }
    let idx = match usize::try_from(index) {
        Ok(i) => i,
        Err(_) => return port,
    };
    if module.model == "PadX" {
        if let Some(lane) = module.labels.first().filter(|l| !l.is_empty()) {
            return if input {
                format!("{lane} (poly)")
            } else {
                format!("{lane} {}", idx + 1)
            };
        }
        return port;
    }
    if matches!(
        module.model.as_str(),
        "Fader" | "Faders2" | "Faders4" | "Buttons"
    ) && let Some(label) = module.labels.get(idx).filter(|l| !l.is_empty())
    {
        return label.clone();
    }
    port
}

pub fn is_port_labeled(module: &PatchModule, index: i32, input: bool) -> bool {
    !resolve_port_label(module, index, String::new(), input).is_empty()
}

/// Enriches Core/Cardinal MIDI Gate and CC ports with their learned note or CC number.
pub fn resolve_midi_learn_name(
    module: &PatchModule,
    index: i32,
    port: String,
    include_meaning: bool,
) -> String {
    if !matches!(module.plugin.as_str(), "Core" | "Cardinal") {
        return port;
    }
    let Ok(idx) = usize::try_from(index) else {
        return port;
    };
    if port.starts_with("Gate ")
        && let Some(&note) = module.learned_notes.get(idx)
        && (0..128).contains(&note)
    {
        return format!("{port} · {}", midi_note_name(note));
    }
    if port.starts_with("Cell ")
        && let Some(&cc) = module.learned_ccs.get(idx)
        && (0..128).contains(&cc)
    {
        let meaning = if include_meaning {
            general_midi_cc_name(cc)
                .map(|m| format!(" ({m})"))
                .unwrap_or_default()
        } else {
            String::new()
        };
        return format!("{port} · CC {cc}{meaning}");
    }
    if port.starts_with("Cell ")
        && let Some(&note) = module.learned_notes.get(idx)
        && (0..128).contains(&note)
    {
        return format!("{port} · {}", midi_note_name(note));
    }
    port
}

fn midi_note_name(note: i32) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    let octave = (note / 12) - 1;
    let pitch = NAMES[(note % 12) as usize];
    format!("{pitch}{octave}")
}

fn general_midi_cc_name(cc: i32) -> Option<&'static str> {
    match cc {
        1 => Some("mod wheel"),
        2 => Some("breath"),
        4 => Some("foot"),
        7 => Some("volume"),
        10 => Some("pan"),
        11 => Some("expression"),
        64 => Some("sustain pedal"),
        71 => Some("resonance"),
        74 => Some("cutoff"),
        _ => None,
    }
}
