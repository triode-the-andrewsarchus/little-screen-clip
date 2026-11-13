use std::collections::BTreeMap;

use crate::fonts::FontNotice;

#[derive(Debug)]
pub(super) struct MandatoryNotices(String);

pub(super) enum NoticeError {
    /// Only PNG resolves notices from fallback kinds, which can find none.
    #[cfg(feature = "png")]
    InvalidProvenance,
    Memory,
}

impl MandatoryNotices {
    pub(super) fn copy_from(value: &Self) -> Result<Self, NoticeError> {
        Ok(Self(try_owned(value.as_json())?))
    }
    /// Nested-limit and forbidden-reference recipes share the error-image
    /// glyphs and their two Font Awesome notices.
    pub(super) fn with_error_image_recipe(
        existing: Option<Self>,
        format: crate::OutputFormat,
    ) -> Result<Self, NoticeError> {
        let mut json = existing.map_or_else(|| String::from("[]"), |notice| notice.0);
        let notices = crate::catalog::fallback_notices(&["nested-resource"], format)
            .map_err(|_| NoticeError::Memory)?;
        for notice in notices {
            // Only our canonical notice writer emits this marker; escaped text
            // cannot contain it. Merge sorted entries without reparsing JSON or
            // allocating an unbounded serde tree.
            const MARKER: &str = "{\"asset_identity\":\"";
            let mut insertion = json.len().saturating_sub(1);
            let mut present = false;
            for (offset, tail) in json.match_indices(MARKER) {
                let _ = tail;
                let identity = json[offset + MARKER.len()..]
                    .split('"')
                    .next()
                    .unwrap_or("");
                if identity == notice.asset_identity {
                    present = true;
                    break;
                }
                if identity > notice.asset_identity {
                    insertion = offset;
                    break;
                }
            }
            if present {
                continue;
            }
            let mut entry = String::new();
            entry
                .try_reserve_exact(
                    notice
                        .asset_identity
                        .len()
                        .saturating_add(notice.text.len().saturating_mul(6))
                        .saturating_add(64),
                )
                .map_err(|_| NoticeError::Memory)?;
            let append = insertion == json.len() - 1;
            if append && json.len() > 2 {
                entry.push(',');
            }
            push_notice(&mut entry, &mut "", notice.asset_identity, notice.text);
            if !append {
                entry.push(',');
            }
            json.try_reserve(entry.len())
                .map_err(|_| NoticeError::Memory)?;
            json.insert_str(insertion, &entry);
        }
        Ok(Self(json))
    }
    #[cfg(test)]
    pub(super) fn from_json(json: String) -> Self {
        Self(json)
    }

    pub(super) fn as_json(&self) -> &str {
        &self.0
    }

    #[cfg(feature = "png")]
    pub(super) fn for_fallbacks(
        fallback_kinds: &[&str],
        format: crate::OutputFormat,
    ) -> Result<Self, NoticeError> {
        let notices = crate::catalog::fallback_notices(fallback_kinds, format)
            .map_err(|_| NoticeError::Memory)?;
        if notices.is_empty() {
            return Err(NoticeError::InvalidProvenance);
        }
        let mut manifest = String::new();
        manifest
            .try_reserve_exact(1_024)
            .map_err(|_| NoticeError::Memory)?;
        manifest.push('[');
        let mut separator = "";
        for notice in notices {
            push_notice(
                &mut manifest,
                &mut separator,
                notice.asset_identity,
                notice.text,
            );
        }
        manifest.push(']');
        Ok(Self(manifest))
    }
}

/// The font notices one artifact carries, keyed by the identity each object
/// is emitted under, so the manifest lists them sorted by identity (spec 12)
/// and carries every identity once (#151): a family attribution under its
/// license identity, a license body under its terms identity, shared by every
/// family licensed under that text, and a subset notice under its semantic
/// subset identity.
#[derive(Default)]
pub(super) struct FontNoticeSet {
    notices: BTreeMap<String, String>,
}

impl FontNoticeSet {
    /// Adds the family's attribution notice and the license body it refers
    /// to; a family or body already present is not repeated.
    pub(super) fn add_family(&mut self, notice: &FontNotice) -> Result<(), NoticeError> {
        if !self.notices.contains_key(notice.license_identity) {
            let attribution = notice
                .attribution_notice()
                .map_err(|_| NoticeError::Memory)?;
            self.insert(notice.license_identity, attribution)?;
        }
        if !self.notices.contains_key(notice.terms_identity) {
            self.insert(notice.terms_identity, try_owned(notice.terms)?)?;
        }
        Ok(())
    }

    /// Adds one subset's Modified Version notice under its semantic identity.
    pub(super) fn add_subset(&mut self, identity: String, text: String) {
        self.notices.insert(identity, text);
    }

    pub(super) fn is_empty(&self) -> bool {
        self.notices.is_empty()
    }

    /// The `notices` array as canonical JSON.
    pub(super) fn into_manifest(self) -> Result<MandatoryNotices, NoticeError> {
        let mut json = String::new();
        json.try_reserve_exact(
            self.notices
                .iter()
                .fold(2_usize, |total, (identity, text)| {
                    total
                        .saturating_add(identity.len())
                        .saturating_add(text.len())
                        .saturating_add(48)
                }),
        )
        .map_err(|_| NoticeError::Memory)?;
        json.push('[');
        let mut separator = "";
        for (identity, text) in &self.notices {
            push_notice(&mut json, &mut separator, identity, text);
        }
        json.push(']');
        Ok(MandatoryNotices(json))
    }

    fn insert(&mut self, identity: &str, text: String) -> Result<(), NoticeError> {
        self.notices.insert(try_owned(identity)?, text);
        Ok(())
    }
}

fn try_owned(value: &str) -> Result<String, NoticeError> {
    let mut owned = String::new();
    owned
        .try_reserve_exact(value.len())
        .map_err(|_| NoticeError::Memory)?;
    owned.push_str(value);
    Ok(owned)
}

fn push_notice(json: &mut String, separator: &mut &'static str, identity: &str, text: &str) {
    json.push_str(separator);
    json.push_str("{\"asset_identity\":\"");
    push_json_string_content(json, identity);
    json.push_str("\",\"text\":\"");
    push_json_string_content(json, text);
    json.push_str("\"}");
    *separator = ",";
}

pub(super) fn push_json_string_content(output: &mut String, value: &str) {
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\u{08}' => output.push_str("\\b"),
            '\u{0c}' => output.push_str("\\f"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character < '\u{20}' => {
                const HEX: &[u8; 16] = b"0123456789abcdef";
                let value = character as u32;
                output.push_str("\\u00");
                output.push(char::from(HEX[((value >> 4) & 0x0f) as usize]));
                output.push(char::from(HEX[(value & 0x0f) as usize]));
            }
            character => output.push(character),
        }
    }
}
