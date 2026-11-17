//! Shared metadata-redaction invariants for backend and host-lifecycle fixtures.
use super::*;

pub(crate) fn redacted_leaf(
    state: &Path,
    id: &str,
    relative: &Path,
    source: &Path,
    payload: &[u8],
) {
    let db = Store::existing(&state.join("catalogs").join(format!("{id}.sqlite")), true).unwrap();
    assert!(scan::entry(&db, "catalog", relative).unwrap().is_none());
    let boundary = scan::entry(&db, "catalog", relative.parent().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(boundary.kind, "excluded");
    assert_eq!(std::fs::read(source.join(relative)).unwrap(), payload);
}
