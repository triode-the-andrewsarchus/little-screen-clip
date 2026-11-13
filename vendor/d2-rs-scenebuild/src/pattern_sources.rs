// SPDX-License-Identifier: MPL-2.0
//! pattern_sources.go: the process-wide parsed built-in paper texture, grain
//! PNG and sketch streak commands. The embedded assets are the bytes of
//! d2renderers/internal/patternassets (verified by their digests), shared
//! from d2-rs-render's asset directory.

use std::sync::OnceLock;

use d2_rs_scene::PathCommand;
use sha2::{Digest, Sha256};

use crate::svgimport_path::{PathLimits, parse_path};

pub(crate) const PAPER_PATTERN_SOURCE_SHA256: &str =
    "98b3c110441c6c02cb685c0a7303e7a36c1faee7fa1f65f6424b48ec51123527";
pub(crate) const PAPER_PATTERN_PATH_COUNT: usize = 1042;
pub(crate) const PAPER_PATTERN_COMMAND_COUNT: usize = 10926;
const PAPER_PATTERN_MASK_PATH_DATA: &str =
    "M75.0871 -0.0878906H-0.0878906V75.0871H75.0871V-0.0878906Z";
pub(crate) const GRAIN_PATTERN_PNG_SOURCE_SHA256: &str =
    "7aec37844fa033cbdb586efaa1955dc6f43d56707c0b1bf91e8b5e32386d8d69";
const GRAIN_PATTERN_PNG_BYTES: usize = 20845;
const GRAIN_PATTERN_PIXEL_WIDTH: i64 = 466;
const GRAIN_PATTERN_PIXEL_HEIGHT: i64 = 349;
#[allow(dead_code)]
const SKETCH_STREAK_COMMAND_COUNT: usize = 687;

static PAPER_SVG: &str = d2_rs_render::assets::PAPER_SVG;
static GRAIN_PNG: &[u8] = d2_rs_render::assets::GRAIN_PNG;
#[allow(dead_code)]
static STREAK_PATH_DATA: &str = d2_rs_render::assets::STREAK_PATH_TXT;

pub(crate) struct PaperPatternPath {
    pub color: u8,
    pub commands: Vec<PathCommand>,
}

pub(crate) struct PaperPatternSource {
    pub paths: Vec<PaperPatternPath>,
}

pub(crate) struct GrainPatternSource {
    pub png: &'static [u8],
    pub pixel_width: i64,
    pub pixel_height: i64,
    pub decoded_bytes: i64,
}

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub(crate) fn shared_paper_pattern_source() -> Result<&'static PaperPatternSource, String> {
    static CACHE: OnceLock<Result<PaperPatternSource, String>> = OnceLock::new();
    CACHE
        .get_or_init(|| parse_paper_pattern_source(PAPER_SVG))
        .as_ref()
        .map_err(Clone::clone)
}

pub(crate) fn shared_grain_pattern_source() -> Result<&'static GrainPatternSource, String> {
    static CACHE: OnceLock<Result<GrainPatternSource, String>> = OnceLock::new();
    CACHE
        .get_or_init(|| parse_grain_pattern_source(GRAIN_PNG))
        .as_ref()
        .map_err(Clone::clone)
}

#[allow(dead_code)]
pub(crate) fn shared_sketch_streak_commands() -> Result<&'static [PathCommand], String> {
    static CACHE: OnceLock<Result<Vec<PathCommand>, String>> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            if STREAK_PATH_DATA.is_empty() {
                return Err("sketch streak path is empty".into());
            }
            let commands = parse_path(
                "sketch streak pattern",
                STREAK_PATH_DATA,
                PathLimits {
                    max_bytes: STREAK_PATH_DATA.len(),
                    max_commands: SKETCH_STREAK_COMMAND_COUNT,
                },
            )?;
            if commands.len() != SKETCH_STREAK_COMMAND_COUNT {
                return Err(format!(
                    "sketch streak command count {}, want {SKETCH_STREAK_COMMAND_COUNT}",
                    commands.len()
                ));
            }
            Ok(commands)
        })
        .as_deref()
        .map_err(Clone::clone)
}

