//! Versioned disk index. Source progress and compressed cell blocks share one transaction.
use super::{BRIDGE, COLS, Class, DATABASE, Label, SIDE, Stroke, TUNNEL, manifest};
use little_clip_core::{
    geography::basemap::{storage::AssetDirectory, tiles::TileId},
    kernel_interface::open_content,
};
use rusqlite::{Connection, OpenFlags, limits::Limit, params};
use std::{collections::BTreeMap, ffi::OsStr};

pub(super) const BLOCK_BYTES: usize = 1 << 20;
pub(super) fn read_only(dir: &AssetDirectory) -> Result<Connection, String> {
    let path = dir.path().join(DATABASE);
    let _source = open_content(&path, false).map_err(err)?;
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(err)?;
    db.execute_batch("PRAGMA trusted_schema=OFF; PRAGMA query_only=ON;")
        .map_err(err)?;
    db.set_limit(Limit::SQLITE_LIMIT_LENGTH, (BLOCK_BYTES * 2 + 4096) as i32)
        .map_err(err)?;
    Ok(db)
}

pub(super) fn writer(dir: &AssetDirectory) -> Result<Connection, String> {
    let path = dir.path().join(DATABASE);
    let _database = match open_content(&path, false) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let staged = dir.stage().map_err(err)?;
            dir.install(staged, OsStr::new(DATABASE)).map_err(err)?
        }
        Err(error) => return Err(err(error)),
    };
    // SQLite rejects OPEN_NOFOLLOW on the deliberately pinned /proc parent.
    // The guarded file is already present, so SQLite only opens an existing database.
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE).map_err(err)?;
    db.execute_batch(
        "PRAGMA trusted_schema=OFF;
         PRAGMA journal_mode=DELETE;
         PRAGMA synchronous=FULL;
         PRAGMA cache_size=-8192;
         CREATE TABLE IF NOT EXISTS imported(shard INTEGER PRIMARY KEY);
         CREATE TABLE IF NOT EXISTS metadata(release TEXT PRIMARY KEY, attribution TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS tile(
             cell INTEGER, shard INTEGER, seq INTEGER, data BLOB NOT NULL,
             PRIMARY KEY(cell, shard, seq)
         ) WITHOUT ROWID;",
    )
    .map_err(err)?;
    Ok(db)
}

pub(super) fn is_complete(db: &Connection) -> rusqlite::Result<bool> {
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM metadata WHERE release = ?1)",
        [&manifest().release],
        |row| row.get(0),
    )
}

pub(super) fn finish(db: &Connection) -> Result<(), String> {
    db.execute(
        "INSERT OR REPLACE INTO metadata VALUES (?1,?2)",
        [&manifest().release, &manifest().attribution],
    )
    .map_err(err)?;
    Ok(())
}

fn cell(tile: TileId) -> u32 {
    tile.y * COLS + tile.x
}

pub(super) fn write(
    db: &Connection,
    tile: TileId,
    shard: usize,
    seq: usize,
    block: &[u8],
) -> Result<(), String> {
    let data = zstd::bulk::compress(block, 3).map_err(err)?;
    db.prepare_cached("INSERT INTO tile VALUES (?1, ?2, ?3, ?4)")
        .and_then(|mut s| s.execute(params![cell(tile), shard, seq, data]))
        .map_err(err)?;
    Ok(())
}

pub(super) fn read(
    db: &Connection,
    tile: TileId,
    mut consume: impl FnMut(&[u8]) -> Result<(), String>,
) -> Result<(), String> {
    let mut query = db
        .prepare("SELECT CASE WHEN length(data)<=?2 THEN data END FROM tile WHERE cell=?1 ORDER BY shard, seq")
        .map_err(err)?;
    let mut rows = query
        .query(params![cell(tile), BLOCK_BYTES * 2])
        .map_err(err)?;
    let mut decoder = zstd::bulk::Decompressor::new().map_err(err)?;
    while let Some(row) = rows.next().map_err(err)? {
        let value = row.get_ref(0).map_err(err)?;
        let compressed = value.as_blob().map_err(err)?;
        let block = decoder
            .decompress(compressed, BLOCK_BYTES * 2)
            .map_err(err)?;
        consume(&block)?;
    }
    Ok(())
}

