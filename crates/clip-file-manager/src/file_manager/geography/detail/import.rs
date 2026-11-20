//! Bounded GeoParquet import. Attribute ranges are resolved in source order
//! and WGS84 linear distance before the resulting paths are clipped to cells.
use super::{
    BRIDGE, COLS, Class, Kind, LEVEL, SIDE, TUNNEL,
    store::{self, Attributes},
};
use geographiclib_rs::{Geodesic, InverseGeodesic};
use little_clip_core::geography::basemap::{
    storage::AssetDirectory, tiles::TileId, wrap_lon_delta,
};
use parquet::{
    file::reader::{FileReader, SerializedFileReader},
    record::{Field, Row},
    schema::types::Type,
};
use rusqlite::Connection;
use std::{
    collections::BTreeMap,
    fs::File,
    sync::atomic::{AtomicBool, Ordering},
};

type Point = [f64; 2];
const MEMORY: usize = 24 << 20;

pub(crate) fn open(directory: &AssetDirectory) -> Result<Connection, String> {
    store::writer(directory)
}

pub(crate) fn imported(db: &Connection, shard: usize) -> Result<bool, String> {
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM imported WHERE shard=?1)",
        [shard],
        |row| row.get(0),
    )
    .map_err(error)
}

pub(crate) fn finish(db: &Connection) -> Result<(), String> {
    let count: usize = db
        .query_row("SELECT COUNT(*) FROM imported", [], |row| row.get(0))
        .map_err(error)?;
    if count != super::manifest().sources.len() {
        return Err("Transportation import has incomplete source coverage".into());
    }
    store::finish(db)
}

pub(crate) fn file(
    db: &mut Connection,
    file: File,
    kind: Kind,
    shard: usize,
    max_final_bytes: u64,
    cancel: &AtomicBool,
) -> Result<(), String> {
    limit_pages(db, max_final_bytes)?;
    let reader = SerializedFileReader::new(file).map_err(error)?;
    let schema = reader.metadata().file_metadata().schema();
    let projection = projection(kind, schema)?;
    let tx = db.transaction().map_err(error)?;
    let mut sink = Sink {
        db: &tx,
        shard,
        parts: BTreeMap::new(),
        bytes: 0,
        seq: 0,
    };
    let geodesic = Geodesic::wgs84();
    for row in reader.get_row_iter(Some(projection)).map_err(error)? {
        if cancel.load(Ordering::Relaxed) {
            return Err("Transportation import cancelled".into());
        }
        let row = row.map_err(error)?;
        match kind {
            Kind::Segment => line(&row, &geodesic, &mut sink)?,
            Kind::Division => place(&row, &mut sink)?,
        }
    }
    sink.flush()?;
    tx.execute("INSERT INTO imported VALUES (?1)", [shard])
        .map_err(error)?;
    tx.commit().map_err(error)
}

fn limit_pages(db: &Connection, max_final_bytes: u64) -> Result<(), String> {
    let page: u64 = db
        .pragma_query_value(None, "page_size", |row| row.get(0))
        .map_err(error)?;
    let limit = max_final_bytes / page;
    let accepted: u64 = db
        .query_row(&format!("PRAGMA max_page_count={limit}"), [], |row| {
            row.get(0)
        })
        .map_err(error)?;
    if accepted > limit || limit == 0 {
        return Err("Transportation index exceeds geography.max_assets_bytes".into());
    }
    Ok(())
}

fn projection(kind: Kind, schema: &Type) -> Result<Type, String> {
    let names: &[&str] = match kind {
        Kind::Segment => &[
            "geometry",
            "subtype",
            "class",
            "subclass",
            "subclass_rules",
            "names",
            "routes",
            "level_rules",
            "road_flags",
            "rail_flags",
        ],
        Kind::Division => &["geometry", "subtype", "class", "names"],
    };
    let fields: Vec<_> = schema
        .get_fields()
        .iter()
        .filter(|f| names.contains(&f.name()))
        .cloned()
        .collect();
    if fields.len() != names.len() {
        return Err("Unexpected Overture GeoParquet schema".into());
    }
    Type::group_type_builder(schema.name())
        .with_fields(fields)
        .build()
        .map_err(error)
}

fn error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

struct Sink<'a> {
    db: &'a Connection,
    shard: usize,
    parts: BTreeMap<TileId, Vec<u8>>,
    bytes: usize,
    seq: usize,
}

