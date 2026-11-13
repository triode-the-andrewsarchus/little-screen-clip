//! Wire color abbreviations and explicit RGB bands from the published syntax.

const COLORS: &[(&str, [u8; 3])] = &[
    ("BK", [0, 0, 0]),
    ("WH", [255, 255, 255]),
    ("GY", [153, 153, 153]),
    ("PK", [255, 102, 204]),
    ("RD", [255, 0, 0]),
    ("OG", [255, 128, 0]),
    ("YE", [255, 255, 0]),
    ("OL", [112, 128, 0]),
    ("GN", [0, 255, 0]),
    ("TQ", [0, 255, 255]),
    ("LB", [160, 223, 255]),
    ("BU", [0, 102, 255]),
    ("VT", [128, 0, 255]),
    ("BN", [137, 89, 86]),
    ("BG", [206, 182, 115]),
    ("IV", [245, 240, 208]),
    ("SL", [112, 128, 144]),
    ("CU", [214, 119, 94]),
    ("SN", [170, 170, 170]),
    ("SR", [132, 135, 140]),
    ("GD", [255, 207, 128]),
];

/// Resolves one to eight wire color bands without interpreting source as SVG.
///
/// # Errors
/// Rejects unknown abbreviations, malformed RGB triplets and excessive bands.
pub fn bands(source: &str) -> Result<Vec<[u8; 3]>, String> {
    // Eight #RRGGBB bands plus seven separators; reject before splitting or
    // allocating a list, including calls made directly by graphical controls.
    if source.len() > 63 {
        return Err("Too many wire color bands".into());
    }
    if source.starts_with('#') {
        let parts: Vec<_> = source.split(':').collect();
        if parts.len() > 8 {
            return Err("Too many wire color bands".into());
        }
        return parts
            .into_iter()
            .map(|part| {
                let bytes = part.as_bytes();
                if bytes.len() != 7
                    || bytes[0] != b'#'
                    || !bytes[1..].iter().all(u8::is_ascii_hexdigit)
                {
                    return Err(format!("Invalid wire RGB color {part:?}"));
                }
                let mut color = [0; 3];
                for (channel, byte) in color.iter_mut().zip(bytes[1..].as_chunks::<2>().0) {
                    let digit = |byte: u8| {
                        if byte.is_ascii_digit() {
                            byte - b'0'
                        } else {
                            byte.to_ascii_lowercase() - b'a' + 10
                        }
                    };
                    *channel = digit(byte[0]) * 16 + digit(byte[1]);
                }
                Ok(color)
            })
            .collect();
    }
    if source.is_empty()
        || source.len() > 16
        || !source.len().is_multiple_of(2)
        || !source.is_ascii()
    {
        return Err(format!("Invalid wire color {source:?}"));
    }
    source
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|code| {
            COLORS
                .iter()
                .find(|(name, _)| name.as_bytes() == code)
                .map(|(_, color)| *color)
                .ok_or_else(|| format!("Unknown wire color in {source:?}"))
        })
        .collect()
}

/// Expands one standard cable color sequence to a bounded conductor count.
///
/// The DIN sequence follows LAPP's published DIN 47100 chart (positions 1–60).
/// Other sequences encode the published WireViz scheme names and pair order.
/// This contains format data, not a translated upstream implementation.
///
/// # Errors
/// Rejects an unknown scheme or a count outside the harness conductor budget.
pub fn palette(code: &str, count: usize) -> Result<Vec<String>, String> {
    if count == 0 || count > super::MAX_PINS {
        return Err(format!(
            "Palette conductor count must be between 1 and {}",
            super::MAX_PINS
        ));
    }
    const DIN: &str = "WH BN GN YE GY PK BU RD BK VT GYPK RDBU WHGN BNGN WHYE YEBN WHGY GYBN WHPK PKBN WHBU BNBU WHRD BNRD WHBK BNBK GYGN YEGY PKGN YEPK GNBU YEBU GNRD YERD GNBK YEBK GYBU PKBU GYRD PKRD GYBK PKBK BUBK RDBK WHBNBK YEGNBK GYPKBK RDBUBK WHGNBK BNGNBK WHYEBK YEBNBK WHGYBK GYBNBK WHPKBK PKBNBK WHBUBK BNBUBK WHRDBK BNRDBK";
    let sequence = match code {
        "DIN" => DIN,
        "IEC" => "BN RD OG YE GN BU VT GY WH BK",
        "BW" => "BK WH",
        "T568A" => "WHGN GN WHOG BU WHBU OG WHBN BN",
        "T568B" => "WHOG OG WHGN BU WHBU GN WHBN BN",
        "TEL" | "TELALT" => {
            return Ok((0..count)
                .map(|index| telephone(code, index % 50))
                .collect());
        }
        _ => return Err(format!("Unknown WireViz color_code {code:?}")),
    };
    let sequence: Vec<_> = sequence.split_ascii_whitespace().collect();
    Ok((0..count)
        .map(|index| sequence[index % sequence.len()].to_owned())
        .collect())
}

fn telephone(code: &str, index: usize) -> String {
    let major = ["WH", "RD", "BK", "YE", "VT"][index / 10];
    let minor = ["BU", "OG", "GN", "BN", "SL"][(index % 10) / 2];
    if code == "TELALT" && index < 10 && index % 2 == 1 {
        minor.into()
    } else if (code == "TEL") == index.is_multiple_of(2) {
        format!("{minor}{major}")
    } else {
        format!("{major}{minor}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn standard_palettes_wrap_at_their_actual_boundaries() {
        for (code, length, first, last) in [
            ("DIN", 60, "WH", "BNRDBK"),
            ("IEC", 10, "BN", "BK"),
            ("BW", 2, "BK", "WH"),
            ("TEL", 50, "BUWH", "VTSL"),
            ("TELALT", 50, "WHBU", "SLVT"),
            ("T568A", 8, "WHGN", "BN"),
            ("T568B", 8, "WHOG", "BN"),
        ] {
            let colors = palette(code, length + 1).unwrap();
            assert_eq!(colors[0], first, "{code}");
            assert_eq!(colors[length - 1], last, "{code}");
            assert_eq!(colors[length], first, "{code}");
            assert!(colors.iter().all(|color| bands(color).is_ok()));
        }
        assert_eq!(palette("DIN", 45).unwrap()[44], "WHBNBK");
        assert_eq!(palette("TELALT", 12).unwrap()[1], "BU");
        assert_eq!(palette("TELALT", 12).unwrap()[10], "RDBU");
        assert_eq!(palette("TELALT", 12).unwrap()[11], "BURD");
        for (code, count) in [("unknown", 3), ("IEC", 0), ("DIN", 513)] {
            assert!(palette(code, count).is_err());
        }
    }
    #[test]
    fn resolves_all_standard_colors_and_striped_rgb_bands() {
        for (name, color) in COLORS {
            assert_eq!(bands(name).unwrap(), [*color]);
        }
        assert_eq!(bands("GNYE").unwrap(), [[0, 255, 0], [255, 255, 0]]);
        assert_eq!(
            bands("#ffFf00:#009900").unwrap(),
            [[255, 255, 0], [0, 153, 0]]
        );
        for source in [
            "",
            "é",
            "GNZ",
            "XX",
            "#00112",
            "#ffffff\"",
            "#xx0000",
            "#000000:red",
        ] {
            assert!(bands(source).is_err(), "{source}");
        }
        assert!(bands(&["#ffffff"; 9].join(":")).is_err());
    }
}
