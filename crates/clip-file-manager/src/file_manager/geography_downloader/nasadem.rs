//! Bulk NASADEM elevation downloads from NASA's current CMR/Earthdata Cloud service.
//! Completed atomic HGT installs survive interruption; reruns skip those tiles.
use super::*;
use little_clip_core::{config::EarthdataCredentials, geography::basemap::parse_hgt_name};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const CMR: &str = "https://cmr.earthdata.nasa.gov/search/granules.json?short_name=NASADEM_HGT&version=001&provider=LPCLOUD&sort_key%5B%5D=producer_granule_id&page_size=2000";
const CLOUD: &str = "https://data.lpdaac.earthdatacloud.nasa.gov/lp-prod-protected/NASADEM_HGT.001";
const HGT_BYTES: u64 = 3601 * 3601 * 2;
const CATALOG_LIMIT: usize = 16 * 1024 * 1024;
static CANCELLED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Serialize, Deserialize, Debug)]
struct Tile {
    stem: String,
    estimated_zip_bytes: u64,
}
impl Tile {
    fn valid(&self) -> bool {
        self.stem == self.stem.to_ascii_lowercase()
            && parse_hgt_name(&self.stem).is_some()
            && self.estimated_zip_bytes > 0
            && self.estimated_zip_bytes < 200_000_000
    }
    fn url(&self) -> String {
        let id = format!("NASADEM_HGT_{}", self.stem);
        format!("{CLOUD}/{id}/{id}.zip")
    }
    fn filename(&self) -> String {
        format!("{}.hgt", self.stem)
    }
}

// Credentials live in a private, retained directory. Commands inherit that
// directory, so arguments and Debug output contain descriptor paths only.
struct Auth {
    directory: fs::File,
    _workspace: tempfile::TempDir,
}
impl Auth {
    fn new(credentials: &EarthdataCredentials) -> Result<Self, String> {
        fn token(value: &str) -> Result<String, String> {
            if value.is_empty() || value.chars().any(char::is_control) {
                return Err(
                    "Set geography.earthdata.username and password to valid nonempty values".into(),
                );
            }
            Ok(format!(
                "\"{}\"",
                value.replace('\\', "\\\\").replace('"', "\\\"")
            ))
        }
        let text = format!(
            "machine urs.earthdata.nasa.gov login {} password {}\n",
            token(&credentials.username)?,
            token(&credentials.password)?
        );
        let workspace = tempfile::tempdir().map_err(|e| e.to_string())?;
        let directory = AssetDirectory::open(workspace.path()).map_err(|e| e.to_string())?;
        directory
            .write(std::ffi::OsStr::new("netrc"), text.as_bytes())
            .map_err(|e| e.to_string())?;
        directory
            .write(std::ffi::OsStr::new("cookies"), b"")
            .map_err(|e| e.to_string())?;
        Ok(Self {
            directory: fs::File::open(directory.path()).map_err(|e| e.to_string())?,
            _workspace: workspace,
        })
    }
    fn request(&self) -> Result<CommandRequest, String> {
        let (request, fd) = CommandRequest::new("curl")
            .inherit_handle(&self.directory)
            .map_err(|e| e.to_string())?;
        let root = format!("/proc/self/fd/{fd}");
        Ok(request
            .args([
                "--disable",
                "--proto",
                "=https",
                "--proto-redir",
                "=https",
                "--netrc-file",
            ])
            .arg(format!("{root}/netrc"))
            .arg("--cookie")
            .arg(format!("{root}/cookies"))
            .arg("--cookie-jar")
            .arg(format!("{root}/cookies")))
    }
}

