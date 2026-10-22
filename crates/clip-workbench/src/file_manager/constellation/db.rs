//! Dedicated SQLite database storage for sample constellation indexing.
//! Independent from other database caches.

use super::generated_model::SampleCategory;
use rusqlite::{Connection, OptionalExtension, params};
use std::path::{Path, PathBuf};

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
}

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

    pub fn open_path(path: PathBuf) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
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
        let conn = self.open()?;
        conn.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;

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
                mtime INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_samples_folder ON samples(folder_path);
            ",
        )
        .map_err(|e| format!("Failed to initialize constellation database schema: {e}"))?;

        let version: i32 = conn
            .query_row("PRAGMA user_version;", [], |row| row.get(0))
            .unwrap_or(0);

        if version < 4 {
            self.migrate_v4_clump_coordinates(&conn)?;
            let _ = conn.execute("PRAGMA user_version = 4;", []);
        }

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
        for item in rows.flatten() {
            let (file_path, cat_str, duration, clarity, brightness) = item;
            let category = SampleCategory::from_str(&cat_str).unwrap_or(SampleCategory::Loop);
            let (x, y) = super::classifier::calculate_organic_coordinates(
                category, brightness, duration, clarity, &file_path,
            );
            updates.push((x as f64, y as f64, file_path));
        }

        if !updates.is_empty() {
            let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
            {
                let mut update_stmt = tx
                    .prepare("UPDATE samples SET x = ?1, y = ?2 WHERE file_path = ?3")
                    .map_err(|e| e.to_string())?;
                for (x, y, file_path) in updates {
                    let _ = update_stmt.execute(params![x, y, file_path]);
                }
            }
            tx.commit().map_err(|e| e.to_string())?;
        }

        Ok(())
    }

    #[allow(dead_code)]
    pub fn is_folder_indexed(&self, folder: &Path) -> Result<Option<usize>, String> {
        let conn = self.open()?;
        let folder_str = folder.to_string_lossy();
        let mut stmt = conn
            .prepare("SELECT sample_count FROM indexed_folders WHERE folder_path = ?1")
            .map_err(|e| e.to_string())?;

        let count: Option<i64> = stmt
            .query_row(params![folder_str], |row| row.get(0))
            .optional()
            .map_err(|e| e.to_string())?;

        Ok(count.map(|c| c.max(0) as usize))
    }

    pub fn load_samples(&self, folder: &Path) -> Result<Vec<ConstellationSample>, String> {
        let conn = self.open()?;
        let folder_str = folder.to_string_lossy();
        let prefix_like = format!("{}%", folder_str);

        let mut stmt = conn
            .prepare(
                "SELECT file_path, folder_path, filename, category, confidence,
                        duration_seconds, pitch_hz, pitch_clarity, brightness, x, y, file_size, mtime
                 FROM samples
                 WHERE folder_path = ?1 OR folder_path LIKE ?2",
            )
            .map_err(|e| e.to_string())?;

        let rows = stmt
            .query_map(params![folder_str, prefix_like], |row| {
                let cat_str: String = row.get(3)?;
                let category = SampleCategory::from_str(&cat_str).unwrap_or(SampleCategory::Loop);

                let file_path_str: String = row.get(0)?;
                let folder_path_str: String = row.get(1)?;

                Ok(ConstellationSample {
                    file_path: PathBuf::from(file_path_str),
                    folder_path: PathBuf::from(folder_path_str),
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
                })
            })
            .map_err(|e| e.to_string())?;

        let mut samples = Vec::new();
        for sample in rows {
            if let Ok(s) = sample {
                samples.push(s);
            }
        }
        Ok(samples)
    }

    pub fn save_samples(
        &self,
        folder: &Path,
        samples: &[ConstellationSample],
    ) -> Result<(), String> {
        let mut conn = self.open()?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;

        let folder_str = folder.to_string_lossy();
        let prefix_like = format!("{}%", folder_str);

        // Clear existing entries for this folder subtree
        tx.execute(
            "DELETE FROM samples WHERE folder_path = ?1 OR folder_path LIKE ?2",
            params![folder_str, prefix_like],
        )
        .map_err(|e| e.to_string())?;

        tx.execute(
            "DELETE FROM indexed_folders WHERE folder_path = ?1 OR folder_path LIKE ?2",
            params![folder_str, prefix_like],
        )
        .map_err(|e| e.to_string())?;

        {
            let mut stmt = tx
                .prepare(
                    "INSERT INTO samples (
                        file_path, folder_path, filename, category, confidence,
                        duration_seconds, pitch_hz, pitch_clarity, brightness, x, y, file_size, mtime
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                )
                .map_err(|e| e.to_string())?;

            for s in samples {
                stmt.execute(params![
                    s.file_path.to_string_lossy(),
                    s.folder_path.to_string_lossy(),
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
            params![folder_str, now, samples.len() as i64],
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
            },
        ];

        db.save_samples(&test_folder, &samples)
            .expect("Failed to save samples");

        assert_eq!(db.is_folder_indexed(&test_folder).unwrap(), Some(2));

        let loaded = db
            .load_samples(&test_folder)
            .expect("Failed to load samples");
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].filename, "punchy_kick.wav");
        assert_eq!(loaded[0].category, SampleCategory::Kick);
        assert!((loaded[0].pitch_hz - 55.0).abs() < 1e-4);
        assert_eq!(loaded[1].filename, "crisp_snare.wav");
        assert_eq!(loaded[1].category, SampleCategory::Snare);

        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
