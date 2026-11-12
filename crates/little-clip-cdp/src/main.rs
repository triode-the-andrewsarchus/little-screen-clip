//! Isolated LGPL-2.1 Composers Desktop Project (CDP8) command-line runner.
//!
//! Reads a JSON [`CdpRunRequest`] from stdin (with `--run`) or emits the
//! embedded process catalog (with `--describe`), and invokes the compiled CDP8
//! executables from the sibling `bin/` directory or `LITTLE_CLIP_CDP_BIN`.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const PROCESSES_JSON: &str = include_str!("../cdp_processes.json");

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CdpProcessSpec {
    pub allowbypass: bool,
    pub category: String,
    pub subcategory: String,
    pub title: String,
    pub short_description: String,
    pub description: String,
    pub inputtype: String,
    pub outputtype: String,
    pub stereo: bool,
    pub outputisstereo: bool,
    #[serde(default)]
    pub parameters: BTreeMap<String, CdpParamSpec>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CdpParamSpec {
    pub paramname: String,
    #[serde(default)]
    pub paramdescription: String,
    #[serde(default)]
    pub flag: String,
    #[serde(default)]
    pub uitype: String,
    #[serde(default)]
    pub minrange: serde_json::Value,
    #[serde(default)]
    pub maxrange: serde_json::Value,
    #[serde(default)]
    pub step: serde_json::Value,
    #[serde(default)]
    pub value: serde_json::Value,
    #[serde(default)]
    pub exponential: bool,
    #[serde(default)]
    pub automatable: bool,
    #[serde(default)]
    pub time: bool,
    #[serde(default)]
    pub fftwindowcount: bool,
    #[serde(default)]
    pub fftwindowsize: bool,
}

#[derive(Debug, Deserialize)]
struct CdpRunRequest {
    process_id: String,
    #[serde(default)]
    inputs: Vec<String>,
    output: String,
    #[serde(default)]
    params: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    breakfiles: BTreeMap<String, Vec<[f64; 2]>>,
    #[serde(default)]
    fft_size: Option<u32>,
    #[serde(default)]
    fft_overlap: Option<u32>,
}

#[derive(Debug, Serialize)]
struct CdpRunResponse {
    ok: bool,
    output_path: Option<String>,
    stdout: String,
    stderr: String,
    error: Option<String>,
}

fn load_catalog() -> Result<BTreeMap<String, CdpProcessSpec>, String> {
    serde_json::from_str(PROCESSES_JSON).map_err(|e| format!("Invalid embedded CDP catalog: {e}"))
}

fn resolve_cdp_bin_dir() -> Result<PathBuf, String> {
    if let Ok(explicit) = std::env::var("LITTLE_CLIP_CDP_BIN") {
        let p = PathBuf::from(explicit);
        if p.is_dir() {
            return Ok(p);
        }
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(parent) = exe.parent()
    {
        for candidate in [parent.join("bin"), parent.join("cdp-helper").join("bin")] {
            if candidate.is_dir() {
                return Ok(candidate);
            }
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        for candidate in [
            cwd.join("target/release/cdp-helper/bin"),
            cwd.join("target/debug/cdp-helper/bin"),
            cwd.join("target/tools/cdp8/bin"),
        ] {
            if candidate.is_dir() {
                return Ok(candidate);
            }
        }
    }
    Err("Could not locate CDP8 binary directory; set LITTLE_CLIP_CDP_BIN or run scripts/build_cdp_helper.py".into())
}

fn parse_wav_info(path: &Path) -> Option<(u16, u32, f64)> {
    let bytes = fs::read(path).ok()?;
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let mut offset = 12usize;
    let mut channels = 0u16;
    let mut sample_rate = 0u32;
    let mut byte_rate = 0u32;
    let mut data_size = 0u32;
    while offset + 8 <= bytes.len() {
        let id = &bytes[offset..offset + 4];
        let size = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().ok()?);
        let payload_start = offset + 8;
        let payload_end = payload_start.saturating_add(size as usize);
        if payload_end > bytes.len() {
            break;
        }
        if id == b"fmt " && size >= 16 {
            channels = u16::from_le_bytes(
                bytes[payload_start + 2..payload_start + 4]
                    .try_into()
                    .ok()?,
            );
            sample_rate = u32::from_le_bytes(
                bytes[payload_start + 4..payload_start + 8]
                    .try_into()
                    .ok()?,
            );
            byte_rate = u32::from_le_bytes(
                bytes[payload_start + 8..payload_start + 12]
                    .try_into()
                    .ok()?,
            );
        } else if id == b"data" {
            data_size = size;
        }
        offset = payload_end + (size as usize & 1);
    }
    if channels == 0 || sample_rate == 0 || byte_rate == 0 {
        return None;
    }
    let duration = f64::from(data_size) / f64::from(byte_rate);
    Some((channels, sample_rate, duration))
}

fn split_process_command(process_id: &str) -> (&str, Vec<&str>) {
    let mut parts = process_id.splitn(3, '_');
    let exe = parts.next().unwrap_or(process_id);
    let mut prefix = Vec::new();
    if let Some(sub) = parts.next() {
        prefix.push(sub);
    }
    if let Some(mode) = parts.next() {
        prefix.push(mode);
    }
    (exe, prefix)
}

fn format_param_value(val: &serde_json::Value) -> Option<String> {
    match val {
        serde_json::Value::Null => None,
        serde_json::Value::Bool(b) => Some(if *b { "1".into() } else { "0".into() }),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::String(s) => Some(s.clone()),
        _ => None,
    }
}

fn run_cdp_binary(bin_dir: &Path, exe: &str, args: &[String]) -> Result<(String, String), String> {
    let path = bin_dir.join(exe);
    if !path.is_file() {
        return Err(format!("CDP8 executable not found: {}", path.display()));
    }
    let output = Command::new(&path)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("Failed to launch {}: {e}", path.display()))?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if !output.status.success() {
        return Err(format!(
            "{exe} failed ({}): {}{}",
            output.status,
            stderr.trim(),
            if stderr.trim().is_empty() {
                stdout.trim()
            } else {
                ""
            }
        ));
    }
    Ok((stdout, stderr))
}

fn build_param_args(
    spec: &CdpProcessSpec,
    req: &CdpRunRequest,
    temp_dir: &Path,
    duration_sec: Option<f64>,
) -> Result<Vec<String>, String> {
    let mut args = Vec::new();
    for (idx, (key, param)) in spec.parameters.iter().enumerate() {
        let supplied = req
            .params
            .get(key)
            .or_else(|| req.params.get(&param.paramname));
        if let Some(points) = req.breakfiles.get(key).filter(|p| !p.is_empty()) {
            let synth_dur = spec
                .parameters
                .iter()
                .find(|(_, p)| p.paramname.eq_ignore_ascii_case("Output Duration"))
                .and_then(|(k, p)| {
                    req.params
                        .get(k)
                        .and_then(|v| v.as_f64())
                        .or_else(|| p.value.as_f64())
                });
            let dur = duration_sec.or(synth_dur).unwrap_or(1.0);
            let all_unit_time = points.iter().all(|[t, _]| (0.0..=1.0).contains(t));
            let brk_path = temp_dir.join(format!("brk_{idx}.txt"));
            let mut body = String::new();
            for [t, v] in points {
                let t_sec = if all_unit_time && dur > 1.0 {
                    t * dur
                } else {
                    *t
                };
                body.push_str(&format!("{t_sec:.6} {v:.6}\n"));
            }
            fs::write(&brk_path, body).map_err(|e| format!("Failed writing breakfile: {e}"))?;
            let path_str = brk_path.to_string_lossy().into_owned();
            if param.flag.starts_with('-') {
                args.push(format!("{}{}", param.flag, path_str));
            } else {
                args.push(path_str);
            }
            continue;
        }

        let val = supplied.unwrap_or(&param.value);
        match param.uitype.as_str() {
            "checkbutton" | "toggle" => {
                let enabled = val.as_bool().unwrap_or_else(|| {
                    val.as_str()
                        .is_some_and(|s| matches!(s.trim(), "1" | "true" | "True"))
                });
                if enabled && !param.flag.is_empty() {
                    args.push(param.flag.clone());
                }
            }
            "optionbutton" => {
                let options: Vec<serde_json::Value> = param
                    .step
                    .as_str()
                    .and_then(|s| serde_json::from_str(s).ok())
                    .unwrap_or_default();
                let selected = if let Some(index) = val.as_u64() {
                    options
                        .get(index as usize)
                        .and_then(format_param_value)
                        .unwrap_or_else(|| index.to_string())
                } else if let Some(index_f) = val.as_f64() {
                    options
                        .get(index_f as usize)
                        .and_then(format_param_value)
                        .unwrap_or_else(|| (index_f as usize).to_string())
                } else {
                    format_param_value(val).unwrap_or_default()
                };
                if !selected.is_empty() {
                    if param.flag.starts_with('-') {
                        args.push(format!("{}{}", param.flag, selected));
                    } else {
                        args.push(selected);
                    }
                }
            }
            _ => {
                let mut raw_str = format_param_value(val).unwrap_or_default();
                if param.time
                    && let (Ok(pct), Some(dur)) = (raw_str.parse::<f64>(), duration_sec)
                {
                    raw_str = format!("{:.6}", dur * (pct / 100.0));
                }
                if !raw_str.is_empty() {
                    if param.flag.starts_with('-') {
                        args.push(format!("{}{}", param.flag, raw_str));
                    } else {
                        args.push(raw_str);
                    }
                }
            }
        }
    }
    Ok(args)
}

fn run_single(
    bin_dir: &Path,
    spec: &CdpProcessSpec,
    req: &CdpRunRequest,
    inputs: &[String],
    output: &str,
    temp_dir: &Path,
) -> Result<(String, String), String> {
    let duration = inputs
        .first()
        .and_then(|p| parse_wav_info(Path::new(p)))
        .map(|(_, _, d)| d);

    if req.process_id == "pvoc_anal" {
        let infile = inputs.first().ok_or("pvoc_anal requires an input file")?;
        let fft = req.fft_size.unwrap_or(1024);
        let ovl = req.fft_overlap.unwrap_or(3);
        let args = vec![
            "anal".into(),
            "1".into(),
            infile.clone(),
            output.to_owned(),
            format!("-c{fft}"),
            format!("-o{ovl}"),
        ];
        return run_cdp_binary(bin_dir, "pvoc", &args);
    }
    if req.process_id == "pvoc_synth" {
        let infile = inputs.first().ok_or("pvoc_synth requires an input file")?;
        let args = vec!["synth".into(), infile.clone(), output.to_owned()];
        return run_cdp_binary(bin_dir, "pvoc", &args);
    }

    let (exe, prefix) = split_process_command(&req.process_id);
    let mut args: Vec<String> = prefix.into_iter().map(str::to_owned).collect();
    args.extend(inputs.iter().cloned());
    args.push(output.to_owned());
    args.extend(build_param_args(spec, req, temp_dir, duration)?);
    run_cdp_binary(bin_dir, exe, &args)
}

fn execute_request(req: &CdpRunRequest) -> Result<(String, String), String> {
    let catalog = load_catalog()?;
    let spec = catalog
        .get(&req.process_id)
        .ok_or_else(|| format!("Unknown CDP process '{}'", req.process_id))?;
    let bin_dir = resolve_cdp_bin_dir()?;
    let temp = tempfile::Builder::new()
        .prefix("clip-cdp-")
        .tempdir()
        .map_err(|e| format!("Failed creating scratch dir: {e}"))?;

    if let Some(parent) = Path::new(&req.output).parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).map_err(|e| format!("Failed creating output dir: {e}"))?;
    }
    if Path::new(&req.output).exists() {
        let _ = fs::remove_file(&req.output);
    }

    // If the process is mono-only on a stereo WAV input, split via channelx,
    // process both channels independently, and re-interleave with submix.
    let first_is_stereo_wav = req
        .inputs
        .first()
        .filter(|p| p.to_ascii_lowercase().ends_with(".wav"))
        .and_then(|p| parse_wav_info(Path::new(p)))
        .is_some_and(|(ch, _, _)| ch == 2);

    if !spec.stereo && first_is_stereo_wav && req.inputs.len() == 1 {
        let split_base = temp.path().join("split.wav");
        run_cdp_binary(
            &bin_dir,
            "channelx",
            &[
                format!("-o{}", split_base.display()),
                req.inputs[0].clone(),
                "1".into(),
                "2".into(),
            ],
        )?;
        let c1 = temp.path().join("split_c1.wav");
        let c2 = temp.path().join("split_c2.wav");
        let ext = Path::new(&req.output)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("wav");
        let out1 = temp.path().join(format!("out_c1.{ext}"));
        let out2 = temp.path().join(format!("out_c2.{ext}"));
        let (out_a, err_a) = run_single(
            &bin_dir,
            spec,
            req,
            &[c1.to_string_lossy().into_owned()],
            &out1.to_string_lossy(),
            temp.path(),
        )?;
        let (out_b, err_b) = run_single(
            &bin_dir,
            spec,
            req,
            &[c2.to_string_lossy().into_owned()],
            &out2.to_string_lossy(),
            temp.path(),
        )?;
        let (out_i, err_i) = run_cdp_binary(
            &bin_dir,
            "submix",
            &[
                "interleave".into(),
                out1.to_string_lossy().into_owned(),
                out2.to_string_lossy().into_owned(),
                req.output.clone(),
            ],
        )?;
        return Ok((
            format!("{out_a}{out_b}{out_i}"),
            format!("{err_a}{err_b}{err_i}"),
        ));
    }

    run_single(&bin_dir, spec, req, &req.inputs, &req.output, temp.path())
}

