//! The crate-local interface to bundled font metadata and verified loading.
//! Only this module knows which external crate supplies the catalog. Exports
//! are explicit: renderer consumers use `crate::fonts`, never the dependency.
//! Excalidraw selection policy remains independent of physical font storage.
//! A loader is local to its caller's preparation scope. It caches physical
//! decodes only: fallback probes, primary-face choices, and SVG font demand
//! remain with the renderer and do not depend on cache hits.

use std::collections::TryReserveError;
use std::sync::Arc;

pub(crate) use excalidraw_fonts::fonts::{Container, Face, FaceId, FamilyId};
pub(crate) use excalidraw_fonts::{
    font_subset_profile_digest_v1, is_cjk_scalar_v1, revision_1, FontNotice, FontSubsetTableAction,
    FontSubsetTableRule, RevisionCatalog as BundledCatalog, FONT_SUBSET_TABLE_RULES_V1,
};
use excalidraw_fonts::{AssetIntegrityError, VerifiedAsset};

// Qualification exercises exact source programs and manifests, rather than
// renderer selection. Keep that inspection surface out of production callers.
#[cfg(test)]
pub(crate) mod inspection {
    pub(crate) use excalidraw_fonts::{
        font_face_order, font_face_unicode_ranges, load_asset, load_noto_emoji_sfnt,
        AssetIntegrityError, AssetRecord, FontLicense, VerifiedAsset, CJK_SCALAR_RANGES_V1,
        NOTO_EMOJI_FAMILY, NOTO_EMOJI_IDENTITY,
    };
}

pub(crate) fn find_face(upstream_path: &str) -> Option<&'static Face> {
    excalidraw_fonts::fonts::find_face(upstream_path)
}

/// The reviewed Latin face used by the isolated nested-SVG viewer.
pub(crate) fn excalifont_latin() -> Option<&'static Face> {
    find_face(excalidraw_fonts::excalifont_basic_latin_sfnt_transformation().source_path)
}

pub(crate) fn font_notice(identity: &str) -> Option<&'static FontNotice> {
    if identity == excalidraw_fonts::NOTO_EMOJI_IDENTITY {
        return FamilyId::NotoEmoji.notice();
    }
    let (path, _) = identity.split_once("@sha256:")?;
    let face = find_face(path)?;
    (face.identity() == identity)
        .then(|| face.family().notice())
        .flatten()
}

/// The family of a manifested identity, resolved as exactly as `font_notice`
/// resolves it; the complete Noto Emoji face is its own family.
#[cfg(feature = "pdf")]
pub(crate) fn font_family(identity: &str) -> Option<FamilyId> {
    if identity == excalidraw_fonts::NOTO_EMOJI_IDENTITY {
        return Some(FamilyId::NotoEmoji);
    }
    let (path, _) = identity.split_once("@sha256:")?;
    let face = find_face(path)?;
    (face.identity() == identity).then(|| face.family())
}

#[derive(Debug)]
pub(crate) enum FontError {
    Integrity(AssetIntegrityError),
    Decode,
    InvalidProgram,
    Memory,
}

impl From<AssetIntegrityError> for FontError {
    fn from(error: AssetIntegrityError) -> Self {
        Self::Integrity(error)
    }
}

impl From<TryReserveError> for FontError {
    fn from(_: TryReserveError) -> Self {
        Self::Memory
    }
}

/// Loads the original container. The qualified SVG subsetter deliberately owns
/// its own decode/derivation path; it must not receive a cached derived subset.
pub(crate) fn load_source(id: FaceId) -> Result<VerifiedAsset, AssetIntegrityError> {
    id.face().load_source()
}

/// Loads the exact admitted source program, preserving the predecoded Latin
/// fast path and the reviewed-decode check for every official chunk.
pub(crate) fn load_sfnt(id: FaceId) -> Result<Vec<u8>, FontError> {
    let face = id.face();
    let bytes = if let Some(predecoded) = face.load_predecoded()? {
        predecoded.into_bytes()
    } else {
        let source = load_source(id)?;
        match face.container() {
            Container::Woff2 => {
                wuff::decompress_woff2(source.bytes()).map_err(|_| FontError::Decode)?
            }
            Container::Sfnt => source.into_bytes(),
        }
    };
    validate_sfnt(id, bytes)
}

fn validate_sfnt(id: FaceId, bytes: Vec<u8>) -> Result<Vec<u8>, FontError> {
    let bytes = id.face().verify_decode(bytes)?;
    crate::shaping::Face::from_slice(&bytes, 0).ok_or(FontError::InvalidProgram)?;
    Ok(bytes)
}

/// A bounded preparation-local cache. Eviction affects physical work only.
#[derive(Default)]
pub(crate) struct FontLoader {
    decoded: Vec<(FaceId, Arc<[u8]>)>,
    sources: Vec<(FaceId, Arc<[u8]>)>,
}

