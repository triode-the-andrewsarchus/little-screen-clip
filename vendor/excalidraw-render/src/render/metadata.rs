mod selection;
#[cfg(test)]
pub(crate) mod test_faults;

use std::fmt::Write as _;

use sha2::{Digest, Sha256};

use crate::diagnostic_registry::MetadataFailure;
use crate::{
    CompatibilityReport, DiagnosticSeverity, ReferenceProfileIdentity, RenderRequest, SceneDocument,
};

use super::FormatEncodeError;

/// The real fallible metadata operation, including the carrier-specific sites.
/// Error classification lives here; test builds can refuse an operation at the
/// same seam after its work, without constructing a diagnostic themselves.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Stage {
    ProvenanceSerialization,
    DiagnosticSummarySerialization,
    SceneSerialization,
    SceneCompression,
    SceneWrapper,
    ManifestSerialization,
    SceneEmbedding,
    ManifestEmbedding,
}
impl Stage {
    fn failure(self) -> MetadataFailure {
        match self {
            Self::ProvenanceSerialization => MetadataFailure::ProvenanceSerialization,
            Self::DiagnosticSummarySerialization => MetadataFailure::DiagnosticSummarySerialization,
            Self::SceneSerialization => MetadataFailure::SceneSerialization,
            Self::SceneCompression => MetadataFailure::SceneCompression,
            Self::SceneWrapper | Self::SceneEmbedding => MetadataFailure::SceneEmbedding,
            Self::ManifestSerialization => MetadataFailure::ManifestSerialization,
            Self::ManifestEmbedding => MetadataFailure::ManifestEmbedding,
        }
    }
}
pub(crate) fn operation<T>(
    stage: Stage,
    action: impl FnOnce() -> Result<T, FormatEncodeError>,
) -> Result<T, FormatEncodeError> {
    let result = action();
    #[cfg(test)]
    let result = result.and_then(|value| test_faults::after(stage).map(|()| value));
    result.map_err(|error| metadata_failure(error, stage.failure()))
}

#[derive(Clone, Copy)]
pub(super) enum ManifestTarget<'a> {
    Scene,
    Frame {
        ordinal: u32,
        kind: crate::FrameKind,
        name: Option<&'a str>,
    },
}

const SCENE_MEDIA_TYPE: &str = "application/vnd.excalidraw+json";
const WRAPPER_PREFIX: &[u8] =
    b"{\"version\":\"1\",\"encoding\":\"bstring\",\"compressed\":true,\"encoded\":\"";

#[derive(Debug)]
pub(super) struct ArtifactMetadata {
    manifest: Option<String>,
    scene: Option<EmbeddedScene>,
}

#[derive(Debug)]
struct EmbeddedScene {
    canonical: Vec<u8>,
    wrapper: Vec<u8>,
    compressed_len: usize,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct MetadataEstimate {
    pub(super) scratch_bytes: u64,
    pub(super) generation_work: u64,
}

impl ArtifactMetadata {
    pub(super) const fn empty() -> Self {
        Self {
            manifest: None,
            scene: None,
        }
    }

    pub(super) fn try_clone(&self) -> Result<Self, FormatEncodeError> {
        let manifest = self
            .manifest
            .as_deref()
            .map(|manifest| try_string(manifest, "artifact_manifest_clone"))
            .transpose()?;
        let scene = self
            .scene
            .as_ref()
            .map(|scene| -> Result<_, FormatEncodeError> {
                Ok(EmbeddedScene {
                    canonical: try_bytes(&scene.canonical, "canonical_scene_clone")?,
                    wrapper: try_bytes(&scene.wrapper, "scene_wrapper_clone")?,
                    compressed_len: scene.compressed_len,
                })
            })
            .transpose()?;
        Ok(Self { manifest, scene })
    }

