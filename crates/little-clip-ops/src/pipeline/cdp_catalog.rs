//! Catalog and parameter metadata for CDP8 / SoundThread DSP nodes.

use super::catalog::NodeTemplate;
use super::types::{NodeKind, PortType};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::OnceLock;

/// Parameter UI and CLI specification for a CDP8 process parameter.
#[derive(Debug, Clone)]
pub struct CdpParamMeta {
    pub key: String,
    pub flag: String,
    pub paramname: String,
    pub paramdescription: String,
    pub uitype: String,
    pub minrange: Option<f64>,
    pub maxrange: Option<f64>,
    pub step: serde_json::Value,
    pub default_value: serde_json::Value,
    pub exponential: bool,
    pub automatable: bool,
}

impl CdpParamMeta {
    /// Returns the default value serialized as a node `props` string.
    pub fn default_prop_string(&self) -> String {
        match &self.default_value {
            serde_json::Value::Null => String::new(),
            serde_json::Value::Bool(b) => {
                if *b {
                    "true".to_string()
                } else {
                    "false".to_string()
                }
            }
            serde_json::Value::Number(n) => n.to_string(),
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        }
    }

    /// Parses option strings for `optionbutton` parameters.
    pub fn options(&self) -> Vec<String> {
        let Some(raw) = self.step.as_str() else {
            return Vec::new();
        };
        let Ok(arr) = serde_json::from_str::<Vec<serde_json::Value>>(raw) else {
            return Vec::new();
        };
        arr.into_iter()
            .map(|v| match v {
                serde_json::Value::String(s) => s,
                other => other.to_string(),
            })
            .collect()
    }
}

/// Complete metadata for a single CDP8 process imported from SoundThread's catalog.
#[derive(Debug, Clone)]
pub struct CdpProcessMeta {
    pub id: String,
    pub title: String,
    pub shortdescription: String,
    pub category: String,
    pub subcategory: String,
    pub menu_path: &'static str,
    pub command: String,
    pub inputcount: u32,
    pub outputextension: String,
    pub allowbypass: bool,
    pub parameters: Vec<CdpParamMeta>,
}

#[derive(Debug, Deserialize)]
struct RawParam {
    #[serde(default)]
    flag: String,
    #[serde(default)]
    paramname: String,
    #[serde(default)]
    paramdescription: String,
    #[serde(default)]
    uitype: String,
    #[serde(default)]
    minrange: serde_json::Value,
    #[serde(default)]
    maxrange: serde_json::Value,
    #[serde(default)]
    step: serde_json::Value,
    #[serde(default)]
    value: serde_json::Value,
    #[serde(default)]
    exponential: bool,
    #[serde(default)]
    automatable: bool,
}

#[derive(Debug, Deserialize)]
struct RawProcess {
    title: String,
    #[serde(default)]
    shortdescription: String,
    category: String,
    #[serde(default)]
    subcategory: String,
    #[serde(default)]
    command: String,
    #[serde(default)]
    inputtype: String,
    #[serde(default)]
    inputcount: Option<u32>,
    #[serde(default)]
    outputextension: String,
    #[serde(default)]
    allowbypass: bool,
    #[serde(default)]
    parameters: BTreeMap<String, RawParam>,
}

struct CdpRegistry {
    metas: BTreeMap<String, CdpProcessMeta>,
    templates: Vec<NodeTemplate>,
}

static REGISTRY: OnceLock<CdpRegistry> = OnceLock::new();

