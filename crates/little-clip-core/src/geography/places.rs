//! Shared offline gazetteer and whole-text clipboard location recognition.
use serde::Deserialize;
use std::sync::LazyLock;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PlaceKind {
    City,
    Country,
    Region,
}

pub type Place = (PlaceKind, String, String, String, [f64; 2], f64);

pub static PLACES: LazyLock<Vec<Place>> = LazyLock::new(|| {
    let bytes = include_bytes!("places/places.json.zst");
    let decoder = zstd::stream::read::Decoder::new(bytes.as_slice()).expect("bundled place data");
    serde_json::from_reader(decoder).expect("valid bundled place catalog")
});

/// A coordinate pair or high-confidence named place.
#[derive(Debug, PartialEq)]
pub struct Location {
    pub name: String,
    /// Latitude, then longitude in degrees.
    pub lat_lon: [f64; 2],
    pub kind: Option<PlaceKind>,
}

/// Resolves a complete, short line of clipboard text without network access.
pub fn resolve(text: &str) -> Option<Location> {
    let text = text.trim();
    if text.is_empty() || text.len() > 128 || text.chars().any(char::is_control) {
        return None;
    }
    if let Some(lat_lon) = coordinates(text) {
        return Some(Location {
            name: text.into(),
            lat_lon,
            kind: None,
        });
    }
    if !text.chars().any(char::is_alphabetic) {
        return None;
    }
    let (query, locality) = text.split_once(',').map_or((text, None), |(name, area)| {
        (name.trim(), Some(area.trim()))
    });
    let candidates = || {
        PLACES.iter().filter(|(_, name, area, alias, _, _)| {
            (same_name(query, name)
                || alias
                    .split(" / ")
                    .any(|alias| !alias.is_empty() && same_name(query, alias)))
                && locality.is_none_or(|part| {
                    !part.is_empty()
                        && (same_name(part, area)
                            || area.split(',').any(|area| same_name(part, area.trim())))
                })
        })
    };
    let best = candidates().max_by(|a, b| a.5.total_cmp(&b.5))?;
    let ambiguous = candidates().any(|other| {
        // The same mapped city and its administrative region can share a name.
        let nearby = (best.4[0] - other.4[0]).abs() < 0.25
            && super::basemap::wrap_lon_delta(best.4[1] - other.4[1]).abs() < 0.25;
        !nearby && (best.0 != other.0 || best.5 < 10.0 * other.5 || best.5 <= 0.0)
    });
    if ambiguous {
        return None;
    }
    // Ordinary lowercase words may coincide with less prominent place names.
    if locality.is_none()
        && !query.contains(' ')
        && !query.chars().any(char::is_uppercase)
        && best.0 == PlaceKind::City
        && best.5 < 1_000_000.0
    {
        return None;
    }
    Some(Location {
        name: best.1.clone(),
        lat_lon: best.4,
        kind: Some(best.0),
    })
}

fn same_name(query: &str, name: &str) -> bool {
    query.eq_ignore_ascii_case(name)
        || (!query.is_ascii() && !name.is_ascii() && query.to_lowercase() == name.to_lowercase())
}

fn coordinates(text: &str) -> Option<[f64; 2]> {
    let (pair, explicit) = text
        .strip_prefix("geo:")
        .map_or((text, false), |s| (s, true));
    let [lat, lon] = if let Some((lat, lon)) = pair.split_once(',') {
        [lat.trim(), lon.trim()]
    } else {
        let [lat, lon] = pair
            .split_whitespace()
            .collect::<Vec<_>>()
            .try_into()
            .ok()?;
        [lat, lon]
    };
    let (lat, lat_marked) = coordinate_axis(lat, 'N', 'S', 90.0)?;
    let (lon, lon_marked) = coordinate_axis(lon, 'E', 'W', 180.0)?;
    (explicit || (lat_marked && lon_marked)).then_some([lat, lon])
}

fn coordinate_axis(text: &str, positive: char, negative: char, limit: f64) -> Option<(f64, bool)> {
    let suffix = text.chars().last()?.to_ascii_uppercase();
    let has_hemisphere = suffix == positive || suffix == negative;
    let numeric = if has_hemisphere {
        &text[..text.len() - 1]
    } else {
        text
    };
    let numeric = numeric.trim_end().trim_end_matches('°').trim();
    if has_hemisphere && numeric.starts_with(['-', '+']) {
        return None;
    }
    let value: f64 = numeric.parse().ok()?;
    let marked = has_hemisphere || text.contains(['.', '°']) || text.starts_with(['-', '+']);
    let value = if suffix == negative { -value } else { value };
    (-limit..=limit).contains(&value).then_some((value, marked))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_coordinate_pairs_and_rejects_prose_and_numeric_lists() {
        for input in [
            "41.8781, -87.6298",
            "41.8781 -87.6298",
            "41.8781° N, 87.6298° W",
        ] {
            assert_eq!(resolve(input).unwrap().lat_lon, [41.8781, -87.6298]);
        }
        assert_eq!(resolve("geo:0,0").unwrap().lat_lon, [0.0, 0.0]);
        for input in [
            "1,2",
            "41.8, -181.0",
            "42.0, 18.0, 12.4",
            "See 41.8, -87.6",
            "Paris\nFrance",
        ] {
            assert!(resolve(input).is_none(), "{input}");
        }
    }

    #[test]
    fn exact_location_matches_honor_qualifiers_and_ambiguity() {
        assert_eq!(resolve("London").unwrap().name, "London");
        assert_eq!(
            resolve("london, ontario").unwrap().lat_lon,
            [42.97, -81.24999]
        );
        assert!(resolve("Paris, France").is_some());
        assert!(resolve("New York").is_some());
        for input in ["Springfield", "Georgia", "nice", "Somewhere near Paris"] {
            assert!(resolve(input).is_none(), "{input}");
        }
    }
}
