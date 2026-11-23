//! SigMF Core recording metadata.
//! https://sigmf.org/

use super::{
    Annotation, Capture, Encoding, Metadata, Tuning, check_cancel, io_error, validate_frequency,
    validate_rate,
};
use serde_json::Value;
use std::{fs::File, ops::Range, os::unix::fs::FileExt, path::Path, sync::atomic::AtomicBool};

const MAX_METADATA_BYTES: u64 = 4 << 20;
const MAX_SEGMENTS: usize = 16_384;
const MAX_LABEL_CHARS: usize = 2048;

pub(super) struct Descriptor {
    pub(super) dataset: Option<String>,
    metadata: Metadata,
    headers: Vec<(u64, u64)>,
    trailing_bytes: u64,
    annotations: Vec<AnnotationInput>,
}

pub(super) struct AnnotationInput {
    start: u64,
    count: Option<u64>,
    frequency: Option<Range<f64>>,
    label: Option<String>,
    comment: Option<String>,
}

impl Descriptor {
    pub(super) fn read(file: &File, cancel: &AtomicBool) -> Result<Self, String> {
        let metadata = file.metadata().map_err(io_error)?;
        if !metadata.is_file() || !(1..=MAX_METADATA_BYTES).contains(&metadata.len()) {
            return Err("SigMF needs a bounded regular metadata file.".into());
        }
        let mut data = vec![0; metadata.len() as usize];
        for (index, chunk) in data.chunks_mut(64 * 1024).enumerate() {
            check_cancel(cancel)?;
            file.read_exact_at(chunk, index as u64 * 64 * 1024)
                .map_err(io_error)?;
        }
        let signal = serde_json::from_slice::<Value>(&data)
            .map_err(|err| format!("Invalid SigMF metadata. {err}"))?;
        check_cancel(cancel)?;
        Self::parse(&signal)
    }

    fn parse(signal: &Value) -> Result<Self, String> {
        let global = signal
            .get("global")
            .filter(|value| value.is_object())
            .ok_or("SigMF metadata lacks its global object.")?;
        let format = Globals::parse(global)?;
        let captures = collection(signal, "captures")?;
        let (tunings, headers) =
            segments(captures, format.sample_offset, format.dataset.is_some())?;
        let annotations = collection(signal, "annotations")?
            .iter()
            .map(|value| annotation(value, format.sample_offset))
            .collect::<Result<Vec<_>, _>>()?;
        if !annotations.is_sorted_by_key(|annotation| annotation.start) {
            return Err("SigMF annotations are not sorted by sample index.".into());
        }
        Ok(Self {
            metadata: Metadata {
                encoding: format.encoding,
                sample_rate_hz: format.sample_rate_hz,
                sample_offset: format.sample_offset,
                hardware: optional_text(global, "core:hw")?,
                description: optional_text(global, "core:description")?,
                tunings,
                annotations: Vec::new(),
            },
            dataset: format.dataset,
            headers,
            trailing_bytes: format.trailing_bytes,
            annotations,
        })
    }

    pub(super) fn attach(self, data: File) -> Result<Capture, String> {
        Capture::from_parts(
            data,
            self.metadata,
            self.headers,
            self.trailing_bytes,
            self.annotations,
        )
    }
}

struct Globals {
    dataset: Option<String>,
    encoding: Encoding,
    sample_rate_hz: Option<f64>,
    sample_offset: u64,
    trailing_bytes: u64,
}

impl Globals {
    fn parse(global: &Value) -> Result<Self, String> {
        let datatype = global["core:datatype"]
            .as_str()
            .ok_or("SigMF capture has no sample datatype.")?;
        let encoding =
            Encoding::from_sigmf(datatype).ok_or("Unsupported SigMF complex sample datatype.")?;
        let sample_rate_hz = optional_number(global, "core:sample_rate")?;
        let sample_offset = optional_index(global, "core:offset")?.unwrap_or(0);
        let trailing_bytes = optional_index(global, "core:trailing_bytes")?.unwrap_or(0);
        validate_rate(sample_rate_hz)?;
        validate_version(global)?;
        let dataset = dataset_name(global)?;
        if dataset.is_none() && trailing_bytes != 0 {
            return Err("SigMF trailing bytes require a non-conforming dataset filename.".into());
        }
        if optional_index(global, "core:num_channels")?.unwrap_or(1) != 1 {
            return Err("Multiple interleaved SigMF channels are not supported.".into());
        }
        Ok(Self {
            dataset,
            encoding,
            sample_rate_hz,
            sample_offset,
            trailing_bytes,
        })
    }
}

fn validate_version(global: &Value) -> Result<(), String> {
    let version = global["core:version"]
        .as_str()
        .ok_or("SigMF capture has no version.")?;
    let mut numbers = version.split('.');
    let major = numbers.next();
    let valid = matches!(major, Some("0" | "1"))
        && numbers
            .by_ref()
            .take(2)
            .filter(|part| part.parse::<u32>().is_ok())
            .count()
            == 2
        && numbers.next().is_none();
    if !valid {
        return Err("Unsupported SigMF metadata version.".into());
    }
    Ok(())
}

