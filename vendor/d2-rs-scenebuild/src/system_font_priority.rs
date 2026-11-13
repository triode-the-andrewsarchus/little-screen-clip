// SPDX-License-Identifier: MPL-2.0
// Path and face ranking from d2renderers/d2fonts/fallback.go.
use crate::FallbackRequest;
use std::collections::BTreeSet;
use std::path::Path;

#[derive(Debug)]
pub(crate) struct RequestProfile {
    cjk: bool,
    arabic: bool,
    hebrew: bool,
    symbol: bool,
    italic: bool,
    mono: bool,
    family: String,
    weight: i64,
}

#[derive(Debug)]
pub(crate) struct FaceProfile {
    pub family: String,
    pub italic: bool,
    pub mono: bool,
    pub weight: i64,
}

impl FaceProfile {
    pub(crate) fn from_face(face: &d2_rs_fontface::ParsedFace) -> Self {
        let description = face.fallback_description();
        Self {
            family: normalize_family(&description.family),
            italic: description.italic,
            mono: description.monospace,
            weight: description.weight,
        }
    }
}

fn has_any(value: &str, parts: &[&str]) -> bool {
    parts.iter().any(|part| value.contains(part))
}

pub(crate) fn normalize_family(value: &str) -> String {
    value
        .to_lowercase()
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect()
}

impl RequestProfile {
    pub(crate) fn new(request: &FallbackRequest) -> Self {
        let family = normalize_family(&request.family);
        let style = request.style.to_lowercase();
        let weight = if request.weight > 0 {
            request.weight
        } else if has_any(&style, &["semibold", "semi-bold"]) {
            600
        } else if style.contains("bold") {
            700
        } else {
            400
        }
        .clamp(100, 900);
        let mut profile = Self {
            cjk: false,
            arabic: false,
            hebrew: false,
            symbol: false,
            italic: has_any(&style, &["italic", "oblique"]),
            mono: has_any(&family, &["mono", "code", "console"]),
            family,
            weight,
        };
        for &r in &request.runes {
            let c = r as u32;
            profile.cjk |= (0x2e80..=0x9fff).contains(&c)
                || (0xf900..=0xfaff).contains(&c)
                || (0x20000..=0x323af).contains(&c);
            profile.arabic |= (0x0600..=0x08ff).contains(&c)
                || (0xfb50..=0xfdff).contains(&c)
                || (0xfe70..=0xfeff).contains(&c);
            profile.hebrew |= (0x0590..=0x05ff).contains(&c) || (0xfb1d..=0xfb4f).contains(&c);
            profile.symbol |= (0x2190..=0x2bff).contains(&c) || (0x1f000..=0x1faff).contains(&c);
        }
        profile
    }

    pub(crate) fn face_priority(&self, face: &FaceProfile) -> i64 {
        let mut penalty = (face.weight - self.weight).abs();
        if face.italic != self.italic {
            penalty += 1000;
        }
        if face.mono != self.mono {
            penalty += if self.mono { 1000 } else { 100 };
        }
        if !self.family.is_empty() && !face.family.is_empty() {
            if face.family == self.family {
                penalty -= 40;
            } else if ["sans", "serif", "mono", "code", "console"]
                .iter()
                .any(|class| face.family.contains(class) && self.family.contains(class))
            {
                penalty -= 10;
            }
        }
        penalty
    }

    pub(crate) fn path_priority(&self, path: &Path) -> i64 {
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase();
        let contains = |parts: &[&str]| has_any(&name, parts);
        let mut semantic = if contains(&["arial unicode", "unifont"]) {
            -10
        } else {
            80
        };
        if (self.cjk
            && contains(&[
                "cjk", "hiragino", "pingfang", "heiti", "gothic", "songti", "mingliu", "simsun",
                "malgun",
            ]))
            || (self.arabic && contains(&["arab", "amiri", "naskh", "kufi"]))
            || (self.hebrew && contains(&["hebrew", "david", "frank"]))
            || (self.symbol
                && contains(&["symbol", "emoji", "dingbat", "dejavu", "unifont", "unicode"]))
        {
            semantic = semantic.min(0);
        }
        if contains(&["noto", "dejavu", "arial unicode", "unifont"]) {
            semantic = semantic.min(20);
        }
        if contains(&["emoji"]) {
            semantic += 10;
        }
        if self.mono && contains(&["mono", "code", "console"]) {
            semantic -= 6;
        }
        let face = FaceProfile {
            family: normalize_family(&name),
            italic: contains(&["italic", "oblique"]),
            mono: contains(&["mono", "code", "console"]),
            weight: filename_weight(&name),
        };
        self.face_priority(&face) + semantic
    }
}

