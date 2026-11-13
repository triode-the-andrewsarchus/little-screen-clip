//! Scene documents: admission of the version-2 envelope and the typed
//! `*Source` accessors the rest of the pipeline reads through. `json` owns the
//! bounded, lossless parser that produces the preserved tree, the RFC 6901
//! pointer builders and the canonical writer; this file adds the envelope
//! check, the public `SceneDocument` and `SceneParseError`, and the original-
//! JSON visitor the ledger charges from.

mod json;

#[cfg(test)]
pub(crate) use json::JsonKind;
pub(crate) use json::{
    finite_json_number, member_value, pointer_child, pointer_index, JsonMember, JsonValue,
};
use json::{
    json_bool, json_i32, json_string, json_u64, visit_canonical_json, write_canonical_json,
    ParserLimit,
};

use std::collections::TryReserveError;
use std::error::Error;
use std::fmt;
use std::sync::Arc;

use crate::{DiagnosticDetails, ErrorCode, ErrorDescriptor, Measurement};

/// An admitted Excalidraw scene document.
#[derive(Clone)]
pub struct SceneDocument {
    _root: Arc<JsonValue>,
    source_bytes: u64,
    #[cfg(test)]
    source_sha256: [u8; 32],
}

impl SceneDocument {
    /// Parses and admits a scene document from UTF-8 JSON bytes.
    ///
    /// # Errors
    ///
    /// Returns a [`SceneParseError`] when the bytes are not UTF-8
    /// (`scene.invalid-utf8`) or JSON (`scene.invalid-json`), when the
    /// document is not a version-2 Excalidraw scene envelope
    /// (`scene.invalid-envelope`), when an object repeats a key
    /// (`scene.duplicate-object-key`), or when it crosses one of the fixed
    /// parser ceilings: 64 MiB of input (`scene.input-too-large`), nesting
    /// depth, entry count, string, key or number-token size, or parser
    /// memory (the other `scene.*` codes). These ceilings are independent of
    /// [`ResourceLimits`](crate::ResourceLimits).
    pub fn parse(bytes: &[u8]) -> Result<Self, SceneParseError> {
        let root = json::parse(bytes).map_err(|error| {
            if matches!(
                error.code,
                ErrorCode::SCENE_INVALID_UTF8 | ErrorCode::SCENE_INVALID_JSON
            ) {
                recognized_export(bytes).unwrap_or(error)
            } else {
                error
            }
        })?;
        validate_envelope(&root)?;

        Ok(Self {
            _root: Arc::new(root),
            source_bytes: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
            #[cfg(test)]
            source_sha256: crate::resource_accounting::sha256_digest(bytes),
        })
    }

