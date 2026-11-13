//! The bounded, lossless JSON parser behind `SceneDocument::parse` and the
//! canonical writer behind the canonicalized original scene document (§4.2).
//!
//! `parse` admits one complete JSON text under the fixed §13.1 ceilings that
//! this file owns, keeping every number as its source token and every member
//! in source order; the envelope is the parent's concern. `JsonValue` is the
//! preserved tree, `member_value` and the typed scalar readers look into it,
//! `pointer_child` and `pointer_index` build RFC 6901 pointers, and
//! `write_canonical_json` and `visit_canonical_json` spell the tree the
//! normative way, into a buffer or to a visitor. Failures are
//! `SceneParseError`s because the parser's ceilings are the scene admission
//! contract.

use std::collections::{BTreeSet, TryReserveError};
use std::sync::Arc;

use crate::{ErrorCode, Measurement, ValueKind};

use super::SceneParseError;

const MAX_INPUT_BYTES: u64 = 67_108_864;
const MAX_JSON_DEPTH: u64 = 128;
const MAX_JSON_STRUCTURE_ENTRIES: u64 = 1_048_576;
const MAX_TOTAL_STRING_BYTES: u64 = 67_108_864;
const MAX_STRING_BYTES: u64 = 67_108_864;
const MAX_OBJECT_KEY_BYTES: u64 = 4 * 1024;
const MAX_NUMBER_TOKEN_BYTES: u64 = 128;
const MAX_PARSE_WORKING_BYTES: u64 = 256 * 1024 * 1024;

const SCALAR_NODE_BYTES: u64 = 64;
const CONTAINER_NODE_BYTES: u64 = 128;
const ACTIVE_FRAME_BYTES: u64 = 64;
const TOKEN_SCRATCH_BASE_BYTES: u64 = 64;
const ALLOCATION_QUANTUM_BYTES: u64 = 64;

const INPUT_BYTES_LIMIT: ParserLimit = ParserLimit {
    code: ErrorCode::SCENE_INPUT_TOO_LARGE,
    message: "scene document exceeds the fixed parser input ceiling",
    field: "max_input_bytes",
    configured: MAX_INPUT_BYTES,
    value_kind: ValueKind::Bytes,
};
const JSON_DEPTH_LIMIT: ParserLimit = ParserLimit {
    code: ErrorCode::SCENE_JSON_DEPTH_EXCEEDED,
    message: "JSON nesting exceeds the fixed parser depth ceiling",
    field: "max_json_depth",
    configured: MAX_JSON_DEPTH,
    value_kind: ValueKind::Depth,
};
const JSON_STRUCTURE_LIMIT: ParserLimit = ParserLimit {
    code: ErrorCode::SCENE_JSON_STRUCTURE_EXCEEDED,
    message: "JSON structure exceeds the fixed parser entry ceiling",
    field: "max_json_structure_entries",
    configured: MAX_JSON_STRUCTURE_ENTRIES,
    value_kind: ValueKind::Count,
};
const TOTAL_STRING_BYTES_LIMIT: ParserLimit = ParserLimit {
    code: ErrorCode::SCENE_STRING_TOO_LARGE,
    message: "decoded strings exceed the fixed aggregate ceiling",
    field: "max_total_string_bytes",
    configured: MAX_TOTAL_STRING_BYTES,
    value_kind: ValueKind::Bytes,
};
const STRING_BYTES_LIMIT: ParserLimit = ParserLimit {
    code: ErrorCode::SCENE_STRING_TOO_LARGE,
    message: "decoded string exceeds the fixed per-string ceiling",
    field: "max_string_bytes",
    configured: MAX_STRING_BYTES,
    value_kind: ValueKind::Bytes,
};
const OBJECT_KEY_BYTES_LIMIT: ParserLimit = ParserLimit {
    code: ErrorCode::SCENE_OBJECT_KEY_TOO_LARGE,
    message: "object key exceeds the fixed parser key ceiling",
    field: "max_object_key_bytes",
    configured: MAX_OBJECT_KEY_BYTES,
    value_kind: ValueKind::Bytes,
};
const NUMBER_TOKEN_BYTES_LIMIT: ParserLimit = ParserLimit {
    code: ErrorCode::SCENE_NUMBER_TOKEN_TOO_LARGE,
    message: "number token exceeds the fixed parser token ceiling",
    field: "max_number_token_bytes",
    configured: MAX_NUMBER_TOKEN_BYTES,
    value_kind: ValueKind::Bytes,
};
const PARSE_WORKING_BYTES_LIMIT: ParserLimit = ParserLimit {
    code: ErrorCode::SCENE_PARSE_MEMORY_EXCEEDED,
    message: "canonical parser live bytes exceed the fixed parser memory ceiling",
    field: "max_parse_working_bytes",
    configured: MAX_PARSE_WORKING_BYTES,
    value_kind: ValueKind::Bytes,
};