fn paper_pattern_color_index(fill: &str) -> Option<u8> {
    Some(match fill {
        "black" => 0,
        "#EFEFEF" => 1,
        "#F5F5F5" => 2,
        "#F7F7F7" => 3,
        "#F9F9F9" => 4,
        "#FCFCFC" => 5,
        "white" => 6,
        _ => return None,
    })
}

fn parse_paper_pattern_source(source: &str) -> Result<PaperPatternSource, String> {
    if digest(source.as_bytes()) != PAPER_PATTERN_SOURCE_SHA256 {
        return Err("paper definition digest does not match its asset ID".into());
    }
    let document =
        roxmltree::Document::parse(source).map_err(|e| format!("parse paper XML: {e}"))?;
    let mut paths: Vec<(String, u8)> = Vec::with_capacity(PAPER_PATTERN_PATH_COUNT + 1);
    for element in document
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "path")
    {
        let (mut data, mut fill) = ("", "");
        for attribute in element.attributes() {
            match attribute.name() {
                "d" => data = attribute.value(),
                "fill" => fill = attribute.value(),
                _ => {}
            }
        }
        let Some(color) = paper_pattern_color_index(fill).filter(|_| !data.is_empty()) else {
            return Err(format!(
                "paper path {} has unsupported fill {} or empty data",
                paths.len(),
                crate::util::q(fill)
            ));
        };
        paths.push((data.to_owned(), color));
    }
    if paths.len() != PAPER_PATTERN_PATH_COUNT + 1
        || paths[0].1 != 6
        || paths[0].0 != PAPER_PATTERN_MASK_PATH_DATA
        || paths[1].1 != 0
    {
        return Err(format!(
            "paper definition topology changed: got {} paths",
            paths.len()
        ));
    }
    let mut parsed = PaperPatternSource {
        paths: Vec::with_capacity(PAPER_PATTERN_PATH_COUNT),
    };
    let mut command_count = 0;
    for (index, (data, color)) in paths.into_iter().skip(1).enumerate() {
        let commands = parse_path(
            "paper pattern",
            &data,
            PathLimits {
                max_bytes: data.len(),
                max_commands: PAPER_PATTERN_COMMAND_COUNT,
            },
        )
        .map_err(|e| format!("paper path {index}: {e}"))?;
        if commands.is_empty() || command_count > PAPER_PATTERN_COMMAND_COUNT - commands.len() {
            return Err(format!("paper path {index} exceeds the command topology"));
        }
        command_count += commands.len();
        parsed.paths.push(PaperPatternPath { color, commands });
    }
    if command_count != PAPER_PATTERN_COMMAND_COUNT {
        return Err(format!(
            "paper command count {command_count}, want {PAPER_PATTERN_COMMAND_COUNT}"
        ));
    }
    Ok(parsed)
}

fn parse_grain_pattern_source(png: &'static [u8]) -> Result<GrainPatternSource, String> {
    if digest(png) != GRAIN_PATTERN_PNG_SOURCE_SHA256 {
        return Err("grain definition digest does not match its asset ID".into());
    }
    if png.len() != GRAIN_PATTERN_PNG_BYTES
        || png.len() < 24
        || &png[..8] != b"\x89PNG\r\n\x1a\n"
        || &png[12..16] != b"IHDR"
    {
        return Err("grain image is not the expected PNG".into());
    }
    let width = i64::from(u32::from_be_bytes(png[16..20].try_into().expect("4 bytes")));
    let height = i64::from(u32::from_be_bytes(png[20..24].try_into().expect("4 bytes")));
    if width != GRAIN_PATTERN_PIXEL_WIDTH || height != GRAIN_PATTERN_PIXEL_HEIGHT {
        return Err(format!("grain dimensions changed to {width}x{height}"));
    }
    Ok(GrainPatternSource {
        png,
        pixel_width: width,
        pixel_height: height,
        decoded_bytes: width * height * 4,
    })
}