impl Sink<'_> {
    fn push(
        &mut self,
        key: TileId,
        attributes: &Attributes<'_>,
        coords: &[[u16; 2]],
    ) -> Result<(), String> {
        // Bound a single record and preserve the joint between any oversized pieces.
        let step = 16_383;
        for start in (0..coords.len()).step_by(step) {
            let chunk = &coords[start..(start + step + 1).min(coords.len())];
            if chunk.len() == 1 && !attributes.class.is_place() {
                break;
            }
            let buf = self.parts.entry(key).or_default();
            let prior = buf.len();
            store::encode(buf, attributes, chunk);
            self.bytes += buf.len() - prior;
            if buf.len() >= store::BLOCK_BYTES {
                store::write(self.db, key, self.shard, self.seq, buf)?;
                self.seq += 1;
                self.bytes -= buf.len();
                self.parts.remove(&key);
            }
        }
        if self.bytes > MEMORY {
            self.flush()?;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), String> {
        for (key, part) in std::mem::take(&mut self.parts) {
            store::write(self.db, key, self.shard, self.seq, &part)?;
            self.seq += 1;
        }
        self.bytes = 0;
        Ok(())
    }
}

fn get<'a>(row: &'a Row, name: &str) -> Option<&'a Field> {
    row.get_column_iter()
        .find(|(key, _)| *key == name)
        .map(|(_, value)| value)
}

fn text<'a>(row: &'a Row, name: &str) -> Option<&'a str> {
    match get(row, name)? {
        Field::Str(value) => Some(value),
        _ => None,
    }
}

fn names(row: &Row) -> Option<&Row> {
    match get(row, "names")? {
        Field::Group(value) => Some(value),
        _ => None,
    }
}

fn list<'a>(row: &'a Row, field: &str) -> &'a [Field] {
    match get(row, field) {
        Some(Field::ListInternal(list)) => list.elements(),
        _ => &[],
    }
}

fn rules<'a>(row: &'a Row, field: &str) -> impl Iterator<Item = &'a Row> {
    list(row, field).iter().filter_map(|value| match value {
        Field::Group(row) => Some(row),
        _ => None,
    })
}

#[derive(Clone, Copy)]
struct Rule<T> {
    between: [f64; 2],
    value: T,
}

fn rule<T>(row: &Row, value: T) -> Option<Rule<T>> {
    let between = match list(row, "between") {
        [] => [0.0, 1.0],
        [Field::Double(a), Field::Double(b)] if 0.0 <= *a && a < b && *b <= 1.0 => [*a, *b],
        _ => return None,
    };
    Some(Rule { between, value })
}

impl<T> Rule<T> {
    fn applies(&self, t: f64) -> bool {
        self.between[0] <= t && t <= self.between[1]
    }
}

fn place(row: &Row, sink: &mut Sink<'_>) -> Result<(), String> {
    let class = match text(row, "subtype") {
        Some("neighborhood" | "microhood" | "macrohood" | "borough" | "localadmin") => {
            Class::Neighborhood
        }
        Some("locality") if text(row, "class") == Some("city") => Class::City,
        Some("locality") => Class::Town,
        _ => return Ok(()),
    };
    let Some(name) = names(row)
        .and_then(|row| text(row, "primary"))
        .filter(|value| !value.is_empty())
    else {
        return Ok(());
    };
    let mut points = coordinates(row, 1)?;
    let point = grid(points.next().ok_or("Missing Division coordinate")?);
    let xy = position(point);
    sink.push(
        tile(xy),
        &Attributes {
            class,
            stack: 0,
            flags: 0,
            name,
            reference: "",
        },
        &[quantize(point, xy)],
    )
}

fn road_class(class: Option<&str>) -> Option<Class> {
    Some(match class? {
        "motorway" => Class::Motorway,
        "trunk" => Class::Trunk,
        "primary" => Class::Primary,
        "secondary" => Class::Secondary,
        "tertiary" => Class::Tertiary,
        "residential" => Class::Residential,
        "unclassified" => Class::Unclassified,
        "living_street" => Class::LivingStreet,
        "service" => Class::Service,
        "track" => Class::Track,
        "pedestrian" => Class::Pedestrian,
        "unknown" => Class::Unknown,
        _ => return None,
    })
}

fn segment_class(row: &Row) -> Option<Class> {
    match (text(row, "subtype"), text(row, "class")) {
        (Some("road"), class) => road_class(class),
        (Some("rail"), _) => Some(Class::Rail),
        // Overture v2 does not give water routes a class, including ferries.
        (Some("water"), _) => Some(Class::WaterRoute),
        _ => None,
    }
}

