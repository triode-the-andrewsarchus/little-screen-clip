// SPDX-License-Identifier: MPL-2.0
//! Rust side of the scenebuild tier of scripts/scene-scoreboard.py, mirroring
//! tools/scene-probe (main.go and scenejson.go).
//!
//! stdin: JSON lines {"board", "themeID", "out", "mode", "assets", "go"}. Each
//! board in source traversal order (root named "board", then
//! board.layers.N, board.scenarios.N, board.steps.N, recursively, each with
//! its child boards stripped) writes <out>/<name>.scene.json, or
//! <out>/<name>.err with the build error. Options: Pad 0, the case theme
//! and config overrides, Appendix, a 4096-region/1 MiB link budget, and no
//! asset or font resolvers. Font assets are written once to the
//! content-addressed `assets` directory and referenced by DataFile.

use std::io::BufRead;
use std::path::{Path, PathBuf};

use d2_rs_scene::{Asset, Document};
use d2_rs_scenebuild::{LinkBudget, Options, build};
use d2_rs_target::Diagram;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

#[derive(serde::Deserialize)]
struct Request {
    board: PathBuf,
    #[serde(rename = "themeID")]
    theme_id: Option<i64>,
    out: PathBuf,
    #[serde(default)]
    assets: PathBuf,
}

fn boards(diagram: &Diagram) -> Vec<(String, Diagram)> {
    fn walk(d: &Diagram, name: String, out: &mut Vec<(String, Diagram)>) {
        if !d.is_folder_only {
            let mut flat = d.clone();
            flat.layers = None;
            flat.scenarios = None;
            flat.steps = None;
            out.push((name.clone(), flat));
        }
        for (kind, children) in [
            ("layers", &d.layers),
            ("scenarios", &d.scenarios),
            ("steps", &d.steps),
        ] {
            for (j, child) in children.iter().flatten().enumerate() {
                if let Some(child) = child {
                    walk(child, format!("{name}.{kind}.{j}"), out);
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(diagram, "board".into(), &mut out);
    out
}

/// scenejson.go encAsset for fonts: one content-addressed copy.
fn encode_document(document: &Document, store: &Path) -> std::io::Result<Value> {
    let mut value = serde_json::to_value(document).map_err(std::io::Error::other)?;
    let mut assets = serde_json::Map::new();
    for (id, asset) in &document.assets {
        let encoded = match asset {
            Asset::FontAsset {
                mime_type,
                data,
                face_index,
                ..
            } => {
                let name: String = Sha256::digest(data)
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect();
                let path = store.join(name);
                if !path.exists() {
                    std::fs::create_dir_all(store)?;
                    std::fs::write(&path, data)?;
                }
                json!({"FontAsset": {"MIMEType": mime_type, "DataFile": path.to_string_lossy(), "FaceIndex": face_index}})
            }
            other => serde_json::to_value(other).map_err(std::io::Error::other)?,
        };
        assets.insert(id.clone(), encoded);
    }
    value["Assets"] = Value::Object(assets);
    Ok(value)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let request: Request = serde_json::from_str(&line)?;
        let diagram: Diagram = serde_json::from_slice(&std::fs::read(&request.board)?)?;
        std::fs::create_dir_all(&request.out)?;
        let overrides = diagram
            .config
            .as_ref()
            .and_then(|c| c.theme_overrides.clone());
        let theme_id = request
            .theme_id
            .or_else(|| diagram.config.as_ref().and_then(|c| c.theme_id));
        for (name, board) in boards(&diagram) {
            let path = request.out.join(&name);
            let options = Options {
                pad: Some(0),
                theme_id,
                theme_overrides: overrides.clone(),
                appendix: true,
                link_budget: LinkBudget {
                    max_regions: 4096,
                    max_string_bytes: 1 << 20,
                },
                ..Default::default()
            };
            match build(&board, &options) {
                Ok(document) => {
                    let encoded = encode_document(&document, &request.assets)?;
                    std::fs::write(
                        path.with_extension_name("scene.json"),
                        serde_json::to_vec(&encoded)?,
                    )?;
                }
                Err(error) => std::fs::write(path.with_extension_name("err"), error.to_string())?,
            }
        }
    }
    Ok(())
}

trait WithExtensionName {
    fn with_extension_name(&self, suffix: &str) -> PathBuf;
}

impl WithExtensionName for PathBuf {
    /// `path + "." + suffix` (board names contain dots, so no set_extension).
    fn with_extension_name(&self, suffix: &str) -> PathBuf {
        let mut name = self.as_os_str().to_owned();
        name.push(".");
        name.push(suffix);
        PathBuf::from(name)
    }
}
