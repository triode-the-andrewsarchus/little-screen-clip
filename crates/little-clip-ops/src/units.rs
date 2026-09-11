use regex::Regex;
use std::sync::LazyLock;

#[derive(Debug, Clone, PartialEq)]
pub struct UnitConversion {
    pub original: String,
    pub primary_conversion: String,
    pub secondary_conversion: Option<String>,
    pub rule_of_thumb: Option<String>,
}

impl UnitConversion {
    pub fn summary_line(&self) -> String {
        let mut s = format!("{} ➔ {}", self.original, self.primary_conversion);
        if let Some(ref sec) = self.secondary_conversion {
            s.push_str(&format!(" ({})", sec));
        }
        if let Some(ref rot) = self.rule_of_thumb {
            s.push_str(&format!(" [{}]", rot));
        }
        s
    }
}

static PATTERN_FEET_INCHES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)^\s*(\d+(?:\.\d+)?)\s*(?:'|ft|feet)\s*(\d+(?:\.\d+)?)\s*(?:"|in|inches)?\s*$"#).unwrap()
});

static PATTERN_SINGLE_VALUE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)^\s*(-?\d+(?:\.\d+)?)\s*([°a-z/]+(?:/[a-z]+)?|'|")\s*$"#).unwrap()
});

pub fn detect_unit_conversion(text: &str) -> Option<UnitConversion> {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.len() > 60 {
        return None;
    }

    // 1. Check composite feet + inches (e.g. 5'11" or 5 ft 10 in)
    if let Some(caps) = PATTERN_FEET_INCHES.captures(trimmed) {
        let feet: f64 = caps[1].parse().ok()?;
        let inches: f64 = caps[2].parse().ok()?;
        let total_inches = feet * 12.0 + inches;
        let cm = total_inches * 2.54;
        let meters = cm / 100.0;

        return Some(UnitConversion {
            original: format!("{}' {}\"", feet, inches),
            primary_conversion: format!("{:.1} cm", cm),
            secondary_conversion: Some(format!("{:.2} m", meters)),
            rule_of_thumb: if cm >= 150.0 && cm <= 205.0 {
                Some("typical human height".to_string())
            } else {
                None
            },
        });
    }

    // 2. Single value + unit
    let caps = PATTERN_SINGLE_VALUE.captures(trimmed)?;
    let val: f64 = caps[1].parse().ok()?;
    let unit_raw = caps[2].to_lowercase();
    let unit = unit_raw.as_str();

    match unit {
        // --- Length: Imperial ---
        "\"" | "in" | "inch" | "inches" => {
            let cm = val * 2.54;
            let (prim, sec) = if val.abs() >= 24.0 {
                let feet = (val / 12.0).floor();
                let rem_in = (val % 12.0).round();
                (
                    format!("{:.1} cm ({:.2} m)", cm, cm / 100.0),
                    Some(format!("{} ft {} in", feet, rem_in)),
                )
            } else {
                (format!("{:.1} cm", cm), Some(format!("{:.2} m", cm / 100.0)))
            };
            Some(UnitConversion {
                original: format!("{} in", val),
                primary_conversion: prim,
                secondary_conversion: sec,
                rule_of_thumb: None,
            })
        }
        "'" | "ft" | "feet" | "foot" => {
            let m = val * 0.3048;
            let inches = (val * 12.0).round();
            Some(UnitConversion {
                original: format!("{} ft", val),
                primary_conversion: format!("{:.2} m", m),
                secondary_conversion: Some(format!("{:.0} in", inches)),
                rule_of_thumb: if val == 6.0 {
                    Some("6 ft = 182.9 cm".to_string())
                } else {
                    None
                },
            })
        }
        "yd" | "yds" | "yard" | "yards" => {
            let m = val * 0.9144;
            let ft = (val * 3.0).round();
            Some(UnitConversion {
                original: format!("{} yd", val),
                primary_conversion: format!("{:.1} m", m),
                secondary_conversion: Some(format!("{:.0} ft", ft)),
                rule_of_thumb: None,
            })
        }
        "mi" | "mile" | "miles" => {
            let km = val * 1.60934;
            Some(UnitConversion {
                original: format!("{} mi", val),
                primary_conversion: format!("{:.2} km", km),
                secondary_conversion: None,
                rule_of_thumb: Some("rule of thumb: 1 mi ≈ 1.6 km (5 mi ≈ 8 km)".to_string()),
            })
        }

        // --- Length: Metric ---
        "mm" | "millimeter" | "millimeters" => {
            let inches = val / 25.4;
            Some(UnitConversion {
                original: format!("{} mm", val),
                primary_conversion: format!("{:.2} in", inches),
                secondary_conversion: Some(format!("{:.1} cm", val / 10.0)),
                rule_of_thumb: None,
            })
        }
        "cm" | "centimeter" | "centimeters" => {
            let total_inches = val / 2.54;
            let feet = (total_inches / 12.0).floor();
            let rem_in = (total_inches % 12.0).round();
            let height_thumb = if val >= 150.0 && val <= 205.0 {
                Some("human height".to_string())
            } else {
                None
            };
            Some(UnitConversion {
                original: format!("{} cm", val),
                primary_conversion: format!("{} ft {} in ({:.1}\")", feet, rem_in, total_inches),
                secondary_conversion: Some(format!("{:.2} m", val / 100.0)),
                rule_of_thumb: height_thumb,
            })
        }
        "m" | "meter" | "meters" => {
            let ft = val * 3.28084;
            let yd = val * 1.09361;
            Some(UnitConversion {
                original: format!("{} m", val),
                primary_conversion: format!("{:.1} ft", ft),
                secondary_conversion: Some(format!("{:.1} yd", yd)),
                rule_of_thumb: None,
            })
        }
        "km" | "kilometer" | "kilometers" => {
            let miles = val * 0.621371;
            Some(UnitConversion {
                original: format!("{} km", val),
                primary_conversion: format!("{:.2} miles", miles),
                secondary_conversion: None,
                rule_of_thumb: Some("rule of thumb: 8 km ≈ 5 mi".to_string()),
            })
        }

        // --- Weight / Mass ---
        "oz" | "ounce" | "ounces" => {
            let g = val * 28.3495;
            let lbs = val / 16.0;
            Some(UnitConversion {
                original: format!("{} oz", val),
                primary_conversion: format!("{:.1} g", g),
                secondary_conversion: if lbs >= 1.0 { Some(format!("{:.2} lbs", lbs)) } else { None },
                rule_of_thumb: None,
            })
        }
        "lb" | "lbs" | "pound" | "pounds" => {
            let kg = val * 0.453592;
            let oz = (val * 16.0).round();
            Some(UnitConversion {
                original: format!("{} lbs", val),
                primary_conversion: format!("{:.2} kg", kg),
                secondary_conversion: Some(format!("{:.0} oz", oz)),
                rule_of_thumb: Some("rule of thumb: 2.2 lbs ≈ 1 kg".to_string()),
            })
        }
        "g" | "gram" | "grams" => {
            let oz = val / 28.3495;
            let lbs = val / 453.592;
            Some(UnitConversion {
                original: format!("{} g", val),
                primary_conversion: format!("{:.2} oz", oz),
                secondary_conversion: if lbs >= 0.5 { Some(format!("{:.2} lbs", lbs)) } else { None },
                rule_of_thumb: None,
            })
        }
        "kg" | "kilo" | "kilogram" | "kilograms" => {
            let lbs = val * 2.20462;
            Some(UnitConversion {
                original: format!("{} kg", val),
                primary_conversion: format!("{:.1} lbs", lbs),
                secondary_conversion: Some(format!("{:.0} g", val * 1000.0)),
                rule_of_thumb: Some("rule of thumb: 1 kg ≈ 2.2 lbs".to_string()),
            })
        }

        // --- Temperature ---
        "f" | "°f" | "degf" => {
            let c = (val - 32.0) * 5.0 / 9.0;
            let rot = if (val - 32.0).abs() < 1.0 {
                Some("water freezing point".to_string())
            } else if (val - 212.0).abs() < 1.0 {
                Some("water boiling point".to_string())
            } else if (val - 98.6).abs() < 1.5 {
                Some("normal body temp".to_string())
            } else if val >= 68.0 && val <= 74.0 {
                Some("comfortable room temp".to_string())
            } else {
                None
            };
            Some(UnitConversion {
                original: format!("{} °F", val),
                primary_conversion: format!("{:.1} °C", c),
                secondary_conversion: None,
                rule_of_thumb: rot,
            })
        }
        "c" | "°c" | "degc" => {
            let f = val * 9.0 / 5.0 + 32.0;
            let rot = if val.abs() < 0.5 {
                Some("water freezing point".to_string())
            } else if (val - 100.0).abs() < 0.5 {
                Some("water boiling point".to_string())
            } else if (val - 37.0).abs() < 0.5 {
                Some("normal body temp".to_string())
            } else if val >= 20.0 && val <= 24.0 {
                Some("comfortable room temp".to_string())
            } else {
                None
            };
            Some(UnitConversion {
                original: format!("{} °C", val),
                primary_conversion: format!("{:.1} °F", f),
                secondary_conversion: None,
                rule_of_thumb: rot,
            })
        }

        // --- Volume ---
        "gal" | "gallon" | "gallons" => {
            let l = val * 3.78541;
            Some(UnitConversion {
                original: format!("{} gal", val),
                primary_conversion: format!("{:.2} L", l),
                secondary_conversion: None,
                rule_of_thumb: Some("rule of thumb: 1 gal ≈ 3.8 L".to_string()),
            })
        }
        "l" | "liter" | "liters" | "litres" => {
            let gal = val * 0.264172;
            let fl_oz = val * 33.814;
            Some(UnitConversion {
                original: format!("{} L", val),
                primary_conversion: format!("{:.2} gal", gal),
                secondary_conversion: Some(format!("{:.1} fl oz", fl_oz)),
                rule_of_thumb: None,
            })
        }
        "ml" | "milliliter" | "milliliters" => {
            let fl_oz = val / 29.5735;
            Some(UnitConversion {
                original: format!("{} ml", val),
                primary_conversion: format!("{:.2} fl oz", fl_oz),
                secondary_conversion: Some(format!("{:.3} L", val / 1000.0)),
                rule_of_thumb: None,
            })
        }
        "floz" | "fl oz" => {
            let ml = val * 29.5735;
            Some(UnitConversion {
                original: format!("{} fl oz", val),
                primary_conversion: format!("{:.1} ml", ml),
                secondary_conversion: None,
                rule_of_thumb: None,
            })
        }

        // --- Speed ---
        "mph" => {
            let kmh = val * 1.60934;
            Some(UnitConversion {
                original: format!("{} mph", val),
                primary_conversion: format!("{:.1} km/h", kmh),
                secondary_conversion: None,
                rule_of_thumb: if val == 60.0 {
                    Some("60 mph ≈ 96.6 km/h (highway speed)".to_string())
                } else {
                    None
                },
            })
        }
        "km/h" | "kph" => {
            let mph = val * 0.621371;
            Some(UnitConversion {
                original: format!("{} km/h", val),
                primary_conversion: format!("{:.1} mph", mph),
                secondary_conversion: None,
                rule_of_thumb: if val == 100.0 {
                    Some("100 km/h ≈ 62 mph".to_string())
                } else {
                    None
                },
            })
        }
        "knots" | "kt" => {
            let mph = val * 1.15078;
            let kmh = val * 1.852;
            Some(UnitConversion {
                original: format!("{} knots", val),
                primary_conversion: format!("{:.1} mph", mph),
                secondary_conversion: Some(format!("{:.1} km/h", kmh)),
                rule_of_thumb: None,
            })
        }

        // --- Digital Storage ---
        "gb" => {
            let gib = val * 1000.0 * 1000.0 * 1000.0 / (1024.0 * 1024.0 * 1024.0);
            Some(UnitConversion {
                original: format!("{} GB", val),
                primary_conversion: format!("{:.2} GiB (binary)", gib),
                secondary_conversion: Some(format!("{:.0} MB", val * 1000.0)),
                rule_of_thumb: Some("1000 MB decimal = 1 GB | 1024 MiB = 1 GiB".to_string()),
            })
        }
        "tb" => {
            let tib = val * 1000.0 * 1000.0 * 1000.0 * 1000.0 / (1024.0 * 1024.0 * 1024.0 * 1024.0);
            Some(UnitConversion {
                original: format!("{} TB", val),
                primary_conversion: format!("{:.2} TiB (binary)", tib),
                secondary_conversion: Some(format!("{:.0} GB", val * 1000.0)),
                rule_of_thumb: None,
            })
        }

        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unit_conversions() {
        let f_conv = detect_unit_conversion("72 F").expect("72 F");
        assert_eq!(f_conv.primary_conversion, "22.2 °C");

        let c_conv = detect_unit_conversion("100°C").expect("100°C");
        assert_eq!(c_conv.primary_conversion, "212.0 °F");
        assert_eq!(c_conv.rule_of_thumb, Some("water boiling point".to_string()));

        let ft_in = detect_unit_conversion("5' 11\"").expect("5' 11\"");
        assert_eq!(ft_in.primary_conversion, "180.3 cm");

        let lbs = detect_unit_conversion("150 lbs").expect("150 lbs");
        assert_eq!(lbs.primary_conversion, "68.04 kg");

        let mi = detect_unit_conversion("10 miles").expect("10 miles");
        assert_eq!(mi.primary_conversion, "16.09 km");

        let kmh = detect_unit_conversion("100 km/h").expect("100 km/h");
        assert_eq!(kmh.primary_conversion, "62.1 mph");

        assert!(detect_unit_conversion("not a unit").is_none());
    }
}