fn leak_str(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

fn leak_slice<T: 'static>(v: Vec<T>) -> &'static [T] {
    Box::leak(v.into_boxed_slice())
}

fn val_to_opt_f64(v: &serde_json::Value) -> Option<f64> {
    match v {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
}

fn cdp_menu_path_for(category: &str, subcategory: &str) -> &'static str {
    match (category, subcategory) {
        ("time", "filter") => "Audio/Filtering",
        ("time", "distort") => "Audio/Distort",
        ("time", "extend") => "Audio/Extend",
        ("time", "granulate") => "Audio/Granulate",
        ("time", "reverb") => "Audio/Reverb",
        ("time", "synthesis") => "Audio/Synthesis",
        ("time", "misc") => "Audio/Utilities",
        ("pvoc", "combine") => "Audio/Spectral/Combine",
        ("pvoc", "spectrum") => "Audio/Spectral/Spectrum",
        ("pvoc", "pitch") => "Audio/Spectral/Pitch",
        ("pvoc", "formants") => "Audio/Spectral/Formants",
        ("pvoc", "time") => "Audio/Spectral/Time",
        ("pvoc", "convert") => "Audio/Spectral/Convert",
        ("pvoc", "amplitude") => "Audio/Spectral/Amplitude",
        ("pvoc", _) => "Audio/Spectral",
        _ => "Audio/Effects",
    }
}

fn registry() -> &'static CdpRegistry {
    REGISTRY.get_or_init(|| {
        let raw: BTreeMap<String, RawProcess> =
            serde_json::from_str(include_str!("cdp_processes.json"))
                .expect("embedded cdp_processes.json must be valid");
        let mut metas = BTreeMap::new();
        let mut templates = Vec::with_capacity(raw.len());

        for (id, proc_raw) in raw {
            let parameters: Vec<CdpParamMeta> = proc_raw
                .parameters
                .into_iter()
                .map(|(key, p)| CdpParamMeta {
                    key,
                    flag: p.flag,
                    paramname: p.paramname,
                    paramdescription: p.paramdescription,
                    uitype: p.uitype,
                    minrange: val_to_opt_f64(&p.minrange),
                    maxrange: val_to_opt_f64(&p.maxrange),
                    step: p.step,
                    default_value: p.value,
                    exponential: p.exponential,
                    automatable: p.automatable,
                })
                .collect();

            let is_spectral_in =
                id == "pvoc_synth" || (proc_raw.category == "pvoc" && id != "pvoc_anal_1");
            let is_spectral_out =
                id == "pvoc_anal_1" || (proc_raw.category == "pvoc" && id != "pvoc_synth");
            let in_port = if is_spectral_in {
                PortType::Spectral
            } else {
                PortType::Audio
            };
            let out_port = if is_spectral_out {
                PortType::Spectral
            } else {
                PortType::Audio
            };

            let inputcount = if proc_raw.inputtype.trim() == "[]" {
                0
            } else {
                proc_raw.inputcount.unwrap_or(1)
            };
            let mut input_vec = if inputcount == 0 {
                Vec::new()
            } else if inputcount >= 2 {
                vec![("In 1", in_port), ("In 2", in_port)]
            } else {
                vec![("In", in_port)]
            };
            let mod_default = parameters
                .iter()
                .find(|p| p.uitype == "hslider" && p.automatable)
                .or_else(|| parameters.iter().find(|p| p.uitype == "hslider"))
                .map(|p| p.key.clone());
            if mod_default.is_some() {
                input_vec.push(("Mod", PortType::Automation));
            }
            let inputs: &'static [(&'static str, PortType)] = leak_slice(input_vec);
            let outputs: &'static [(&'static str, PortType)] = leak_slice(vec![("Out", out_port)]);

            let mut default_props = vec![("process_id", leak_str(id.clone()))];
            if let Some(target_key) = mod_default {
                default_props.push(("mod_target", leak_str(target_key)));
            }
            if proc_raw.allowbypass {
                default_props.push(("bypass", "false"));
            }
            for param in &parameters {
                default_props.push((
                    leak_str(param.key.clone()),
                    leak_str(param.default_prop_string()),
                ));
            }

            let mut tags = vec!["cdp", leak_str(proc_raw.category.clone())];
            if !proc_raw.subcategory.is_empty() {
                tags.push(leak_str(proc_raw.subcategory.clone()));
            }
            tags.push(leak_str(id.clone()));

            let menu_path = cdp_menu_path_for(&proc_raw.category, &proc_raw.subcategory);
            templates.push(NodeTemplate {
                id: leak_str(format!("cdp_{id}")),
                name: leak_str(proc_raw.title.clone()),
                category: "Audio",
                description: leak_str(proc_raw.shortdescription.clone()),
                tags: leak_slice(tags),
                kind: NodeKind::CdpProcess,
                inputs,
                outputs,
                default_props: leak_slice(default_props),
            });

            metas.insert(
                id.clone(),
                CdpProcessMeta {
                    id,
                    title: proc_raw.title,
                    shortdescription: proc_raw.shortdescription,
                    category: proc_raw.category,
                    subcategory: proc_raw.subcategory,
                    menu_path,
                    command: proc_raw.command,
                    inputcount,
                    outputextension: proc_raw.outputextension,
                    allowbypass: proc_raw.allowbypass,
                    parameters,
                },
            );
        }

        CdpRegistry { metas, templates }
    })
}