    /// Discovers live ordinary and magic frame occurrences in source order.
    pub fn frames(&self) -> crate::FrameRefs<'_> {
        crate::FrameRefs::new(self)
    }

    pub(crate) fn same_identity(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self._root, &other._root)
    }

    /// Hashes the identity [`same_identity`](Self::same_identity) compares.
    pub(crate) fn hash_identity<H: std::hash::Hasher>(&self, state: &mut H) {
        std::ptr::hash(Arc::as_ptr(&self._root), state);
    }

    pub(crate) fn canonicalized_original_bytes(&self) -> Result<Vec<u8>, TryReserveError> {
        let mut bytes = Vec::new();
        write_canonical_json(&self._root, &mut bytes)?;
        Ok(bytes)
    }

    pub(crate) fn visit_canonicalized_original(&self, visitor: &mut impl FnMut(&[u8])) {
        visit_canonical_json(&self._root, visitor);
    }

    pub(crate) fn element_occurrence_count(&self) -> usize {
        let JsonValue::Object(root) = &*self._root else {
            return 0;
        };
        let Some(JsonValue::Array(elements)) = member_value(root, "elements") else {
            return 0;
        };
        elements.len()
    }

    pub(crate) fn element_value(&self, index: usize) -> Option<&JsonValue> {
        let JsonValue::Object(root) = &*self._root else {
            return None;
        };
        let Some(JsonValue::Array(elements)) = member_value(root, "elements") else {
            return None;
        };
        elements.get(index)
    }

    pub(crate) fn root_members(&self) -> &[JsonMember] {
        match &*self._root {
            JsonValue::Object(members) => members,
            _ => &[],
        }
    }

    pub(crate) fn app_state_members(&self) -> &[JsonMember] {
        let Some(JsonValue::Object(members)) = member_value(self.root_members(), "appState") else {
            return &[];
        };
        members
    }

    pub(crate) fn app_state_member_position(&self, key: &str) -> Option<usize> {
        self.app_state_members()
            .iter()
            .position(|member| &*member.key == key)
    }

    pub(crate) fn view_background_color(&self) -> SourceField<&str> {
        source_field(self.app_state_members(), "viewBackgroundColor", json_string)
    }

    pub(crate) fn source_owner_order(&self, root_member: &str, child_index: usize) -> u64 {
        let root_position = match &*self._root {
            JsonValue::Object(members) => members
                .iter()
                .position(|member| &*member.key == root_member)
                .unwrap_or(usize::MAX),
            _ => usize::MAX,
        };
        u64::try_from(root_position)
            .unwrap_or(u64::MAX)
            .saturating_mul(u64::from(u32::MAX) + 1)
            .saturating_add(u64::try_from(child_index).unwrap_or(u64::MAX))
    }

    pub(crate) fn element_at(&self, index: usize) -> Option<ElementSource<'_>> {
        let JsonValue::Object(root) = &*self._root else {
            return None;
        };
        let JsonValue::Array(elements) = member_value(root, "elements")? else {
            return None;
        };
        let value = elements.get(index)?;
        let JsonValue::Object(element) = value else {
            return None;
        };
        let kind = match json_string(member_value(element, "type")?)? {
            "rectangle" => ElementKind::Rectangle,
            "diamond" => ElementKind::Diamond,
            "ellipse" => ElementKind::Ellipse,
            "text" => ElementKind::Text,
            "line" => ElementKind::Line,
            "draw" => ElementKind::LegacyDraw,
            "arrow" => ElementKind::Arrow,
            "freedraw" => ElementKind::Freedraw,
            "image" => ElementKind::Image,
            "frame" => ElementKind::Frame,
            "magicframe" => ElementKind::MagicFrame,
            "iframe" => ElementKind::Iframe,
            "embeddable" => ElementKind::Embeddable,
            _ => return None,
        };
        Some(ElementSource {
            element: value,
            kind,
            id: source_field(element, "id", json_string),
            index: source_nullable_string(element, "index"),
            x: source_field(element, "x", finite_json_number),
            y: source_field(element, "y", finite_json_number),
            width: source_field(element, "width", finite_json_number),
            height: source_field(element, "height", finite_json_number),
            angle: source_field(element, "angle", finite_json_number),
            stroke_color: source_field(element, "strokeColor", json_string),
            background_color: source_field(element, "backgroundColor", json_string),
            fill_style: source_field(element, "fillStyle", json_string),
            stroke_width: source_field(element, "strokeWidth", finite_json_number),
            stroke_style: source_field(element, "strokeStyle", json_string),
            roundness: source_roundness(element, "roundness"),
            stroke_sharpness: source_field(element, "strokeSharpness", json_string),
            roughness: source_field(element, "roughness", finite_json_number),
            opacity: source_field(element, "opacity", finite_json_number),
            seed: source_field(element, "seed", json_i32),
            is_deleted: source_field(element, "isDeleted", json_bool),
            frame_id: source_nullable_string(element, "frameId"),
            link: source_nullable_string(element, "link"),
            updated_is_missing: member_value(element, "updated").is_none(),
            points: source_point_array(element, "points"),
            pressures: source_number_array(element, "pressures"),
            simulate_pressure: source_field(element, "simulatePressure", json_bool),
            start_binding: source_binding(element, "startBinding"),
            end_binding: source_binding(element, "endBinding"),
            start_arrowhead: source_nullable_string(element, "startArrowhead"),
            end_arrowhead: source_nullable_string(element, "endArrowhead"),
            elbowed: source_field(element, "elbowed", json_bool),
            file_id: source_nullable_string(element, "fileId"),
            image_status: source_field(element, "status", json_string),
            image_scale: source_number_array(element, "scale"),
            image_crop: source_image_crop(element, "crop"),
            name: source_nullable_string(element, "name"),
            font_size: source_field(element, "fontSize", finite_json_number),
            font_family: source_field(element, "fontFamily", json_i32),
            font: source_field(element, "font", json_string),
            text: source_field(element, "text", json_string),
            original_text: source_field(element, "originalText", json_string),
            container_id: source_nullable_string(element, "containerId"),
            text_align: source_field(element, "textAlign", json_string),
            vertical_align: source_field(element, "verticalAlign", json_string),
            line_height: source_field(element, "lineHeight", finite_json_number),
        })
    }

    pub(crate) fn frame_rendering(&self) -> SourceField<FrameRenderingSource<'_>> {
        let JsonValue::Object(root) = &*self._root else {
            return SourceField::Invalid;
        };
        let Some(JsonValue::Object(app_state)) = member_value(root, "appState") else {
            return SourceField::Missing;
        };
        source_field(app_state, "frameRendering", |value| {
            let JsonValue::Object(members) = value else {
                return None;
            };
            Some(FrameRenderingSource { members })
        })
    }

    pub(crate) fn file_record_count(&self) -> usize {
        let JsonValue::Object(root) = &*self._root else {
            return 0;
        };
        let Some(JsonValue::Object(files)) = member_value(root, "files") else {
            return 0;
        };
        files.len()
    }

    pub(crate) fn file_record_at(&self, index: usize) -> Option<FileRecordSource<'_>> {
        let JsonValue::Object(root) = &*self._root else {
            return None;
        };
        let JsonValue::Object(files) = member_value(root, "files")? else {
            return None;
        };
        let member = files.get(index)?;
        Some(FileRecordSource {
            key: &member.key,
            value: &member.value,
        })
    }

    pub(crate) fn element_is_silent(&self, index: usize) -> bool {
        let JsonValue::Object(root) = &*self._root else {
            return false;
        };
        let Some(JsonValue::Array(elements)) = member_value(root, "elements") else {
            return false;
        };
        let Some(JsonValue::Object(element)) = elements.get(index) else {
            return false;
        };
        matches!(
            member_value(element, "isDeleted"),
            Some(JsonValue::Bool(true))
        )
    }

    pub(crate) const fn source_bytes(&self) -> u64 {
        self.source_bytes
    }

    #[cfg(test)]
    pub(crate) const fn source_sha256(&self) -> &[u8; 32] {
        &self.source_sha256
    }

    /// The JSON kind at an RFC 6901 pointer into the admitted document, or
    /// `None` when the pointer names nothing.
    #[cfg(test)]
    pub(crate) fn json_kind_at(&self, pointer: &str) -> Option<JsonKind> {
        let mut value: &JsonValue = &self._root;
        if pointer.is_empty() {
            return Some(value.kind());
        }
        for segment in pointer.strip_prefix('/')?.split('/') {
            let segment = segment.replace("~1", "/").replace("~0", "~");
            value = match value {
                JsonValue::Object(members) => member_value(members, &segment)?,
                JsonValue::Array(values) => values.get(segment.parse::<usize>().ok()?)?,
                JsonValue::Null
                | JsonValue::Bool(_)
                | JsonValue::Number(_)
                | JsonValue::String(_) => {
                    return None;
                }
            };
        }
        Some(value.kind())
    }

    pub(crate) fn visit_original_json_entries<E>(
        &self,
        mut visit: impl FnMut(OriginalJsonEntry<'_>) -> Result<(), E>,
    ) -> Result<(), OriginalJsonVisitError<E>> {
        visit_json_value(&self._root, "", false, &mut visit)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ElementKind {
    Rectangle,
    Diamond,
    Ellipse,
    Text,
    Line,
    LegacyDraw,
    Arrow,
    Freedraw,
    Image,
    Frame,
    MagicFrame,
    Iframe,
    Embeddable,
}

impl ElementKind {
    pub(crate) const fn scene_surface_kind(self) -> &'static str {
        match self {
            Self::Rectangle => "rectangle",
            Self::Diamond => "diamond",
            Self::Ellipse => "ellipse",
            Self::Text => "text",
            Self::Line => "line",
            Self::LegacyDraw => "draw",
            Self::Arrow => "arrow",
            Self::Freedraw => "freedraw",
            Self::Image => "image",
            Self::Frame => "frame",
            Self::MagicFrame => "magicframe",
            Self::Iframe => "iframe",
            Self::Embeddable => "embeddable",
        }
    }
}

pub(crate) struct ElementSource<'a> {
    element: &'a JsonValue,
    pub(crate) kind: ElementKind,
    pub(crate) id: SourceField<&'a str>,
    pub(crate) index: SourceField<Option<&'a str>>,
    pub(crate) x: SourceField<f64>,
    pub(crate) y: SourceField<f64>,
    pub(crate) width: SourceField<f64>,
    pub(crate) height: SourceField<f64>,
    pub(crate) angle: SourceField<f64>,
    pub(crate) stroke_color: SourceField<&'a str>,
    pub(crate) background_color: SourceField<&'a str>,
    pub(crate) fill_style: SourceField<&'a str>,
    pub(crate) stroke_width: SourceField<f64>,
    pub(crate) stroke_style: SourceField<&'a str>,
    pub(crate) roundness: SourceField<Option<RoundnessSource<'a>>>,
    pub(crate) stroke_sharpness: SourceField<&'a str>,
    pub(crate) roughness: SourceField<f64>,
    pub(crate) opacity: SourceField<f64>,
    pub(crate) seed: SourceField<i32>,
    pub(crate) is_deleted: SourceField<bool>,
    pub(crate) frame_id: SourceField<Option<&'a str>>,
    pub(crate) link: SourceField<Option<&'a str>>,
    pub(crate) updated_is_missing: bool,
    pub(crate) points: SourceField<PointArraySource<'a>>,
    pub(crate) pressures: SourceField<NumberArraySource<'a>>,
    pub(crate) simulate_pressure: SourceField<bool>,
    pub(crate) start_binding: SourceField<Option<BindingSource<'a>>>,
    pub(crate) end_binding: SourceField<Option<BindingSource<'a>>>,
    pub(crate) start_arrowhead: SourceField<Option<&'a str>>,
    pub(crate) end_arrowhead: SourceField<Option<&'a str>>,
    pub(crate) elbowed: SourceField<bool>,
    pub(crate) file_id: SourceField<Option<&'a str>>,
    pub(crate) image_status: SourceField<&'a str>,
    pub(crate) image_scale: SourceField<NumberArraySource<'a>>,
    pub(crate) image_crop: SourceField<Option<ImageCropSource<'a>>>,
    pub(crate) name: SourceField<Option<&'a str>>,
    pub(crate) font_size: SourceField<f64>,
    pub(crate) font_family: SourceField<i32>,
    pub(crate) font: SourceField<&'a str>,
    pub(crate) text: SourceField<&'a str>,
    pub(crate) original_text: SourceField<&'a str>,
    pub(crate) container_id: SourceField<Option<&'a str>>,
    pub(crate) text_align: SourceField<&'a str>,
    pub(crate) vertical_align: SourceField<&'a str>,
    pub(crate) line_height: SourceField<f64>,
}

