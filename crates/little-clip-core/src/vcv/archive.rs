//! Reader for VCV Rack / Cardinal `.vcv` patch files (zstd tar bundles and raw JSON).

use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::Path;

const ZSTD_MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];
pub const MAX_PATCH_BYTES: u64 = 64 * 1024 * 1024;
const MAX_DECOMPRESSED_BYTES: u64 = 64 * 1024 * 1024;

/// A port reference on a module inside a patch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PortRef {
    pub module_id: i64,
    pub port_id: i32,
}

/// One module placed in a VCV Rack or Cardinal patch.
#[derive(Clone, Debug)]
pub struct PatchModule {
    pub id: i64,
    pub plugin: String,
    pub model: String,
    pub version: Option<String>,
    pub params: BTreeMap<i32, f64>,
    pub left_module_id: Option<i64>,
    pub right_module_id: Option<i64>,
    pub row: i32,
    pub column: i32,
    pub bypassed: bool,
    pub linked_source_id: Option<i64>,
    pub labels: Vec<String>,
    pub learned_notes: Vec<i32>,
    pub learned_ccs: Vec<i32>,
    pub raw_data: Option<Value>,
}

impl PatchModule {
    /// Returns the catalog lookup key `"Plugin/Model"`.
    pub fn catalog_key(&self) -> String {
        format!("{}/{}", self.plugin, self.model)
    }
}

/// One patch cable connecting an output port to an input port.
#[derive(Clone, Debug)]
pub struct PatchCable {
    pub id: i64,
    pub from: PortRef,
    pub to: PortRef,
    pub color: Option<String>,
}

/// Parsed contents of a `.vcv` patch (`patch.json`) or stoermelder STRIP preset (`.vcvss`).
#[derive(Clone, Debug)]
pub struct PatchDocument {
    pub version: String,
    pub modules: Vec<PatchModule>,
    pub cables: Vec<PatchCable>,
    pub is_strip: bool,
}

impl PatchDocument {
    /// Returns the number of distinct Eurorack rows occupied by modules in this patch,
    /// or `0` for stoermelder STRIP selection files (`.vcvss`).
    pub fn row_count(&self) -> usize {
        if self.is_strip {
            return 0;
        }
        self.modules
            .iter()
            .map(|module| module.row)
            .collect::<BTreeSet<_>>()
            .len()
    }
}

/// Returns whether `path` uses stoermelder STRIP's `.vcvss` extension.
pub fn is_vcv_strip_path(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("vcvss"))
}

/// Reads and parses a `.vcv` or `.vcvss` patch from raw bytes (either zstd+tar archive or raw UTF-8 JSON).
pub fn read_patch_bytes(bytes: &[u8]) -> Result<PatchDocument, String> {
    if bytes.is_empty() {
        return Err("Patch file is empty".to_string());
    }
    let json_bytes = if bytes.len() >= 4 && bytes[..4] == ZSTD_MAGIC {
        extract_patch_json_from_zst_tar(bytes)?
    } else {
        if bytes.len() as u64 > MAX_PATCH_BYTES {
            return Err("Patch JSON exceeds size limit".to_string());
        }
        bytes.to_vec()
    };
    let text = strip_utf8_bom(&json_bytes);
    let root: Value =
        serde_json::from_slice(text).map_err(|e| format!("Invalid patch JSON: {e}"))?;
    parse_patch_value(&root)
}

/// Reads and parses raw patch bytes while preserving strip semantics for `.vcvss` paths.
pub fn read_patch_bytes_for_path(path: &Path, bytes: &[u8]) -> Result<PatchDocument, String> {
    let mut doc = read_patch_bytes(bytes)?;
    if is_vcv_strip_path(path) {
        doc.is_strip = true;
    }
    Ok(doc)
}