fn excluded_subclass(subclass: &str) -> bool {
    matches!(
        subclass,
        "driveway" | "parking_aisle" | "sidewalk" | "steps"
    )
}

struct Scope<'a> {
    class: Class,
    excluded: Vec<Rule<()>>,
    levels: Vec<Rule<i8>>,
    flags: Vec<Rule<u8>>,
    refs: Vec<Rule<&'a str>>,
    names: Vec<Rule<&'a str>>,
    primary: &'a str,
}

impl<'a> Scope<'a> {
    fn from_row(row: &'a Row, class: Class) -> Self {
        let subclass: Vec<_> = rules(row, "subclass_rules")
            .filter(|r| text(r, "value").is_some_and(excluded_subclass))
            .filter_map(|r| rule(r, ()))
            .collect();
        let levels: Vec<_> = rules(row, "level_rules")
            .filter_map(|r| {
                let Field::Int(level) = get(r, "value")? else {
                    return None;
                };
                rule(r, (*level).clamp(-32, 32) as i8)
            })
            .collect();
        let flags: Vec<_> = rules(
            row,
            if class == Class::Rail {
                "rail_flags"
            } else {
                "road_flags"
            },
        )
        .filter_map(|r| {
            let values = list(r, "values").iter().fold(0, |flags, value| {
                flags
                    | match value {
                        Field::Str(v) if v == "is_bridge" => BRIDGE,
                        Field::Str(v) if v == "is_tunnel" => TUNNEL,
                        _ => 0,
                    }
            });
            (values != 0).then(|| rule(r, values)).flatten()
        })
        .collect();
        let refs: Vec<_> = rules(row, "routes")
            .filter_map(|r| rule(r, text(r, "ref")?))
            .collect();
        let names = names(row);
        let primary = names.and_then(|r| text(r, "primary")).unwrap_or_default();
        let names: Vec<_> = names
            .into_iter()
            .flat_map(|n| rules(n, "rules"))
            .filter(|r| {
                matches!(text(r, "variant"), Some("common" | "official"))
                    && !list(r, "between").is_empty()
            })
            .filter_map(|r| rule(r, text(r, "value")?))
            .collect();
        Self {
            class,
            excluded: subclass,
            levels,
            flags,
            refs,
            names,
            primary,
        }
    }

    fn stops(&self) -> Vec<f64> {
        let mut stops = vec![0.0, 1.0];
        stops.extend(self.excluded.iter().flat_map(|r| r.between));
        stops.extend(self.levels.iter().flat_map(|r| r.between));
        stops.extend(self.flags.iter().flat_map(|r| r.between));
        stops.extend(self.refs.iter().flat_map(|r| r.between));
        stops.extend(self.names.iter().flat_map(|r| r.between));
        stops.sort_unstable_by(f64::total_cmp);
        stops.dedup();
        stops
    }

    fn at<'b>(&self, t: f64, refs: &'b mut String) -> Option<Attributes<'b>>
    where
        'a: 'b,
    {
        if self.excluded.iter().any(|rule| rule.applies(t)) {
            return None;
        }
        let flags = self
            .flags
            .iter()
            .filter(|r| r.applies(t))
            .fold(0, |flags, r| flags | r.value);
        let stack = self.levels.iter().find(|r| r.applies(t)).map_or_else(
            || i8::from(flags & BRIDGE != 0) - i8::from(flags & TUNNEL != 0),
            |r| r.value,
        );
        let name = self
            .names
            .iter()
            .find(|r| r.applies(t))
            .map_or(self.primary, |r| r.value);
        refs.clear();
        for rule in self.refs.iter().filter(|r| r.applies(t)).take(12) {
            if refs.split(" / ").any(|existing| existing == rule.value) {
                continue;
            }
            if !refs.is_empty() {
                refs.push_str(" / ");
            }
            refs.push_str(rule.value);
        }
        Some(Attributes {
            class: self.class,
            stack,
            flags,
            name,
            reference: refs,
        })
    }
}

fn line(row: &Row, geodesic: &Geodesic, sink: &mut Sink<'_>) -> Result<(), String> {
    let Some(class) = segment_class(row) else {
        return Ok(());
    };
    if text(row, "subclass").is_some_and(excluded_subclass) {
        return Ok(());
    }
    let points: Vec<_> = coordinates(row, 2)?.collect();
    draw_scoped(&Scope::from_row(row, class), &points, geodesic, sink)
}