impl ElementSource<'_> {
    pub(crate) fn canonical_element_bytes(&self) -> Result<Vec<u8>, TryReserveError> {
        let mut bytes = Vec::new();
        write_canonical_json(self.element, &mut bytes)?;
        Ok(bytes)
    }

    pub(crate) fn member_position(&self, key: &str) -> Option<usize> {
        let JsonValue::Object(members) = self.element else {
            return None;
        };
        members.iter().position(|member| &*member.key == key)
    }
}

#[derive(Clone, Copy)]
pub(crate) struct PointArraySource<'a> {
    values: &'a [JsonValue],
}

impl PointArraySource<'_> {
    pub(crate) fn len(&self) -> usize {
        self.values.len()
    }

    pub(crate) fn point(&self, index: usize) -> SourceField<[f64; 2]> {
        let Some(value) = self.values.get(index) else {
            return SourceField::Missing;
        };
        parse_point(value).map_or(SourceField::Invalid, SourceField::Value)
    }

    pub(crate) fn invalid_component(&self, index: usize) -> Option<usize> {
        let JsonValue::Array(values) = self.values.get(index)? else {
            return None;
        };
        if values.len() < 2 {
            return Some(values.len());
        }
        (0..2).find(|component| finite_json_number(&values[*component]).is_none())
    }

    pub(crate) fn first_invalid_point(&self) -> Option<(usize, Option<usize>)> {
        (0..self.values.len()).find_map(|index| {
            matches!(self.point(index), SourceField::Invalid)
                .then(|| (index, self.invalid_component(index)))
        })
    }

    pub(crate) fn component_count(&self, index: usize) -> Option<usize> {
        let JsonValue::Array(values) = self.values.get(index)? else {
            return None;
        };
        Some(values.len())
    }
}