#[derive(Clone, Copy)]
pub(super) struct ParserLimit {
    pub(super) code: ErrorCode,
    pub(super) message: &'static str,
    pub(super) field: &'static str,
    pub(super) configured: u64,
    pub(super) value_kind: ValueKind,
}

/// Parses one complete JSON text under the fixed parser ceilings.
pub(super) fn parse(bytes: &[u8]) -> Result<JsonValue, SceneParseError> {
    let input_bytes = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    if input_bytes > MAX_INPUT_BYTES {
        return Err(SceneParseError::limit(
            INPUT_BYTES_LIMIT,
            Measurement::Attempted { value: input_bytes },
            None,
        ));
    }
    if std::str::from_utf8(bytes).is_err() {
        return Err(SceneParseError::new(
            ErrorCode::SCENE_INVALID_UTF8,
            "scene document is not valid UTF-8",
            None,
            None,
        ));
    }

    let mut parser = Parser::new(bytes);
    let root = parser.parse_value(1, false)?;
    parser.skip_whitespace();
    if parser.index != bytes.len() {
        return Err(parser.invalid_json(true));
    }
    Ok(root)
}

pub(crate) enum JsonValue {
    Null,
    Bool(bool),
    Number(Box<str>),
    String(Box<str>),
    Array(Box<[JsonValue]>),
    Object(Box<[JsonMember]>),
}

/// The kind of one JSON value, with container lengths, for tests that reason
/// about locations.
#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum JsonKind {
    Null,
    Bool,
    Number,
    String,
    Array(usize),
    Object(usize),
}

#[cfg(test)]
impl JsonValue {
    pub(crate) const fn kind(&self) -> JsonKind {
        match self {
            Self::Null => JsonKind::Null,
            Self::Bool(_) => JsonKind::Bool,
            Self::Number(_) => JsonKind::Number,
            Self::String(_) => JsonKind::String,
            Self::Array(values) => JsonKind::Array(values.len()),
            Self::Object(members) => JsonKind::Object(members.len()),
        }
    }
}

pub(crate) struct JsonMember {
    pub(crate) key: Arc<str>,
    pub(crate) value: JsonValue,
}

struct ParsedString {
    value: String,
    scratch_bytes: u64,
}

struct Parser<'a> {
    bytes: &'a [u8],
    index: usize,
    path: Vec<String>,
    structure_entries: u64,
    total_string_bytes: u64,
    working_bytes: u64,
}