/// Reads and parses a `.vcv` or `.vcvss` patch file from disk with a size guard.
pub fn read_patch_path(path: &Path) -> Result<PatchDocument, String> {
    let mut file = std::fs::File::open(path).map_err(|e| format!("Cannot open patch: {e}"))?;
    let meta = file
        .metadata()
        .map_err(|e| format!("Cannot stat patch: {e}"))?;
    if meta.len() > MAX_PATCH_BYTES {
        return Err("Patch file exceeds 64 MiB limit".to_string());
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    file.by_ref()
        .take(MAX_PATCH_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("Cannot read patch: {e}"))?;
    if bytes.len() as u64 > MAX_PATCH_BYTES {
        return Err("Patch file exceeds 64 MiB limit".to_string());
    }
    read_patch_bytes_for_path(path, &bytes)
}

fn extract_patch_json_from_zst_tar(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let decoder = zstd::stream::Decoder::new(bytes)
        .map_err(|e| format!("Cannot initialize zstd decoder: {e}"))?;
    let limited = decoder.take(MAX_DECOMPRESSED_BYTES + 1);
    let mut archive = tar::Archive::new(limited);
    let entries = archive
        .entries()
        .map_err(|e| format!("Invalid .vcv tar archive: {e}"))?;

    for entry_result in entries {
        let mut entry = entry_result.map_err(|e| format!("Corrupt tar entry in .vcv: {e}"))?;
        let path = entry
            .path()
            .map_err(|e| format!("Invalid tar entry path: {e}"))?;
        let path_str = path.to_string_lossy();
        let normalized = path_str.trim_start_matches("./");
        if normalized == "patch.json" || normalized.ends_with("/patch.json") {
            let size = entry.size();
            if size > MAX_DECOMPRESSED_BYTES {
                return Err("patch.json in archive exceeds 64 MiB limit".to_string());
            }
            let mut out = Vec::with_capacity(size.min(4 * 1024 * 1024) as usize);
            entry
                .by_ref()
                .take(MAX_DECOMPRESSED_BYTES + 1)
                .read_to_end(&mut out)
                .map_err(|e| format!("Cannot read patch.json from .vcv archive: {e}"))?;
            if out.len() as u64 > MAX_DECOMPRESSED_BYTES {
                return Err("patch.json in archive exceeds 64 MiB limit".to_string());
            }
            return Ok(out);
        }
    }
    Err("Archive does not contain patch.json".to_string())
}

fn strip_utf8_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes)
}

fn parse_optional_array<'a>(
    obj: &'a serde_json::Map<String, Value>,
    key: &str,
) -> Result<&'a [Value], String> {
    match obj.get(key) {
        None => Ok(&[]),
        Some(value) => value
            .as_array()
            .map(Vec::as_slice)
            .ok_or_else(|| format!("Patch {key} is not a JSON array")),
    }
}

fn parse_patch_module(
    idx: usize,
    item: &Value,
    override_pos: Option<(i32, i32)>,
) -> Result<PatchModule, String> {
    let Some(m) = item.as_object() else {
        return Err(format!("Module #{idx} is not a JSON object"));
    };
    let id = m
        .get("id")
        .and_then(json_i64)
        .ok_or_else(|| format!("Module #{idx} is missing integer id"))?;
    let plugin = m
        .get("plugin")
        .and_then(json_str)
        .ok_or_else(|| format!("Module {id} is missing plugin"))?
        .to_string();
    let model = m
        .get("model")
        .and_then(json_str)
        .ok_or_else(|| format!("Module {id} is missing model"))?
        .to_string();
    let mod_version = m.get("version").and_then(json_str).map(str::to_string);
    let left_module_id = m.get("leftModuleId").and_then(json_i64);
    let right_module_id = m.get("rightModuleId").and_then(json_i64);
    let bypassed = m.get("bypass").and_then(Value::as_bool).unwrap_or(false);
    let (column, row) = match override_pos {
        Some(pos) => pos,
        None => parse_position(m, id)?,
    };
    let params = parse_params(m.get("params"));

    let data = m.get("data").cloned();
    let linked_source_id = data
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|d| d.get("srcId"))
        .and_then(json_i64);
    let labels = parse_labels(data.as_ref());
    let learned_notes = parse_learned_ints(data.as_ref(), "notes");
    let learned_ccs = parse_learned_ints(data.as_ref(), "ccs");

    Ok(PatchModule {
        id,
        plugin,
        model,
        version: mod_version,
        params,
        left_module_id,
        right_module_id,
        row,
        column,
        bypassed,
        linked_source_id,
        labels,
        learned_notes,
        learned_ccs,
        raw_data: data,
    })
}