#[derive(Clone, Copy)]
pub(crate) struct NumberArraySource<'a> {
    values: &'a [JsonValue],
}

impl NumberArraySource<'_> {
    pub(crate) fn len(&self) -> usize {
        self.values.len()
    }

    pub(crate) fn number(&self, index: usize) -> SourceField<f64> {
        let Some(value) = self.values.get(index) else {
            return SourceField::Missing;
        };
        finite_json_number(value).map_or(SourceField::Invalid, SourceField::Value)
    }
}

#[derive(Clone, Copy)]
pub(crate) struct BindingSource<'a> {
    members: &'a [JsonMember],
}

impl BindingSource<'_> {
    pub(crate) fn element_id(&self) -> SourceField<&str> {
        source_field(self.members, "elementId", json_string)
    }

    pub(crate) fn focus(&self) -> SourceField<f64> {
        match member_value(self.members, "focus") {
            None | Some(JsonValue::Null | JsonValue::Bool(false)) => SourceField::Missing,
            Some(JsonValue::String(value)) if value.is_empty() => SourceField::Missing,
            Some(value) => {
                finite_json_number(value).map_or(SourceField::Invalid, SourceField::Value)
            }
        }
    }

    pub(crate) fn gap(&self) -> SourceField<f64> {
        source_field(self.members, "gap", finite_json_number)
    }

    pub(crate) fn fixed_point(&self) -> SourceField<[f64; 2]> {
        match member_value(self.members, "fixedPoint") {
            None | Some(JsonValue::Null) => SourceField::Missing,
            Some(JsonValue::Array(values)) if values.len() != 2 => SourceField::Invalid,
            Some(value) => parse_point(value).map_or(SourceField::Invalid, SourceField::Value),
        }
    }

    pub(crate) fn fixed_point_invalid_component(&self) -> Option<usize> {
        let JsonValue::Array(values) = member_value(self.members, "fixedPoint")? else {
            return None;
        };
        if values.len() < 2 {
            return Some(values.len());
        }
        (0..2).find(|component| finite_json_number(&values[*component]).is_none())
    }

    pub(crate) fn member_position(&self, key: &str) -> Option<usize> {
        self.members.iter().position(|member| &*member.key == key)
    }

    pub(crate) fn members(&self) -> &[JsonMember] {
        self.members
    }
}

#[derive(Clone, Copy)]
pub(crate) struct ImageCropSource<'a> {
    members: &'a [JsonMember],
}

#[derive(Clone, Copy)]
pub(crate) struct RoundnessSource<'a> {
    members: &'a [JsonMember],
}

impl RoundnessSource<'_> {
    pub(crate) fn kind(&self) -> SourceField<i32> {
        source_field(self.members, "type", json_i32)
    }

    pub(crate) fn value(&self) -> SourceField<f64> {
        source_field(self.members, "value", finite_json_number)
    }

    pub(crate) fn member_position(&self, key: &str) -> Option<usize> {
        self.members.iter().position(|member| &*member.key == key)
    }

    pub(crate) fn members(&self) -> &[JsonMember] {
        self.members
    }
}

impl ImageCropSource<'_> {
    pub(crate) fn len(&self) -> usize {
        self.members.len()
    }

    pub(crate) fn number(&self, key: &str) -> SourceField<f64> {
        source_field(self.members, key, finite_json_number)
    }

    pub(crate) fn member_position(&self, key: &str) -> Option<usize> {
        self.members.iter().position(|member| &*member.key == key)
    }

    pub(crate) fn members(&self) -> &[JsonMember] {
        self.members
    }
}

#[derive(Clone, Copy)]
pub(crate) struct FrameRenderingSource<'a> {
    members: &'a [JsonMember],
}

impl FrameRenderingSource<'_> {
    pub(crate) fn boolean(&self, key: &str) -> SourceField<bool> {
        source_field(self.members, key, json_bool)
    }

    pub(crate) fn member_position(&self, key: &str) -> Option<usize> {
        self.members.iter().position(|member| &*member.key == key)
    }

    pub(crate) fn members(&self) -> &[JsonMember] {
        self.members
    }
}