fn err(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Attributes<'a> {
    pub class: Class,
    pub stack: i8,
    pub flags: u8,
    pub name: &'a str,
    pub reference: &'a str,
}

pub(super) fn encode(out: &mut Vec<u8>, feature: &Attributes<'_>, points: &[[u16; 2]]) {
    let short = |s: &str| s.floor_char_boundary(s.len().min(192));
    let name = &feature.name.as_bytes()[..short(feature.name)];
    let reference = &feature.reference.as_bytes()[..short(feature.reference)];
    out.extend_from_slice(&[
        feature.class as u8,
        feature.stack as u8,
        feature.flags,
        name.len() as u8,
        reference.len() as u8,
    ]);
    out.extend_from_slice(&(points.len() as u16).to_le_bytes());
    out.extend_from_slice(name);
    out.extend_from_slice(reference);
    for position in points {
        for coordinate in position {
            out.extend_from_slice(&coordinate.to_le_bytes());
        }
    }
}

pub(super) struct Record<'a> {
    feature: Attributes<'a>,
    pub points: Vec<[f32; 2]>,
}

pub(super) fn records(
    mut bytes: &[u8],
    tile: TileId,
) -> impl Iterator<Item = Result<Record<'_>, String>> {
    std::iter::from_fn(move || {
        if bytes.is_empty() {
            return None;
        }
        let result = decode(&mut bytes, tile);
        if result.is_none() {
            bytes = &[];
        }
        Some(result.ok_or_else(|| "Invalid transportation tile record".into()))
    })
}

fn decode<'a>(input: &mut &'a [u8], tile: TileId) -> Option<Record<'a>> {
    let [class, stack, flags, name_len, ref_len, lo, hi] = input.get(..7)?.try_into().ok()?;
    let class = *Class::ALL.get(class as usize)?;
    let count = u16::from_le_bytes([lo, hi]) as usize;
    let name_end = 7 + name_len as usize;
    let coords_start = name_end + ref_len as usize;
    let end = coords_start + count * 4;
    let name = std::str::from_utf8(input.get(7..name_end)?).ok()?;
    let reference = std::str::from_utf8(input.get(name_end..coords_start)?).ok()?;
    let [west, north, _] = tile.bounds();
    let points = input
        .get(coords_start..end)?
        .chunks_exact(4)
        .map(|p| {
            [
                (west + SIDE * f64::from(u16::from_le_bytes([p[0], p[1]])) / 65535.0) as f32,
                (north - SIDE * f64::from(u16::from_le_bytes([p[2], p[3]])) / 65535.0) as f32,
            ]
        })
        .collect();
    *input = &input[end..];
    Some(Record {
        feature: Attributes {
            class,
            stack: stack as i8,
            flags,
            name,
            reference,
        },
        points,
    })
}

impl Record<'_> {
    pub fn append(&self, batches: &mut BTreeMap<Stroke, Vec<f32>>) {
        let Attributes {
            class,
            stack,
            flags,
            ..
        } = self.feature;
        if class.is_place() {
            return;
        }
        for fill in [false, true] {
            if !fill && (flags & BRIDGE == 0 && stack <= 0 || flags & TUNNEL != 0) {
                continue;
            }
            let data = batches
                .entry(Stroke {
                    stack,
                    fill,
                    class,
                    flags,
                })
                .or_default();
            for pair in self.points.windows(2) {
                data.extend_from_slice(&[pair[0][0], pair[0][1], pair[1][0], pair[1][1], 0.0, 0.0]);
            }
        }
    }

    pub fn label(&self) -> Option<Label> {
        let Attributes {
            name,
            reference,
            class,
            ..
        } = self.feature;
        let text = match (name, reference) {
            ("", "") => return None,
            ("", reference) => reference.into(),
            (name, "") => name.into(),
            (name, reference) if name == reference => name.into(),
            (name, reference) => format!("{name} · {reference}"),
        };
        let (a, b) = if class.is_place() {
            let p = *self.points.first()?;
            (p, p)
        } else {
            let length =
                |pair: &[[f32; 2]]| (pair[0][0] - pair[1][0]).hypot(pair[0][1] - pair[1][1]);
            let best = self
                .points
                .windows(2)
                .max_by(|a, b| length(a).total_cmp(&length(b)))?;
            (best[0], best[1])
        };
        Some(Label { class, text, a, b })
    }
}
