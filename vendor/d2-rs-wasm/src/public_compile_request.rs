// SPDX-License-Identifier: MPL-2.0
//! d2wasm.CompileRequest's encoding/json boundary, before source compilation.
use base64::{
    Engine as _,
    engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig, STANDARD},
};
use d2_rs_core::stable_sort::StableSort;
use serde::{
    Deserialize, Deserializer,
    de::{IgnoredAny, MapAccess, Visitor},
};
use serde_json::{Map, Value};
use std::{cell::RefCell, collections::BTreeMap, fmt};

#[derive(Default)]
struct PublicFonts {
    registered: bool,
    // AddFontFamily writes regular/italic/bold/semibold in this order.
    face_errors: [Option<String>; 4],
}
thread_local! {
    static PUBLIC_FONTS: RefCell<PublicFonts> = RefCell::new(PublicFonts::default());
}

fn register_fonts_and_initialize_ruler(options: &Map<String, Value>) -> Result<(), String> {
    PUBLIC_FONTS.with(|state| {
        let mut fonts = state.borrow_mut();
        let keys = ["fontRegular", "fontItalic", "fontBold", "fontSemibold"];
        if keys.iter().any(|key| options.contains_key(*key)) {
            for (index, key) in keys.iter().enumerate() {
                if let Some(Value::String(encoded)) = options.get(*key) {
                    let bytes = STANDARD.decode(encoded).expect("normalized font base64");
                    // Go commits FontFaces before Sfnt2Woff can fail. Keep
                    // that partial write visible to a later NewRuler.
                    fonts.face_errors[index] = ruler_font_error(&bytes);
                    validate_font_woff(&bytes)?;
                } else {
                    fonts.face_errors[index] = None; // SourceSansPro fallback.
                }
            }
            fonts.registered = true;
        }
        if fonts.registered {
            // textmeasure.NewRuler traverses FontStyles in this order.
            for index in [0, 2, 3, 1] {
                if let Some(error) = &fonts.face_errors[index] {
                    return Err(format!("text ruler cannot be initialized: {error}"));
                }
            }
        }
        Ok(())
    })
}

fn ruler_font_error(bytes: &[u8]) -> Option<String> {
    // sfnt.Collection.initialize first reads16 bytes, then its signature.
    if bytes.len() < 16 {
        return Some("sfnt: invalid bounds".into());
    }
    let magic = u32::from_be_bytes(bytes[..4].try_into().unwrap());
    if !matches!(
        magic,
        0x00010000 | 0x4f54544f | 0x74727565 | 0x74746366 | 0x00000100
    ) {
        return Some("sfnt: invalid font collection".into());
    }
    d2_rs_text::measure::TrueTypeFont::parse(bytes)
        .err()
        .map(|e| e.to_string())
}

// encoding/json.foldName uses Unicode simple-fold equivalents of ASCII keys.
// Long s and Kelvin sign are the only non-ASCII members of those orbits.
fn fold_field_name(key: &str) -> String {
    key.chars()
        .map(|c| match c {
            'ſ' => 's',
            'K' => 'k',
            _ => c.to_ascii_lowercase(),
        })
        .collect()
}

#[derive(Default)]
struct Request {
    files: Option<BTreeMap<String, Option<String>>>,
    path: Option<String>,
    options: Option<Options>,
}
impl<'de> Deserialize<'de> for Request {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Fields;
        impl<'de> Visitor<'de> for Fields {
            type Value = Request;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("CompileRequest object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Request, A::Error> {
                let mut out = Request::default();
                while let Some(key) = map.next_key::<String>()? {
                    match fold_field_name(&key).as_str() {
                        "fs" => {
                            let files =
                                map.next_value::<Option<BTreeMap<String, Option<String>>>>()?;
                            if let Some(files) = files {
                                out.files.get_or_insert_default().extend(files);
                            } else {
                                out.files = None;
                            }
                        }
                        "inputpath" => out.path = map.next_value()?,
                        "options" => {
                            if let Some(options) = map.next_value::<Option<Options>>()? {
                                out.options.get_or_insert_default().0.extend(options.0);
                            } else {
                                out.options = None;
                            }
                        }
                        _ => {
                            map.next_value::<IgnoredAny>()?;
                        }
                    }
                }
                Ok(out)
            }
        }
        d.deserialize_map(Fields)
    }
}