fn dataset_name(global: &Value) -> Result<Option<String>, String> {
    if let Some(dataset) = global.get("core:dataset") {
        let name = dataset.as_str().ok_or("Invalid SigMF dataset filename.")?;
        let path = Path::new(name);
        if path.file_name().is_none_or(|base| base != path.as_os_str())
            || name.contains(['/', '\\', '\0'])
        {
            return Err("SigMF dataset must name a file in the metadata directory.".into());
        }
        return Ok(Some(name.to_owned()));
    }
    if global["core:metadata_only"].as_bool() == Some(true) {
        return Err("This SigMF metadata is distributed without captured samples.".into());
    }
    Ok(None)
}

type Headers = Vec<(u64, u64)>;

fn segments(
    captures: &[Value],
    first_sample: u64,
    nonconforming: bool,
) -> Result<(Vec<Tuning>, Headers), String> {
    let mut previous = None;
    let mut headers = Vec::new();
    let mut skipped = 0_u64;
    let mut tunings = Vec::new();
    for capture in captures {
        let tuning = tuning(capture, first_sample)?;
        if previous.is_some_and(|index| index >= tuning.start_sample) {
            return Err("SigMF capture segments must be sorted and distinct.".into());
        }
        previous = Some(tuning.start_sample);
        let count = optional_index(capture, "core:header_bytes")?.unwrap_or(0);
        if count != 0 {
            if !nonconforming {
                return Err(
                    "SigMF segment headers require a non-conforming dataset filename.".into(),
                );
            }
            skipped = skipped
                .checked_add(count)
                .ok_or("SigMF segment headers overflow.")?;
            headers.push((tuning.start_sample, skipped));
        }
        tunings.push(tuning);
    }
    Ok((tunings, headers))
}

fn tuning(capture: &Value, first_sample: u64) -> Result<Tuning, String> {
    let start_sample = relative_start(capture, first_sample)?;
    let center_hz = optional_number(capture, "core:frequency")?;
    validate_frequency(center_hz)?;
    Ok(Tuning {
        start_sample,
        center_hz,
        datetime: optional_text(capture, "core:datetime")?,
        original_sample: optional_index(capture, "core:global_index")?,
    })
}

fn annotation(value: &Value, first_sample: u64) -> Result<AnnotationInput, String> {
    let low = optional_number(value, "core:freq_lower_edge")?;
    let high = optional_number(value, "core:freq_upper_edge")?;
    let frequency = match (low, high) {
        (Some(low), Some(high)) if low.is_finite() && high.is_finite() && low <= high => {
            Some(low..high)
        }
        (None, None) => None,
        _ => return Err("SigMF annotation has invalid frequency bounds.".into()),
    };
    Ok(AnnotationInput {
        start: relative_start(value, first_sample)?,
        count: optional_index(value, "core:sample_count")?,
        frequency,
        label: optional_text(value, "core:label")?,
        comment: optional_text(value, "core:comment")?,
    })
}

pub(super) fn annotations(
    input: Vec<AnnotationInput>,
    tunings: &[Tuning],
    count: u64,
) -> Result<Vec<Annotation>, String> {
    input
        .into_iter()
        .map(|annotation| {
            let start = annotation.start;
            let after = tunings.partition_point(|capture| capture.start_sample <= start);
            let end = match annotation.count {
                Some(length) => start
                    .checked_add(length)
                    .ok_or("SigMF annotation length overflows.")?,
                None => tunings
                    .get(after)
                    .map_or(count, |capture| capture.start_sample),
            };
            if start >= count || end > count {
                return Err("SigMF annotation falls outside the dataset.".into());
            }
            Ok(Annotation {
                samples: start..end,
                frequency_hz: annotation.frequency,
                label: annotation.label,
                comment: annotation.comment,
            })
        })
        .collect()
}

fn relative_start(value: &Value, first_sample: u64) -> Result<u64, String> {
    optional_index(value, "core:sample_start")?
        .and_then(|index| index.checked_sub(first_sample))
        .ok_or_else(|| "SigMF sample start is missing or predates the dataset.".into())
}

fn collection<'a>(signal: &'a Value, key: &str) -> Result<&'a [Value], String> {
    signal
        .get(key)
        .and_then(Value::as_array)
        .filter(|items| items.len() <= MAX_SEGMENTS)
        .map(Vec::as_slice)
        .ok_or_else(|| format!("SigMF {key} must be a bounded array."))
}

fn optional_index(value: &Value, key: &str) -> Result<Option<u64>, String> {
    value
        .get(key)
        .map(|value| {
            value
                .as_u64()
                .filter(|index| *index <= i64::MAX as u64)
                .ok_or_else(|| format!("Invalid SigMF integer {key}."))
        })
        .transpose()
}

fn optional_number(value: &Value, key: &str) -> Result<Option<f64>, String> {
    value
        .get(key)
        .map(|value| {
            value
                .as_f64()
                .ok_or_else(|| format!("Invalid SigMF number {key}."))
        })
        .transpose()
}

fn optional_text(value: &Value, key: &str) -> Result<Option<String>, String> {
    value
        .get(key)
        .map(|value| {
            value
                .as_str()
                .map(|value| value.chars().take(MAX_LABEL_CHARS).collect())
                .ok_or_else(|| format!("Invalid SigMF text {key}."))
        })
        .transpose()
}