pub(crate) struct FileRecordSource<'a> {
    key: &'a str,
    value: &'a JsonValue,
}

impl<'a> FileRecordSource<'a> {
    pub(crate) fn key(&self) -> &'a str {
        self.key
    }

    pub(crate) fn is_record(&self) -> bool {
        matches!(self.value, JsonValue::Object(_))
    }

    pub(crate) fn members(&self) -> Option<&[JsonMember]> {
        let JsonValue::Object(members) = self.value else {
            return None;
        };
        Some(members)
    }

    pub(crate) fn id(&self) -> SourceField<&str> {
        let JsonValue::Object(members) = self.value else {
            return SourceField::Invalid;
        };
        source_field(members, "id", json_string)
    }

    pub(crate) fn created(&self) -> SourceField<u64> {
        let JsonValue::Object(members) = self.value else {
            return SourceField::Invalid;
        };
        source_field(members, "created", json_u64)
    }

    pub(crate) fn data_url(&self) -> SourceField<&str> {
        let JsonValue::Object(members) = self.value else {
            return SourceField::Invalid;
        };
        source_field(members, "dataURL", json_string)
    }

    pub(crate) fn mime_type(&self) -> SourceField<&str> {
        let JsonValue::Object(members) = self.value else {
            return SourceField::Invalid;
        };
        source_field(members, "mimeType", json_string)
    }

    pub(crate) fn member_position(&self, key: &str) -> Option<usize> {
        let JsonValue::Object(members) = self.value else {
            return None;
        };
        members.iter().position(|member| &*member.key == key)
    }
}

#[derive(Clone, Copy)]
pub(crate) enum SourceField<T> {
    Missing,
    Value(T),
    Invalid,
}

pub(crate) enum OriginalJsonVisitError<E> {
    Visitor(E),
    MemoryUnavailable { json_pointer: String },
}

#[derive(Clone, Copy)]
pub(crate) struct OriginalJsonEntry<'a> {
    pub(crate) kind: OriginalJsonEntryKind,
    pub(crate) json_pointer: &'a str,
    pub(crate) is_element_occurrence: bool,
}

#[derive(Clone, Copy)]
pub(crate) enum OriginalJsonEntryKind {
    Node,
    Member,
}

impl fmt::Debug for SceneDocument {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SceneDocument")
            .finish_non_exhaustive()
    }
}

/// Indicates that scene-document bytes could not be admitted.
///
/// [`Error::source`] is always `None`: JSON decoder failures are flattened
/// into the stable code and message.
#[derive(Clone, Debug)]
pub struct SceneParseError {
    code: ErrorCode,
    message: String,
    json_pointer: Option<String>,
    details: Option<Box<DiagnosticDetails>>,
}

impl SceneParseError {
    fn new(
        code: ErrorCode,
        message: impl Into<String>,
        json_pointer: Option<String>,
        details: Option<DiagnosticDetails>,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            json_pointer,
            details: details.map(Box::new),
        }
    }

    fn invalid_json(json_pointer: Option<String>) -> Self {
        Self::new(
            ErrorCode::SCENE_INVALID_JSON,
            "scene document is not valid JSON",
            json_pointer,
            None,
        )
    }

    fn invalid_envelope(json_pointer: &str, message: impl Into<String>) -> Self {
        Self::new(
            ErrorCode::SCENE_INVALID_ENVELOPE,
            message,
            Some(json_pointer.to_owned()),
            None,
        )
    }

    fn unsupported_input(json_pointer: Option<&str>, message: &str) -> Self {
        Self::new(
            ErrorCode::SCENE_UNSUPPORTED_INPUT,
            message,
            json_pointer.map(str::to_owned),
            None,
        )
    }

    fn limit(limit: ParserLimit, measurement: Measurement, json_pointer: Option<String>) -> Self {
        Self::new(
            limit.code,
            limit.message,
            json_pointer,
            Some(DiagnosticDetails::ParseLimit {
                field: limit.field,
                configured: limit.configured,
                value_kind: limit.value_kind,
                measurement,
            }),
        )
    }

    /// Returns the stable machine-readable error code.
    pub const fn code(&self) -> ErrorCode {
        self.code
    }

    /// Returns the narrowest reliable scene-document JSON pointer.
    pub fn json_pointer(&self) -> Option<&str> {
        self.json_pointer.as_deref()
    }

    /// Returns typed details required by this failure's stable code.
    pub fn details(&self) -> Option<&DiagnosticDetails> {
        self.details.as_deref()
    }

    /// Returns this failure's stable serializable descriptor.
    pub fn descriptor(&self) -> ErrorDescriptor {
        ErrorDescriptor::scene_parse(
            self.code,
            self.to_string(),
            self.json_pointer.clone(),
            self.details.as_deref().cloned(),
        )
    }
}

impl fmt::Display for SceneParseError {
    /// The failure's message; a parser-limit crossing appends the measured
    /// value, the limit field and its fixed ceiling.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)?;
        if let Some(crossing) = self.details().and_then(DiagnosticDetails::limit_crossing) {
            write!(formatter, ": {crossing}")?;
        }
        Ok(())
    }
}

impl Error for SceneParseError {}

