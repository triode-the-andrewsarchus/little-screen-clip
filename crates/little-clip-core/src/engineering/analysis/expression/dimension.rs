use std::fmt;

/// Physical dimensions carried by a numerical signal.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Dimension {
    volts: i8,
    amps: i8,
    seconds: i8,
    full_scale: i8,
}

impl Dimension {
    /// Voltage measured in volts.
    pub const VOLTAGE: Self = Self::new(1, 0, 0, 0);
    /// Electric current measured in amperes.
    pub const CURRENT: Self = Self::new(0, 1, 0, 0);
    /// Duration measured in seconds.
    pub const TIME: Self = Self::new(0, 0, 1, 0);
    /// Frequency measured in Hertz.
    pub const FREQUENCY: Self = Self::new(0, 0, -1, 0);
    /// Electric power measured in watts.
    pub const POWER: Self = Self::new(1, 1, 0, 0);
    /// Normalized amplitude relative to full scale.
    pub const FULL_SCALE: Self = Self::new(0, 0, 0, 1);
    /// A ratio or count without a unit.
    pub const SCALAR: Self = Self::new(0, 0, 0, 0);
    const CHARGE: Self = Self::new(0, 1, 1, 0);
    const ENERGY: Self = Self::new(1, 1, 1, 0);
    const RESISTANCE: Self = Self::new(1, -1, 0, 0);

    const fn new(volts: i8, amps: i8, seconds: i8, full_scale: i8) -> Self {
        Self {
            volts,
            amps,
            seconds,
            full_scale,
        }
    }

    /// Recognizes the base unit reported by a capture adapter.
    pub fn from_unit(unit: &str) -> Option<Self> {
        Some(match unit {
            "" => Self::SCALAR,
            "V" | "v" => Self::VOLTAGE,
            "A" | "a" => Self::CURRENT,
            "W" | "w" => Self::POWER,
            "s" => Self::TIME,
            "Hz" | "hz" => Self::FREQUENCY,
            "J" => Self::ENERGY,
            "C" => Self::CHARGE,
            "Ω" => Self::RESISTANCE,
            "FS" => Self::FULL_SCALE,
            _ => return None,
        })
    }

    pub(super) fn literal(suffix: &str) -> Option<(Self, f64)> {
        if suffix.is_empty() {
            return None;
        }
        if let Some(unit) = Self::from_unit(suffix) {
            return Some((unit, 1.0));
        }
        for (prefix, scale) in [
            ("G", 1e9),
            ("M", 1e6),
            ("k", 1e3),
            ("m", 1e-3),
            ("µ", 1e-6),
            ("μ", 1e-6),
            ("u", 1e-6),
            ("n", 1e-9),
            ("p", 1e-12),
        ] {
            if let Some(rest) = suffix.strip_prefix(prefix)
                && !rest.is_empty()
                && let Some(unit) = Self::from_unit(rest)
            {
                return Some((unit, scale));
            }
        }
        None
    }

    pub(super) fn voltage_power(self) -> Option<i8> {
        (self.amps == 0 && self.seconds == 0 && self.full_scale == 0).then_some(self.volts)
    }

    pub(super) fn combine(self, rhs: Self, divide: bool) -> Option<Self> {
        let operation = if divide {
            i8::checked_sub
        } else {
            i8::checked_add
        };
        Some(Self::new(
            operation(self.volts, rhs.volts)?,
            operation(self.amps, rhs.amps)?,
            operation(self.seconds, rhs.seconds)?,
            operation(self.full_scale, rhs.full_scale)?,
        ))
    }

    pub(super) fn square_root(self) -> Option<Self> {
        [self.volts, self.amps, self.seconds, self.full_scale]
            .iter()
            .all(|power| power % 2 == 0)
            .then(|| {
                Self::new(
                    self.volts / 2,
                    self.amps / 2,
                    self.seconds / 2,
                    self.full_scale / 2,
                )
            })
    }
}

impl fmt::Display for Dimension {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match *self {
            Self::VOLTAGE => "V",
            Self::CURRENT => "A",
            Self::POWER => "W",
            Self::TIME => "s",
            Self::FREQUENCY => "Hz",
            Self::CHARGE => "C",
            Self::ENERGY => "J",
            Self::RESISTANCE => "Ω",
            Self::FULL_SCALE => "FS",
            Self::SCALAR => "",
            _ => {
                let mut compound = false;
                for (unit, power) in [
                    ("V", self.volts),
                    ("A", self.amps),
                    ("s", self.seconds),
                    ("FS", self.full_scale),
                ] {
                    if power == 0 {
                        continue;
                    }
                    if compound {
                        output.write_str("·")?;
                    }
                    if power == 1 {
                        output.write_str(unit)?;
                    } else {
                        write!(output, "{unit}^{power}")?;
                    }
                    compound = true;
                }
                return Ok(());
            }
        };
        output.write_str(label)
    }
}
