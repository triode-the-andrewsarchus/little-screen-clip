//! Regional 1″ HGT downloads, planned from the provider's coverage/ZIP metadata.
//! Planning transfers only the catalog and ZIP directory, never terrain payloads.
use super::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const HOST: &str = "https://viewfinderpanoramas.org/dem1/";
const CATALOG: &str =
    "https://viewfinderpanoramas.org/Coverage%20map%20viewfinderpanoramas_org1.htm";
const HGT_BYTES: u64 = 3601 * 3601 * 2;
const META_LIMIT: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Tile {
    pub name: String,
    pub bytes: u64,
    pub crc: u32,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Archive {
    pub name: String,
    pub download_bytes: u64,
    pub tiles: Vec<Tile>,
}
#[derive(Clone, Debug)]
pub struct Plan {
    pub archives: Vec<Archive>,
    pub download_bytes: u64,
    pub installed_bytes: u64,
    pub peak_bytes: u64,
    pub already_installed: usize,
    pub budget_remaining: u64,
}

fn range(url: &str, start: u64, total: u64, cancel: &AtomicBool) -> Result<Vec<u8>, String> {
    let count = total
        .checked_sub(start)
        .filter(|n| *n <= META_LIMIT as u64)
        .ok_or("Viewfinder ZIP directory exceeds the metadata limit")?;
    let bytes = read_url(
        url,
        &[
            "--range".into(),
            format!("{start}-{}", total - 1),
            "--max-filesize".into(),
            count.to_string(),
        ],
        count as usize + 1,
        cancel,
    )?;
    if bytes.len() != count as usize {
        return Err("Viewfinder server did not supply the requested ZIP metadata range".into());
    }
    Ok(bytes)
}

fn tile_name(name: &str) -> bool {
    let b = name.as_bytes();
    b.len() == 11
        && matches!(b[0], b'N' | b'S' | b'n' | b's')
        && b[1..3].iter().all(u8::is_ascii_digit)
        && matches!(b[3], b'E' | b'W' | b'e' | b'w')
        && b[4..7].iter().all(u8::is_ascii_digit)
        && name[7..].eq_ignore_ascii_case(".hgt")
        && name[1..3].parse::<u32>().is_ok_and(|n| n <= 90)
        && name[4..7].parse::<u32>().is_ok_and(|n| n <= 180)
}
fn archive_name(name: &str) -> bool {
    name.ends_with(".zip")
        && name.len() > 4
        && name.len() < 64
        && name[..name.len() - 4]
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
}
fn valid_archive(a: &Archive) -> bool {
    archive_name(&a.name)
        && (1..=1_000_000_000_000).contains(&a.download_bytes)
        && !a.tiles.is_empty()
        && a.tiles.len() <= 32_768
        && a.tiles
            .iter()
            .all(|t| tile_name(&t.name) && t.bytes == HGT_BYTES)
}
fn cached_archive(directory: &AssetDirectory, name: &str) -> Option<Archive> {
    let a: Archive = cached_json(directory, &format!("{name}.json"), META_LIMIT)?;
    (a.name == name && valid_archive(&a)).then_some(a)
}
fn metadata(name: &str, cancel: &AtomicBool) -> Result<Archive, String> {
    let url = format!("{HOST}{name}");
    let headers = read_url(&url, &["--head".into()], 65_536, cancel)?;
    let total = String::from_utf8_lossy(&headers)
        .lines()
        .rev()
        .find_map(|s| {
            let (k, v) = s.split_once(':')?;
            k.eq_ignore_ascii_case("content-length")
                .then(|| v.trim().parse::<u64>().ok())
                .flatten()
        })
        .filter(|n| *n >= 22)
        .ok_or("Viewfinder server did not report the archive size")?;
    let mut start = total.saturating_sub(65_557);
    let mut tail = range(&url, start, total, cancel)?;
    let end = tail
        .windows(4)
        .rposition(|w| w == b"PK\x05\x06")
        .ok_or("Missing ZIP directory record")?;
    let record = tail
        .get(end..end + 22)
        .ok_or("Truncated ZIP directory record")?;
    let offset = u32::from_le_bytes(record[16..20].try_into().unwrap()) as u64;
    let entries = u16::from_le_bytes(record[10..12].try_into().unwrap()) as usize;
    if offset == u32::MAX as u64 {
        return Err(
            "This Viewfinder archive uses unsupported ZIP64 metadata; choose a smaller region"
                .into(),
        );
    }
    if offset < start {
        start = offset;
        tail = range(&url, start, total, cancel)?;
    }
    let tiles = directory_tiles(&tail, start, offset, entries)?;
    let a = Archive {
        name: name.into(),
        download_bytes: total,
        tiles,
    };
    if !valid_archive(&a) {
        return Err("Archive does not contain supported 1″ HGT tiles".into());
    }
    Ok(a)
}

// zip::ZipArchive opens local payload headers to expose file sizes. A planned
// download must not fetch those headers, so read only standard central records.
fn directory_tiles(
    tail: &[u8],
    start: u64,
    offset: u64,
    entries: usize,
) -> Result<Vec<Tile>, String> {
    if entries == 0 || entries > 32_768 {
        return Err("Invalid Viewfinder tile count".into());
    }
    let mut pos = offset
        .checked_sub(start)
        .ok_or("Invalid ZIP directory offset")? as usize;
    let mut tiles = Vec::new();
    let mut names = std::collections::BTreeSet::new();
    for _ in 0..entries {
        let h = tail
            .get(pos..pos + 46)
            .ok_or("Truncated ZIP central record")?;
        if &h[..4] != b"PK\x01\x02" {
            return Err("Invalid ZIP central record".into());
        }
        let name_len = u16::from_le_bytes(h[28..30].try_into().unwrap()) as usize;
        let extra = u16::from_le_bytes(h[30..32].try_into().unwrap()) as usize;
        let comment = u16::from_le_bytes(h[32..34].try_into().unwrap()) as usize;
        let name = std::str::from_utf8(
            tail.get(pos + 46..pos + 46 + name_len)
                .ok_or("Truncated ZIP filename")?,
        )
        .map_err(|e| e.to_string())?;
        let name = name.rsplit('/').next().unwrap_or("");
        if name.to_ascii_lowercase().ends_with(".hgt") {
            if !names.insert(name.to_ascii_lowercase()) {
                return Err("Duplicate Viewfinder tile names".into());
            }
            tiles.push(Tile {
                name: name.into(),
                bytes: u32::from_le_bytes(h[24..28].try_into().unwrap()) as u64,
                crc: u32::from_le_bytes(h[16..20].try_into().unwrap()),
            });
        }
        pos = pos
            .checked_add(46 + name_len + extra + comment)
            .filter(|p| *p <= tail.len())
            .ok_or("Truncated ZIP directory")?;
    }
    Ok(tiles)
}

fn coverage(html: &str, bounds: [f64; 4]) -> Result<Vec<String>, String> {
    // The provider's published map is an equirectangular 1800 × 900 image.
    if !html.contains("width=\"1800\"") || !html.contains("height=\"900\"") {
        return Err("Viewfinder coverage-map format changed".into());
    }
    let areas = regex::Regex::new(r#"<[aA][rR][eE][aA][ \t\r\n][^>]*>"#).unwrap();
    let attrs = regex::Regex::new(r#"([a-z]+)="([^"]*)""#).unwrap();
    let mut selected = std::collections::BTreeSet::new();
    for area in areas.find_iter(html) {
        let a: BTreeMap<_, _> = attrs
            .captures_iter(area.as_str())
            .map(|c| (c[1].to_ascii_lowercase(), c[2].to_owned()))
            .collect();
        let Some(url) = a.get("href").and_then(|u| u.strip_prefix(HOST)) else {
            continue;
        };
        if !archive_name(url) {
            continue;
        }
        let Some(coords) = a.get("coords") else {
            continue;
        };
        let c: Vec<f64> = coords
            .split(',')
            .filter_map(|v| v.trim().parse().ok())
            .collect();
        if c.len() != 4 || c.iter().any(|v| !v.is_finite()) {
            continue;
        }
        let [west, south, east, north] = [
            c[0] / 5.0 - 180.0,
            90.0 - c[3] / 5.0,
            c[2] / 5.0 - 180.0,
            90.0 - c[1] / 5.0,
        ];
        if south < bounds[3]
            && north > bounds[1]
            && [-360.0, 0.0, 360.0]
                .into_iter()
                .any(|shift| west + shift < bounds[2] && east + shift > bounds[0])
        {
            selected.insert(url.to_string());
        }
    }
    Ok(selected.into_iter().collect())
}
fn present(directory: Option<&AssetDirectory>, tile: &Tile) -> bool {
    directory.is_some_and(|directory| {
        little_clip_core::geography::basemap::hgt::present(
            directory,
            std::ffi::OsStr::new(&tile.name),
            tile.bytes,
        )
    })
}

pub fn plan(config: &AppConfig, bounds: [f64; 4], cancel: &AtomicBool) -> Result<Plan, String> {
    if bounds.iter().any(|v| !v.is_finite())
        || bounds[2] <= bounds[0]
        || bounds[3] <= bounds[1]
        || bounds[2] - bounds[0] > 18.0
        || bounds[3] - bounds[1] > 12.0
    {
        return Err("Zoom into a region smaller than 18° × 12° to plan 1″ terrain".into());
    }
    let root = AssetDirectory::create(&config.geo_assets_dir()).map_err(|e| e.to_string())?;
    let cache =
        AssetDirectory::create(&root.path().join("viewfinder")).map_err(|e| e.to_string())?;
    let bytes = match cache.open_file(std::ffi::OsStr::new("catalog.html")) {
        Ok(f) if f.metadata().is_ok_and(|m| m.len() < META_LIMIT as u64) => {
            let mut bytes = Vec::new();
            f.take(META_LIMIT as u64)
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            bytes
        }
        _ => {
            let bytes = read_url(CATALOG, &[], META_LIMIT, cancel)?;
            cache
                .write(std::ffi::OsStr::new("catalog.html"), &bytes)
                .map_err(|e| e.to_string())?;
            bytes
        }
    };
    let names = coverage(&String::from_utf8_lossy(&bytes), bounds)?;
    if names.is_empty() {
        return Err(
            "Viewfinder has no published 1″ coverage for this region; GEBCO remains available"
                .into(),
        );
    }
    if names.len() > 16 {
        return Err(
            "This view covers more than 16 archives; zoom in to download a smaller region".into(),
        );
    }
    let terrain = AssetDirectory::open(&root.path().join("terrain")).ok();
    let mut plan = Plan {
        archives: Vec::new(),
        download_bytes: 0,
        installed_bytes: 0,
        peak_bytes: 0,
        already_installed: 0,
        budget_remaining: config
            .geography
            .max_assets_bytes
            .saturating_sub(dir_size(&root.path())),
    };
    let mut unique = std::collections::BTreeSet::new();
    let mut retained = 0;
    for name in names {
        if cancel.load(Ordering::Relaxed) {
            return Err("Planning cancelled".into());
        }
        let a = match cached_archive(&cache, &name) {
            Some(a) => a,
            None => {
                let a = metadata(&name, cancel).map_err(|e| format!("{name}: {e}"))?;
                cache
                    .write(
                        std::ffi::OsStr::new(&format!("{name}.json")),
                        &serde_json::to_vec(&a).map_err(|e| e.to_string())?,
                    )
                    .map_err(|e| e.to_string())?;
                a
            }
        };
        if a.tiles.iter().all(|t| present(terrain.as_ref(), t)) {
            plan.already_installed += 1;
            continue;
        }
        let mut expanded = 0;
        for t in &a.tiles {
            if unique.insert(t.name.to_ascii_lowercase()) {
                plan.installed_bytes += t.bytes;
            }
            expanded += t.bytes;
        }
        plan.download_bytes += a.download_bytes;
        plan.peak_bytes = plan.peak_bytes.max(retained + a.download_bytes + expanded);
        retained += expanded;
        plan.archives.push(a);
    }
    Ok(plan)
}

pub(super) fn download(session: &DownloadSession<'_>, plan: &Plan) -> Result<(), String> {
    for (i, a) in plan.archives.iter().enumerate() {
        if !valid_archive(a) {
            return Err("Invalid Viewfinder download plan".into());
        }
        let directory = AssetDirectory::create(&session.root.path().join("terrain"))
            .map_err(|e| e.to_string())?;
        if a.tiles.iter().all(|t| present(Some(&directory), t)) {
            continue;
        }
        session.report(
            i as f32 / plan.archives.len() as f32,
            &format!("Downloading Viewfinder {}…", a.name),
        );
        let expanded = a.tiles.iter().map(|t| t.bytes).sum::<u64>();
        if session.remaining() < a.download_bytes + expanded {
            return Err(
                "Viewfinder download exceeds the geography disk budget (including temporary ZIP)"
                    .into(),
            );
        }
        let file = download_to_file(
            &directory.path(),
            &a.name,
            &format!("{HOST}{}", a.name),
            session.remaining(),
            session.cancelled,
            |f, m| session.report((i as f32 + f) / plan.archives.len() as f32, m),
        )?;
        if !matches_plan(&file, a)? {
            if let Ok(cache) = AssetDirectory::open(&session.root.path().join("viewfinder")) {
                let _ = fs::remove_file(cache.path().join(format!("{}.json", a.name)));
            }
            return Err(
                "Viewfinder archive changed; prepare a new download plan to refresh its sizes"
                    .into(),
            );
        }
        extract_matching_from_zip(&file, session.cancelled, &[".hgt"])?;
        if !a.tiles.iter().all(|t| present(Some(&directory), t)) {
            return Err("Viewfinder archive did not install all expected tiles".into());
        }
    }
    Ok(())
}

fn matches_plan(file: &Downloaded, a: &Archive) -> Result<bool, String> {
    if file.file.metadata().map_err(|e| e.to_string())?.len() != a.download_bytes {
        return Ok(false);
    }
    let zip = zip::ZipArchive::new(file.file.try_clone().map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    if zip.len() > 65_536 {
        return Err("Viewfinder archive has too many entries".into());
    }
    let mut actual = Vec::new();
    for i in 0..zip.len() {
        let f = zip.by_index_data(i).map_err(|e| e.to_string())?;
        let member_name = f.name().map_err(|error| error.to_string())?;
        let name = member_name.rsplit('/').next().unwrap_or("");
        if name.to_ascii_lowercase().ends_with(".hgt") {
            actual.push((name.to_string(), f.size(), f.crc32()));
        }
    }
    let mut expected: Vec<_> = a
        .tiles
        .iter()
        .map(|t| (t.name.clone(), t.bytes, t.crc))
        .collect();
    actual.sort();
    expected.sort();
    Ok(actual == expected)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn central_metadata_counts_hgt_without_reading_any_payload() {
        use std::io::Cursor;
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file("K14/N41W097.hgt", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(&[0, 20, 0, 40]).unwrap();
        let bytes = writer.finish().unwrap().into_inner();
        let zip = zip::ZipArchive::new(Cursor::new(&bytes)).unwrap();
        let offset = zip.central_directory_start();
        let tail = &bytes[offset as usize..];
        let tiles = directory_tiles(tail, offset, offset, 1).unwrap();
        assert_eq!(tiles.len(), 1);
        assert_eq!(tiles[0].name, "N41W097.hgt");
        assert_eq!(tiles[0].bytes, 4);
        assert!(directory_tiles(&tail[..20], offset, offset, 1).is_err());
        assert!(directory_tiles(tail, offset, offset, 32_769).is_err());
        assert!(!valid_archive(&Archive {
            name: "K14.zip".into(),
            download_bytes: 100,
            tiles
        }));
    }

    #[test]
    fn cached_region_estimates_all_archive_bytes_and_skips_installed_tiles() {
        let temp = tempfile::tempdir().unwrap();
        let mut config = AppConfig::default();
        config.geography.assets_dir = Some(temp.path().to_path_buf());
        let root = AssetDirectory::create(temp.path()).unwrap();
        let cache = AssetDirectory::create(&root.path().join("viewfinder")).unwrap();
        let html=br#"<img width="1800" height="900"><area coords="390,230,420,250" href="https://viewfinderpanoramas.org/dem1/K14.zip">"#;
        cache
            .write(std::ffi::OsStr::new("catalog.html"), html)
            .unwrap();
        let a = Archive {
            name: "K14.zip".into(),
            download_bytes: 143764215,
            tiles: vec![Tile {
                name: "N41W097.hgt".into(),
                bytes: HGT_BYTES,
                crc: 1,
            }],
        };
        cache
            .write(
                std::ffi::OsStr::new("K14.zip.json"),
                &serde_json::to_vec(&a).unwrap(),
            )
            .unwrap();
        let cancel = AtomicBool::new(false);
        let p = plan(&config, [-97.0, 41.0, -96.1, 42.0], &cancel).unwrap();
        assert_eq!(p.download_bytes, 143764215);
        assert_eq!(p.installed_bytes, HGT_BYTES);
        assert_eq!(p.peak_bytes, 143764215 + HGT_BYTES);
        let terrain = AssetDirectory::create(&root.path().join("terrain")).unwrap();
        let f = terrain.stage().unwrap();
        f.as_file().set_len(HGT_BYTES).unwrap();
        terrain
            .install(f, std::ffi::OsStr::new("N41W097.hgt"))
            .unwrap();
        let p = plan(&config, [-97.0, 41.0, -96.1, 42.0], &cancel).unwrap();
        assert_eq!(p.download_bytes, 0);
        assert_eq!(p.already_installed, 1);
    }

    #[test]
    #[ignore = "reads the explicitly selected offline acceptance dataset"]
    fn installed_provider_catalog_and_archive_metadata_plan_a_real_region() {
        let mut config = AppConfig::default();
        config.geography.assets_dir = Some(
            std::env::var_os("LITTLE_CLIP_GEOGRAPHY_ACCEPTANCE_DIR")
                .expect("set acceptance dataset directory")
                .into(),
        );
        let p = plan(&config, [-97.0, 41.0, -96.1, 42.0], &AtomicBool::new(false)).unwrap();
        if p.archives.is_empty() {
            assert_eq!(p.already_installed, 1);
        } else {
            assert_eq!(p.download_bytes, 143764215);
            assert_eq!(p.installed_bytes, 24 * HGT_BYTES);
        }
    }

    #[test]
    fn coverage_selects_region_and_dateline_without_following_arbitrary_urls() {
        let html = r#"<img width="1800" height="900"><area coords="390,230,420,250" href="https://viewfinderpanoramas.org/dem1/K14.zip"><area coords="420,230,450,250" href="https://viewfinderpanoramas.org/dem1/K15.zip"><area coords="1795,440,1800,450" href="https://viewfinderpanoramas.org/dem1/A60.zip"><area coords="0,440,5,450" href="https://viewfinderpanoramas.org/dem1/A01.zip"><area coords="390,230,420,250" href="https://other.invalid/evil.zip">"#;
        assert_eq!(
            coverage(html, [-97.0, 41.0, -96.1, 42.0]).unwrap(),
            vec!["K14.zip"]
        );
        assert_eq!(
            coverage(html, [179.5, 0.1, 180.5, 1.0]).unwrap(),
            vec!["A01.zip", "A60.zip"]
        );
        assert!(!tile_name("../N41W096.hgt"));
        assert!(!archive_name("../K14.zip"));
    }
}