/// The export a document that is neither UTF-8 nor JSON recognizably is
/// (spec section 4.1): a PNG, SVG or PDF image is `scene.unsupported-input`
/// rather than a malformed scene. `None` for anything else.
fn recognized_export(bytes: &[u8]) -> Option<SceneParseError> {
    let text = bytes
        .strip_prefix("\u{feff}".as_bytes())
        .unwrap_or(bytes)
        .trim_ascii_start();
    let message = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "this is a PNG image, not a scene document; open an image exported with an \
         embedded scene in Excalidraw and save it as an .excalidraw file"
    } else if text.starts_with(b"<svg")
        || text.starts_with(b"<?xml")
        || text.starts_with(b"<!DOCTYPE svg")
    {
        "this is an SVG image, not a scene document; open an image exported with an \
         embedded scene in Excalidraw and save it as an .excalidraw file"
    } else if bytes.starts_with(b"%PDF-") {
        "this is a PDF document, not a scene document; render the .excalidraw file it \
         was exported from"
    } else {
        return None;
    };
    Some(SceneParseError::unsupported_input(None, message))
}

/// Admits the version-2 envelope (spec section 4.1). Every rejection is
/// `scene.invalid-envelope` at the offending or expected member, except the
/// library and clipboard payloads most often mistaken for scene documents,
/// which are `scene.unsupported-input` at `/type`; the message says what is
/// wrong.
fn validate_envelope(root: &JsonValue) -> Result<(), SceneParseError> {
    let JsonValue::Object(members) = root else {
        return Err(SceneParseError::invalid_envelope(
            "",
            format!(
                "scene document must be a JSON object, not {}",
                json_kind_phrase(root)
            ),
        ));
    };

    match member_value(members, "type") {
        Some(JsonValue::String(value)) if &**value == "excalidraw" => {}
        Some(JsonValue::String(value)) if &**value == "excalidrawlib" => {
            return Err(SceneParseError::unsupported_input(
                Some("/type"),
                "this is an Excalidraw library (type \"excalidrawlib\"), not a scene document; \
                 render a scene saved as an .excalidraw file instead",
            ));
        }
        Some(JsonValue::String(value)) if &**value == "excalidraw/clipboard" => {
            return Err(SceneParseError::unsupported_input(
                Some("/type"),
                "this is an Excalidraw clipboard payload (type \"excalidraw/clipboard\"), \
                 not a scene document; paste it into Excalidraw and save the scene instead",
            ));
        }
        Some(JsonValue::String(value)) => {
            return Err(SceneParseError::invalid_envelope(
                "/type",
                format!("scene document type must be \"excalidraw\", not {value:?}"),
            ));
        }
        Some(other) => {
            return Err(SceneParseError::invalid_envelope(
                "/type",
                format!(
                    "scene document type must be the string \"excalidraw\", not {}",
                    json_kind_phrase(other)
                ),
            ));
        }
        None => {
            return Err(SceneParseError::invalid_envelope(
                "/type",
                "scene document has no \"type\" member; expected \"excalidraw\"",
            ));
        }
    }
    match member_value(members, "version") {
        Some(JsonValue::Number(value)) if &**value == "2" => {}
        Some(JsonValue::Number(value)) => {
            return Err(SceneParseError::invalid_envelope(
                "/version",
                format!("scene envelope version {value} is not supported; only version 2 is"),
            ));
        }
        Some(other) => {
            return Err(SceneParseError::invalid_envelope(
                "/version",
                format!(
                    "scene envelope version must be the number 2, not {}",
                    json_kind_phrase(other)
                ),
            ));
        }
        None => {
            return Err(SceneParseError::invalid_envelope(
                "/version",
                "scene document has no \"version\" member; expected envelope version 2",
            ));
        }
    }
    require_container(
        members,
        "elements",
        "/elements",
        "an array",
        true,
        |value| matches!(value, JsonValue::Array(_)),
    )?;
    // Excalidraw's load path restores an absent `appState` to its defaults
    // and an absent `files` to an empty map, and older excalidraw.com exports
    // omit `files`; either may be absent, but not another kind.
    require_container(
        members,
        "appState",
        "/appState",
        "an object",
        false,
        |value| matches!(value, JsonValue::Object(_)),
    )?;
    require_container(members, "files", "/files", "an object", false, |value| {
        matches!(value, JsonValue::Object(_))
    })?;
    Ok(())
}

/// Requires the root member `key`, when present or `required`, to be of the
/// expected container kind, which `expected` names for the message.
fn require_container(
    members: &[JsonMember],
    key: &str,
    pointer: &str,
    expected: &str,
    required: bool,
    accepts: impl FnOnce(&JsonValue) -> bool,
) -> Result<(), SceneParseError> {
    match member_value(members, key) {
        Some(value) if accepts(value) => Ok(()),
        Some(value) => Err(SceneParseError::invalid_envelope(
            pointer,
            format!(
                "scene document {key:?} must be {expected}, not {}",
                json_kind_phrase(value)
            ),
        )),
        None if !required => Ok(()),
        None => Err(SceneParseError::invalid_envelope(
            pointer,
            format!("scene document has no {key:?} member; expected {expected}"),
        )),
    }
}

/// The JSON kind of `value` as a message phrase: `an array`, `null`.
fn json_kind_phrase(value: &JsonValue) -> &'static str {
    match value {
        JsonValue::Null => "null",
        JsonValue::Bool(_) => "a boolean",
        JsonValue::Number(_) => "a number",
        JsonValue::String(_) => "a string",
        JsonValue::Array(_) => "an array",
        JsonValue::Object(_) => "an object",
    }
}