impl<'a> Parser<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            index: 0,
            path: Vec::new(),
            structure_entries: 0,
            total_string_bytes: 0,
            working_bytes: 0,
        }
    }

    fn parse_value(
        &mut self,
        depth: u64,
        pointer_is_reliable: bool,
    ) -> Result<JsonValue, SceneParseError> {
        self.skip_whitespace();
        let pointer = pointer_is_reliable.then(|| self.pointer());

        match self.peek() {
            Some(b'{') => {
                let pointer = pointer.or_else(|| Some(self.pointer()));
                self.check_depth(depth, pointer.clone())?;
                self.admit_structure(pointer.clone())?;
                self.acquire(CONTAINER_NODE_BYTES, pointer.clone())?;
                self.parse_object(depth, pointer)
            }
            Some(b'[') => {
                let pointer = pointer.or_else(|| Some(self.pointer()));
                self.check_depth(depth, pointer.clone())?;
                self.admit_structure(pointer.clone())?;
                self.acquire(CONTAINER_NODE_BYTES, pointer.clone())?;
                self.parse_array(depth, pointer)
            }
            Some(b'"') => {
                let pointer = pointer.or_else(|| Some(self.pointer()));
                self.admit_structure(pointer.clone())?;
                self.acquire(SCALAR_NODE_BYTES, pointer.clone())?;
                let parsed = self.parse_string(false, pointer.clone())?;
                let storage_bytes = 64 + allocated_bytes(parsed.value.len());
                self.acquire(storage_bytes, pointer.clone())?;
                self.release(parsed.scratch_bytes);
                Ok(JsonValue::String(parsed.value.into_boxed_str()))
            }
            Some(b'-' | b'0'..=b'9') => {
                let pointer = pointer.or_else(|| Some(self.pointer()));
                self.admit_structure(pointer.clone())?;
                self.acquire(SCALAR_NODE_BYTES, pointer.clone())?;
                let value = self.parse_number(pointer.clone())?;
                Ok(JsonValue::Number(value))
            }
            Some(b't') => {
                let pointer = pointer.or_else(|| Some(self.pointer()));
                self.admit_structure(pointer.clone())?;
                self.acquire(SCALAR_NODE_BYTES, pointer.clone())?;
                self.parse_literal(b"true", pointer.clone())?;
                Ok(JsonValue::Bool(true))
            }
            Some(b'f') => {
                let pointer = pointer.or_else(|| Some(self.pointer()));
                self.admit_structure(pointer.clone())?;
                self.acquire(SCALAR_NODE_BYTES, pointer.clone())?;
                self.parse_literal(b"false", pointer.clone())?;
                Ok(JsonValue::Bool(false))
            }
            Some(b'n') => {
                let pointer = pointer.or_else(|| Some(self.pointer()));
                self.admit_structure(pointer.clone())?;
                self.acquire(SCALAR_NODE_BYTES, pointer.clone())?;
                self.parse_literal(b"null", pointer.clone())?;
                Ok(JsonValue::Null)
            }
            _ => Err(SceneParseError::invalid_json(pointer)),
        }
    }

    fn parse_object(
        &mut self,
        depth: u64,
        object_pointer: Option<String>,
    ) -> Result<JsonValue, SceneParseError> {
        self.index += 1;
        self.acquire(ACTIVE_FRAME_BYTES, object_pointer.clone())?;
        self.skip_whitespace();
        if self.consume(b'}') {
            self.release(ACTIVE_FRAME_BYTES);
            return Ok(JsonValue::Object(Box::new([])));
        }

        let mut members = Vec::new();
        let mut seen = BTreeSet::<Arc<str>>::new();
        loop {
            self.skip_whitespace();
            if self.peek() != Some(b'"') {
                return Err(SceneParseError::invalid_json(object_pointer));
            }

            let parsed_key = self.parse_string(true, object_pointer.clone())?;
            let key: Arc<str> = parsed_key.value.into();
            let member_pointer = self.pointer_with(&key);
            self.admit_structure(Some(member_pointer.clone()))?;
            if seen.contains(key.as_ref()) {
                return Err(SceneParseError::new(
                    ErrorCode::SCENE_DUPLICATE_OBJECT_KEY,
                    "scene document contains a duplicate object member",
                    Some(member_pointer),
                    None,
                ));
            }
            self.acquire(
                64 + allocated_bytes(key.len()),
                Some(member_pointer.clone()),
            )?;
            self.release(parsed_key.scratch_bytes);
            seen.insert(Arc::clone(&key));

            self.skip_whitespace();
            if !self.consume(b':') {
                return Err(SceneParseError::invalid_json(Some(member_pointer)));
            }

            self.path.push(key.to_string());
            let value = self.parse_value(depth + 1, true)?;
            self.require_value_delimiter(Some(self.pointer()))?;
            self.path.pop();
            members.push(JsonMember { key, value });

            self.skip_whitespace();
            if self.consume(b'}') {
                self.release(ACTIVE_FRAME_BYTES);
                return Ok(JsonValue::Object(members.into_boxed_slice()));
            }
            if !self.consume(b',') {
                return Err(SceneParseError::invalid_json(object_pointer));
            }
        }
    }

    fn parse_array(
        &mut self,
        depth: u64,
        array_pointer: Option<String>,
    ) -> Result<JsonValue, SceneParseError> {
        self.index += 1;
        self.acquire(ACTIVE_FRAME_BYTES, array_pointer.clone())?;
        self.skip_whitespace();
        if self.consume(b']') {
            self.release(ACTIVE_FRAME_BYTES);
            return Ok(JsonValue::Array(Box::new([])));
        }

        let mut values = Vec::new();
        let mut index = 0_u64;
        loop {
            self.skip_whitespace();
            if !self.next_token_starts_value() {
                return Err(SceneParseError::invalid_json(array_pointer.clone()));
            }
            self.path.push(index.to_string());
            let value = self.parse_value(depth + 1, true)?;
            self.require_value_delimiter(Some(self.pointer()))?;
            self.path.pop();
            values.push(value);

            self.skip_whitespace();
            if self.consume(b']') {
                self.release(ACTIVE_FRAME_BYTES);
                return Ok(JsonValue::Array(values.into_boxed_slice()));
            }
            if !self.consume(b',') {
                return Err(SceneParseError::invalid_json(array_pointer));
            }
            index += 1;
        }
    }

    fn parse_string(
        &mut self,
        is_key: bool,
        pointer: Option<String>,
    ) -> Result<ParsedString, SceneParseError> {
        self.index += 1;
        self.acquire(TOKEN_SCRATCH_BASE_BYTES, pointer.clone())?;
        let mut scratch_bytes = TOKEN_SCRATCH_BASE_BYTES;
        let mut value = String::new();

        loop {
            let byte = self
                .peek()
                .ok_or_else(|| SceneParseError::invalid_json(pointer.clone()))?;
            match byte {
                b'"' => {
                    self.index += 1;
                    return Ok(ParsedString {
                        value,
                        scratch_bytes,
                    });
                }
                b'\\' => {
                    self.index += 1;
                    let escape = self
                        .peek()
                        .ok_or_else(|| SceneParseError::invalid_json(pointer.clone()))?;
                    self.index += 1;
                    let character = match escape {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{0008}',
                        b'f' => '\u{000c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => self.parse_unicode_escape(pointer.clone())?,
                        _ => return Err(SceneParseError::invalid_json(pointer)),
                    };
                    self.push_string_character(
                        &mut value,
                        character,
                        is_key,
                        &mut scratch_bytes,
                        pointer.clone(),
                    )?;
                }
                0x00..=0x1f => return Err(SceneParseError::invalid_json(pointer)),
                _ => {
                    let start = self.index;
                    let run_limit = self.direct_string_run_limit(value.len(), is_key);
                    while self.index - start < run_limit
                        && matches!(self.peek(), Some(byte) if byte >= 0x20 && byte != b'"' && byte != b'\\')
                    {
                        self.index += 1;
                    }
                    while matches!(self.peek(), Some(0x80..=0xbf)) {
                        self.index += 1;
                    }
                    self.push_string_source_run(
                        &mut value,
                        start,
                        self.index,
                        is_key,
                        &mut scratch_bytes,
                        pointer.clone(),
                    )?;
                }
            }
        }
    }

    fn direct_string_run_limit(&self, current_string_bytes: usize, is_key: bool) -> usize {
        let current_string_bytes =
            u64::try_from(current_string_bytes).expect("parser input length fits in u64");
        let mut limit = MAX_STRING_BYTES - current_string_bytes + 1;
        limit = limit.min(MAX_TOTAL_STRING_BYTES - self.total_string_bytes + 1);
        if is_key {
            limit = limit.min(MAX_OBJECT_KEY_BYTES - current_string_bytes + 1);
        }
        usize::try_from(limit).expect("fixed parser string limits fit in usize")
    }

    fn parse_unicode_escape(&mut self, pointer: Option<String>) -> Result<char, SceneParseError> {
        let first = self.parse_hex_quad(pointer.clone())?;
        let scalar = if (0xd800..=0xdbff).contains(&first) {
            if self.bytes.get(self.index..self.index + 2) != Some(b"\\u") {
                return Err(SceneParseError::invalid_json(pointer));
            }
            self.index += 2;
            let second = self.parse_hex_quad(pointer.clone())?;
            if !(0xdc00..=0xdfff).contains(&second) {
                return Err(SceneParseError::invalid_json(pointer));
            }
            0x1_0000 + ((u32::from(first) - 0xd800) << 10) + (u32::from(second) - 0xdc00)
        } else if (0xdc00..=0xdfff).contains(&first) {
            return Err(SceneParseError::invalid_json(pointer));
        } else {
            u32::from(first)
        };

        char::from_u32(scalar).ok_or_else(|| SceneParseError::invalid_json(pointer))
    }

    fn parse_hex_quad(&mut self, pointer: Option<String>) -> Result<u16, SceneParseError> {
        let mut value = 0_u16;
        for _ in 0..4 {
            let digit = self
                .peek()
                .and_then(hex_value)
                .ok_or_else(|| SceneParseError::invalid_json(pointer.clone()))?;
            self.index += 1;
            value = value * 16 + u16::from(digit);
        }
        Ok(value)
    }

    fn push_string_character(
        &mut self,
        value: &mut String,
        character: char,
        is_key: bool,
        scratch_bytes: &mut u64,
        pointer: Option<String>,
    ) -> Result<(), SceneParseError> {
        self.admit_decoded_string_bytes(
            value.len(),
            character.len_utf8(),
            is_key,
            scratch_bytes,
            pointer,
        )?;
        value.push(character);
        Ok(())
    }

    fn push_string_source_run(
        &mut self,
        value: &mut String,
        start: usize,
        end: usize,
        is_key: bool,
        scratch_bytes: &mut u64,
        pointer: Option<String>,
    ) -> Result<(), SceneParseError> {
        self.admit_decoded_string_bytes(value.len(), end - start, is_key, scratch_bytes, pointer)?;
        value.push_str(
            std::str::from_utf8(&self.bytes[start..end])
                .expect("input UTF-8 was validated before parsing"),
        );
        Ok(())
    }

    fn admit_decoded_string_bytes(
        &mut self,
        current_string_bytes: usize,
        added_bytes: usize,
        is_key: bool,
        scratch_bytes: &mut u64,
        pointer: Option<String>,
    ) -> Result<(), SceneParseError> {
        let decoded_bytes = u64::try_from(current_string_bytes + added_bytes)
            .expect("parser input length fits in u64");

        if is_key && decoded_bytes > MAX_OBJECT_KEY_BYTES {
            return Err(SceneParseError::limit(
                OBJECT_KEY_BYTES_LIMIT,
                Measurement::AtLeast {
                    value: decoded_bytes,
                },
                pointer,
            ));
        }
        if decoded_bytes > MAX_STRING_BYTES {
            return Err(SceneParseError::limit(
                STRING_BYTES_LIMIT,
                Measurement::AtLeast {
                    value: decoded_bytes,
                },
                pointer,
            ));
        }

        let added_bytes = u64::try_from(added_bytes).expect("parser input length fits in u64");
        let total_string_bytes = self.total_string_bytes + added_bytes;
        if total_string_bytes > MAX_TOTAL_STRING_BYTES {
            return Err(SceneParseError::limit(
                TOTAL_STRING_BYTES_LIMIT,
                Measurement::AtLeast {
                    value: total_string_bytes,
                },
                pointer,
            ));
        }

        let next_scratch_bytes = TOKEN_SCRATCH_BASE_BYTES + allocated_bytes(decoded_bytes as usize);
        let scratch_growth = next_scratch_bytes - *scratch_bytes;
        if self.working_bytes.saturating_add(scratch_growth) > MAX_PARSE_WORKING_BYTES {
            let first_crossing_growth =
                ((MAX_PARSE_WORKING_BYTES - self.working_bytes) / ALLOCATION_QUANTUM_BYTES + 1)
                    * ALLOCATION_QUANTUM_BYTES;
            return Err(SceneParseError::limit(
                PARSE_WORKING_BYTES_LIMIT,
                Measurement::Attempted {
                    value: self.working_bytes + first_crossing_growth,
                },
                pointer,
            ));
        }
        self.working_bytes += scratch_growth;
        *scratch_bytes = next_scratch_bytes;
        self.total_string_bytes = total_string_bytes;
        Ok(())
    }

    fn parse_number(&mut self, pointer: Option<String>) -> Result<Box<str>, SceneParseError> {
        let start = self.index;
        self.acquire(TOKEN_SCRATCH_BASE_BYTES, pointer.clone())?;
        let mut token_bytes = 0_u64;
        let mut scratch_bytes = TOKEN_SCRATCH_BASE_BYTES;

        if self.peek() == Some(b'-') {
            self.consume_number_byte(&mut token_bytes, &mut scratch_bytes, pointer.clone())?;
        }

        match self.peek() {
            Some(b'0') => {
                self.consume_number_byte(&mut token_bytes, &mut scratch_bytes, pointer.clone())?;
            }
            Some(b'1'..=b'9') => {
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.consume_number_byte(
                        &mut token_bytes,
                        &mut scratch_bytes,
                        pointer.clone(),
                    )?;
                }
            }
            _ => return Err(SceneParseError::invalid_json(pointer)),
        }

        if self.peek() == Some(b'.') {
            if !matches!(self.bytes.get(self.index + 1), Some(b'0'..=b'9')) {
                return Err(SceneParseError::invalid_json(pointer));
            }
            self.consume_number_byte(&mut token_bytes, &mut scratch_bytes, pointer.clone())?;
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.consume_number_byte(&mut token_bytes, &mut scratch_bytes, pointer.clone())?;
            }
        }

        if matches!(self.peek(), Some(b'e' | b'E')) {
            let mut first_exponent_digit = self.index + 1;
            if matches!(self.bytes.get(first_exponent_digit), Some(b'+' | b'-')) {
                first_exponent_digit += 1;
            }
            if !matches!(self.bytes.get(first_exponent_digit), Some(b'0'..=b'9')) {
                return Err(SceneParseError::invalid_json(pointer));
            }
            self.consume_number_byte(&mut token_bytes, &mut scratch_bytes, pointer.clone())?;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.consume_number_byte(&mut token_bytes, &mut scratch_bytes, pointer.clone())?;
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.consume_number_byte(&mut token_bytes, &mut scratch_bytes, pointer.clone())?;
            }
        }

        self.acquire(64 + allocated_bytes(token_bytes as usize), pointer)?;
        self.release(scratch_bytes);
        Ok(std::str::from_utf8(&self.bytes[start..self.index])
            .expect("JSON number tokens are ASCII")
            .into())
    }

    fn consume_number_byte(
        &mut self,
        token_bytes: &mut u64,
        scratch_bytes: &mut u64,
        pointer: Option<String>,
    ) -> Result<(), SceneParseError> {
        self.index += 1;
        *token_bytes += 1;
        if *token_bytes > MAX_NUMBER_TOKEN_BYTES {
            return Err(SceneParseError::limit(
                NUMBER_TOKEN_BYTES_LIMIT,
                Measurement::AtLeast {
                    value: *token_bytes,
                },
                pointer,
            ));
        }

        let next_scratch_bytes = TOKEN_SCRATCH_BASE_BYTES + allocated_bytes(*token_bytes as usize);
        self.acquire(next_scratch_bytes - *scratch_bytes, pointer)?;
        *scratch_bytes = next_scratch_bytes;
        Ok(())
    }

    fn parse_literal(
        &mut self,
        expected: &[u8],
        pointer: Option<String>,
    ) -> Result<(), SceneParseError> {
        if self.bytes.get(self.index..self.index + expected.len()) != Some(expected) {
            return Err(SceneParseError::invalid_json(pointer));
        }
        self.index += expected.len();
        Ok(())
    }

    fn require_value_delimiter(&self, pointer: Option<String>) -> Result<(), SceneParseError> {
        if matches!(
            self.peek(),
            None | Some(b' ' | b'\t' | b'\n' | b'\r' | b',' | b']' | b'}')
        ) {
            Ok(())
        } else {
            Err(SceneParseError::invalid_json(pointer))
        }
    }

    fn check_depth(&self, depth: u64, pointer: Option<String>) -> Result<(), SceneParseError> {
        if depth <= MAX_JSON_DEPTH {
            return Ok(());
        }
        Err(SceneParseError::limit(
            JSON_DEPTH_LIMIT,
            Measurement::Attempted { value: depth },
            pointer,
        ))
    }

    fn admit_structure(&mut self, pointer: Option<String>) -> Result<(), SceneParseError> {
        let attempted = self.structure_entries + 1;
        if attempted > MAX_JSON_STRUCTURE_ENTRIES {
            return Err(SceneParseError::limit(
                JSON_STRUCTURE_LIMIT,
                Measurement::Attempted { value: attempted },
                pointer,
            ));
        }
        self.structure_entries = attempted;
        Ok(())
    }

    fn acquire(&mut self, bytes: u64, pointer: Option<String>) -> Result<(), SceneParseError> {
        let attempted = self.working_bytes.saturating_add(bytes);
        if attempted > MAX_PARSE_WORKING_BYTES {
            return Err(SceneParseError::limit(
                PARSE_WORKING_BYTES_LIMIT,
                Measurement::Attempted { value: attempted },
                pointer,
            ));
        }
        self.working_bytes = attempted;
        Ok(())
    }

    fn release(&mut self, bytes: u64) {
        self.working_bytes -= bytes;
    }

    fn invalid_json(&self, pointer_is_reliable: bool) -> SceneParseError {
        SceneParseError::invalid_json(pointer_is_reliable.then(|| self.pointer()))
    }

    fn pointer(&self) -> String {
        let mut pointer = String::new();
        for segment in &self.path {
            pointer.push('/');
            push_pointer_segment(&mut pointer, segment);
        }
        pointer
    }

    fn pointer_with(&self, segment: &str) -> String {
        let mut pointer = self.pointer();
        pointer.push('/');
        push_pointer_segment(&mut pointer, segment);
        pointer
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.index += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.index).copied()
    }

    fn next_token_starts_value(&self) -> bool {
        matches!(
            self.peek(),
            Some(b'{' | b'[' | b'"' | b'-' | b'0'..=b'9' | b't' | b'f' | b'n')
        )
    }

    fn consume(&mut self, expected: u8) -> bool {
        if self.peek() == Some(expected) {
            self.index += 1;
            true
        } else {
            false
        }
    }
}