#[derive(Default)]
struct Options<const COMPILE: bool = true>(Map<String, Value>);
impl<'de, const COMPILE: bool> Deserialize<'de> for Options<COMPILE> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Fields<const COMPILE: bool>;
        impl<'de, const COMPILE: bool> Visitor<'de> for Fields<COMPILE> {
            type Value = Options<COMPILE>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("CompileOptions object")
            }
            fn visit_map<A: MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Options<COMPILE>, A::Error> {
                let mut out = Map::new();
                while let Some(key) = map.next_key::<String>()? {
                    let (canonical, value) = match fold_field_name(&key).as_str() {
                        "layout" if COMPILE => (
                            "layout",
                            serde_json::to_value(map.next_value::<Option<String>>()?).unwrap(),
                        ),
                        "target" => (
                            "target",
                            serde_json::to_value(map.next_value::<Option<String>>()?).unwrap(),
                        ),
                        "salt" => (
                            "salt",
                            serde_json::to_value(map.next_value::<Option<String>>()?).unwrap(),
                        ),
                        "asciimode" => (
                            "asciiMode",
                            serde_json::to_value(map.next_value::<Option<String>>()?).unwrap(),
                        ),
                        "pad" => (
                            "pad",
                            serde_json::to_value(map.next_value::<Option<i64>>()?).unwrap(),
                        ),
                        "themeid" => (
                            "themeID",
                            serde_json::to_value(map.next_value::<Option<i64>>()?).unwrap(),
                        ),
                        "darkthemeid" => (
                            "darkThemeID",
                            serde_json::to_value(map.next_value::<Option<i64>>()?).unwrap(),
                        ),
                        "animateinterval" => (
                            "animateInterval",
                            serde_json::to_value(map.next_value::<Option<i64>>()?).unwrap(),
                        ),
                        "scale" => (
                            "scale",
                            serde_json::to_value(map.next_value::<Option<f64>>()?).unwrap(),
                        ),
                        "sketch" => (
                            "sketch",
                            serde_json::to_value(map.next_value::<Option<bool>>()?).unwrap(),
                        ),
                        "isometric" => (
                            "isometric",
                            serde_json::to_value(map.next_value::<Option<bool>>()?).unwrap(),
                        ),
                        "center" => (
                            "center",
                            serde_json::to_value(map.next_value::<Option<bool>>()?).unwrap(),
                        ),
                        "forceappendix" => (
                            "forceAppendix",
                            serde_json::to_value(map.next_value::<Option<bool>>()?).unwrap(),
                        ),
                        "noxmltag" => (
                            "noXMLTag",
                            serde_json::to_value(map.next_value::<Option<bool>>()?).unwrap(),
                        ),
                        "ascii" => (
                            "ascii",
                            serde_json::to_value(map.next_value::<Option<bool>>()?).unwrap(),
                        ),
                        "themeoverrides" => (
                            "themeOverrides",
                            serde_json::to_value(
                                map.next_value::<Option<Overrides>>()?.map(|v| v.0),
                            )
                            .unwrap(),
                        ),
                        "darkthemeoverrides" => (
                            "darkThemeOverrides",
                            serde_json::to_value(
                                map.next_value::<Option<Overrides>>()?.map(|v| v.0),
                            )
                            .unwrap(),
                        ),
                        name @ ("fontregular" | "fontitalic" | "fontbold" | "fontsemibold")
                            if COMPILE =>
                        {
                            let name = match name {
                                "fontregular" => "fontRegular",
                                "fontitalic" => "fontItalic",
                                "fontbold" => "fontBold",
                                _ => "fontSemibold",
                            };
                            let value = decode_bytes(map.next_value()?)
                                .map_err(serde::de::Error::custom)?;
                            (
                                name,
                                value.map_or(Value::Null, |bytes| {
                                    Value::String(STANDARD.encode(bytes))
                                }),
                            )
                        }
                        _ => {
                            map.next_value::<IgnoredAny>()?;
                            continue;
                        }
                    };
                    out.insert(canonical.to_owned(), value);
                }
                Ok(Options(out))
            }
        }
        d.deserialize_map(Fields::<COMPILE>)
    }
}