    pub(super) fn build(
        scene_document: &SceneDocument,
        profile: &ReferenceProfileIdentity,
        report: &CompatibilityReport,
        mandatory_notices: Option<&super::notices::MandatoryNotices>,
        selection: MetadataSelection,
        target: ManifestTarget<'_>,
    ) -> Result<Self, FormatEncodeError> {
        let canonical = selection
            .embedded_scene
            .then(|| {
                operation(Stage::SceneSerialization, || {
                    scene_document
                        .canonicalized_original_bytes()
                        .map_err(|_| memory("canonical_original_scene"))
                })
            })
            .transpose()?;
        let scene_digest = canonical.as_deref().map(sha256_string).transpose()?;
        let scene = canonical
            .map(|canonical| -> Result<_, FormatEncodeError> {
                let compressed = operation(Stage::SceneCompression, || stored_zlib(&canonical))?;
                let wrapper = operation(Stage::SceneWrapper, || scene_wrapper(&compressed))?;
                Ok(EmbeddedScene {
                    canonical,
                    wrapper,
                    compressed_len: compressed.len(),
                })
            })
            .transpose()?;
        let notices = mandatory_notices.map_or("[]", super::notices::MandatoryNotices::as_json);
        let has_manifest = selection.provenance
            || selection.diagnostic_summary
            || selection.embedded_scene
            || mandatory_notices.is_some();
        let manifest = has_manifest
            .then(|| {
                operation(Stage::ManifestSerialization, || {
                    manifest_json(
                        profile,
                        report,
                        notices,
                        scene_digest.as_deref(),
                        selection,
                        target,
                    )
                })
            })
            .transpose()?;
        let metadata = Self { manifest, scene };
        #[cfg(debug_assertions)]
        debug_assert_eq!(
            Self::estimate(
                scene_document,
                profile,
                report,
                mandatory_notices,
                selection,
                target,
            ),
            MetadataEstimate {
                scratch_bytes: metadata.scratch_bytes(),
                generation_work: metadata.generation_work(),
            }
        );
        Ok(metadata)
    }

    pub(super) fn estimate(
        scene: &SceneDocument,
        profile: &ReferenceProfileIdentity,
        report: &CompatibilityReport,
        mandatory_notices: Option<&super::notices::MandatoryNotices>,
        selection: MetadataSelection,
        target: ManifestTarget<'_>,
    ) -> MetadataEstimate {
        let scene = selection
            .embedded_scene
            .then(|| embedded_scene_estimate(scene));
        let notices = mandatory_notices.map_or("[]", super::notices::MandatoryNotices::as_json);
        let has_manifest = selection.provenance
            || selection.diagnostic_summary
            || selection.embedded_scene
            || mandatory_notices.is_some();
        let manifest_len = has_manifest.then(|| {
            manifest_json_len(
                profile,
                report,
                notices,
                selection.embedded_scene,
                selection,
                target,
            )
        });
        let scene_scratch = scene.map_or(0, |scene| {
            aligned(scene.canonical_len).saturating_add(aligned(scene.compressed_len))
        });
        let scratch_bytes = 512_u64
            .saturating_add(scene_scratch)
            .saturating_add(manifest_len.map_or(0, aligned));
        let generation_work = scene
            .map_or(0, |scene| {
                scene
                    .canonical_len
                    .saturating_add(scene.compressed_len)
                    .saturating_add(scene.wrapper_len)
            })
            .saturating_add(manifest_len.unwrap_or(0));
        MetadataEstimate {
            scratch_bytes,
            generation_work,
        }
    }

    pub(super) fn manifest(&self) -> Option<&str> {
        self.manifest.as_deref()
    }

    #[cfg(any(feature = "pdf", test))]
    pub(super) fn canonical_scene(&self) -> Option<&[u8]> {
        self.scene.as_ref().map(|scene| scene.canonical.as_slice())
    }

    pub(super) fn scene_wrapper(&self) -> Option<&[u8]> {
        self.scene.as_ref().map(|scene| scene.wrapper.as_slice())
    }

    /// The estimate `estimate` must reproduce; only the debug assertion in
    /// `build` and the estimate tests compare against it.
    #[cfg(any(debug_assertions, test))]
    pub(super) fn scratch_bytes(&self) -> u64 {
        let scene = self.scene.as_ref().map_or(0_u64, |scene| {
            aligned(u64::try_from(scene.canonical.len()).unwrap_or(u64::MAX)).saturating_add(
                aligned(u64::try_from(scene.compressed_len).unwrap_or(u64::MAX)),
            )
        });
        512_u64.saturating_add(scene).saturating_add(
            self.manifest.as_ref().map_or(0_u64, |manifest| {
                aligned(u64::try_from(manifest.len()).unwrap_or(u64::MAX))
            }),
        )
    }

