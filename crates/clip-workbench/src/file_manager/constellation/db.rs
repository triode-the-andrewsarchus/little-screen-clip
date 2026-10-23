//! Dedicated SQLite database storage for sample constellation indexing.
//! Independent from other database caches. Schema version six stores Unix paths as
//! BLOB keys so invalid UTF-8 bytes cannot alias a different display-equivalent
//! filename. Names remain display text. Older TEXT keys migrate transactionally;
//! paths already lost by older versions require re-indexing their source folders.

use super::generated_model::SampleCategory;
use rusqlite::{Connection, OptionalExtension, params};
use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

/// A cached acoustic observation, including its original path bytes and display name.
///
/// Features describe a bounded analysis window; `duration_seconds` retains complete
/// container duration when the decoder exposes it. This is not a live filesystem
/// record: source changes require re-indexing, and playback opens guarded inputs.
#[derive(Clone, Debug)]
pub struct ConstellationSample {
    pub file_path: PathBuf,
    pub folder_path: PathBuf,
    pub filename: String,
    pub category: SampleCategory,
    pub confidence: f32,
    pub duration_seconds: f32,
    pub pitch_hz: f32,
    pub pitch_clarity: f32,
    pub brightness: f32,
    pub x: f32,
    pub y: f32,
    pub file_size: u64,
    pub mtime: i64,
    pub sample_rate: u32,
    pub channels: u16,
    pub bit_depth: u16,
}

/// Opens short-lived SQLite connections for transactional index snapshots.
///
/// Construction and all methods perform I/O and belong on workers. Schema changes,
/// data conversion, and their version markers commit together, including rollback
/// when a legacy row cannot be migrated.
pub struct ConstellationDb {
    path: PathBuf,
}

