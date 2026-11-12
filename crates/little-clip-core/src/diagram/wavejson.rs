//! Data-only WaveJSON parsing with budgets before recursive deserialization.
use json_five::tokenize::TokType;
use serde_json::Value;

const MAX_SOURCE: usize = 256 * 1024;

/// Recognizes a small, structurally distinctive WaveJSON clipboard or JSON file.
///
/// Large documents remain available through explicit `.wavejson`/`.wavedrom`
/// extensions. This hint does not run the rendering engine or resolve resources.
pub fn signature(source: &str) -> bool {
    if source.len() > 32 * 1024
        || !source
            .trim_start_matches('\u{feff}')
            .trim_start()
            .starts_with('{')
    {
        return false;
    }
    let Ok(value) = parse(source) else {
        return false;
    };
    signal_lanes(&value).any(|lane| lane.get("wave").is_some_and(Value::is_string))
        || value
            .get("reg")
            .and_then(Value::as_array)
            .is_some_and(|fields| fields.iter().any(|field| field.get("bits").is_some()))
        || value
            .get("assign")
            .and_then(Value::as_array)
            .is_some_and(|assign| assign.iter().any(Value::is_array))
}

/// Visits signal objects in the same declaration order as the reference engine.
///
/// Group labels are skipped and spacer objects remain lanes. Traversal is
/// iterative; callers can also use it on data not obtained through [`parse`].
pub fn signal_lanes(value: &Value) -> impl Iterator<Item = &Value> {
    let mut pending: Vec<_> = value.get("signal").into_iter().collect();
    std::iter::from_fn(move || {
        while let Some(value) = pending.pop() {
            match value {
                Value::Array(items) => pending.extend(items.iter().rev()),
                Value::Object(_) => return Some(value),
                _ => (),
            }
        }
        None
    })
}

/// Parses finite WaveJSON data without evaluating JavaScript expressions.
///
/// # Errors
/// Returns syntax, depth/token/source-budget or unsupported figure/skin errors.
pub fn parse(source: &str) -> Result<Value, String> {
    if source.len() > MAX_SOURCE || source.contains('\0') {
        return Err("WaveJSON exceeds the source budget or contains NUL".into());
    }
    let source = source.trim_start_matches('\u{feff}');
    let tokens = json_five::tokenize_str(source).map_err(|error| error.to_string())?;
    if tokens.tok_spans.len() > 32_768 {
        return Err("WaveJSON exceeds the token budget".into());
    }
    let mut depth = 0_usize;
    for (start, kind, end) in &tokens.tok_spans {
        match kind {
            TokType::LeftBrace | TokType::LeftBracket => {
                depth += 1;
                if depth > 64 {
                    return Err("WaveJSON exceeds the nesting budget".into());
                }
            }
            TokType::RightBrace | TokType::RightBracket => {
                depth = depth
                    .checked_sub(1)
                    .ok_or("Unexpected WaveJSON closing delimiter")?;
            }
            TokType::Nan | TokType::Infinity => {
                return Err("WaveJSON numbers must be finite".into());
            }
            TokType::Float | TokType::Exponent => {
                if !source[*start..*end]
                    .parse::<f64>()
                    .is_ok_and(f64::is_finite)
                {
                    return Err("WaveJSON numbers must be finite".into());
                }
            }
            _ => (),
        }
    }
    let value: Value = json5::from_str(source).map_err(|error| error.to_string())?;
    validate_figure(&value)?;
    Ok(value)
}

fn validate_figure(value: &Value) -> Result<(), String> {
    let families = ["signal", "reg", "assign"];
    let selected: Vec<_> = families
        .into_iter()
        .filter_map(|key| value.get(key).map(|value| (key, value)))
        .collect();
    let [(family, items)] = selected.as_slice() else {
        return Err("WaveJSON requires exactly one signal, reg or assign figure".into());
    };
    let items = items
        .as_array()
        .filter(|items| !items.is_empty())
        .ok_or("WaveJSON figure must be a nonempty array")?;
    if items.len() > 256 {
        return Err("WaveJSON exceeds the figure item budget".into());
    }
    if *family == "signal" {
        validate_signals(value)?;
    }
    if let Some(skin) = value.pointer("/config/skin") {
        match skin.as_str() {
            Some("default" | "narrow" | "dark" | "lowkey" | "narrower" | "narrowerer") => (),
            _ => return Err("Unknown WaveDrom skin; bundled skins are default, narrow, dark, lowkey, narrower and narrowerer".into()),
        }
    }
    Ok(())
}

fn validate_signals(value: &Value) -> Result<(), String> {
    let mut cells = 0;
    let mut lanes = signal_lanes(value).peekable();
    if lanes.peek().is_none() {
        return Err("WaveJSON requires at least one signal object".into());
    }
    for (index, item) in lanes.enumerate() {
        if index >= 256 {
            return Err("WaveJSON exceeds the signal-lane budget".into());
        }
        if let Some(wave) = item.get("wave") {
            let wave = wave.as_str().ok_or("A WaveJSON wave must be a string")?;
            cells += wave.chars().count();
            if cells > 8192 {
                return Err("WaveJSON exceeds the waveform-cell budget".into());
            }
        }
        for field in ["period", "phase"] {
            if let Some(value) = item.get(field) {
                let value = value
                    .as_f64()
                    .ok_or("WaveJSON period/phase must be numbers")?;
                if value.abs() > 4096.0 || (field == "period" && value <= 0.0) {
                    return Err("WaveJSON period/phase is outside the preview budget".into());
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wavejson_signatures_require_distinctive_structures_and_data_stays_data() {
        for source in [
            "{signal: [{name:'clock', wave:'p...'}]}",
            "{reg:[{bits:4,name:'op'}]}",
            "{assign:[['out',['&','a','b']]]}",
        ] {
            assert!(signature(source), "{source}");
            assert!(parse(source).is_ok());
            assert_eq!(
                super::super::Format::from_source_extension("json5", source),
                Some(super::super::Format::Wavedrom)
            );
            assert_eq!(
                super::super::Format::from_snippet(source),
                Some(super::super::Format::Wavedrom)
            );
        }
        for source in [
            "{signal:'ordinary data'}",
            "{reg:[{name:'ordinary'}]}",
            "{text:'signal: [{wave: p}]'}",
            "{signal: [{wave: (() => '01')()}]}",
        ] {
            assert!(!signature(source), "{source}");
        }
        let source = "{ // comment\n signal:[{name:'Unicode 日本語', wave:'01x', period:.5,phase:-.25,}], extra:0x20, }";
        let parsed = parse(source).unwrap();
        assert_eq!(parsed["signal"][0]["period"], 0.5);
        assert_eq!(parsed["extra"], 32);
    }

    #[test]
    fn finite_depth_skin_and_source_budgets_fail_before_layout() {
        for source in [
            "{}".to_owned(),
            "{signal:[]}".to_owned(),
            "{signal:[],reg:[]}".to_owned(),
            "{signal:[{wave:'01',period:0}]}".to_owned(),
            "{signal:[{wave:'01',phase:Infinity}]}".to_owned(),
            "{signal:[{wave:'01',period:1e999}]}".to_owned(),
            "{signal:[{wave:'01'}],config:{skin:'https://example.com/skin'}}".to_owned(),
            format!("{{signal:{}0{}}}", "[".repeat(65), "]".repeat(65)),
            format!("{{signal:[{{wave:'{}'}}]}}", "0".repeat(8193)),
        ] {
            assert!(parse(&source).is_err(), "{source}");
        }
        assert!(parse(&" ".repeat(MAX_SOURCE + 1)).is_err());
    }
}