    #[cfg(any(debug_assertions, test))]
    pub(super) fn generation_work(&self) -> u64 {
        self.scene
            .as_ref()
            .map_or(0_u64, |scene| {
                u64::try_from(scene.canonical.len())
                    .unwrap_or(u64::MAX)
                    .saturating_add(u64::try_from(scene.compressed_len).unwrap_or(u64::MAX))
                    .saturating_add(u64::try_from(scene.wrapper.len()).unwrap_or(u64::MAX))
            })
            .saturating_add(self.manifest.as_ref().map_or(0_u64, |manifest| {
                u64::try_from(manifest.len()).unwrap_or(u64::MAX)
            }))
    }
}

#[derive(Clone, Copy)]
struct EmbeddedSceneEstimate {
    canonical_len: u64,
    compressed_len: u64,
    wrapper_len: u64,
}

fn embedded_scene_estimate(scene: &SceneDocument) -> EmbeddedSceneEstimate {
    let mut canonical_len = 0_u64;
    scene.visit_canonicalized_original(&mut |bytes| {
        canonical_len =
            canonical_len.saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
    });
    let block_count = canonical_len.max(1).div_ceil(u64::from(u16::MAX));
    let compressed_len = 2_u64
        .saturating_add(block_count.saturating_mul(5))
        .saturating_add(canonical_len)
        .saturating_add(4);
    let mut escaped_compressed_len = escaped_byte_len(0x78).saturating_add(escaped_byte_len(0x01));
    let mut offset = 0_u64;
    let mut adler_a = 1_u32;
    let mut adler_b = 0_u32;
    if canonical_len == 0 {
        for byte in [1, 0, 0, 0xff, 0xff] {
            escaped_compressed_len = escaped_compressed_len.saturating_add(escaped_byte_len(byte));
        }
    }
    scene.visit_canonicalized_original(&mut |bytes| {
        for byte in bytes {
            if offset.is_multiple_of(u64::from(u16::MAX)) {
                let block_len = (canonical_len - offset).min(u64::from(u16::MAX)) as u16;
                let final_block = offset + u64::from(block_len) == canonical_len;
                for header in [
                    u8::from(final_block),
                    block_len.to_le_bytes()[0],
                    block_len.to_le_bytes()[1],
                    (!block_len).to_le_bytes()[0],
                    (!block_len).to_le_bytes()[1],
                ] {
                    escaped_compressed_len =
                        escaped_compressed_len.saturating_add(escaped_byte_len(header));
                }
            }
            escaped_compressed_len = escaped_compressed_len.saturating_add(escaped_byte_len(*byte));
            adler_a = (adler_a + u32::from(*byte)) % 65_521;
            adler_b = (adler_b + adler_a) % 65_521;
            offset = offset.saturating_add(1);
        }
    });
    for byte in ((adler_b << 16) | adler_a).to_be_bytes() {
        escaped_compressed_len = escaped_compressed_len.saturating_add(escaped_byte_len(byte));
    }
    EmbeddedSceneEstimate {
        canonical_len,
        compressed_len,
        wrapper_len: u64::try_from(WRAPPER_PREFIX.len())
            .unwrap_or(u64::MAX)
            .saturating_add(escaped_compressed_len)
            .saturating_add(2),
    }
}

fn escaped_byte_len(byte: u8) -> u64 {
    match byte {
        b'"' | b'\\' | 0x08 | 0x09 | 0x0a | 0x0c | 0x0d => 2,
        0x00..=0x1f => 6,
        _ => 1,
    }
}

#[derive(Clone, Copy, Default)]
pub(super) struct MetadataSelection {
    pub(super) provenance: bool,
    pub(super) diagnostic_summary: bool,
    pub(super) embedded_scene: bool,
}

impl MetadataSelection {
    pub(super) fn requested(request: &RenderRequest) -> Self {
        Self {
            provenance: request.metadata_options().provenance(),
            diagnostic_summary: request.metadata_options().diagnostic_summary(),
            embedded_scene: request.metadata_options().embedded_scene(),
        }
    }
}

fn manifest_json_len(
    profile: &ReferenceProfileIdentity,
    report: &CompatibilityReport,
    notices: &str,
    has_scene_digest: bool,
    selection: MetadataSelection,
    target: ManifestTarget<'_>,
) -> u64 {
    let mut len = match target {
        ManifestTarget::Scene => byte_len("{\"schema_version\":1,\"provenance\":"),
        ManifestTarget::Frame {
            ordinal,
            kind,
            name,
        } => {
            let mut len = byte_len("{\"schema_version\":2,\"target\":{\"kind\":\"")
                .saturating_add(byte_len(kind.as_str()))
                .saturating_add(byte_len("\",\"ordinal\":"))
                .saturating_add(decimal_len(u64::from(ordinal)));
            if let Some(name) = name {
                len = len
                    .saturating_add(byte_len(",\"name\":\""))
                    .saturating_add(json_string_content_len(name))
                    .saturating_add(1);
            }
            len.saturating_add(byte_len("},\"provenance\":"))
        }
    };
    if selection.provenance {
        len = len
            .saturating_add(byte_len("{\"renderer_version\":\""))
            .saturating_add(byte_len(env!("CARGO_PKG_VERSION")))
            .saturating_add(byte_len("\",\"compatibility_revision\":"))
            .saturating_add(decimal_len(profile.compatibility_revision().get()))
            .saturating_add(byte_len(",\"excalidraw_release\":\""))
            .saturating_add(byte_len(profile.excalidraw_release()))
            .saturating_add(byte_len("\",\"reference_profile_digest\":\""))
            .saturating_add(byte_len(profile.reference_profile_digest()))
            .saturating_add(2);
    } else {
        len = len.saturating_add(4);
    }
    len = len.saturating_add(byte_len(",\"diagnostic_summary\":"));
    if selection.diagnostic_summary {
        len = len.saturating_add(diagnostic_summary_len(report));
    } else {
        len = len.saturating_add(4);
    }
    len = len
        .saturating_add(byte_len(",\"notices\":"))
        .saturating_add(byte_len(notices))
        .saturating_add(byte_len(",\"embedded_scene\":"));
    if has_scene_digest {
        len = len
            .saturating_add(byte_len("{\"name\":\"scene.excalidraw\",\"media_type\":\""))
            .saturating_add(byte_len(SCENE_MEDIA_TYPE))
            .saturating_add(byte_len("\",\"sha256\":\""))
            .saturating_add(71)
            .saturating_add(2);
    } else {
        len = len.saturating_add(4);
    }
    len.saturating_add(1)
}

fn diagnostic_summary_len(report: &CompatibilityReport) -> u64 {
    let info = report
        .diagnostics()
        .iter()
        .filter(|diagnostic| diagnostic.severity() == DiagnosticSeverity::Info)
        .count();
    let warning = report.len().saturating_sub(info);
    let mut len = byte_len("{\"snapshot\":\"pre_metadata\",\"total\":")
        .saturating_add(decimal_len(u64::try_from(report.len()).unwrap_or(u64::MAX)))
        .saturating_add(byte_len(",\"by_severity\":{\"info\":"))
        .saturating_add(decimal_len(u64::try_from(info).unwrap_or(u64::MAX)))
        .saturating_add(byte_len(",\"warning\":"))
        .saturating_add(decimal_len(u64::try_from(warning).unwrap_or(u64::MAX)))
        .saturating_add(byte_len("},\"by_code\":["));
    let mut unique = 0_u64;
    for (index, diagnostic) in report.diagnostics().iter().enumerate() {
        let code = diagnostic.code().as_str();
        if report.diagnostics()[..index]
            .iter()
            .any(|prior| prior.code().as_str() == code)
        {
            continue;
        }
        let count = report
            .diagnostics()
            .iter()
            .filter(|candidate| candidate.code().as_str() == code)
            .count();
        len = len
            .saturating_add(u64::from(unique != 0))
            .saturating_add(byte_len("{\"code\":\""))
            .saturating_add(byte_len(code))
            .saturating_add(byte_len("\",\"count\":"))
            .saturating_add(decimal_len(u64::try_from(count).unwrap_or(u64::MAX)))
            .saturating_add(1);
        unique = unique.saturating_add(1);
    }
    len.saturating_add(2)
}

fn byte_len(value: &str) -> u64 {
    u64::try_from(value.len()).unwrap_or(u64::MAX)
}

fn decimal_len(value: u64) -> u64 {
    if value == 0 {
        1
    } else {
        u64::from(value.ilog10()) + 1
    }
}

fn manifest_json(
    profile: &ReferenceProfileIdentity,
    report: &CompatibilityReport,
    notices: &str,
    scene_digest: Option<&str>,
    selection: MetadataSelection,
    target: ManifestTarget<'_>,
) -> Result<String, FormatEncodeError> {
    // Include escaped frame names as well as every selected component so none
    // of the following pushes can grow the string through an infallible path.
    let capacity = usize::try_from(manifest_json_len(
        profile,
        report,
        notices,
        scene_digest.is_some(),
        selection,
        target,
    ))
    .map_err(|_| metadata_size_error())?;
    let mut manifest = String::new();
    manifest
        .try_reserve_exact(capacity)
        .map_err(|_| memory("artifact_manifest"))?;
    match target {
        ManifestTarget::Scene => manifest.push_str("{\"schema_version\":1,\"provenance\":"),
        ManifestTarget::Frame {
            ordinal,
            kind,
            name,
        } => {
            manifest.push_str("{\"schema_version\":2,\"target\":{\"kind\":\"");
            manifest.push_str(kind.as_str());
            manifest.push_str("\",\"ordinal\":");
            write!(manifest, "{ordinal}").map_err(|_| metadata_size_error())?;
            if let Some(name) = name {
                manifest.push_str(",\"name\":\"");
                super::notices::push_json_string_content(&mut manifest, name);
                manifest.push('"');
            }
            manifest.push_str("},\"provenance\":");
        }
    }
    if selection.provenance {
        operation(Stage::ProvenanceSerialization, || {
            manifest.push_str("{\"renderer_version\":\"");
            manifest.push_str(env!("CARGO_PKG_VERSION"));
            manifest.push_str("\",\"compatibility_revision\":");
            write!(manifest, "{}", profile.compatibility_revision().get())
                .map_err(|_| metadata_size_error())?;
            manifest.push_str(",\"excalidraw_release\":\"");
            manifest.push_str(profile.excalidraw_release());
            manifest.push_str("\",\"reference_profile_digest\":\"");
            manifest.push_str(profile.reference_profile_digest());
            manifest.push_str("\"}");
            Ok(())
        })?;
    } else {
        manifest.push_str("null");
    }
    manifest.push_str(",\"diagnostic_summary\":");
    if selection.diagnostic_summary {
        operation(Stage::DiagnosticSummarySerialization, || {
            push_diagnostic_summary(&mut manifest, report)
        })?;
    } else {
        manifest.push_str("null");
    }
    manifest.push_str(",\"notices\":");
    manifest.push_str(notices);
    manifest.push_str(",\"embedded_scene\":");
    if let Some(digest) = scene_digest {
        manifest.push_str("{\"name\":\"scene.excalidraw\",\"media_type\":\"");
        manifest.push_str(SCENE_MEDIA_TYPE);
        manifest.push_str("\",\"sha256\":\"");
        manifest.push_str(digest);
        manifest.push_str("\"}");
    } else {
        manifest.push_str("null");
    }
    manifest.push('}');
    Ok(manifest)
}

fn json_string_content_len(value: &str) -> u64 {
    value.chars().fold(0_u64, |len, character| {
        len.saturating_add(match character {
            '"' | '\\' | '\u{08}' | '\u{0c}' | '\n' | '\r' | '\t' => 2,
            character if character < '\u{20}' => 6,
            character => u64::try_from(character.len_utf8()).unwrap_or(u64::MAX),
        })
    })
}

fn push_diagnostic_summary(
    output: &mut String,
    report: &CompatibilityReport,
) -> Result<(), FormatEncodeError> {
    let mut info = 0_u64;
    let mut warning = 0_u64;
    let mut by_code = Vec::new();
    by_code
        .try_reserve_exact(report.len())
        .map_err(|_| memory("diagnostic_summary_counts"))?;
    for diagnostic in report.diagnostics() {
        match diagnostic.severity() {
            DiagnosticSeverity::Info => info += 1,
            DiagnosticSeverity::Warning => warning += 1,
        }
        if let Some((_, count)) = by_code
            .iter_mut()
            .find(|(code, _)| *code == diagnostic.code().as_str())
        {
            *count += 1;
        } else {
            by_code.push((diagnostic.code().as_str(), 1_u64));
        }
    }
    by_code.sort_unstable_by_key(|(code, _)| *code);
    output.push_str("{\"snapshot\":\"pre_metadata\",\"total\":");
    write!(output, "{}", report.len()).map_err(|_| metadata_size_error())?;
    output.push_str(",\"by_severity\":{\"info\":");
    write!(output, "{info}").map_err(|_| metadata_size_error())?;
    output.push_str(",\"warning\":");
    write!(output, "{warning}").map_err(|_| metadata_size_error())?;
    output.push_str("},\"by_code\":[");
    let mut separator = "";
    for (code, count) in by_code {
        output.push_str(separator);
        output.push_str("{\"code\":\"");
        output.push_str(code);
        output.push_str("\",\"count\":");
        write!(output, "{count}").map_err(|_| metadata_size_error())?;
        output.push('}');
        separator = ",";
    }
    output.push_str("]}");
    Ok(())
}

fn scene_wrapper(compressed: &[u8]) -> Result<Vec<u8>, FormatEncodeError> {
    let capacity = WRAPPER_PREFIX
        .len()
        .checked_add(compressed.len().saturating_mul(6))
        .and_then(|length| length.checked_add(2))
        .ok_or_else(metadata_size_error)?;
    let mut wrapper = Vec::new();
    wrapper
        .try_reserve_exact(capacity)
        .map_err(|_| memory("embedded_scene_wrapper"))?;
    wrapper.extend_from_slice(WRAPPER_PREFIX);
    for byte in compressed {
        match byte {
            b'"' => wrapper.extend_from_slice(b"\\\""),
            b'\\' => wrapper.extend_from_slice(b"\\\\"),
            0x08 => wrapper.extend_from_slice(b"\\b"),
            0x09 => wrapper.extend_from_slice(b"\\t"),
            0x0a => wrapper.extend_from_slice(b"\\n"),
            0x0c => wrapper.extend_from_slice(b"\\f"),
            0x0d => wrapper.extend_from_slice(b"\\r"),
            0x00..=0x1f => {
                const HEX: &[u8; 16] = b"0123456789abcdef";
                wrapper.extend_from_slice(b"\\u00");
                wrapper.push(HEX[usize::from(byte >> 4)]);
                wrapper.push(HEX[usize::from(byte & 0x0f)]);
            }
            byte => wrapper.push(*byte),
        }
    }
    wrapper.extend_from_slice(b"\"}");
    Ok(wrapper)
}

fn stored_zlib(source: &[u8]) -> Result<Vec<u8>, FormatEncodeError> {
    let block_count = source.len().max(1).div_ceil(usize::from(u16::MAX));
    let capacity = 2_usize
        .checked_add(block_count.checked_mul(5).ok_or_else(metadata_size_error)?)
        .and_then(|length| length.checked_add(source.len()))
        .and_then(|length| length.checked_add(4))
        .ok_or_else(metadata_size_error)?;
    let mut zlib = Vec::new();
    zlib.try_reserve_exact(capacity)
        .map_err(|_| memory("embedded_scene_compression"))?;
    zlib.extend_from_slice(&[0x78, 0x01]);
    let mut offset = 0_usize;
    loop {
        let length = (source.len() - offset).min(usize::from(u16::MAX));
        let final_block = offset + length == source.len();
        zlib.push(u8::from(final_block));
        let length = u16::try_from(length).map_err(|_| metadata_size_error())?;
        zlib.extend_from_slice(&length.to_le_bytes());
        zlib.extend_from_slice(&(!length).to_le_bytes());
        zlib.extend_from_slice(&source[offset..offset + usize::from(length)]);
        offset += usize::from(length);
        if final_block {
            break;
        }
    }
    let mut a = 1_u32;
    let mut b = 0_u32;
    for byte in source {
        a = (a + u32::from(*byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    zlib.extend_from_slice(&((b << 16) | a).to_be_bytes());
    Ok(zlib)
}

fn sha256_string(bytes: &[u8]) -> Result<String, FormatEncodeError> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(bytes);
    let mut value = String::new();
    value
        .try_reserve_exact(71)
        .map_err(|_| memory("embedded_scene_digest"))?;
    value.push_str("sha256:");
    for byte in digest {
        value.push(char::from(HEX[usize::from(byte >> 4)]));
        value.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    Ok(value)
}

fn try_string(value: &str, subject: &'static str) -> Result<String, FormatEncodeError> {
    let mut cloned = String::new();
    cloned
        .try_reserve_exact(value.len())
        .map_err(|_| memory(subject))?;
    cloned.push_str(value);
    Ok(cloned)
}

fn try_bytes(value: &[u8], subject: &'static str) -> Result<Vec<u8>, FormatEncodeError> {
    let mut cloned = Vec::new();
    cloned
        .try_reserve_exact(value.len())
        .map_err(|_| memory(subject))?;
    cloned.extend_from_slice(value);
    Ok(cloned)
}

fn memory(subject: &'static str) -> FormatEncodeError {
    FormatEncodeError::MemoryUnavailable {
        subject_kind: "metadata",
        subject,
    }
}

fn metadata_size_error() -> FormatEncodeError {
    FormatEncodeError::Encoding("artifact metadata size exceeds the supported range".to_owned())
}

pub(super) fn metadata_failure(
    error: FormatEncodeError,
    failure: MetadataFailure,
) -> FormatEncodeError {
    match error {
        FormatEncodeError::MemoryUnavailable { .. }
        | FormatEncodeError::FontUnavailable(_)
        | FormatEncodeError::SvgFontSubset { .. }
        | FormatEncodeError::Metadata(_) => error,
        FormatEncodeError::Encoding(_) => FormatEncodeError::Metadata(failure),
    }
}

fn aligned(bytes: u64) -> u64 {
    bytes.saturating_add(63) / 64 * 64
}

/// The admitted scene, reference profile, and compatibility report that the
/// metadata unit tests and the PNG chunk assembly test build artifact
/// metadata from.
#[cfg(test)]
pub(super) fn test_fixture() -> (
    SceneDocument,
    &'static ReferenceProfileIdentity,
    CompatibilityReport,
) {
    const SCENE: &[u8] = br#"{"type":"excalidraw","version":2,"elements":[{"id":"rect","type":"rectangle","x":0,"y":0,"width":40,"height":30,"updated":1}],"appState":{},"files":{}}"#;
    let scene = SceneDocument::parse(SCENE).expect("metadata fixture scene is admitted");
    let profile = crate::Renderer::new().profile_identity();
    let request = RenderRequest::builder(crate::OutputFormat::Svg)
        .build()
        .expect("metadata fixture request is valid");
    let report = crate::Renderer::new()
        .render(&scene, &request)
        .expect("metadata fixture renders")
        .report()
        .clone();
    (scene, profile, report)
}

#[cfg(test)]
mod tests {
    use super::stored_zlib;

    #[test]
    fn stored_zlib_uses_exact_65535_byte_block_boundaries() {
        let Ok(one_block) = stored_zlib(&vec![b'a'; 65_535]) else {
            panic!("one block encodes");
        };
        assert_eq!(&one_block[..7], &[0x78, 0x01, 0x01, 0xff, 0xff, 0x00, 0x00]);
        assert_eq!(one_block.len(), 2 + 5 + 65_535 + 4);

        let Ok(two_blocks) = stored_zlib(&vec![b'a'; 65_536]) else {
            panic!("two blocks encode");
        };
        assert_eq!(
            &two_blocks[..7],
            &[0x78, 0x01, 0x00, 0xff, 0xff, 0x00, 0x00]
        );
        assert_eq!(
            &two_blocks[2 + 5 + 65_535..2 + 5 + 65_535 + 5],
            &[0x01, 0x01, 0x00, 0xfe, 0xff]
        );
        assert_eq!(two_blocks.len(), 2 + 10 + 65_536 + 4);
    }
}

#[cfg(test)]
mod agreement_tests {
    use super::super::notices::MandatoryNotices;
    use super::*;
    use crate::FrameKind;

    /// The manifest length is estimated before the manifest is built, and the
    /// estimate is charged against the resource ledger. If the two disagree the
    /// ledger is wrong, so every selection and target shape is checked rather
    /// than relying on the `debug_assertions`-only check inside `build`.
    #[test]
    fn manifest_length_estimate_matches_the_serialized_manifest() {
        let (scene, profile, report) = test_fixture();
        for notices in [None, Some(notices())] {
            for selection in selections() {
                for target in targets() {
                    if !has_manifest(selection, notices.as_ref()) {
                        continue;
                    }
                    let notice_json = notices.as_ref().map_or("[]", MandatoryNotices::as_json);
                    let digest = match selection.embedded_scene {
                        true => {
                            let canonical = scene
                                .canonicalized_original_bytes()
                                .unwrap_or_else(|_| panic!("canonical scene is representable"));
                            Some(
                                sha256_string(&canonical)
                                    .unwrap_or_else(|_| panic!("scene digest is representable")),
                            )
                        }
                        false => None,
                    };
                    let manifest = manifest_json(
                        profile,
                        &report,
                        notice_json,
                        digest.as_deref(),
                        selection,
                        target,
                    )
                    .unwrap_or_else(|_| panic!("manifest serializes"));
                    let estimated = manifest_json_len(
                        profile,
                        &report,
                        notice_json,
                        selection.embedded_scene,
                        selection,
                        target,
                    );
                    assert_eq!(
                        estimated,
                        manifest.len() as u64,
                        "{} {} manifest length estimate diverges",
                        selection_label(selection),
                        target_label(target)
                    );
                }
            }
        }
    }

    /// `estimate` is the charge the ledger commits before generation; `build`
    /// is what generation actually consumes. Release builds skip the
    /// `debug_assert_eq!` inside `build`, so the agreement is asserted here too.
    #[test]
    fn metadata_estimate_matches_the_built_metadata() {
        let (scene, profile, report) = test_fixture();
        for notices in [None, Some(notices())] {
            for selection in selections() {
                for target in targets() {
                    let built = ArtifactMetadata::build(
                        &scene,
                        profile,
                        &report,
                        notices.as_ref(),
                        selection,
                        target,
                    )
                    .unwrap_or_else(|_| panic!("metadata builds"));
                    assert_eq!(
                        ArtifactMetadata::estimate(
                            &scene,
                            profile,
                            &report,
                            notices.as_ref(),
                            selection,
                            target,
                        ),
                        MetadataEstimate {
                            scratch_bytes: built.scratch_bytes(),
                            generation_work: built.generation_work(),
                        },
                        "{} {} metadata estimate diverges",
                        selection_label(selection),
                        target_label(target)
                    );
                }
            }
        }
    }

    fn notices() -> MandatoryNotices {
        MandatoryNotices::from_json(
            "[{\"asset_identity\":\"fixture\",\"text\":\"notice\"}]".to_owned(),
        )
    }

    fn selections() -> Vec<MetadataSelection> {
        let mut selections = Vec::new();
        for provenance in [false, true] {
            for diagnostic_summary in [false, true] {
                for embedded_scene in [false, true] {
                    selections.push(MetadataSelection {
                        provenance,
                        diagnostic_summary,
                        embedded_scene,
                    });
                }
            }
        }
        selections
    }

    fn targets() -> Vec<ManifestTarget<'static>> {
        vec![
            ManifestTarget::Scene,
            ManifestTarget::Frame {
                ordinal: 1,
                kind: FrameKind::Frame,
                name: None,
            },
            ManifestTarget::Frame {
                ordinal: 12,
                kind: FrameKind::MagicFrame,
                name: Some("Road \"map\"\n"),
            },
        ]
    }

    fn has_manifest(selection: MetadataSelection, notices: Option<&MandatoryNotices>) -> bool {
        selection.provenance
            || selection.diagnostic_summary
            || selection.embedded_scene
            || notices.is_some()
    }

    fn selection_label(selection: MetadataSelection) -> String {
        format!(
            "provenance={} diagnostics={} scene={}",
            selection.provenance, selection.diagnostic_summary, selection.embedded_scene
        )
    }

    fn target_label(target: ManifestTarget<'_>) -> String {
        match target {
            ManifestTarget::Scene => "scene".to_owned(),
            ManifestTarget::Frame {
                ordinal,
                kind,
                name,
            } => format!(
                "frame ordinal={ordinal} kind={} name={name:?}",
                kind.as_str()
            ),
        }
    }
}