fn parse_page(bytes: &[u8]) -> Result<Vec<Tile>, String> {
    let document: serde_json::Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    let entries = document
        .pointer("/feed/entry")
        .and_then(|v| v.as_array())
        .ok_or("Invalid NASADEM catalog response")?;
    if entries.len() > 2000 {
        return Err("NASA catalog page exceeds its entry limit".into());
    }
    entries
        .iter()
        .map(|v| {
            let stem = v["producer_granule_id"]
                .as_str()
                .and_then(|s| s.strip_prefix("NASADEM_HGT_"))
                .ok_or("Invalid NASADEM granule name")?;
            let mb = v["granule_size"]
                .as_str()
                .and_then(|s| s.parse::<f64>().ok())
                .filter(|n| n.is_finite() && *n > 0.0)
                .ok_or("NASADEM granule has no size estimate")?;
            let tile = Tile {
                stem: stem.into(),
                estimated_zip_bytes: (mb * 1_048_576.0).ceil() as u64,
            };
            tile.valid()
                .then_some(tile)
                .ok_or_else(|| "NASADEM catalog contains an unsupported granule".into())
        })
        .collect()
}
#[derive(Serialize, Deserialize)]
struct Catalog {
    expected_count: usize,
    tiles: Vec<Tile>,
}
impl Catalog {
    fn valid(&self) -> bool {
        !self.tiles.is_empty()
            && self.tiles.len() == self.expected_count
            && self.tiles.len() <= 20_000
            && self.tiles.iter().all(Tile::valid)
            && self
                .tiles
                .iter()
                .map(|t| &t.stem)
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                == self.tiles.len()
    }
}
fn page_response(bytes: &[u8]) -> Result<(usize, Vec<Tile>), String> {
    let split = bytes
        .windows(4)
        .rposition(|w| w == b"\r\n\r\n")
        .ok_or("NASA catalog omitted HTTP headers")?;
    let headers = std::str::from_utf8(&bytes[..split]).map_err(|e| e.to_string())?;
    let expected = headers
        .lines()
        .rev()
        .find_map(|line| {
            let (k, v) = line.split_once(':')?;
            k.eq_ignore_ascii_case("cmr-hits")
                .then(|| v.trim().parse::<usize>().ok())
                .flatten()
        })
        .filter(|n| *n > 0 && *n <= 20_000)
        .ok_or("NASA catalog omitted a supported granule count")?;
    Ok((expected, parse_page(&bytes[split + 4..])?))
}

