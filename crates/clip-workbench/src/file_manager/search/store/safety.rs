//! Apply current hard content policy to cached metadata without opening sources.
use super::{count_vectors, record_storage_fault};
use crate::file_manager::search::policy;
use rusqlite::functions::FunctionFlags;
use std::{ffi::OsStr, os::unix::ffi::OsStrExt, path::Path};

pub(super) fn apply_content_policy(
    connection: &mut rusqlite::Connection,
    fault: &std::cell::Cell<bool>,
) -> Result<u64, String> {
    connection
        .create_scalar_function(
            "clip_forbidden",
            2,
            FunctionFlags::SQLITE_UTF8
                | FunctionFlags::SQLITE_DETERMINISTIC
                | FunctionFlags::SQLITE_DIRECTONLY,
            |context| {
                let bytes: Vec<u8> = context.get(0)?;
                let file: bool = context.get(1)?;
                let path = Path::new(OsStr::from_bytes(&bytes));
                Ok(policy::excluded(path).is_some() || (file && policy::secret_file(path)))
            },
        )
        .map_err(|e| e.to_string())?;
    let tx = connection
        .transaction()
        .map_err(|e| record_storage_fault(fault, e))?;
    // Component policy, not substring matching; prefix-like public names stay.
    // Delete only files whose last owners are forbidden. Unrelated orphan
    // inputs may belong to interrupted reconciliation and remain reusable.
    // Cascades retire memberships/checkpoints/FTS; no source stat/read occurs.
    tx.execute_batch("DELETE FROM files WHERE EXISTS(SELECT 1 FROM paths p WHERE p.file_id=files.id AND clip_forbidden(p.path,1)) AND NOT EXISTS(SELECT 1 FROM paths p WHERE p.file_id=files.id AND NOT clip_forbidden(p.path,1));
        DELETE FROM paths WHERE clip_forbidden(path,1);
        DELETE FROM memberships WHERE NOT EXISTS(SELECT 1 FROM paths p WHERE p.path=memberships.path);
        DELETE FROM segments WHERE NOT EXISTS(SELECT 1 FROM files f WHERE f.id=segments.file_id);
        DELETE FROM directories WHERE clip_forbidden(path,0);
        UPDATE roots SET available=0,state='disabled',error='Excluded by content safety policy' WHERE clip_forbidden(path,0) AND (available<>0 OR state<>'disabled');")
        .map_err(|e| record_storage_fault(fault, e))?;
    tx.commit().map_err(|e| record_storage_fault(fault, e))?;
    connection
        .remove_function("clip_forbidden", 2)
        .map_err(|e| e.to_string())?;
    count_vectors(connection)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_manager::search::store::TestFixture;
    use rusqlite::params;

    fn relocate_reference(fixture: &TestFixture, path: &Path) {
        let db = &fixture.store.connection;
        let old = fixture.entry.path.as_os_str().as_bytes();
        db.execute(
            "INSERT INTO paths(path,parent,file_id) SELECT ?1,?2,file_id FROM paths WHERE path=?3",
            params![
                path.as_os_str().as_bytes(),
                path.parent().unwrap().as_os_str().as_bytes(),
                old
            ],
        )
        .unwrap();
        db.execute(
            "UPDATE memberships SET path=?1 WHERE path=?2",
            params![path.as_os_str().as_bytes(), old],
        )
        .unwrap();
        db.execute("DELETE FROM paths WHERE path=?1", [old])
            .unwrap();
    }

    #[test]
    fn secret_filename_retires_cached_text_without_disabling_its_public_root() {
        let fixture = TestFixture::new();
        relocate_reference(&fixture, &fixture.root.path.join(".env"));
        let fixture = fixture.reopen();
        assert_eq!(fixture.store.vector_count, 0);
        assert_eq!(
            fixture
                .store
                .connection
                .query_row("SELECT COUNT(*) FROM segment_fts", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(fixture.store.roots().unwrap().len(), 1);
    }

    #[test]
    fn reopening_retires_legacy_private_records_even_when_the_source_is_absent() {
        for name in [".mozilla", ".thunderbird"] {
            let fixture = TestFixture::new();
            let folder = fixture.root.path.join(name);
            let path = folder.join("profile.txt");
            relocate_reference(&fixture, &path);
            fixture
                .store
                .connection
                .execute(
                    "UPDATE roots SET path=?1,state='ready',available=1",
                    [folder.as_os_str().as_bytes()],
                )
                .unwrap();
            assert!(!folder.exists()); // Cache policy cannot depend on source access.
            let fixture = fixture.reopen();
            let db = &fixture.store.connection;
            for table in ["paths", "memberships", "files", "segments", "segment_fts"] {
                let count: i64 = db
                    .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                        row.get(0)
                    })
                    .unwrap();
                assert_eq!(count, 0, "{name}: {table}");
            }
            let state: (String, i64) = fixture
                .store
                .connection
                .query_row("SELECT state,available FROM roots", [], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })
                .unwrap();
            assert_eq!(state, ("disabled".into(), 0));
        }
    }

    #[test]
    fn forbidden_membership_does_not_remove_public_paths_or_their_shared_vector() {
        let fixture = TestFixture::new();
        let path = fixture.root.path.join(".mozilla/profile.txt");
        let db = &fixture.store.connection;
        let root = fixture.store.root_id(&fixture.root.path).unwrap().unwrap();
        db.execute(
            "INSERT INTO paths(path,parent,file_id) VALUES (?1,?2,?3)",
            params![
                path.as_os_str().as_bytes(),
                path.parent().unwrap().as_os_str().as_bytes(),
                fixture.file
            ],
        )
        .unwrap();
        db.execute(
            "INSERT INTO memberships(root_id,path,generation,verified) VALUES (?1,?2,1,1)",
            params![root, path.as_os_str().as_bytes()],
        )
        .unwrap();
        let control = fixture.root.path.join(".mozilla-notes/profile.txt");
        db.execute(
            "INSERT INTO paths(path,parent,file_id) VALUES (?1,?2,?3)",
            params![
                control.as_os_str().as_bytes(),
                control.parent().unwrap().as_os_str().as_bytes(),
                fixture.file
            ],
        )
        .unwrap();
        db.execute(
            "INSERT INTO memberships(root_id,path,generation,verified) VALUES (?1,?2,1,1)",
            params![root, control.as_os_str().as_bytes()],
        )
        .unwrap();
        let fixture = fixture.reopen();
        assert_eq!(fixture.store.vector_count, 1);
        let count: i64 = fixture
            .store
            .connection
            .query_row("SELECT COUNT(*) FROM paths", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 2);
        assert_eq!(
            fixture
                .store
                .connection
                .query_row(
                    "SELECT COUNT(*) FROM paths WHERE path=?1",
                    [path.as_os_str().as_bytes()],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
    }
}