pub(crate) fn member_value<'a>(members: &'a [JsonMember], key: &str) -> Option<&'a JsonValue> {
    members
        .iter()
        .find(|member| &*member.key == key)
        .map(|member| &member.value)
}

pub(super) fn json_string(value: &JsonValue) -> Option<&str> {
    let JsonValue::String(value) = value else {
        return None;
    };
    Some(value)
}

pub(super) fn json_bool(value: &JsonValue) -> Option<bool> {
    let JsonValue::Bool(value) = value else {
        return None;
    };
    Some(*value)
}

pub(crate) fn finite_json_number(value: &JsonValue) -> Option<f64> {
    let JsonValue::Number(value) = value else {
        return None;
    };
    value
        .parse::<f64>()
        .ok()
        .filter(|number| number.is_finite())
}

pub(super) fn json_i32(value: &JsonValue) -> Option<i32> {
    let JsonValue::Number(value) = value else {
        return None;
    };
    value.parse().ok()
}

pub(super) fn json_u64(value: &JsonValue) -> Option<u64> {
    let JsonValue::Number(value) = value else {
        return None;
    };
    value.parse().ok()
}

pub(crate) fn pointer_child(parent: &str, segment: &str) -> Result<String, TryReserveError> {
    let escaped_bytes = segment.len().saturating_mul(2);
    let capacity = parent
        .len()
        .checked_add(escaped_bytes)
        .and_then(|length| length.checked_add(1))
        .unwrap_or(usize::MAX);
    let mut pointer = String::new();
    pointer.try_reserve(capacity)?;
    pointer.push_str(parent);
    pointer.push('/');
    push_pointer_segment(&mut pointer, segment);
    Ok(pointer)
}

