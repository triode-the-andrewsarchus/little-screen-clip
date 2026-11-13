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

#[cfg(test)]
mod tests {
    use super::*;
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
