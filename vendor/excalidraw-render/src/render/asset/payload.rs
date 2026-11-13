//! Asset payload access: the bytes one `fileId` names in the scene's file
//! records (a `data:` URL whose base64 body is sized and validated before it is
//! decoded) or from the caller's supplemental assets, each bounded by the
//! request's asset byte ceiling, together with the media-type claims the
//! diagnostics compare against the sniffed kind.

use base64::Engine as _;

use crate::scene::SourceField;
use crate::{SceneDocument, SupplementalAssets};

use super::{try_copy, try_owned, Authority, Claims, ResolveError};

pub(super) struct Payload {
    pub(super) bytes: Vec<u8>,
    pub(super) claims: Claims,
}

pub(super) fn payload(
    scene: &SceneDocument,
    supplemental: &SupplementalAssets,
    file_id: &str,
    authority: Authority,
    max_asset_bytes: u64,
) -> Result<Payload, ResolveError> {
    match authority {
        Authority::Scene => scene_payload(scene, file_id, max_asset_bytes),
        Authority::Supplemental => {
            let asset = supplemental.get(file_id).ok_or(ResolveError::Missing)?;
            let attempted = u64::try_from(asset.bytes().len()).unwrap_or(u64::MAX);
            if attempted > max_asset_bytes {
                return Err(ResolveError::AssetBytes { attempted });
            }
            Ok(Payload {
                bytes: try_copy(asset.bytes())?,
                claims: Claims {
                    mime_type: asset.advisory_media_type().map(try_owned).transpose()?,
                    data_url_media_type: None,
                },
            })
        }
        Authority::Missing => Err(ResolveError::Missing),
    }
}

pub(crate) fn data_url_decoded_len(data_url: &str) -> u64 {
    let Some((_, encoded)) = split_data_url(data_url) else {
        return 0;
    };
    decoded_prefix(encoded).0
}

fn scene_payload(
    scene: &SceneDocument,
    file_id: &str,
    max_asset_bytes: u64,
) -> Result<Payload, ResolveError> {
    let record = (0..scene.file_record_count())
        .filter_map(|index| scene.file_record_at(index))
        .find(|record| record.key() == file_id)
        .ok_or(ResolveError::Missing)?;
    let SourceField::Value(data_url) = record.data_url() else {
        return Err(ResolveError::Malformed);
    };
    if !data_url.starts_with("data:") {
        return Err(ResolveError::Forbidden);
    }
    let (media_type, encoded) = split_data_url(data_url).ok_or(ResolveError::Malformed)?;
    let (attempted, valid) = decoded_prefix(encoded);
    if attempted > max_asset_bytes {
        return Err(ResolveError::AssetBytes { attempted });
    }
    if !valid {
        return Err(ResolveError::Malformed);
    }
    let estimate = usize::try_from(attempted).map_err(|_| ResolveError::Memory)?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(estimate)
        .map_err(|_| ResolveError::Memory)?;
    bytes.resize(estimate, 0);
    let length = base64::engine::general_purpose::STANDARD
        .decode_slice(encoded, &mut bytes)
        .map_err(|_| ResolveError::Malformed)?;
    bytes.truncate(length);
    Ok(Payload {
        bytes,
        claims: Claims {
            mime_type: match record.mime_type() {
                SourceField::Value(value) => Some(try_owned(value)?),
                SourceField::Missing | SourceField::Invalid => None,
            },
            data_url_media_type: Some(try_owned(media_type)?),
        },
    })
}

fn split_data_url(data_url: &str) -> Option<(&str, &str)> {
    data_url.strip_prefix("data:")?.split_once(";base64,")
}

pub(super) fn decoded_prefix(encoded: &str) -> (u64, bool) {
    let mut decoded = 0_u64;
    let quanta = encoded.as_bytes().chunks(4);
    let quantum_count = quanta.len();
    for (index, quantum) in quanta.enumerate() {
        if quantum.len() != 4 {
            return (decoded, false);
        }
        if base64_value(quantum[0]).is_none() {
            return (decoded, false);
        }
        let Some(second) = base64_value(quantum[1]) else {
            return (decoded, false);
        };
        let (bytes, terminal) = match (quantum[2], quantum[3]) {
            (b'=', b'=') if second & 0x0f == 0 => (1, true),
            (third, b'=') if base64_value(third).is_some_and(|value| value & 0x03 == 0) => {
                (2, true)
            }
            (third, fourth) if base64_value(third).is_some() && base64_value(fourth).is_some() => {
                (3, false)
            }
            _ => return (decoded, false),
        };
        decoded = decoded.saturating_add(bytes);
        if terminal {
            return (decoded, index + 1 == quantum_count);
        }
    }
    (decoded, true)
}

const fn base64_value(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}