pub(crate) fn pointer_index(parent: &str, index: usize) -> Result<String, TryReserveError> {
    let capacity = parent.len().saturating_add(21);
    let mut pointer = String::new();
    pointer.try_reserve(capacity)?;
    pointer.push_str(parent);
    pointer.push('/');

    let mut digits = [0_u8; 20];
    let mut remaining = index;
    let mut start = digits.len();
    loop {
        start -= 1;
        digits[start] = u8::try_from(remaining % 10).unwrap_or(0);
        remaining /= 10;
        if remaining == 0 {
            break;
        }
    }
    for digit in &digits[start..] {
        pointer.push(char::from(b'0' + *digit));
    }
    Ok(pointer)
}

pub(super) fn write_canonical_json(
    value: &JsonValue,
    bytes: &mut Vec<u8>,
) -> Result<(), TryReserveError> {
    let mut result = Ok(());
    visit_canonical_json(value, &mut |chunk| {
        if result.is_ok() {
            result = bytes
                .try_reserve(chunk.len())
                .map(|()| bytes.extend_from_slice(chunk));
        }
    });
    result
}

pub(super) fn visit_canonical_json(value: &JsonValue, visitor: &mut impl FnMut(&[u8])) {
    match value {
        JsonValue::Null => visitor(b"null"),
        JsonValue::Bool(true) => visitor(b"true"),
        JsonValue::Bool(false) => visitor(b"false"),
        JsonValue::Number(number) => visitor(number.as_bytes()),
        JsonValue::String(string) => visit_canonical_string(string, visitor),
        JsonValue::Array(values) => {
            visitor(b"[");
            for (index, value) in values.iter().enumerate() {
                if index != 0 {
                    visitor(b",");
                }
                visit_canonical_json(value, visitor);
            }
            visitor(b"]");
        }
        JsonValue::Object(members) => {
            visitor(b"{");
            for (index, member) in members.iter().enumerate() {
                if index != 0 {
                    visitor(b",");
                }
                visit_canonical_string(&member.key, visitor);
                visitor(b":");
                visit_canonical_json(&member.value, visitor);
            }
            visitor(b"}");
        }
    }
}