/// Parses a `serde_json::Value` representing `patch.json` or a `.vcvss` strip preset into a normalized `PatchDocument`.
pub fn parse_patch_value(root: &Value) -> Result<PatchDocument, String> {
    let obj = root
        .as_object()
        .ok_or_else(|| "Patch root is not a JSON object".to_string())?;
    let version = obj
        .get("version")
        .and_then(json_str)
        .unwrap_or("")
        .to_string();

    let (mut modules, is_strip) =
        if let Some(raw_modules) = obj.get("modules").and_then(Value::as_array) {
            let mut modules = Vec::with_capacity(raw_modules.len());
            for (idx, item) in raw_modules.iter().enumerate() {
                modules.push(parse_patch_module(idx, item, None)?);
            }
            (modules, false)
        } else if obj.contains_key("rightModules") || obj.contains_key("leftModules") {
            let left_modules = parse_optional_array(obj, "leftModules")?;
            let right_modules = parse_optional_array(obj, "rightModules")?;
            let mut modules = Vec::with_capacity(left_modules.len() + right_modules.len());
            // stoermelder STRIP stores leftModules right-to-left and rightModules left-to-right;
            // emit left-to-right across row 0 so layout order matches physical Rack placement.
            for (slot, item) in left_modules
                .iter()
                .rev()
                .chain(right_modules.iter())
                .enumerate()
            {
                let col = i32::try_from(slot).unwrap_or(i32::MAX);
                modules.push(parse_patch_module(slot, item, Some((col, 0)))?);
            }
            (modules, true)
        } else {
            return Err("Patch does not contain a modules array".to_string());
        };

    // Preserve rack visual order (top-to-bottom row, then left-to-right column).
    modules.sort_by_key(|m| (m.row, m.column, m.id));

    let mut cables = Vec::new();
    if let Some(raw_cables) = obj.get("cables").and_then(Value::as_array) {
        for (idx, item) in raw_cables.iter().enumerate() {
            let Some(c) = item.as_object() else {
                continue;
            };
            let id = c.get("id").and_then(json_i64).unwrap_or(idx as i64);
            let Some(output_module_id) = c.get("outputModuleId").and_then(json_i64) else {
                continue;
            };
            let Some(output_id) = c.get("outputId").and_then(json_i32) else {
                continue;
            };
            let Some(input_module_id) = c.get("inputModuleId").and_then(json_i64) else {
                continue;
            };
            let Some(input_id) = c.get("inputId").and_then(json_i32) else {
                continue;
            };
            let color = c.get("color").and_then(json_str).map(str::to_string);
            cables.push(PatchCable {
                id,
                from: PortRef {
                    module_id: output_module_id,
                    port_id: output_id,
                },
                to: PortRef {
                    module_id: input_module_id,
                    port_id: input_id,
                },
                color,
            });
        }
    }

    Ok(PatchDocument {
        version,
        modules,
        cables,
        is_strip,
    })
}

fn parse_position(obj: &serde_json::Map<String, Value>, id: i64) -> Result<(i32, i32), String> {
    let Some(pos) = obj.get("pos") else {
        return Ok((0, 0));
    };
    let Some(arr) = pos.as_array() else {
        return Err(format!("Module {id} has non-array pos"));
    };
    if arr.len() != 2 {
        return Err(format!("Module {id} has invalid pos length"));
    }
    let Some(x) = arr[0].as_f64().filter(|v| v.is_finite()) else {
        return Err(format!("Module {id} has invalid pos.x"));
    };
    let Some(y) = arr[1].as_f64().filter(|v| v.is_finite()) else {
        return Err(format!("Module {id} has invalid pos.y"));
    };
    Ok((x.round() as i32, y.round() as i32))
}

fn parse_params(params: Option<&Value>) -> BTreeMap<i32, f64> {
    let mut map = BTreeMap::new();
    let Some(arr) = params.and_then(Value::as_array) else {
        return map;
    };
    for (idx, item) in arr.iter().enumerate() {
        let Some(obj) = item.as_object() else {
            continue;
        };
        let pid = obj.get("id").and_then(json_i32).unwrap_or(idx as i32);
        if let Some(val) = obj.get("value").and_then(Value::as_f64)
            && val.is_finite()
        {
            map.insert(pid, val);
        }
    }
    map
}

fn parse_labels(data: Option<&Value>) -> Vec<String> {
    let Some(obj) = data.and_then(Value::as_object) else {
        return Vec::new();
    };
    let Some(arr) = obj.get("labels").and_then(Value::as_array) else {
        return Vec::new();
    };
    arr.iter()
        .map(|v| v.as_str().unwrap_or("").trim().to_string())
        .collect()
}

fn parse_learned_ints(data: Option<&Value>, key: &str) -> Vec<i32> {
    let Some(obj) = data.and_then(Value::as_object) else {
        return Vec::new();
    };
    let Some(arr) = obj.get(key).and_then(Value::as_array) else {
        return Vec::new();
    };
    arr.iter().map(|v| json_i32(v).unwrap_or(-1)).collect()
}

fn json_str(v: &Value) -> Option<&str> {
    v.as_str()
}

fn json_i64(v: &Value) -> Option<i64> {
    if let Some(i) = v.as_i64() {
        return Some(i);
    }
    if let Some(u) = v.as_u64() {
        return i64::try_from(u).ok();
    }
    let f = v.as_f64()?;
    if f.is_finite() && f.fract() == 0.0 && f >= i64::MIN as f64 && f < i64::MAX as f64 {
        Some(f as i64)
    } else {
        None
    }
}

fn json_i32(v: &Value) -> Option<i32> {
    json_i64(v).and_then(|n| i32::try_from(n).ok())
}
