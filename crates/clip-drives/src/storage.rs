//! Explicit metadata accounting; never part of the frequent status polling path.
use crate::{Result, cancelled, root::Root, store::Store};
use std::{
    os::unix::fs::MetadataExt,
    path::Path,
    sync::{Arc, atomic::AtomicBool},
    time::{Duration, Instant},
};

const LIMIT: Duration = Duration::from_secs(30);
#[cfg(test)]
mod tests;

#[derive(Default)]
struct Size {
    records: u64,
    bytes: u64,
}

fn check(cancel: &AtomicBool, deadline: Instant) -> Result<()> {
    cancelled(cancel)?;
    if Instant::now() >= deadline {
        return Err("Local metadata measurement exceeded its 30-second budget".into());
    }
    Ok(())
}

pub(crate) fn measure(state: Arc<Root>, cancel: Arc<AtomicBool>) -> Result<String> {
    let deadline = Instant::now() + LIMIT;
    check(&cancel, deadline)?;
    state.validate()?;
    // O_PATH pins do not disturb SQLite's process-wide POSIX record locks.
    let directory = state.directory(Path::new(""), false)?;
    let input = state.pin_child(&directory, std::ffi::OsStr::new("drives.sqlite"))?;
    let identity = input.metadata()?;
    if !identity.is_file() {
        return Err("Drive registry database is not an ordinary file".into());
    }
    state_file_bytes(&state, &cancel, deadline)?;
    let db = Store::existing(&state.path.join("drives.sqlite"), true)?;
    let pending = cancel.clone();
    db.0.progress_handler(10_000, Some(move || check(&pending, deadline).is_err()));
    let result = snapshot(&db, &state, &cancel, deadline);
    check(&cancel, deadline)?;
    state.validate()?;
    if !state.same_file(Path::new("drives.sqlite"), identity.dev(), identity.ino())? {
        return Err("Drive registry database was replaced during measurement".into());
    }
    result
}

fn rows(db: &Store, query: &str) -> Result<Size> {
    Ok(db.0.query_row(query, [], |row| {
        Ok(Size {
            records: row.get(0)?,
            bytes: row.get(1)?,
        })
    })?)
}

fn snapshot(db: &Store, state: &Root, cancel: &AtomicBool, deadline: Instant) -> Result<String> {
    let transaction = db.0.unchecked_transaction()?;
    let baseline = rows(
        db,
        "SELECT count(*),coalesce(sum(octet_length(pair)+length(path)+octet_length(hash)),0) FROM baseline",
    )?;
    let inventory = rows(
        db,
        "SELECT count(*),coalesce(sum(octet_length(scan)+length(path)+length(parent)+length(name)+octet_length(json)),0) FROM entries",
    )?;
    let pending = rows(db, "SELECT sum(records),sum(bytes) FROM (
        SELECT count(*) records,coalesce(sum(octet_length(pair)+length(path)+octet_length(id)),0) bytes FROM conflict_paths
        UNION ALL SELECT count(*),coalesce(sum(octet_length(pair)+length(prefix)),0) FROM sync_blocked
        UNION ALL SELECT count(*),coalesce(sum(octet_length(pair)+length(path)),0) FROM sync_dirs
        UNION ALL SELECT count(*),coalesce(sum(octet_length(pair)+length(path)),0) FROM ram_pending)")?;
    let registry = rows(
        db,
        "SELECT count(*),coalesce(sum(octet_length(kind)+octet_length(id)+octet_length(json)),0) FROM entities",
    )?;
    let (catalogs, unavailable) = catalogs(db, state, cancel, deadline)?;
    transaction.commit()?;
    let state_bytes = state_file_bytes(state, cancel, deadline)?;
    Ok(format!(
        "Local drive metadata\nSync baselines: {} paths · {} logical bytes\nSync/RAM inventories: {} entries · {} logical bytes\nPending paths and sync bookkeeping: {} records · {} logical bytes\nDrive registry (settings, jobs and headers): {} records · {} logical bytes\nSaved catalogs (optional): {} snapshots · {} available file bytes · {} unavailable\nRegistry database, policy snapshot and working files: {state_bytes} apparent bytes\nLogical row payloads are contained in registry storage and exclude page/index overhead. Saved catalog files are separate. File sizes can change during measurement.",
        baseline.records,
        baseline.bytes,
        inventory.records,
        inventory.bytes,
        pending.records,
        pending.bytes,
        registry.records,
        registry.bytes,
        catalogs.records,
        catalogs.bytes,
        unavailable
    ))
}

fn state_file_bytes(state: &Root, cancel: &AtomicBool, deadline: Instant) -> Result<u64> {
    let mut bytes = 0_u64;
    for name in [
        "drives.sqlite",
        "drives.sqlite-wal",
        "drives.sqlite-shm",
        "index-policy.json",
    ] {
        check(cancel, deadline)?;
        if let Some(metadata) = state.metadata(Path::new(name))? {
            if !metadata.is_file() {
                return Err("Drive metadata state path changed file type".into());
            }
            bytes = bytes
                .checked_add(metadata.len())
                .ok_or("State file sizes overflowed")?;
        }
    }
    Ok(bytes)
}

fn catalogs(
    db: &Store,
    state: &Root,
    cancel: &AtomicBool,
    deadline: Instant,
) -> Result<(Size, u64)> {
    let mut size = Size::default();
    let mut unavailable = 0;
    let mut after = String::new();
    loop {
        check(cancel, deadline)?;
        let page = db.ids("catalog", &after, 128)?;
        let Some(last) = page.last() else {
            return Ok((size, unavailable));
        };
        after = last.clone();
        for id in page {
            check(cancel, deadline)?;
            uuid::Uuid::parse_str(&id).map_err(|_| "Invalid saved catalog identifier")?;
            size.records += 1;
            let path = Path::new("catalogs").join(format!("{id}.sqlite"));
            match state.metadata(&path) {
                Ok(Some(metadata)) if metadata.is_file() => {
                    size.bytes = size
                        .bytes
                        .checked_add(metadata.len())
                        .ok_or("Catalog file sizes overflowed")?;
                }
                Ok(None) => unavailable += 1,
                Err(crate::Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                    unavailable += 1
                }
                Err(error) => return Err(error),
                Ok(_) => return Err("Saved catalog path changed file type".into()),
            }
        }
    }
}
