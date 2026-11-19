//! Read-only SQLite snapshots for file previews.

use rusqlite::{
    Connection, DatabaseName,
    config::DbConfig,
    hooks::{AuthAction, AuthContext, Authorization},
    limits::Limit,
    serialize::OwnedData,
};
use std::{
    ptr::NonNull,
    time::{Duration, Instant},
};

/// Opens bounded, checkpointed SQLite bytes as an in-memory read-only database.
///
/// The caller supplies a size-limited snapshot and a maximum SQLite value size
/// in bytes. SQLite reads only the supplied main-file bytes.
///
/// # Errors
/// Returns an error if SQLite cannot allocate or open the snapshot or apply
/// the connection settings.
pub fn open(bytes: &[u8], max_value_bytes: i32) -> rusqlite::Result<Connection> {
    let mut connection = Connection::open_in_memory()?;
    connection.set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)?;
    connection.set_db_config(DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, false)?;
    connection.set_db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_VIEW, false)?;
    for (limit, value) in [
        (Limit::SQLITE_LIMIT_LENGTH, max_value_bytes),
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

/// Limits a connection to reads and selects that originate outside schema objects.
pub fn authorize_reads(connection: &Connection) {
    connection.authorizer(Some(|context: AuthContext<'_>| {
        if context.accessor.is_some() {
            return Authorization::Deny;
        }
        match context.action {
            AuthAction::Read { .. } | AuthAction::Select => Authorization::Allow,
            _ => Authorization::Deny,
        }
    }));
}