impl FontLoader {
    pub(crate) fn load_source(&mut self, id: FaceId) -> Result<Arc<[u8]>, FontError> {
        if let Some((_, bytes)) = self.sources.iter().find(|(cached, _)| *cached == id) {
            return Ok(Arc::clone(bytes));
        }
        let bytes: Arc<[u8]> = load_source(id)?.into_bytes().into();
        Self::retain(&mut self.sources, id, Arc::clone(&bytes))?;
        Ok(bytes)
    }

    pub(crate) fn load_sfnt(&mut self, id: FaceId) -> Result<Arc<[u8]>, FontError> {
        if let Some((_, bytes)) = self.decoded.iter().find(|(cached, _)| *cached == id) {
            return Ok(Arc::clone(bytes));
        }
        let bytes = if let Some(predecoded) = id.face().load_predecoded()? {
            predecoded.into_bytes()
        } else {
            let source = self.load_source(id)?;
            match id.face().container() {
                Container::Woff2 => {
                    wuff::decompress_woff2(&source).map_err(|_| FontError::Decode)?
                }
                Container::Sfnt => {
                    let mut bytes = Vec::new();
                    bytes.try_reserve_exact(source.len())?;
                    bytes.extend_from_slice(&source);
                    bytes
                }
            }
        };
        let bytes: Arc<[u8]> = validate_sfnt(id, bytes)?.into();
        Self::retain(&mut self.decoded, id, Arc::clone(&bytes))?;
        Ok(bytes)
    }

    fn retain(
        cache: &mut Vec<(FaceId, Arc<[u8]>)>,
        id: FaceId,
        bytes: Arc<[u8]>,
    ) -> Result<(), FontError> {
        // Each representation cache retains at most 16 MiB. Large programs
        // can still be used; they simply bypass retention.
        const MAX_BYTES: usize = 16 * 1024 * 1024;
        if bytes.len() <= MAX_BYTES {
            while cache.iter().map(|(_, bytes)| bytes.len()).sum::<usize>()
                > MAX_BYTES - bytes.len()
            {
                cache.remove(0);
            }
            cache.try_reserve(1)?;
            cache.push((id, bytes));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use excalidraw_fonts::fonts::{find_face, FamilyId};

    #[test]
    fn loader_returns_reviewed_programs_and_reuses_only_physical_bytes() {
        let latin =
            find_face(excalidraw_fonts::excalifont_basic_latin_sfnt_transformation().source_path)
                .unwrap();
        let mut loader = FontLoader::default();
        for face in [
            latin,
            &FamilyId::Liberation.faces()[0],
            &FamilyId::NotoEmoji.faces()[0],
        ] {
            let id = face.id();
            let first = loader.load_sfnt(id).unwrap();
            let second = loader.load_sfnt(id).unwrap();
            assert!(Arc::ptr_eq(&first, &second));
            let original_cached = loader.load_source(id).unwrap();
            assert!(Arc::ptr_eq(
                &original_cached,
                &loader.load_source(id).unwrap()
            ));
            let original = load_source(id).unwrap();
            let expected = match face.container() {
                Container::Woff2 => wuff::decompress_woff2(original.bytes()).unwrap(),
                Container::Sfnt => original.into_bytes(),
            };
            assert_eq!(first.as_ref(), expected);
            assert_eq!(first.len(), face.decoded_length());
        }
        // A different preparation session shares no physical cache or policy.
        let other = FontLoader::default().load_sfnt(latin.id()).unwrap();
        let same = loader.load_sfnt(latin.id()).unwrap();
        assert_eq!(other, same);
        assert!(!Arc::ptr_eq(&other, &same));
    }

    #[test]
    fn cache_evicts_in_insertion_order_and_bypasses_oversized_programs() {
        let ids = [
            FamilyId::Liberation.faces()[0].id(),
            FamilyId::NotoEmoji.faces()[0].id(),
        ];
        let mut cache = Vec::new();
        let bytes: Arc<[u8]> = vec![0; 9 * 1024 * 1024].into();
        FontLoader::retain(&mut cache, ids[0], Arc::clone(&bytes)).unwrap();
        FontLoader::retain(&mut cache, ids[1], bytes).unwrap();
        assert_eq!(cache.len(), 1);
        assert_eq!(cache[0].0, ids[1]);
        FontLoader::retain(&mut cache, ids[0], vec![0; 17 * 1024 * 1024].into()).unwrap();
        assert_eq!(cache.len(), 1);
        assert_eq!(cache[0].0, ids[1]);
    }

    #[cfg(not(feature = "cjk"))]
    #[test]
    fn loader_preserves_unavailable_identity_without_populating_cache() {
        let face = &FamilyId::Xiaolai.faces()[0];
        let mut loader = FontLoader::default();
        for _ in 0..2 {
            let FontError::Integrity(error) = loader.load_sfnt(face.id()).unwrap_err() else {
                panic!("unavailable is distinct from decode failure");
            };
            assert!(error.is_unavailable());
            assert_eq!(error.asset_identity(), face.identity());
        }
        assert!(loader.decoded.is_empty());
    }
}