fn visit_canonical_string(value: &str, visitor: &mut impl FnMut(&[u8])) {
    const HEX: &[u8; 16] = b"0123456789abcdef";

    visitor(b"\"");
    for character in value.chars() {
        match character {
            '"' => visitor(br#"\""#),
            '\\' => visitor(br#"\\"#),
            '\u{0008}' => visitor(br"\b"),
            '\t' => visitor(br"\t"),
            '\n' => visitor(br"\n"),
            '\u{000c}' => visitor(br"\f"),
            '\r' => visitor(br"\r"),
            '\u{0000}'..='\u{001f}' => {
                let control = character as usize;
                visitor(br"\u00");
                visitor(&HEX[control >> 4..=control >> 4]);
                let low = control & 0x0f;
                visitor(&HEX[low..=low]);
            }
            _ => {
                let mut encoded = [0; 4];
                visitor(character.encode_utf8(&mut encoded).as_bytes());
            }
        }
    }
    visitor(b"\"");
}

fn allocated_bytes(bytes: usize) -> u64 {
    if bytes == 0 {
        0
    } else {
        let bytes = u64::try_from(bytes).unwrap_or(u64::MAX);
        bytes.saturating_add(ALLOCATION_QUANTUM_BYTES - 1) / ALLOCATION_QUANTUM_BYTES
            * ALLOCATION_QUANTUM_BYTES
    }
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn push_pointer_segment(pointer: &mut String, segment: &str) {
    for character in segment.chars() {
        match character {
            '~' => pointer.push_str("~0"),
            '/' => pointer.push_str("~1"),
            character => pointer.push(character),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{parse, write_canonical_json, JsonKind, JsonValue, SceneParseError};
    use crate::ErrorCode;

    fn canonical(value: &JsonValue) -> Vec<u8> {
        let mut bytes = Vec::new();
        write_canonical_json(value, &mut bytes).expect("canonical serialization succeeds");
        bytes
    }

    fn rejection(bytes: &[u8]) -> SceneParseError {
        match parse(bytes) {
            Ok(_) => panic!("{} is admitted", String::from_utf8_lossy(bytes)),
            Err(error) => error,
        }
    }

    #[test]
    fn parses_any_json_text_without_a_scene_envelope() {
        let root = parse(br#" [ 1.50 , "a\u0062" , { "c" : null , "d" : [ true , false ] } ] "#)
            .expect("a bare JSON array is admitted");

        assert_eq!(root.kind(), JsonKind::Array(3));
        assert_eq!(
            canonical(&root),
            br#"[1.50,"ab",{"c":null,"d":[true,false]}]"#
        );
    }

    #[test]
    fn rejects_a_duplicate_object_member_at_its_pointer() {
        let error = rejection(br#"{"outer":{"a":1,"a":2}}"#);

        assert_eq!(error.code(), ErrorCode::SCENE_DUPLICATE_OBJECT_KEY);
        assert_eq!(error.json_pointer(), Some("/outer/a"));
    }

    #[test]
    fn rejects_trailing_content_and_invalid_utf8() {
        assert_eq!(rejection(b"{} x").code(), ErrorCode::SCENE_INVALID_JSON);
        assert_eq!(rejection(b"\"\xff\"").code(), ErrorCode::SCENE_INVALID_UTF8);
    }
}