fn draw_scoped(
    scope: &Scope<'_>,
    points: &[Point],
    geodesic: &Geodesic,
    sink: &mut Sink<'_>,
) -> Result<(), String> {
    let stops = scope.stops();
    let path_distances = (stops.len() > 2).then(|| distances(points, geodesic));
    let mut refs_text = String::new();
    for stop in stops.windows(2) {
        let Some(draw) = scope.at((stop[0] + stop[1]) * 0.5, &mut refs_text) else {
            continue;
        };
        if let Some(distances) = &path_distances {
            let part = slice(points, distances, stop[0], stop[1]);
            clip(&part, &draw, sink)?;
        } else {
            clip(points, &draw, sink)?;
        }
    }
    Ok(())
}

fn coordinates(row: &Row, kind: u32) -> Result<impl Iterator<Item = Point>, String> {
    let Some(Field::Bytes(value)) = get(row, "geometry") else {
        return Err("Overture geometry missing".into());
    };
    let wkb = value.data();
    let order = *wkb.first().ok_or("Empty Overture geometry")?;
    if order > 1 {
        return Err("Invalid Overture WKB byte order".into());
    }
    let get_int = |i: usize| {
        let raw = wkb.get(i..i + 4)?.try_into().ok()?;
        Some(if order == 1 {
            u32::from_le_bytes(raw)
        } else {
            u32::from_be_bytes(raw)
        })
    };
    if get_int(1) != Some(kind) {
        return Err("Unexpected Overture WKB geometry type".into());
    }
    let (start, count) = if kind == 1 {
        (5, 1)
    } else {
        (9, get_int(5).ok_or("Invalid Overture WKB length")? as usize)
    };
    if count > 250_000 || wkb.len() != start + count * 16 {
        return Err("Invalid or oversized Overture WKB geometry".into());
    }
    let read = move |b: &[u8]| {
        let bytes = [b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]];
        if order == 1 {
            f64::from_le_bytes(bytes)
        } else {
            f64::from_be_bytes(bytes)
        }
    };
    let points = wkb[start..]
        .chunks_exact(16)
        .map(move |p| [read(&p[..8]), read(&p[8..])]);
    if !points.clone().all(valid_position) {
        return Err("Out-of-range Overture position".into());
    }
    Ok(points)
}

fn valid_position([lon, lat]: Point) -> bool {
    (-180.0..=180.0).contains(&lon) && (-90.0..=90.0).contains(&lat)
}

fn distances(points: &[Point], earth: &Geodesic) -> Vec<f64> {
    let mut d = 0.0;
    std::iter::once(0.0)
        .chain(points.windows(2).map(|pair| {
            d += InverseGeodesic::<f64>::inverse(
                earth, pair[0][1], pair[0][0], pair[1][1], pair[1][0],
            );
            d
        }))
        .collect()
}

fn slice(points: &[Point], distances: &[f64], low: f64, high: f64) -> Vec<Point> {
    let Some(&last) = distances.last() else {
        return Vec::new();
    };
    if last <= 0.0 {
        return Vec::new();
    }
    let start = low * last;
    let end = high * last;
    let a = distances.partition_point(|&v| v <= start).saturating_sub(1);
    let b = distances
        .partition_point(|&v| v < end)
        .min(points.len() - 1);
    let at = |i: usize, d: f64| {
        let next = (i + 1).min(points.len() - 1);
        let span = distances[next] - distances[i];
        let f = if span > 0.0 {
            (d - distances[i]) / span
        } else {
            0.0
        };
        let [x, y] = points[i];
        let [nx, ny] = points[next];
        let lon = x + wrap_lon_delta(nx - x) * f;
        [wrap_lon_delta(lon), y + (ny - y) * f]
    };
    std::iter::once(at(a, start))
        .chain(points[(a + 1).min(b)..b].iter().copied())
        .chain(std::iter::once(at(b.saturating_sub(1), end)))
        .collect()
}

fn grid([lon, lat]: Point) -> Point {
    [(lon + 180.0) / SIDE, (90.0 - lat) / SIDE]
}
fn position([x, y]: Point) -> [i32; 2] {
    [
        x.floor() as i32,
        (y.floor() as i32).clamp(0, (COLS / 2 - 1) as i32),
    ]
}
fn tile([x, y]: [i32; 2]) -> TileId {
    TileId {
        level: LEVEL,
        x: x.rem_euclid(COLS as i32) as u32,
        y: y as u32,
    }
}
fn quantize(point: Point, cell: [i32; 2]) -> [u16; 2] {
    std::array::from_fn(|i| {
        ((point[i] - f64::from(cell[i])) * 65535.0)
            .round()
            .clamp(0.0, 65535.0) as u16
    })
}