struct Overrides(Map<String, Value>);
impl<'de> Deserialize<'de> for Overrides {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Fields;
        impl<'de> Visitor<'de> for Fields {
            type Value = Overrides;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("ThemeOverrides object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Overrides, A::Error> {
                let mut out = Map::new();
                while let Some(key) = map.next_key::<String>()? {
                    let key = fold_field_name(&key);
                    if matches!(
                        key.as_str(),
                        "n1" | "n2"
                            | "n3"
                            | "n4"
                            | "n5"
                            | "n6"
                            | "n7"
                            | "b1"
                            | "b2"
                            | "b3"
                            | "b4"
                            | "b5"
                            | "b6"
                            | "aa2"
                            | "aa4"
                            | "aa5"
                            | "ab4"
                            | "ab5"
                    ) {
                        out.insert(
                            key,
                            serde_json::to_value(map.next_value::<Option<String>>()?).unwrap(),
                        );
                    } else {
                        map.next_value::<IgnoredAny>()?;
                    }
                }
                out.retain(|_, value| !value.is_null());
                Ok(Overrides(out))
            }
        }
        d.deserialize_map(Fields)
    }
}

fn decode_bytes(value: Value) -> Result<Option<Vec<u8>>, String> {
    match value {
        Value::Null => Ok(None),
        Value::String(text) => {
            // encoding/base64.StdEncoding accepts CR/LF and nonzero pad bits.
            let decoder = GeneralPurpose::new(
                &base64::alphabet::STANDARD,
                GeneralPurposeConfig::new().with_decode_allow_trailing_bits(true),
            );
            decoder
                .decode(text.replace(['\r', '\n'], ""))
                .map(Some)
                .map_err(|e| e.to_string())
        }
        Value::Array(values) => values
            .into_iter()
            .map(|v| {
                if v.is_null() {
                    return Ok(0);
                }
                v.as_u64()
                    .and_then(|n| u8::try_from(n).ok())
                    .ok_or_else(|| "expected byte".to_owned())
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
        _ => Err("expected base64 string or byte array".into()),
    }
}

fn decode_request(json: &str) -> Result<Request, String> {
    let input = d2_rs_lib::prepare_go_json_input(json.as_bytes())
        .map_err(|_| "invalid JSON input".to_owned())?;
    serde_json::from_slice::<Option<Request>>(&input)
        .map_err(|_| "invalid JSON input".to_owned())
        .map(Option::unwrap_or_default)
}

/// GetELKGraph decodes the full CompileRequest schema, but does not apply its
/// options or register fonts. Keep that distinction from the Compile entrypoint.
pub(super) fn elk_input(json: &str) -> Result<(BTreeMap<String, String>, String), String> {
    let request = decode_request(json)?;
    let files = request
        .files
        .ok_or_else(|| "missing 'fs' field in input JSON".to_owned())?;
    let path = request.path.unwrap_or_else(|| "index".to_owned());
    if !files.contains_key(&path) {
        return Err(format!("missing '{path}' file in input fs"));
    }
    Ok((
        files
            .into_iter()
            .map(|(key, value)| (key, value.unwrap_or_default()))
            .collect(),
        path,
    ))
}

pub(super) fn prepare(json: &str) -> Result<String, String> {
    let request = decode_request(json)?;
    let files = request
        .files
        .ok_or_else(|| "missing 'fs' field in input JSON".to_owned())?;
    let path = request.path.unwrap_or_else(|| "index".to_owned());
    if !files.contains_key(&path) {
        return Err(format!("missing '{path}' file in input fs"));
    }
    let mut options = request.options.unwrap_or_default().0;
    options.retain(|_, value| !value.is_null());
    register_fonts_and_initialize_ruler(&options)?;
    if options.get("ascii") == Some(&Value::Bool(true))
        && options.get("layout").is_none_or(|v| v == "dagre")
    {
        options.insert("layout".into(), Value::String("elk".into()));
    }
    let files: BTreeMap<_, _> = files
        .into_iter()
        .map(|(key, value)| (key, value.unwrap_or_default()))
        .collect();
    // Compile only consumes these options. Target/animation/export options
    // are returned for Render; passing them to the native source helper here
    // would select boards or reject render-only values during Compile.
    let mut source_options = serde_json::json!({"path":path,"files":files,"utf16_positions":true});
    for (public, native) in [
        ("layout", "engine"),
        ("themeID", "theme_id"),
        ("darkThemeID", "dark_theme_id"),
        ("sketch", "sketch"),
        ("isometric", "isometric"),
        ("fontRegular", "font_regular"),
        ("fontItalic", "font_italic"),
        ("fontBold", "font_bold"),
        ("fontSemibold", "font_semibold"),
    ] {
        if let Some(value) = options.get(public) {
            source_options[native] = value.clone();
        }
    }
    source_options["public_render_options"] = Value::Object(
        super::public_render_options::FIELDS
            .into_iter()
            .filter_map(|key| {
                options
                    .get(key)
                    .map(|value| (key.to_owned(), value.clone()))
            })
            .collect(),
    );
    serde_json::to_string(&serde_json::json!({"fs":files,"inputPath":path,"options":options,"sourceOptions":source_options}))
        .map_err(|e| e.to_string())
}

fn font_fault(message: String) -> String {
    format!("panic recovered: runtime error: {message}\nnative source context: lib/font.Sfnt2Woff")
}
fn font_slice(bytes: &[u8], start: usize, end: usize) -> Result<&[u8], String> {
    if start > bytes.len() {
        return Err(font_fault(format!(
            "slice bounds out of range [{start}:{}]",
            bytes.len()
        )));
    }
    if end > bytes.len() {
        return Err(font_fault(format!(
            "slice bounds out of range [:{end}] with capacity {}",
            bytes.len()
        )));
    }
    if start > end {
        return Err(font_fault(format!(
            "slice bounds out of range [{start}:{end}]"
        )));
    }
    Ok(&bytes[start..end])
}
fn read_u16(bytes: &[u8]) -> Result<u16, String> {
    if bytes.len() < 2 {
        return Err(font_fault(format!(
            "index out of range [1] with length {}",
            bytes.len()
        )));
    }
    Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
}
fn read_u32(bytes: &[u8]) -> Result<u32, String> {
    if bytes.len() < 4 {
        return Err(font_fault(format!(
            "index out of range [3] with length {}",
            bytes.len()
        )));
    }
    Ok(u32::from_be_bytes(bytes[..4].try_into().unwrap()))
}
/// Validation accesses/checksums from lib/font.Sfnt2Woff. Actual font metrics
/// remain in the native TrueType parser; no WOFF encoder or Go runtime is used.
fn validate_font_woff(bytes: &[u8]) -> Result<(), String> {
    let tables = read_u16(font_slice(bytes, 4, bytes.len())?)? as usize;
    let mut entries = Vec::with_capacity(tables);
    for i in 0..tables {
        let entry = font_slice(bytes, 12 + i * 16, bytes.len())?;
        let tag = font_slice(entry, 0, 4)?;
        let checksum = read_u32(font_slice(entry, 4, entry.len())?)?;
        let offset = read_u32(font_slice(entry, 8, entry.len())?)? as usize;
        let length = read_u32(font_slice(entry, 12, entry.len())?)? as usize;
        entries.push((tag, checksum, offset, length));
    }
    entries.stable_sort_by_key(|entry| entry.0);
    for &(tag, checksum, offset, length) in &entries {
        if tag != b"head" {
            let aligned = (length as u32).wrapping_add(3) & !3;
            let data = font_slice(bytes, offset, offset + aligned as usize)?;
            let actual = data.chunks_exact(4).fold(0u32, |sum, c| {
                sum.wrapping_add(u32::from_be_bytes(c.try_into().unwrap()))
            });
            if actual != checksum {
                return Err(format!(
                    "custom fonts could not be initialized: failed to encode ttf to woff: checksum error in table: {}",
                    String::from_utf8_lossy(tag)
                ));
            }
        }
    }
    font_slice(bytes, 0, 12)?;
    for &(tag, _, offset, length) in &entries {
        let data = font_slice(bytes, offset, offset + length)?;
        if tag == b"head" {
            read_u16(font_slice(data, 4, data.len())?)?;
            read_u16(font_slice(data, 6, data.len())?)?;
            read_u32(data)?;
            font_slice(data, 8, 12)?;
        }
    }
    Ok(())
}

/// Decode only RenderOptions. Compile-only fields are unknown to Go Render
/// and must be ignored even when they contain otherwise invalid font data.
pub(super) fn render_options(json: &str) -> Result<Map<String, Value>, String> {
    let decoded: Option<Options<false>> =
        serde_json::from_str(if json.trim().is_empty() { "{}" } else { json })
            .map_err(|_| "invalid JSON input".to_owned())?;
    let mut options = decoded.unwrap_or_default().0;
    options.retain(|_, value| !value.is_null());
    Ok(options)
}
pub(super) fn initialize_render_ruler() -> Result<(), String> {
    register_fonts_and_initialize_ruler(&Map::new())
}