fn source_field<'a, T>(
    members: &'a [JsonMember],
    key: &str,
    parse: impl FnOnce(&'a JsonValue) -> Option<T>,
) -> SourceField<T> {
    match member_value(members, key) {
        None => SourceField::Missing,
        Some(value) => parse(value).map_or(SourceField::Invalid, SourceField::Value),
    }
}

fn source_roundness<'a>(
    members: &'a [JsonMember],
    key: &str,
) -> SourceField<Option<RoundnessSource<'a>>> {
    source_field(members, key, |value| match value {
        JsonValue::Null => Some(None),
        JsonValue::Object(members) => Some(Some(RoundnessSource { members })),
        _ => None,
    })
}

fn source_nullable_string<'a>(
    members: &'a [JsonMember],
    key: &str,
) -> SourceField<Option<&'a str>> {
    source_field(members, key, |value| match value {
        JsonValue::Null => Some(None),
        JsonValue::String(value) => Some(Some(value.as_ref())),
        _ => None,
    })
}

fn source_point_array<'a>(
    members: &'a [JsonMember],
    key: &str,
) -> SourceField<PointArraySource<'a>> {
    source_field(members, key, |value| {
        let JsonValue::Array(values) = value else {
            return None;
        };
        Some(PointArraySource { values })
    })
}

fn source_number_array<'a>(
    members: &'a [JsonMember],
    key: &str,
) -> SourceField<NumberArraySource<'a>> {
    source_field(members, key, |value| {
        let JsonValue::Array(values) = value else {
            return None;
        };
        Some(NumberArraySource { values })
    })
}

fn source_binding<'a>(
    members: &'a [JsonMember],
    key: &str,
) -> SourceField<Option<BindingSource<'a>>> {
    source_field(members, key, |value| match value {
        JsonValue::Null => Some(None),
        JsonValue::Object(members) => Some(Some(BindingSource { members })),
        _ => None,
    })
}

fn source_image_crop<'a>(
    members: &'a [JsonMember],
    key: &str,
) -> SourceField<Option<ImageCropSource<'a>>> {
    source_field(members, key, |value| match value {
        JsonValue::Null => Some(None),
        JsonValue::Object(members) => Some(Some(ImageCropSource { members })),
        _ => None,
    })
}

fn parse_point(value: &JsonValue) -> Option<[f64; 2]> {
    let JsonValue::Array(values) = value else {
        return None;
    };
    if values.len() < 2 {
        return None;
    }
    Some([
        finite_json_number(&values[0])?,
        finite_json_number(&values[1])?,
    ])
}

fn visit_json_value<E>(
    value: &JsonValue,
    json_pointer: &str,
    is_element_occurrence: bool,
    visit: &mut impl FnMut(OriginalJsonEntry<'_>) -> Result<(), E>,
) -> Result<(), OriginalJsonVisitError<E>> {
    let mut root_pointer = String::new();
    if root_pointer.try_reserve_exact(json_pointer.len()).is_err() {
        return Err(OriginalJsonVisitError::MemoryUnavailable {
            json_pointer: String::new(),
        });
    }
    root_pointer.push_str(json_pointer);

    let mut pending = Vec::new();
    if pending.try_reserve_exact(1).is_err() {
        return Err(OriginalJsonVisitError::MemoryUnavailable {
            json_pointer: root_pointer,
        });
    }
    pending.push(OriginalJsonVisit::Node {
        value,
        json_pointer: root_pointer,
        is_element_occurrence,
    });

    while let Some(entry) = pending.pop() {
        match entry {
            OriginalJsonVisit::Member {
                value,
                json_pointer,
            } => {
                visit(OriginalJsonEntry {
                    kind: OriginalJsonEntryKind::Member,
                    json_pointer: &json_pointer,
                    is_element_occurrence: false,
                })
                .map_err(OriginalJsonVisitError::Visitor)?;
                if pending.try_reserve(1).is_err() {
                    return Err(OriginalJsonVisitError::MemoryUnavailable { json_pointer });
                }
                pending.push(OriginalJsonVisit::Node {
                    value,
                    json_pointer,
                    is_element_occurrence: false,
                });
            }
            OriginalJsonVisit::Node {
                value,
                json_pointer,
                is_element_occurrence,
            } => {
                visit(OriginalJsonEntry {
                    kind: OriginalJsonEntryKind::Node,
                    json_pointer: &json_pointer,
                    is_element_occurrence,
                })
                .map_err(OriginalJsonVisitError::Visitor)?;

                match value {
                    JsonValue::Array(values) => {
                        if pending.try_reserve(values.len()).is_err() {
                            return Err(OriginalJsonVisitError::MemoryUnavailable { json_pointer });
                        }
                        let elements = json_pointer == "/elements";
                        for (index, value) in values.iter().enumerate().rev() {
                            let child_pointer = match pointer_index(&json_pointer, index) {
                                Ok(pointer) => pointer,
                                Err(_) => {
                                    return Err(OriginalJsonVisitError::MemoryUnavailable {
                                        json_pointer,
                                    });
                                }
                            };
                            pending.push(OriginalJsonVisit::Node {
                                value,
                                json_pointer: child_pointer,
                                is_element_occurrence: elements,
                            });
                        }
                    }
                    JsonValue::Object(members) => {
                        if pending.try_reserve(members.len()).is_err() {
                            return Err(OriginalJsonVisitError::MemoryUnavailable { json_pointer });
                        }
                        for member in members.iter().rev() {
                            let child_pointer = match pointer_child(&json_pointer, &member.key) {
                                Ok(pointer) => pointer,
                                Err(_) => {
                                    return Err(OriginalJsonVisitError::MemoryUnavailable {
                                        json_pointer,
                                    });
                                }
                            };
                            pending.push(OriginalJsonVisit::Member {
                                value: &member.value,
                                json_pointer: child_pointer,
                            });
                        }
                    }
                    JsonValue::Null
                    | JsonValue::Bool(_)
                    | JsonValue::Number(_)
                    | JsonValue::String(_) => {}
                }
            }
        }
    }
    Ok(())
}

enum OriginalJsonVisit<'a> {
    Node {
        value: &'a JsonValue,
        json_pointer: String,
        is_element_occurrence: bool,
    },
    Member {
        value: &'a JsonValue,
        json_pointer: String,
    },
}