fn catalog(root: &AssetDirectory, cancel: &AtomicBool) -> Result<Vec<Tile>, String> {
    if let Some(cached) = cached_json::<Catalog>(root, "catalog-v1.json", CATALOG_LIMIT)
        && cached.valid()
    {
        return Ok(cached.tiles);
    }
    let mut tiles = BTreeMap::new();
    let mut expected_count = None;
    for page in 1..=11 {
        println!("Reading NASADEM catalog page {page}…");
        let mut last = Err("NASA catalog unavailable".to_string());
        for _ in 0..3 {
            last = read_url(
                &format!("{CMR}&page_num={page}"),
                &["--include".into()],
                CATALOG_LIMIT,
                cancel,
            )
            .and_then(|b| page_response(&b));
            if last.is_ok() || cancel.load(Ordering::Relaxed) {
                break;
            }
        }
        let (expected, entries) = last?;
        if expected_count.is_some_and(|n| n != expected) {
            return Err("NASA catalog changed during pagination; retry planning".into());
        }
        expected_count = Some(expected);
        let count = entries.len();
        for tile in entries {
            if tiles.insert(tile.stem.clone(), tile).is_some() {
                return Err("NASA catalog repeated a granule; retry planning".into());
            }
        }
        if count < 2000 {
            if tiles.len() != expected {
                return Err("NASA catalog is incomplete; retry planning".into());
            }
            let tiles: Vec<_> = tiles.into_values().collect();
            root.write(
                std::ffi::OsStr::new("catalog-v1.json"),
                &serde_json::to_vec(&Catalog {
                    expected_count: expected,
                    tiles: tiles.clone(),
                })
                .map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            return Ok(tiles);
        }
    }
    Err("NASADEM catalog exceeds its 20,000-tile limit".into())
}
fn present(directory: &AssetDirectory, tile: &Tile) -> bool {
    little_clip_core::geography::basemap::hgt::present(
        directory,
        std::ffi::OsStr::new(&tile.filename()),
        HGT_BYTES,
    )
}
fn validate_zip(file: &Downloaded, tile: &Tile) -> Result<(), String> {
    let zip =
        zip::ZipArchive::new(file.file.try_clone().map_err(|e| e.to_string())?).map_err(|_| {
            "NASA did not return a valid ZIP; check Earthdata login/application authorization"
                .to_string()
        })?;
    if zip.len() > 64 {
        return Err("NASADEM archive exceeds its entry limit".into());
    }
    let mut found = 0;
    for i in 0..zip.len() {
        let f = zip.by_index_data(i).map_err(|e| e.to_string())?;
        let member_name = f.name().map_err(|error| error.to_string())?;
        let name = Path::new(member_name.as_ref())
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
        if name.to_ascii_lowercase().ends_with(".hgt") {
            if name != tile.filename() || f.size() != HGT_BYTES {
                return Err("NASADEM archive has unexpected elevation data".into());
            }
            found += 1;
        }
    }
    if found != 1 {
        return Err("NASADEM archive has no unique 1″ elevation tile".into());
    }
    Ok(())
}
fn download_tile(
    directory: &AssetDirectory,
    tile: &Tile,
    auth: &Auth,
    budget: u64,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let maximum = (tile.estimated_zip_bytes * 2 + HGT_BYTES + 65_536).min(budget);
    let file = download_with_request(
        &directory.path().join(format!("{}.zip", tile.stem)),
        &tile.url(),
        maximum,
        cancel,
        |_, _| {},
        auth.request()?,
    )?;
    validate_zip(&file, tile)?;
    extract_matching_from_zip(&file, cancel, &[&tile.filename()])?;
    if !present(directory, tile) {
        return Err("NASADEM tile installation is incomplete".into());
    }
    Ok(())
}
fn download_retry(
    directory: &AssetDirectory,
    tile: &Tile,
    auth: &Auth,
    budget: u64,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let mut last = Err("Download not attempted".into());
    for attempt in 1..=3 {
        last = download_tile(directory, tile, auth, budget, cancel);
        if last.as_ref().err().is_some_and(|e: &String| {
            e.contains("error: 401") || e.contains("error: 403") || e.contains("check Earthdata")
        }) {
            return Err("Earthdata authentication/authorization failed; verify credentials and authorize LP DAAC access in Earthdata Login".into());
        }
        if last.is_ok() || cancel.load(Ordering::Relaxed) {
            return last;
        }
        for _ in 0..attempt * 10 {
            if cancel.load(Ordering::Relaxed) {
                return last;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    last
}

struct Pending {
    tile: Tile,
    reserved: u64,
    job: Task<Result<(), String>>,
}
// The coordinator alone owns disk reservations and completion reporting. Workers
// install tiles, but never race one another when charging the shared budget.
struct Progress {
    remaining: u64,
    complete: usize,
    failures: Vec<String>,
    fatal: Option<String>,
}
impl Progress {
    fn finish(
        &mut self,
        tile: &Tile,
        reserved: u64,
        result: Result<Result<(), String>, little_clip_core::JobError>,
    ) {
        self.remaining += reserved;
        match result {
            Ok(Ok(())) => {
                self.remaining -= HGT_BYTES;
                self.complete += 1;
                println!(
                    "Installed {} ({} completed this run)",
                    tile.filename(),
                    self.complete
                );
            }
            Ok(Err(error)) => {
                if error.starts_with("Earthdata authentication") {
                    self.fatal = Some(error.clone());
                }
                self.failures.push(format!("{}: {error}", tile.filename()));
            }
            Err(little_clip_core::JobError::QueueFull) => {
                self.fatal = Some("NASADEM worker admission failed; rerun to resume".into());
            }
            Err(error) => self
                .failures
                .push(format!("{}: {error:?}", tile.filename())),
        }
    }
}
fn download(
    root: &AssetDirectory,
    config: &AppConfig,
    tiles: &[Tile],
    cancel: &AtomicBool,
) -> Result<(), String> {
    let directory = Arc::new(
        AssetDirectory::create(&root.path().join("nasadem/terrain")).map_err(|e| e.to_string())?,
    );
    let workers = BackgroundJobs::new(4, 8);
    let auth = (0..4)
        .map(|_| Auth::new(&config.geography.earthdata).map(Arc::new))
        .collect::<Result<Vec<_>, _>>()?;
    let mut pending: Vec<Option<Pending>> = (0..4).map(|_| None).collect();
    let mut progress = Progress {
        remaining: config
            .geography
            .max_assets_bytes
            .saturating_sub(dir_size(&root.path())),
        complete: 0,
        failures: Vec::new(),
        fatal: None,
    };
    let mut next = 0;
    loop {
        // Collect all results before making admission decisions. Authentication
        // failure in any lane stops the whole batch before another tile starts.
        for slot in &mut pending {
            if let Some(active) = slot
                && let Some(result) = active.job.poll()
            {
                let active = slot.take().unwrap();
                progress.finish(&active.tile, active.reserved, result);
            }
        }
        let stopping = cancel.load(Ordering::Relaxed)
            || progress.fatal.is_some()
            || progress.failures.len() >= 10;
        if stopping {
            for active in pending.iter().flatten() {
                active.job.token.store(true, Ordering::Relaxed);
            }
        } else {
            for (lane, slot) in pending
                .iter_mut()
                .enumerate()
                .filter(|(_, slot)| slot.is_none())
            {
                if next == tiles.len() {
                    break;
                }
                let tile = tiles[next].clone();
                next += 1;
                if present(&directory, &tile) {
                    continue;
                }
                let reserved = HGT_BYTES + tile.estimated_zip_bytes * 2 + 65_536;
                if reserved > progress.remaining {
                    progress.fatal = Some(
                        "NASADEM exceeds geography.max_assets_bytes; completed tiles are retained"
                            .into(),
                    );
                    break;
                }
                progress.remaining -= reserved;
                println!("[{next}/{}] Downloading {}", tiles.len(), tile.filename());
                let source = tile.clone();
                let directory = directory.clone();
                let auth = auth[lane].clone();
                let job = Task::start_unbounded(&workers, move |cancel| {
                    download_retry(&directory, &source, &auth, reserved, cancel)
                });
                *slot = Some(Pending {
                    tile,
                    reserved,
                    job,
                });
            }
        }
        if pending.iter().all(Option::is_none) && (next == tiles.len() || stopping) {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    if cancel.load(Ordering::Relaxed) {
        return Err("NASADEM cancelled; completed tiles are retained. Rerun to resume".into());
    }
    if let Some(error) = progress.fatal {
        return Err(error);
    }
    if !progress.failures.is_empty() {
        return Err(format!(
            "{} tiles failed; rerun to resume:\n{}",
            progress.failures.len(),
            progress.failures.join("\n")
        ));
    }
    println!("NASADEM download complete ({} selected tiles)", tiles.len());
    Ok(())
}

extern "C" fn cancel_signal(_: libc::c_int) {
    CANCELLED.store(true, Ordering::Relaxed);
}
fn signal_cancellation() -> Result<(), String> {
    // SAFETY: this disposable CLI installs only handlers that perform an atomic
    // store. No GUI executes here; child reaping/staging cleanup stay on this thread.
    unsafe {
        if libc::signal(
            libc::SIGINT,
            cancel_signal as *const () as libc::sighandler_t,
        ) == libc::SIG_ERR
            || libc::signal(
                libc::SIGTERM,
                cancel_signal as *const () as libc::sighandler_t,
            ) == libc::SIG_ERR
        {
            return Err("Could not install download cancellation handlers".into());
        }
    }
    Ok(())
}

struct Options {
    config_path: Option<PathBuf>,
    plan_only: bool,
    limit: Option<usize>,
}
impl Options {
    fn parse(args: &[std::ffi::OsString]) -> Result<Self, String> {
        let mut config_path = None;
        let mut plan_only = false;
        let mut limit = None;
        let mut i = 0;
        while i < args.len() {
            match args[i].to_str() {
                Some("--plan") => plan_only = true,
                Some("--config") => {
                    i += 1;
                    config_path = Some(PathBuf::from(args.get(i).ok_or("Missing config path")?));
                }
                Some("--limit") => {
                    i += 1;
                    limit = Some(
                        args.get(i)
                            .and_then(|v| v.to_str())
                            .and_then(|s| s.parse::<usize>().ok())
                            .filter(|n| *n > 0)
                            .ok_or("--limit requires a positive tile count")?,
                    );
                }
                _ => {
                    return Err(
                        "Usage: --download-nasadem [--config FILE] [--plan] [--limit N]".into(),
                    );
                }
            }
            i += 1;
        }
        Ok(Self {
            config_path,
            plan_only,
            limit,
        })
    }
}

pub(crate) fn cli(args: &[std::ffi::OsString]) -> Result<(), String> {
    let Options {
        config_path,
        plan_only,
        limit,
    } = Options::parse(args)?;
    signal_cancellation()?;
    let config = if let Some(path) = config_path {
        AppConfig::load_from_path(&path)
            .map_err(|_| "Could not load download config".to_string())?
    } else {
        AppConfig::load()
    };
    let root = AssetDirectory::create(&config.geo_assets_dir()).map_err(|e| e.to_string())?;
    let _owner = download_owner(&root)?;
    let nasa = AssetDirectory::create(&root.path().join("nasadem")).map_err(|e| e.to_string())?;
    let mut tiles = catalog(&nasa, &CANCELLED)?;
    if let Some(limit) = limit {
        tiles.truncate(limit);
    }
    let directory =
        AssetDirectory::create(&nasa.path().join("terrain")).map_err(|e| e.to_string())?;
    let pending: Vec<_> = tiles
        .iter()
        .filter(|t| !present(&directory, t))
        .cloned()
        .collect();
    let estimated: u64 = pending.iter().map(|t| t.estimated_zip_bytes).sum();
    let raw = pending.len() as u64 * HGT_BYTES;
    let mut staging: Vec<_> = pending
        .iter()
        .map(|t| t.estimated_zip_bytes * 2 + 65_536)
        .collect();
    staging.sort_unstable_by(|a, b| b.cmp(a));
    let staging = staging.into_iter().take(4).sum::<u64>();
    let remaining = config
        .geography
        .max_assets_bytes
        .saturating_sub(dir_size(&root.path()));
    println!(
        "NASADEM: {} tiles; {} already installed; {} pending",
        tiles.len(),
        tiles.len() - pending.len(),
        pending.len()
    );
    println!(
        "Estimated remaining transfer: {:.2} GB; uncompressed upper bound: {:.2} GB; budget available: {:.2} GB",
        estimated as f64 / 1e9,
        raw as f64 / 1e9,
        remaining as f64 / 1e9
    );
    println!("Terrain is stored losslessly as .hgt.zst; budget estimates use uncompressed sizes.");
    println!(
        "Elevation destination: {}",
        config.geo_assets_dir().join("nasadem/terrain").display()
    );
    if plan_only || pending.is_empty() {
        return Ok(());
    }
    if raw + staging > remaining {
        return Err("Increase geography.max_assets_bytes to accommodate the full selected dataset (including temporary ZIP space)".into());
    }
    download(&root, &config, &pending, &CANCELLED)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cached_catalog_rejects_incomplete_or_duplicate_tiles() {
        let tile = Tile {
            stem: "n41w097".into(),
            estimated_zip_bytes: 100,
        };
        let mut catalog = Catalog {
            expected_count: 1,
            tiles: vec![tile.clone()],
        };
        assert!(catalog.valid());
        catalog.expected_count = 2;
        assert!(!catalog.valid());
        catalog.tiles.push(tile);
        assert!(!catalog.valid());
        catalog.tiles[1].stem = "n41w098".into();
        assert!(catalog.valid());
    }

    #[test]
    fn batch_reservations_release_zip_space_and_charge_installed_grids() {
        let tile = Tile {
            stem: "n41w097".into(),
            estimated_zip_bytes: 100,
        };
        let mut progress = Progress {
            remaining: 0,
            complete: 0,
            failures: vec![],
            fatal: None,
        };
        progress.finish(&tile, HGT_BYTES + 200, Ok(Ok(())));
        assert_eq!(progress.remaining, 200);
        assert_eq!(progress.complete, 1);
        progress.finish(&tile, HGT_BYTES + 200, Ok(Err("Transfer failed".into())));
        assert_eq!(progress.remaining, HGT_BYTES + 400);
        assert_eq!(progress.complete, 1);
        assert_eq!(progress.failures.len(), 1);
        assert!(progress.fatal.is_none());
        progress.finish(
            &tile,
            0,
            Ok(Err("Earthdata authentication/authorization failed".into())),
        );
        assert!(progress.fatal.is_some());
    }

    #[test]
    fn download_options_validate_values_before_accessing_assets() {
        let args = |items: &[&str]| {
            items
                .iter()
                .map(std::ffi::OsString::from)
                .collect::<Vec<_>>()
        };
        let options = Options::parse(&args(&[
            "--config",
            "/tmp/config",
            "--plan",
            "--limit",
            "4",
        ]))
        .unwrap();
        assert_eq!(options.config_path, Some(PathBuf::from("/tmp/config")));
        assert_eq!(options.limit, Some(4));
        assert!(options.plan_only);
        for invalid in [
            vec!["--config"],
            vec!["--limit", "0"],
            vec!["--limit", "bad"],
            vec!["--unknown"],
        ] {
            assert!(Options::parse(&args(&invalid)).is_err());
        }
    }

    #[test]
    fn cmr_http_headers_validate_catalog_counts_across_proxy_headers() {
        let response=b"HTTP/1.1 200 Connection established\r\n\r\nHTTP/2 200\r\nCMR-Hits: 14520\r\n\r\n{\"feed\":{\"entry\":[]}}";
        let (count, tiles) = page_response(response).unwrap();
        assert_eq!(count, 14520);
        assert!(tiles.is_empty());
        assert!(page_response(b"HTTP/2 200\r\n\r\n{\"feed\":{\"entry\":[]}}").is_err());
    }

    #[test]
    fn cmr_catalog_uses_validated_tile_names_and_finite_size_estimates() {
        let good=br#"{"feed":{"entry":[{"producer_granule_id":"NASADEM_HGT_n41w097","granule_size":"7.5"}]}}"#;
        let tiles = parse_page(good).unwrap();
        assert_eq!(tiles[0].estimated_zip_bytes, 7_864_320);
        assert!(
            tiles[0]
                .url()
                .ends_with("NASADEM_HGT_n41w097/NASADEM_HGT_n41w097.zip")
        );
        for name in ["../../bad", "n99e001", "n41w097/other"] {
            let value = serde_json::json!({"feed":{"entry":[{"producer_granule_id":format!("NASADEM_HGT_{name}"),"granule_size":"1"}]}});
            assert!(parse_page(&serde_json::to_vec(&value).unwrap()).is_err());
        }
    }
    #[test]
    fn earthdata_request_keeps_secrets_out_of_command_arguments() {
        let auth = Auth::new(&EarthdataCredentials {
            username: "private-test-user".into(),
            password: "private-test-pass!".into(),
        })
        .unwrap();
        let debug = format!("{:?}", auth.request().unwrap());
        assert!(!debug.contains("private-test-user"));
        assert!(!debug.contains("private-test-pass"));
        assert!(debug.contains("--netrc-file"));
        assert!(debug.contains("/proc/self/fd/"));
    }
}