fn main() {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("--describe") => {
            print!("{PROCESSES_JSON}");
        }
        Some("--run") => {
            let mut input = String::new();
            if let Err(e) = io::stdin().read_to_string(&mut input) {
                let resp = CdpRunResponse {
                    ok: false,
                    output_path: None,
                    stdout: String::new(),
                    stderr: String::new(),
                    error: Some(format!("Failed reading stdin: {e}")),
                };
                println!("{}", serde_json::to_string(&resp).unwrap());
                std::process::exit(1);
            }
            let req: CdpRunRequest = match serde_json::from_str(&input) {
                Ok(r) => r,
                Err(e) => {
                    let resp = CdpRunResponse {
                        ok: false,
                        output_path: None,
                        stdout: String::new(),
                        stderr: String::new(),
                        error: Some(format!("Invalid JSON request: {e}")),
                    };
                    println!("{}", serde_json::to_string(&resp).unwrap());
                    std::process::exit(1);
                }
            };
            match execute_request(&req) {
                Ok((stdout, stderr)) => {
                    let resp = CdpRunResponse {
                        ok: true,
                        output_path: Some(req.output),
                        stdout,
                        stderr,
                        error: None,
                    };
                    println!("{}", serde_json::to_string(&resp).unwrap());
                }
                Err(err) => {
                    let resp = CdpRunResponse {
                        ok: false,
                        output_path: None,
                        stdout: String::new(),
                        stderr: String::new(),
                        error: Some(err),
                    };
                    println!("{}", serde_json::to_string(&resp).unwrap());
                    std::process::exit(1);
                }
            }
        }
        _ => {
            eprintln!("Usage: little-clip-cdp [--describe | --run]");
            std::process::exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_loads_all_122_cdp_processes() {
        let cat = load_catalog().expect("catalog parses");
        assert_eq!(cat.len(), 122);
        assert!(cat.contains_key("distort_average"));
        assert!(cat.contains_key("blur_blur"));
        assert!(cat.contains_key("pvoc_synth"));
    }

    #[test]
    fn split_command_handles_subcommands_and_modes() {
        assert_eq!(
            split_process_command("distort_average"),
            ("distort", vec!["average"])
        );
        assert_eq!(
            split_process_command("modify_radical_4"),
            ("modify", vec!["radical", "4"])
        );
        assert_eq!(
            split_process_command("bounce_bounce"),
            ("bounce", vec!["bounce"])
        );
    }

    #[test]
    fn param_args_respect_flags_toggles_and_options() {
        let cat = load_catalog().unwrap();
        let spec = &cat["specfnu_specfnu_3"];
        let mut params = BTreeMap::new();
        params.insert("param3".into(), serde_json::json!(true));
        let req = CdpRunRequest {
            process_id: "specfnu_specfnu_3".into(),
            inputs: vec![],
            output: "out.ana".into(),
            params,
            breakfiles: BTreeMap::new(),
            fft_size: None,
            fft_overlap: None,
        };
        let tmp = tempfile::tempdir().unwrap();
        let args = build_param_args(spec, &req, tmp.path(), None).unwrap();
        assert!(args.contains(&"-x".to_string()));
    }
}