fn clip(points: &[Point], attributes: &Attributes<'_>, sink: &mut Sink<'_>) -> Result<(), String> {
    let Some(&first) = points.first() else {
        return Ok(());
    };
    let mut a = grid(first);
    let mut cell = position(a);
    let mut section = Vec::new();
    let mut stops = Vec::new();
    const PERIOD: f64 = COLS as f64;
    for &point in &points[1..] {
        let mut b = grid(point);
        b[0] += ((a[0] - b[0]) / PERIOD).round() * PERIOD;
        let dx = b[0] - a[0];
        let dy = b[1] - a[1];
        stops.clear();
        stops.extend([0.0, 1.0]);
        for (start, end) in [(a[0], b[0]), (a[1], b[1])] {
            let low = start.min(end).floor() as i32 + 1;
            let high = start.max(end).ceil() as i32;
            stops.extend((low..high).map(|boundary| (f64::from(boundary) - start) / (end - start)));
        }
        stops.sort_unstable_by(f64::total_cmp);
        stops.dedup();
        for sub in stops.windows(2) {
            let at = |t| [a[0] + dx * t, a[1] + dy * t];
            let next = position(at((sub[0] + sub[1]) * 0.5));
            if cell != next {
                sink.push(tile(cell), attributes, &section)?;
                section.clear();
                cell = next;
            }
            let start = quantize(at(sub[0]), cell);
            let end = quantize(at(sub[1]), cell);
            if section.last() != Some(&start) {
                section.push(start);
            }
            if section.last() != Some(&end) {
                section.push(end);
            }
        }
        a = b;
    }
    sink.push(tile(cell), attributes, &section)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scopes_apply_before_dateline_clipping_and_unnamed_grid_streets_survive() {
        let root = tempfile::tempdir().unwrap();
        let directory = AssetDirectory::open(root.path()).unwrap();
        let db = open(&directory).unwrap();
        let mut sink = Sink {
            db: &db,
            shard: 0,
            parts: BTreeMap::new(),
            bytes: 0,
            seq: 0,
        };
        let between = [0.45, 0.55];
        let scope = Scope {
            class: Class::Residential,
            excluded: Vec::new(),
            levels: vec![Rule { between, value: 2 }],
            flags: vec![Rule {
                between,
                value: BRIDGE,
            }],
            refs: vec![Rule {
                between,
                value: "R-4",
            }],
            names: Vec::new(),
            primary: "",
        };
        draw_scoped(
            &scope,
            &[[179.8, 0.05], [-179.8, 0.05]],
            &Geodesic::wgs84(),
            &mut sink,
        )
        .unwrap();
        sink.flush().unwrap();
        assert!(!store::is_complete(&db).unwrap());
        assert!(finish(&db).is_err());
        store::finish(&db).unwrap();

        let source = super::super::Status::load(root.path()).source.unwrap();
        let loads = |lon| {
            source
                .prepare(TileId::at(LEVEL, 0.05, lon), &AtomicBool::new(false))
                .unwrap()
        };
        let east = loads(179.92);
        let west = loads(-179.92);
        for tile in [east, west] {
            assert!(tile.labels.iter().any(|label| label.text == "R-4"));
            assert!(tile.batches.iter().any(|batch| matches!(
                batch.layer,
                super::super::Layer::Detailed(stroke) if stroke.stack == 2 && stroke.flags == BRIDGE)));
            assert!(tile.batches.iter().any(|batch| matches!(
                batch.layer, super::super::Layer::Detailed(stroke) if stroke.stack == 0 && stroke.fill)));
        }
        assert!(loads(0.0).batches.is_empty());
    }

    #[test]
    fn pinned_manifest_accounts_for_all_segments_and_division_points() {
        let manifest = super::super::manifest();
        assert_eq!(manifest.sources.len(), 129);
        assert_eq!(
            manifest
                .sources
                .iter()
                .map(|object| object.bytes)
                .sum::<u64>(),
            super::super::DOWNLOAD_BYTES
        );
        assert!(
            manifest
                .sources
                .iter()
                .all(|entry| entry.bytes < super::super::STAGING_BYTES)
        );
        assert!(manifest.sources.iter().all(|entry| {
            let suffix = match entry.kind {
                Kind::Segment => "theme=transportation/type=segment",
                Kind::Division => "theme=divisions/type=division",
            };
            entry
                .key
                .starts_with(&format!("release/{}/{suffix}/", manifest.release))
                && entry.key.ends_with(".parquet")
        }));
    }

    #[test]
    #[ignore = "requires local row-group fixtures from the pinned Overture release"]
    fn imports_overture_pilot_and_reads_it_offline() {
        let fixture = std::path::PathBuf::from(
            std::env::var_os("LITTLE_CLIP_OVERTURE_FIXTURE").expect("Set the fixture root"),
        );
        let root = tempfile::tempdir().unwrap();
        let dir = AssetDirectory::open(root.path()).unwrap();
        let mut db = open(&dir).unwrap();
        let cancel = AtomicBool::new(false);
        let mut original_bytes = 0;
        for (shard, kind) in [
            (16, Kind::Segment),
            (17, Kind::Segment),
            (40, Kind::Segment),
            (101, Kind::Segment),
            (125, Kind::Segment),
            (126, Kind::Segment),
            (128, Kind::Division),
        ] {
            let partition = if matches!(kind, Kind::Segment) {
                "segment"
            } else {
                "division"
            };
            let source =
                File::open(fixture.join(format!("pilot-{partition}-{shard}.parquet"))).unwrap();
            original_bytes += source.metadata().unwrap().len();
            file(&mut db, source, kind, shard, 1 << 30, &cancel).unwrap();
            assert!(imported(&db, shard).unwrap());
        }
        store::finish(&db).unwrap();
        drop(db);
        let status = super::super::Status::load(root.path());
        println!(
            "Pilot Parquet input: {original_bytes} bytes; installed index: {} bytes",
            status.bytes
        );
        let source = status.source.unwrap();
        for (name, lat, lon) in [
            ("Seattle", 47.61, -122.34),
            ("Lisbon", 38.71, -9.14),
            ("Tokyo", 35.68, 139.77),
            ("Sydney", -33.86, 151.21),
        ] {
            let cell = TileId::at(LEVEL, lat, lon);
            let mut labels = Vec::new();
            let mut strokes = Vec::new();
            for y in cell.y - 1..=cell.y + 1 {
                for x in cell.x - 1..=cell.x + 1 {
                    let tile = source.prepare(TileId { x, y, ..cell }, &cancel).unwrap();
                    labels.extend(tile.labels.into_iter().map(|label| label.text));
                    strokes.extend(tile.batches.into_iter().filter_map(|batch| {
                        let super::super::Layer::Detailed(stroke) = batch.layer else {
                            return None;
                        };
                        Some(stroke)
                    }));
                }
            }
            assert!(
                strokes.iter().any(|s| s.class == Class::Residential),
                "{name}: street grid"
            );
            assert!(
                strokes.iter().any(|s| s.class == Class::Rail),
                "{name}: rail"
            );
            assert!(
                strokes.iter().any(|s| s.class == Class::WaterRoute),
                "{name}: ferry/water route"
            );
            assert!(
                strokes.iter().any(|s| s.stack != 0 || s.flags != 0),
                "{name}: grade separation"
            );
            assert!(labels.len() > 10, "{name}: labels");
            println!(
                "{name}: {} rendered stroke groups; {} labels; sample {:?}",
                strokes.len(),
                labels.len(),
                labels.iter().take(5).collect::<Vec<_>>()
            );
        }
        assert!(
            source
                .prepare(TileId::at(LEVEL, 0.0, -150.0), &cancel)
                .unwrap()
                .batches
                .is_empty()
        );
    }
}

#[cfg(test)]
#[test]
#[ignore = "measures the index produced by one complete upstream Parquet object"]
fn measures_complete_overture_shard() {
    let path = std::env::var_os("LITTLE_CLIP_OVERTURE_SHARD").expect("Set the Parquet object path");
    let root = tempfile::tempdir().unwrap();
    let directory = AssetDirectory::open(root.path()).unwrap();
    let mut db = open(&directory).unwrap();
    let input = File::open(path).unwrap();
    let bytes = input.metadata().unwrap().len();
    let start = std::time::Instant::now();
    file(
        &mut db,
        input,
        Kind::Segment,
        0,
        1 << 30,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(imported(&db, 0).unwrap());
    println!(
        "Complete Parquet object: {bytes} bytes; installed index: {} bytes; elapsed: {:.1}s",
        super::database_bytes(&directory),
        start.elapsed().as_secs_f32(),
    );
}