#[cfg(test)]
mod tests {
    use super::SceneDocument;

    #[test]
    fn canonicalized_original_scene_document_uses_normative_json_spelling() {
        let source = br#"
        {
            "\u0074ype": "excalidraw",
            "version": 2,
            "elements": [
                null,
                true,
                false,
                {
                    "id": "deleted",
                    "isDeleted": true,
                    "unknown": "\/\u00e9\u2028\u2029\uD83D\uDE00",
                    "controls": "\u0000\b\t\n\f\r\u001F\"\\\/"
                }
            ],
            "appState": {"zoom": {"value": 1E+00}, "opaque": -0},
            "files": {
                "unused": {"mimeType": "image/png", "dataURL": "future"},
                "broken": false
            },
            "futureRoot": [3.00, 1e-2]
        }
        "#;
        let expected = concat!(
            r#"{"type":"excalidraw","version":2,"elements":[null,true,false,{"id":"deleted","isDeleted":true,"unknown":"/"#,
            "\u{00e9}\u{2028}\u{2029}\u{1f600}",
            r#"","controls":"\u0000\b\t\n\f\r\u001f\"\\/"}],"appState":{"zoom":{"value":1E+00},"opaque":-0},"files":{"unused":{"mimeType":"image/png","dataURL":"future"},"broken":false},"futureRoot":[3.00,1e-2]}"#,
        );

        let scene = SceneDocument::parse(source).expect("preservation vector is admitted");

        assert_eq!(
            scene
                .canonicalized_original_bytes()
                .expect("canonical scene serialization succeeds"),
            expected.as_bytes()
        );
    }

    #[test]
    fn canonicalized_original_scene_document_discards_whitespace_and_escape_spelling() {
        let escaped = br#" {
            "\u0074ype": "excalidraw",
            "version": 2,
            "elements": [{"label": "\u0061\/\u00e9"}],
            "appState": {},
            "files": {}
        } "#;
        let direct = concat!(
            r#"{"type":"excalidraw","version":2,"elements":[{"label":"a/"#,
            "\u{00e9}",
            r#""}],"appState":{},"files":{}}"#,
        );

        let escaped = SceneDocument::parse(escaped).expect("escaped vector is admitted");
        let direct_scene =
            SceneDocument::parse(direct.as_bytes()).expect("direct vector is admitted");

        assert_eq!(
            escaped
                .canonicalized_original_bytes()
                .expect("escaped scene serialization succeeds"),
            direct_scene
                .canonicalized_original_bytes()
                .expect("direct scene serialization succeeds")
        );
        assert_eq!(
            escaped
                .canonicalized_original_bytes()
                .expect("escaped scene serialization succeeds"),
            direct.as_bytes()
        );
    }

    #[test]
    fn canonicalized_original_scene_document_preserves_order_and_number_tokens() {
        let member_order_a = br#"{"type":"excalidraw","version":2,"elements":[],"appState":{"future":{"first":1,"second":2}},"files":{}}"#;
        let member_order_b = br#"{"type":"excalidraw","version":2,"elements":[],"appState":{"future":{"second":2,"first":1}},"files":{}}"#;
        let array_order_a =
            br#"{"type":"excalidraw","version":2,"elements":[1,2],"appState":{},"files":{}}"#;
        let array_order_b =
            br#"{"type":"excalidraw","version":2,"elements":[2,1],"appState":{},"files":{}}"#;
        let number_tokens = br#"{"type":"excalidraw","version":2,"elements":[0,-0,1,1.0,1e0,1E+00,10.00e-01],"appState":{},"files":{}}"#;

        let canonical = |source: &[u8]| {
            SceneDocument::parse(source)
                .expect("distinction vector is admitted")
                .canonicalized_original_bytes()
                .expect("distinction vector serialization succeeds")
        };

        assert_ne!(canonical(member_order_a), canonical(member_order_b));
        assert_eq!(canonical(member_order_a), member_order_a);
        assert_eq!(canonical(member_order_b), member_order_b);
        assert_ne!(canonical(array_order_a), canonical(array_order_b));
        assert_eq!(canonical(array_order_a), array_order_a);
        assert_eq!(canonical(array_order_b), array_order_b);
        assert_eq!(canonical(number_tokens), number_tokens);
    }
}