/// Returns all 122 CDP8 / SoundThread node templates.
pub fn all_cdp_templates() -> &'static [NodeTemplate] {
    &registry().templates
}

/// Finds a CDP8 node template by either `"cdp_<process_id>"` or `"<process_id>"`.
pub fn find_cdp_template(id_or_process: &str) -> Option<&'static NodeTemplate> {
    let normalized = id_or_process.strip_prefix("cdp_").unwrap_or(id_or_process);
    registry()
        .templates
        .iter()
        .find(|t| t.id.strip_prefix("cdp_") == Some(normalized))
}

/// Finds full CDP8 parameter metadata by `"<process_id>"` or `"cdp_<process_id>"`.
pub fn find_cdp_meta(process_id: &str) -> Option<&'static CdpProcessMeta> {
    let normalized = process_id.strip_prefix("cdp_").unwrap_or(process_id);
    registry().metas.get(normalized)
}

/// Returns the hierarchical menu path (e.g. `"Audio/Filtering"`) for a CDP node template ID.
pub fn cdp_menu_path(id_or_process: &str) -> &'static str {
    find_cdp_meta(id_or_process)
        .map(|m| m.menu_path)
        .unwrap_or("Audio/Effects")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_all_122_cdp_templates_with_expected_ports() {
        let templates = all_cdp_templates();
        assert_eq!(templates.len(), 122);
        assert!(templates.iter().all(|t| t.category == "Audio"));

        let pvoc_anal = find_cdp_template("pvoc_anal_1").expect("pvoc_anal_1 exists");
        assert_eq!(pvoc_anal.inputs, &[("In", PortType::Audio)]);
        assert_eq!(pvoc_anal.outputs, &[("Out", PortType::Spectral)]);
        assert_eq!(
            pvoc_anal.first_compatible_input(PortType::Audio),
            Some("In")
        );
        assert_eq!(pvoc_anal.first_compatible_input(PortType::Text), None);

        let blur = find_cdp_template("cdp_blur_blur").expect("blur_blur exists");
        assert_eq!(
            blur.inputs,
            &[("In", PortType::Spectral), ("Mod", PortType::Automation)]
        );
        assert_eq!(blur.outputs, &[("Out", PortType::Spectral)]);
        assert_eq!(blur.menu_path(), "Audio/Spectral/Time");
        assert_eq!(blur.first_compatible_input(PortType::Audio), None);
        assert_eq!(blur.first_compatible_input(PortType::Spectral), Some("In"));

        let noise = find_cdp_template("cdp_synth_noise").expect("synth_noise exists");
        assert_eq!(noise.inputs, &[("Mod", PortType::Automation)]);
        assert_eq!(noise.first_compatible_input(PortType::Audio), None);
        assert_eq!(
            noise.first_compatible_input(PortType::Automation),
            Some("Mod")
        );

        let pvoc_synth = find_cdp_template("pvoc_synth").expect("pvoc_synth exists");
        assert_eq!(pvoc_synth.inputs, &[("In", PortType::Spectral)]);
        assert_eq!(pvoc_synth.outputs, &[("Out", PortType::Audio)]);
        assert_eq!(
            pvoc_synth.first_compatible_output(PortType::Audio),
            Some("Out")
        );
    }
}