/// Select faces from one already-admitted collection in the same order as
/// SystemFallbackResolver.ResolveFallbacks. Coverage is charged on every
/// pass, including repeated checks after another face has been selected.
/// The caller supplies the go-text face description once that metadata port
/// is available; keeping it lazy matches Go's profiling of covering faces.
#[allow(dead_code)]
pub(crate) fn select_collection_faces(
    collection: &d2_rs_fontface::Collection,
    remaining: &mut BTreeSet<char>,
    request: &RequestProfile,
    coverage_checks: &mut u64,
    max_coverage_checks: u64,
    mut describe: impl FnMut(&d2_rs_fontface::ParsedFace) -> FaceProfile,
    mut retain: impl FnMut(usize) -> Result<(), String>,
) -> Result<(), String> {
    let mut selected = vec![false; collection.num_faces()];
    let mut profiles: Vec<Option<FaceProfile>> =
        (0..collection.num_faces()).map(|_| None).collect();
    while !remaining.is_empty() {
        let mut best: Option<(usize, i64, Vec<char>)> = None;
        for index in 0..collection.num_faces().min(u16::MAX as usize + 1) {
            if selected[index] {
                continue;
            }
            let Ok(face) = collection.face(index) else {
                continue;
            };
            let mut covered = Vec::new();
            for &value in remaining.iter() {
                if *coverage_checks >= max_coverage_checks {
                    let first = *remaining.first().unwrap() as u32;
                    return Err(format!(
                        "d2fonts: system font coverage checks exceed limit {max_coverage_checks} across this resolver while resolving U+{first:04X}"
                    ));
                }
                *coverage_checks += 1;
                if face
                    .supports_renderable_rune(value)
                    .map_err(|e| e.to_string())?
                {
                    covered.push(value);
                }
            }
            if covered.is_empty() {
                continue;
            }
            let profile = profiles[index].get_or_insert_with(|| describe(&face));
            let priority = request.face_priority(profile);
            if best.as_ref().is_none_or(|(_, old_priority, old_coverage)| {
                priority < *old_priority
                    || priority == *old_priority && covered.len() > old_coverage.len()
            }) {
                best = Some((index, priority, covered));
            }
        }
        let Some((index, _, covered)) = best else {
            break;
        };
        retain(index)?;
        selected[index] = true;
        for value in covered {
            remaining.remove(&value);
        }
    }
    Ok(())
}

fn filename_weight(name: &str) -> i64 {
    for (parts, weight) in [
        (&["extrablack", "ultrablack", "superblack"][..], 900),
        (&["black", "heavy"][..], 900),
        (
            &["extrabold", "extra-bold", "ultrabold", "ultra-bold"][..],
            800,
        ),
        (&["semibold", "semi-bold", "demibold", "demi-bold"][..], 600),
        (&["bold"][..], 700),
        (&["medium"][..], 500),
        (
            &["extralight", "extra-light", "ultralight", "ultra-light"][..],
            200,
        ),
        (
            &["semilight", "semi-light", "demilight", "demi-light"][..],
            350,
        ),
        (&["thin"][..], 100),
        (&["light"][..], 300),
    ] {
        if has_any(name, parts) {
            return weight;
        }
    }
    400
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_charges_coverage_and_only_removes_retained_runes() {
        let data = std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../d2-rs-text/assets/SourceSansPro-Regular.ttf"),
        )
        .unwrap();
        let collection = d2_rs_fontface::parse_face_collection_with_limit(&data, 1).unwrap();
        let request = FallbackRequest {
            runes: vec!['A', '\u{10ffff}'],
            family: String::new(),
            style: String::new(),
            weight: 400,
        };
        let profile = RequestProfile::new(&request);
        let describe = |_: &d2_rs_fontface::ParsedFace| FaceProfile {
            family: String::new(),
            italic: false,
            mono: false,
            weight: 400,
        };
        let mut remaining = request.runes.iter().copied().collect();
        let mut checks = 0;
        let error = select_collection_faces(
            &collection,
            &mut remaining,
            &profile,
            &mut checks,
            2,
            describe,
            |_| Err("retention limit".into()),
        )
        .unwrap_err();
        assert_eq!(error, "retention limit");
        assert_eq!(checks, 2);
        assert_eq!(remaining, request.runes.iter().copied().collect());

        checks = 0;
        let mut selected = Vec::new();
        select_collection_faces(
            &collection,
            &mut remaining,
            &profile,
            &mut checks,
            2,
            describe,
            |index| {
                selected.push(index);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(selected, [0]);
        assert_eq!(remaining, ['\u{10ffff}'].into_iter().collect());
        assert_eq!(checks, 2);
    }

    // d2fonts/fallback_test.go TestFallbackPathPriorityPreservesRequestedStyle.
    #[test]
    fn preserves_requested_style() {
        let request = |family: &str, style: &str, weight| FallbackRequest {
            runes: vec!['A'],
            family: family.into(),
            style: style.into(),
            weight,
        };
        let italic = RequestProfile::new(&request("SourceSansPro", "italic", 400));
        assert!(
            italic.path_priority(Path::new("Custom-Italic.ttf"))
                < italic.path_priority(Path::new("Custom-Regular.ttf"))
        );
        let mono = RequestProfile::new(&request("SourceCodePro", "bold", 700));
        assert!(
            mono.path_priority(Path::new("CustomMono-Bold.ttf"))
                < mono.path_priority(Path::new("Custom-Regular.ttf"))
        );
        let bold = RequestProfile::new(&request("Source Sans Pro", "bold", 700));
        assert!(
            bold.path_priority(Path::new("NotoSansArabic-Bold.ttf"))
                < bold.path_priority(Path::new("Arial Unicode.ttf"))
        );
    }
}