impl ConstellationDb {
    pub fn default_path() -> PathBuf {
        let base = if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
            PathBuf::from(xdg).join("little-screen-clip")
        } else if let Ok(home) = std::env::var("HOME") {
            PathBuf::from(home).join(".local/share/little-screen-clip")
        } else {
            PathBuf::from(".")
        };
        base.join("constellation.sqlite")
    }

    /// Opens a cache at an explicit path and completes any required migrations.
    ///
    /// # Errors
    /// Returns an error for directory, SQLite, or migration failures. Failed
    /// migrations retain their previous schema version and committed records.
    pub fn open_path(path: PathBuf) -> Result<Self, String> {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Cannot create index directory: {e}"))?;
        }
        let db = Self { path };
        db.init_schema()?;
        Ok(db)
    }

    pub fn new() -> Result<Self, String> {
        Self::open_path(Self::default_path())
    }

    fn open(&self) -> Result<Connection, String> {
        Connection::open(&self.path)
            .map_err(|e| format!("Failed to open constellation database: {e}"))
    }

    fn init_schema(&self) -> Result<(), String> {
        let mut connection = self.open()?;
        connection
            .execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;")
            .map_err(|e| e.to_string())?;
        let conn = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS indexed_folders (
                folder_path TEXT PRIMARY KEY,
                indexed_at INTEGER NOT NULL,
                sample_count INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS samples (
                file_path TEXT PRIMARY KEY,
                folder_path TEXT NOT NULL,
                filename TEXT NOT NULL,
                category TEXT NOT NULL,
                confidence REAL NOT NULL,
                duration_seconds REAL NOT NULL,
                pitch_hz REAL NOT NULL,
                pitch_clarity REAL NOT NULL,
                brightness REAL NOT NULL,
                x REAL NOT NULL,
                y REAL NOT NULL,
                file_size INTEGER NOT NULL,
                mtime INTEGER NOT NULL,
                sample_rate INTEGER DEFAULT 0,
                channels INTEGER DEFAULT 0,
                bit_depth INTEGER DEFAULT 0
            );

            CREATE INDEX IF NOT EXISTS idx_samples_folder ON samples(folder_path);
            ",
        )
        .map_err(|e| format!("Failed to initialize constellation database schema: {e}"))?;

        let version: i32 = conn
            .query_row("PRAGMA user_version;", [], |row| row.get(0))
            .map_err(|e| e.to_string())?;

        if version < 4 {
            self.migrate_v4_clump_coordinates(&conn)?;
            conn.execute("PRAGMA user_version = 4;", [])
                .map_err(|e| e.to_string())?;
        }

        if version < 5 {
            self.migrate_v5_audio_specs(&conn)?;
        }

        if version < 6 {
            // SQLite TEXT cannot preserve arbitrary Unix filenames. Convert old
            // UTF-8 keys once; subsequent reads/writes bind paths as BLOB bytes.
            conn.execute_batch("UPDATE samples SET file_path = CAST(file_path AS BLOB), folder_path = CAST(folder_path AS BLOB);
                UPDATE indexed_folders SET folder_path = CAST(folder_path AS BLOB);
                PRAGMA user_version = 6;").map_err(|error| format!("Cannot migrate sample index paths: {error}"))?;
        }
        conn.commit().map_err(|error| error.to_string())
    }

    /// Adds technical audio columns and advances their version in the caller's transaction.
    fn migrate_v5_audio_specs(&self, conn: &Connection) -> Result<(), String> {
        let columns = conn
            .prepare("PRAGMA table_info(samples)")
            .map_err(|e| e.to_string())?
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        for column in ["sample_rate", "channels", "bit_depth"] {
            if !columns.iter().any(|existing| existing == column) {
                conn.execute(
                    &format!("ALTER TABLE samples ADD COLUMN {column} INTEGER DEFAULT 0"),
                    [],
                )
                .map_err(|e| e.to_string())?;
            }
        }
        conn.execute("PRAGMA user_version = 5;", [])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    fn migrate_v4_clump_coordinates(&self, conn: &Connection) -> Result<(), String> {
        let mut stmt = conn
            .prepare(
                "SELECT file_path, category, duration_seconds, pitch_clarity, brightness FROM samples",
            )
            .map_err(|e| e.to_string())?;

        let rows = stmt
            .query_map([], |row| {
                let file_path: String = row.get(0)?;
                let cat_str: String = row.get(1)?;
                let duration: f64 = row.get(2)?;
                let clarity: f64 = row.get(3)?;
                let brightness: f64 = row.get(4)?;
                Ok((
                    file_path,
                    cat_str,
                    duration as f32,
                    clarity as f32,
                    brightness as f32,
                ))
            })
            .map_err(|e| e.to_string())?;

        let mut updates = Vec::new();
        for item in rows {
            let (file_path, cat_str, duration, clarity, brightness) =
                item.map_err(|e| e.to_string())?;
            let category = SampleCategory::from_str(&cat_str).unwrap_or(SampleCategory::Loop);
            let (x, y) = super::classifier::calculate_organic_coordinates(
                category, brightness, duration, clarity, &file_path,
            );
            updates.push((x as f64, y as f64, file_path));
        }

        // The caller owns the migration transaction, including its version update.
        let mut update = conn
            .prepare("UPDATE samples SET x = ?1, y = ?2 WHERE file_path = ?3")
            .map_err(|e| e.to_string())?;
        for (x, y, file_path) in updates {
            update
                .execute(params![x, y, file_path])
                .map_err(|e| e.to_string())?;
        }

        Ok(())
    }

    /// Returns the last recorded count for an explicitly indexed root.
    ///
    /// Counts describe that root's indexing transaction, not a fresh filesystem
    /// inventory or a recomputed total after independent subtree re-indexing.
    ///
    /// # Errors
    /// Returns an error if SQLite cannot open or query the cache.
    #[allow(dead_code)]
    pub fn is_folder_indexed(&self, folder: &Path) -> Result<Option<usize>, String> {
        let conn = self.open()?;
        let folder_key = folder.as_os_str().as_bytes();
        let mut stmt = conn
            .prepare("SELECT sample_count FROM indexed_folders WHERE folder_path = ?1")
            .map_err(|e| e.to_string())?;

        let count: Option<i64> = stmt
            .query_row(params![folder_key], |row| row.get(0))
            .optional()
            .map_err(|e| e.to_string())?;

        Ok(count.map(|c| c.max(0) as usize))
    }

    /// Loads a folder and its descendants with byte-exact subtree boundaries.
    ///
    /// Results are ordered by stored path bytes and do not revalidate audio files.
    /// Invalid rows fail the request rather than silently yielding a partial map.
    ///
    /// # Errors
    /// Returns an error for SQLite access or invalid stored field types/ranges.
    pub fn load_samples(&self, folder: &Path) -> Result<Vec<ConstellationSample>, String> {
        let conn = self.open()?;
        let folder_key = folder.as_os_str().as_bytes();
        let prefix_key = folder.join("");

        let mut stmt = conn
            .prepare(
                "SELECT file_path, folder_path, filename, category, confidence,
                        duration_seconds, pitch_hz, pitch_clarity, brightness, x, y, file_size, mtime,
                        COALESCE(sample_rate, 0), COALESCE(channels, 0), COALESCE(bit_depth, 0)
                 FROM samples
                 WHERE folder_path = ?1 OR substr(folder_path, 1, length(?2)) = ?2 ORDER BY file_path",
            )
            .map_err(|e| e.to_string())?;

        let rows = stmt
            .query_map(
                params![folder_key, prefix_key.as_os_str().as_bytes()],
                |row| {
                    let cat_str: String = row.get(3)?;
                    let category =
                        SampleCategory::from_str(&cat_str).unwrap_or(SampleCategory::Loop);

                    let file_path_bytes: Vec<u8> = row.get(0)?;
                    let folder_path_bytes: Vec<u8> = row.get(1)?;

                    Ok(ConstellationSample {
                        file_path: PathBuf::from(OsString::from_vec(file_path_bytes)),
                        folder_path: PathBuf::from(OsString::from_vec(folder_path_bytes)),
                        filename: row.get(2)?,
                        category,
                        confidence: row.get::<_, f64>(4)? as f32,
                        duration_seconds: row.get::<_, f64>(5)? as f32,
                        pitch_hz: row.get::<_, f64>(6)? as f32,
                        pitch_clarity: row.get::<_, f64>(7)? as f32,
                        brightness: row.get::<_, f64>(8)? as f32,
                        x: row.get::<_, f64>(9)? as f32,
                        y: row.get::<_, f64>(10)? as f32,
                        file_size: row.get::<_, i64>(11)? as u64,
                        mtime: row.get(12)?,
                        sample_rate: row.get(13)?,
                        channels: row.get(14)?,
                        bit_depth: row.get(15)?,
                    })
                },
            )
            .map_err(|e| e.to_string())?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }

    /// Atomically replaces one complete subtree snapshot, including empty results.
    ///
    /// Callers must finish discovery and analysis before invoking this method.
    /// Folder names and Unix path bytes are compared literally; wildcard characters
    /// and display-equivalent invalid UTF-8 names cannot affect other subtrees.
    ///
    /// # Errors
    /// Returns an error on SQLite failures, rolling the entire replacement back.
    pub fn save_samples(
        &self,
        folder: &Path,
        samples: &[ConstellationSample],
    ) -> Result<(), String> {
        let mut conn = self.open()?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;

        let folder_key = folder.as_os_str().as_bytes();
        let prefix_key = folder.join("");

        // Clear existing entries for this folder subtree
        tx.execute(
            "DELETE FROM samples WHERE folder_path = ?1 OR substr(folder_path, 1, length(?2)) = ?2",
            params![folder_key, prefix_key.as_os_str().as_bytes()],
        )
        .map_err(|e| e.to_string())?;

        tx.execute(
            "DELETE FROM indexed_folders WHERE folder_path = ?1 OR substr(folder_path, 1, length(?2)) = ?2",
            params![folder_key, prefix_key.as_os_str().as_bytes()],
        )
        .map_err(|e| e.to_string())?;

        {
            let mut stmt = tx
                .prepare(
                    "INSERT INTO samples (
                        file_path, folder_path, filename, category, confidence,
                        duration_seconds, pitch_hz, pitch_clarity, brightness, x, y, file_size, mtime,
                        sample_rate, channels, bit_depth
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
                )
                .map_err(|e| e.to_string())?;

            for s in samples {
                stmt.execute(params![
                    s.file_path.as_os_str().as_bytes(),
                    s.folder_path.as_os_str().as_bytes(),
                    s.filename,
                    s.category.as_str(),
                    s.confidence as f64,
                    s.duration_seconds as f64,
                    s.pitch_hz as f64,
                    s.pitch_clarity as f64,
                    s.brightness as f64,
                    s.x as f64,
                    s.y as f64,
                    s.file_size as i64,
                    s.mtime,
                    s.sample_rate as i64,
                    s.channels as i64,
                    s.bit_depth as i64,
                ])
                .map_err(|e| e.to_string())?;
            }
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        tx.execute(
            "INSERT INTO indexed_folders (folder_path, indexed_at, sample_count) VALUES (?1, ?2, ?3)",
            params![folder_key, now, samples.len() as i64],
        )
        .map_err(|e| e.to_string())?;

        tx.commit().map_err(|e| e.to_string())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_constellation_db_roundtrip() {
        let temp_dir =
            std::env::temp_dir().join(format!("constellation_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&temp_dir);
        let db_path = temp_dir.join("test_constellation.sqlite");
        let _ = std::fs::remove_file(&db_path);

        let db = ConstellationDb::open_path(db_path.clone()).expect("Failed to create test db");

        let test_folder = PathBuf::from("/samples/drums");
        assert_eq!(db.is_folder_indexed(&test_folder).unwrap(), None);

        let samples = vec![
            ConstellationSample {
                file_path: test_folder.join("punchy_kick.wav"),
                folder_path: test_folder.clone(),
                filename: "punchy_kick.wav".to_string(),
                category: SampleCategory::Kick,
                confidence: 0.95,
                duration_seconds: 0.45,
                pitch_hz: 55.0,
                pitch_clarity: 0.88,
                brightness: 1200.0,
                x: -0.65,
                y: 0.70,
                file_size: 44100,
                mtime: 1700000000,
                sample_rate: 44100,
                channels: 2,
                bit_depth: 24,
            },
            ConstellationSample {
                file_path: test_folder.join("crisp_snare.wav"),
                folder_path: test_folder.clone(),
                filename: "crisp_snare.wav".to_string(),
                category: SampleCategory::Snare,
                confidence: 0.91,
                duration_seconds: 0.32,
                pitch_hz: 180.0,
                pitch_clarity: 0.65,
                brightness: 4500.0,
                x: 0.35,
                y: 0.50,
                file_size: 32000,
                mtime: 1700000001,
                sample_rate: 48000,
                channels: 1,
                bit_depth: 16,
            },
        ];

        db.save_samples(&test_folder, &samples)
            .expect("Failed to save samples");

        assert_eq!(db.is_folder_indexed(&test_folder).unwrap(), Some(2));

        let mut loaded = db
            .load_samples(&test_folder)
            .expect("Failed to load samples");
        loaded.sort_by(|a, b| b.filename.cmp(&a.filename));
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].filename, "punchy_kick.wav");
        assert_eq!(loaded[0].category, SampleCategory::Kick);
        assert!((loaded[0].pitch_hz - 55.0).abs() < 1e-4);
        assert_eq!(loaded[0].sample_rate, 44100);
        assert_eq!(loaded[0].channels, 2);
        assert_eq!(loaded[0].bit_depth, 24);
        assert_eq!(loaded[1].filename, "crisp_snare.wav");
        assert_eq!(loaded[1].category, SampleCategory::Snare);
        assert_eq!(loaded[1].sample_rate, 48000);
        assert_eq!(loaded[1].channels, 1);
        assert_eq!(loaded[1].bit_depth, 16);

        for folder in [
            "/samples/drums-extra",
            "/samples/drums/sub",
            "/samples/dr_ms",
            "/samples/dr%ms",
        ] {
            let mut sample = samples[0].clone();
            sample.folder_path = PathBuf::from(folder);
            sample.file_path = sample.folder_path.join("kick.wav");
            db.save_samples(&sample.folder_path, &[sample.clone()])
                .unwrap();
        }
        assert_eq!(db.load_samples(&test_folder).unwrap().len(), 3);
        assert_eq!(
            db.load_samples(Path::new("/samples/dr_ms")).unwrap().len(),
            1
        );
        assert_eq!(
            db.load_samples(Path::new("/samples/dr%ms")).unwrap().len(),
            1
        );
        db.save_samples(&test_folder, &[]).unwrap();
        assert!(db.load_samples(&test_folder).unwrap().is_empty());
        assert_eq!(
            db.is_folder_indexed(Path::new("/samples/drums-extra"))
                .unwrap(),
            Some(1)
        );
        assert_eq!(db.load_samples(Path::new("/")).unwrap().len(), 3);

        let legacy = db.open().unwrap();
        legacy.execute_batch("UPDATE samples SET file_path = CAST(file_path AS TEXT), folder_path = CAST(folder_path AS TEXT);
            UPDATE indexed_folders SET folder_path = CAST(folder_path AS TEXT);
            PRAGMA user_version = 3;
            CREATE TRIGGER fail_coordinate_migration BEFORE UPDATE ON samples BEGIN SELECT RAISE(ABORT, 'owned migration failure'); END;").unwrap();
        assert!(ConstellationDb::open_path(db_path.clone()).is_err());
        let version: i32 = legacy
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            version, 3,
            "failed migrations must not advance schema versions"
        );
        legacy
            .execute_batch("DROP TRIGGER fail_coordinate_migration")
            .unwrap();
        drop(legacy);
        ConstellationDb::open_path(db_path.clone()).unwrap();

        let raw_folder = PathBuf::from(OsString::from_vec(b"/samples/raw_\xff".to_vec()));
        let visible_folder = PathBuf::from(raw_folder.to_string_lossy().as_ref());
        let mut raw_sample = samples[0].clone();
        raw_sample.folder_path = raw_folder.clone();
        raw_sample.file_path = raw_folder.join(OsString::from_vec(b"kick_\xfe.wav".to_vec()));
        db.save_samples(&raw_folder, &[raw_sample.clone()]).unwrap();
        assert_eq!(
            db.load_samples(&raw_folder).unwrap()[0].file_path,
            raw_sample.file_path
        );
        assert!(db.load_samples(&visible_folder).unwrap().is_empty());
        assert_eq!(db.is_folder_indexed(&raw_folder).unwrap(), Some(1));
        db.save_samples(&visible_folder, &[]).unwrap();
        assert_eq!(db.load_samples(&raw_folder).unwrap().len(), 1);

        // Emulate a v5 database's UTF-8 TEXT keys, then reopen through migration.
        let legacy = db.open().unwrap();
        legacy.execute_batch("UPDATE samples SET file_path = CAST(file_path AS TEXT), folder_path = CAST(folder_path AS TEXT);
            UPDATE indexed_folders SET folder_path = CAST(folder_path AS TEXT);
            PRAGMA user_version = 5;").unwrap();
        drop(legacy);
        let migrated = ConstellationDb::open_path(db_path.clone()).unwrap();
        assert_eq!(
            migrated.load_samples(&raw_folder).unwrap()[0].file_path,
            raw_sample.file_path
        );
        assert_eq!(
            migrated
                .load_samples(Path::new("/samples/drums-extra"))
                .unwrap()
                .len(),
            1
        );

        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
