//! Bounded SQLite snapshots. SQL is generated here, never accepted from the database or UI.
use crate::file_source::read_file_snapshot;
use rusqlite::{
    Connection, DatabaseName,
    config::DbConfig,
    hooks::{AuthAction, AuthContext, Authorization},
    limits::Limit,
    serialize::OwnedData,
    types::ValueRef,
};
use std::{
    path::Path,
    ptr::NonNull,
    sync::Arc,
    time::{Duration, Instant},
};

const MAX_BYTES: usize = 128 * 1024 * 1024;
pub(super) const PAGE_ROWS: usize = 200;
const MAX_COLUMNS: usize = 32;
#[derive(Clone)]
pub(super) struct Column {
    pub name: String,
    pub kind: String,
    pub primary: bool,
    pub required: bool,
    pub hidden: bool,
}
#[derive(Clone)]
pub(super) struct Table {
    pub name: String,
    pub kind: String,
    pub sql: String,
    pub columns: Vec<Column>,
    pub relationships: Vec<(String, String, String)>,
}
pub(super) struct Document {
    pub bytes: Arc<[u8]>,
    pub tables: Vec<Table>,
    pub wal: bool,
    pub truncated: bool,
    pub user_version: i64,
    pub application_id: i64,
}
pub(super) struct Page {
    pub rows: Vec<Vec<String>>,
    pub columns: Vec<String>,
    pub more: bool,
    pub offset: usize,
    pub limited_columns: bool,
}
impl Document {
    pub fn read(path: &Path) -> Result<Self, String> {
        Self::parse(read_file_snapshot(path, MAX_BYTES)?)
    }
    fn parse(bytes: Vec<u8>) -> Result<Self, String> {
        if bytes.len() < 100 || !bytes.starts_with(b"SQLite format 3\0") {
            return Err("Not a SQLite 3 database".into());
        }
        let wal = bytes[18] == 2 || bytes[19] == 2;
        let connection = open(&bytes).map_err(|e| e.to_string())?;
        let user_version = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(|e| e.to_string())?;
        let application_id = connection
            .pragma_query_value(None, "application_id", |row| row.get(0))
            .map_err(|e| e.to_string())?;
        let mut tables = schema_tables(&connection).map_err(|e| e.to_string())?;
        let truncated = tables.len() > 512;
        tables.truncate(512);
        // Only ordinary tables are introspected; virtual tables and views stay metadata-only.
        for table in &mut tables {
            if table.kind == "table" {
                describe(&connection, table)
                    .map_err(|e| format!("{}: {e}", clipped(&table.name)))?;
            }
        }
        Ok(Self {
            bytes: bytes.into(),
            tables,
            wal,
            truncated,
            user_version,
            application_id,
        })
    }
    pub fn page(&self, index: usize, offset: usize) -> Result<Page, String> {
        let table = self.tables.get(index).ok_or("Unknown table")?;
        let columns: Vec<_> = table
            .columns
            .iter()
            .filter(|c| !c.hidden)
            .take(MAX_COLUMNS)
            .map(|c| c.name.clone())
            .collect();
        if columns.is_empty() {
            return Err("This object is metadata-only; views, virtual tables and generated columns are not evaluated.".into());
        }
        if offset > 1_000_000 {
            return Err("Row browsing stops at the 1,000,000-row offset limit".into());
        }
        let connection = open(&self.bytes).map_err(|e| e.to_string())?;
        connection.authorizer(Some(|context: AuthContext<'_>| {
            if context.accessor.is_some() {
                return Authorization::Deny;
            }
            match context.action {
                AuthAction::Read { .. } | AuthAction::Select => Authorization::Allow,
                _ => Authorization::Deny,
            }
        }));
        let sql = format!(
            "SELECT {} FROM {} LIMIT {} OFFSET {}",
            columns
                .iter()
                .map(|name| quote(name))
                .collect::<Vec<_>>()
                .join(","),
            quote(&table.name),
            PAGE_ROWS + 1,
            offset
        );
        let mut statement = connection.prepare(&sql).map_err(|e| e.to_string())?;
        let mut cursor = statement.query([]).map_err(|e| e.to_string())?;
        let mut rows = Vec::new();
        while let Some(row) = cursor.next().map_err(|e| e.to_string())? {
            let mut cells = Vec::new();
            for column in 0..columns.len() {
                cells.push(cell(row.get_ref(column).map_err(|e| e.to_string())?));
            }
            rows.push(cells);
        }
        let more = rows.len() > PAGE_ROWS;
        rows.truncate(PAGE_ROWS);
        Ok(Page {
            rows,
            more,
            offset,
            limited_columns: table.columns.iter().filter(|c| !c.hidden).count() > MAX_COLUMNS,
            columns,
        })
    }
}
fn open(bytes: &[u8]) -> rusqlite::Result<Connection> {
    let mut connection = Connection::open_in_memory()?;
    connection.set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)?;
    connection.set_db_config(DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, false)?;
    connection.set_db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_VIEW, false)?;
    for (limit, value) in [
        (Limit::SQLITE_LIMIT_LENGTH, 1_048_576),
        (Limit::SQLITE_LIMIT_SQL_LENGTH, 65_536),
        (Limit::SQLITE_LIMIT_COLUMN, 256),
        (Limit::SQLITE_LIMIT_EXPR_DEPTH, 20),
        (Limit::SQLITE_LIMIT_COMPOUND_SELECT, 3),
        (Limit::SQLITE_LIMIT_VDBE_OP, 100_000),
        (Limit::SQLITE_LIMIT_FUNCTION_ARG, 8),
        (Limit::SQLITE_LIMIT_ATTACHED, 0),
        (Limit::SQLITE_LIMIT_TRIGGER_DEPTH, 0),
        (Limit::SQLITE_LIMIT_WORKER_THREADS, 0),
    ] {
        connection.set_limit(limit, value)?;
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut steps = 0;
    connection.progress_handler(
        1000,
        Some(move || {
            steps += 1000;
            steps > 2_000_000 || Instant::now() >= deadline
        }),
    );
    // https://www.sqlite.org/c3ref/deserialize.html advises a 20-byte allocation
    // margin for untrusted content. The safe stream loader allocates only len.
    let allocation_size = bytes
        .len()
        .checked_add(20)
        .filter(|size| *size <= isize::MAX as usize)
        .ok_or(rusqlite::Error::InvalidQuery)?;
    // SAFETY: the scalar size is bounded above; SQLite returns null or a new
    // allocation of that size, subsequently transferred once through OwnedData.
    let pointer = NonNull::new(
        unsafe { rusqlite::ffi::sqlite3_malloc64(allocation_size as u64) }.cast::<u8>(),
    )
    .ok_or(rusqlite::Error::InvalidQuery)?;
    {
        // SAFETY: the checked non-null allocation owns allocation_size writable
        // bytes, within isize::MAX and disjoint from bytes. This borrow ends
        // before ownership is handed to SQLite, including its cleanup authority.
        let buffer = unsafe { std::slice::from_raw_parts_mut(pointer.as_ptr(), allocation_size) };
        buffer.fill(0);
        buffer[..bytes.len()].copy_from_slice(bytes);
        // Inspect only the checkpointed main file, without opening sidecars.
        if bytes.len() > 19 {
            buffer[18..20].fill(1);
        }
    }
    // SAFETY: pointer is uniquely SQLite-allocated, len bytes plus an initialized
    // margin. OwnedData transfers it exactly once; FREEONCLOSE frees it on native
    // failure too. DatabaseName::Main cannot fail schema-name conversion.
    let snapshot = unsafe { OwnedData::from_raw_nonnull(pointer, bytes.len()) };
    connection.deserialize(DatabaseName::Main, snapshot, true)?;
    connection.execute_batch(
        "PRAGMA query_only=ON; PRAGMA temp_store=MEMORY; PRAGMA cache_size=-4096;",
    )?;
    Ok(connection)
}
fn schema_tables(connection: &Connection) -> rusqlite::Result<Vec<Table>> {
    let mut tables = Vec::new();
    let mut statement = connection.prepare("SELECT s.name,l.type,s.sql FROM sqlite_schema s JOIN pragma_table_list l ON l.name=s.name AND l.schema='main' WHERE s.type IN ('table','view') AND s.name NOT LIKE 'sqlite_%' ORDER BY s.name LIMIT 513")?;
    let mut records = statement.query([])?;
    while let Some(row) = records.next()? {
        let name: String = row.get(0)?;
        let kind: String = row.get(1)?;
        let sql: String = row.get::<_, Option<String>>(2)?.unwrap_or_default();
        tables.push(Table {
            name,
            kind,
            sql,
            columns: Vec::new(),
            relationships: Vec::new(),
        });
    }
    Ok(tables)
}
fn describe(connection: &Connection, table: &mut Table) -> rusqlite::Result<()> {
    let mut statement = connection
        .prepare("SELECT name,type,\"notnull\",pk,hidden FROM pragma_table_xinfo(?1) LIMIT 257")?;
    table.columns = statement
        .query_map([&table.name], column)?
        .collect::<rusqlite::Result<_>>()?;
    let mut statement = connection
        .prepare("SELECT \"from\",\"table\",\"to\" FROM pragma_foreign_key_list(?1) LIMIT 256")?;
    table.relationships = statement
        .query_map([&table.name], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get::<_, Option<String>>(2)?
                    .unwrap_or_else(|| "primary key".into()),
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(())
}
fn column(row: &rusqlite::Row<'_>) -> rusqlite::Result<Column> {
    Ok(Column {
        name: row.get(0)?,
        kind: row.get(1)?,
        required: row.get::<_, i64>(2)? != 0,
        primary: row.get::<_, i64>(3)? != 0,
        hidden: row.get::<_, i64>(4)? != 0,
    })
}
fn quote(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}
pub(super) fn clipped(value: &str) -> String {
    if value.len() <= 512 {
        value.into()
    } else {
        format!("{}…", &value[..value.floor_char_boundary(512)])
    }
}
fn cell(value: ValueRef<'_>) -> String {
    match value {
        ValueRef::Null => "NULL".into(),
        ValueRef::Integer(value) => value.to_string(),
        ValueRef::Real(value) => value.to_string(),
        ValueRef::Text(bytes) => clipped(&String::from_utf8_lossy(bytes)),
        ValueRef::Blob(bytes) => format!("BLOB · {} bytes", bytes.len()),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn document(sql: &str) -> Document {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(sql).unwrap();
        let bytes = connection.serialize(DatabaseName::Main).unwrap();
        Document::parse(bytes.to_vec()).unwrap()
    }
    #[test]
    fn quoted_names_values_and_relationships_survive_read_only_paging() {
        let document = document(
            "CREATE TABLE ports(id INTEGER PRIMARY KEY,name TEXT); CREATE TABLE \"route\"\"name\"(id INTEGER,port INTEGER REFERENCES ports(id),payload BLOB); INSERT INTO ports VALUES(1,'Harbor'),(2,NULL); INSERT INTO \"route\"\"name\" VALUES(7,1,x'001122');",
        );
        assert_eq!(document.tables[1].name, "route\"name");
        assert_eq!(
            document.tables[1].relationships[0],
            ("port".into(), "ports".into(), "id".into())
        );
        assert_eq!(document.page(0, 0).unwrap().rows[1][1], "NULL");
        assert_eq!(document.page(1, 0).unwrap().rows[0][2], "BLOB · 3 bytes");
        assert!(document.page(99, 0).is_err());
        assert!(document.page(0, 1_000_001).is_err());
    }
    #[test]
    fn views_virtual_tables_and_generated_expressions_are_not_evaluated() {
        let document = document(
            "CREATE TABLE data(id INTEGER,generated TEXT AS (printf('%999999999s',id))); INSERT INTO data(id) VALUES(1); CREATE VIEW expanded AS SELECT randomblob(1000000000) FROM data; CREATE VIRTUAL TABLE search USING fts5(content);",
        );
        assert_eq!(document.page(0, 0).unwrap().rows, vec![vec!["1"]]);
        for (index, table) in document
            .tables
            .iter()
            .enumerate()
            .filter(|(_, t)| t.columns.is_empty())
        {
            assert!(document.page(index, 0).is_err(), "{}", table.name);
        }
    }
    #[test]
    fn snapshot_connection_refuses_mutation_and_additional_databases() {
        let document = document("CREATE TABLE data(value TEXT); INSERT INTO data VALUES('kept');");
        let connection = open(&document.bytes).unwrap();
        assert!(connection.execute_batch("DELETE FROM data").is_err());
        assert!(
            connection
                .execute_batch("ATTACH DATABASE ':memory:' AS another")
                .is_err()
        );
        assert_eq!(document.page(0, 0).unwrap().rows, vec![vec!["kept"]]);
    }
    #[test]
    fn wal_headers_are_disclosed_and_only_the_copy_is_adjusted() {
        let original = document(
            "CREATE TABLE data(value TEXT); INSERT INTO data VALUES('checkpoint'); PRAGMA user_version=7; PRAGMA application_id=42;",
        );
        let mut bytes = original.bytes.to_vec();
        bytes[18] = 2;
        bytes[19] = 2;
        let document = Document::parse(bytes).unwrap();
        assert!(document.wal);
        assert_eq!((document.user_version, document.application_id), (7, 42));
        assert_eq!(document.page(0, 0).unwrap().rows, vec![vec!["checkpoint"]]);
        assert_eq!(&document.bytes[18..20], &[2, 2]);
    }
    #[test]
    fn pages_are_bounded_and_large_text_is_marked_without_copying_blobs() {
        let document = document(
            "CREATE TABLE data(value TEXT,payload BLOB); WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<205) INSERT INTO data SELECT printf('%0600d',x),zeroblob(1024) FROM n;",
        );
        let first = document.page(0, 0).unwrap();
        assert!(first.more);
        assert_eq!(first.rows.len(), PAGE_ROWS);
        assert!(first.rows[0][0].ends_with('…'));
        assert_eq!(first.rows[0][1], "BLOB · 1024 bytes");
        assert_eq!(document.page(0, PAGE_ROWS).unwrap().rows.len(), 5);
        assert!(Document::parse(b"not a database".to_vec()).is_err());
    }
}
