//! Restartable local HGT migration. No network access or elevation pyramid.
//! cargo run -p little-clip-core --example compress_terrain -- DIRECTORY [--limit N]
use little_clip_core::{
    file_inventory::{Identity, identity},
    file_source::{FileSource, rename_noreplace},
    geography::basemap::{hgt, parse_hgt_name, storage::AssetDirectory},
};
use std::{
    ffi::OsStr,
    fs, io,
    path::Path,
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};
static CANCEL: AtomicBool = AtomicBool::new(false);
extern "C" fn cancel_signal(_: libc::c_int) {
    CANCEL.store(true, Ordering::Relaxed);
}

// Quarantine before removal, then check the relocated object. Never unlink a
// replacement at the old name; failed recovery preserves the quarantine path.
fn retire(directory: &AssetDirectory, name: &OsStr, expected: Identity) -> io::Result<()> {
    little_clip_core::kernel_interface::pin_content_directory(&directory.path())?;
    let (_, temporary) = directory.stage()?.into_parts();
    let quarantine = temporary.keep()?;
    fs::remove_file(&quarantine)?;
    let source = directory.path().join(name);
    rename_noreplace(&source, &quarantine)?;
    let result = fs::symlink_metadata(&quarantine).and_then(|metadata| {
        if !metadata.is_file() || !expected.after_rename_matches(identity(&metadata)) {
            return Err(io::Error::other("Source changed before retirement"));
        }
        fs::remove_file(&quarantine)
    });
    if let Err(error) = result {
        if rename_noreplace(&quarantine, &source).is_err() {
            return Err(io::Error::other(format!(
                "{error}; source retained at {}",
                quarantine.display()
            )));
        }
        return Err(error);
    }
    Ok(())
}

fn verified_existing(directory: &AssetDirectory, name: &OsStr, raw: &[u8]) -> Option<fs::File> {
    let file = directory.open_file(name).ok()?;
    let bytes = hgt::read_stored(&file).ok()?;
    (hgt::decode(bytes, true).ok()?.as_slice() == raw).then_some(file)
}

fn encoded_stage(
    directory: &AssetDirectory,
    raw: &[u8],
    cancel: &AtomicBool,
) -> io::Result<tempfile::NamedTempFile> {
    let mut staged = directory.stage()?;
    hgt::encode(&mut &raw[..], &mut staged, raw.len() as u64, cancel)?;
    let bytes = hgt::read_stored(staged.as_file())?;
    if hgt::decode(bytes, true)?.as_slice() != raw {
        return Err(io::Error::other(
            "Compressed terrain verification failed; raw source retained",
        ));
    }
    Ok(staged)
}

fn convert(
    directory: &AssetDirectory,
    name: &OsStr,
    cancel: &AtomicBool,
) -> io::Result<(u64, u64)> {
    let file = directory.open_file(name)?;
    let source = FileSource::capture_file(&file).map_err(io::Error::other)?;
    let expected = identity(&file.metadata()?);
    let raw = hgt::decode(hgt::read_stored(&file)?, false)?;
    let compressed_name = hgt::compressed_name(name);
    // A previous run may have installed a verified frame before interruption.
    let existing = verified_existing(directory, &compressed_name, &raw);
    let installed = if let Some(file) = existing {
        file
    } else {
        let staged = encoded_stage(directory, &raw, cancel)?;
        source.check_file(&file).map_err(io::Error::other)?;
        directory.install(staged, &compressed_name)?
    };
    let stored = installed.metadata()?.len();
    if cancel.load(Ordering::Relaxed) {
        return Err(io::Error::other("Cancelled; raw source retained"));
    }
    // Durably publish the frame before retiring its source.
    // The guard retains an O_PATH pin; open that same directory for fsync.
    fs::File::open(directory.path())?.sync_all()?;
    source.check_file(&file).map_err(io::Error::other)?;
    retire(directory, name, expected)?;
    Ok((raw.len() as u64, stored))
}

fn inventory(directory: &AssetDirectory, limit: usize) -> io::Result<Vec<std::ffi::OsString>> {
    let mut names = Vec::new();
    for entry in fs::read_dir(directory.path())?.take(65_536) {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_file()
            && path
                .extension()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.eq_ignore_ascii_case("hgt"))
            && path
                .file_stem()
                .and_then(|s| s.to_str())
                .and_then(parse_hgt_name)
                .is_some()
        {
            names.push(entry.file_name());
        }
    }
    names.sort();
    names.truncate(limit);
    Ok(names)
}

fn options() -> Result<(std::ffi::OsString, usize), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let path = args
        .first()
        .ok_or("Usage: compress_terrain DIRECTORY [--limit N]")?;
    let limit = match args.get(1).and_then(|s| s.to_str()) {
        None => usize::MAX,
        Some("--limit") if args.len() == 3 => args[2].to_str().ok_or("Invalid limit")?.parse()?,
        _ => return Err("Usage: compress_terrain DIRECTORY [--limit N]".into()),
    };
    Ok((path.to_owned(), limit))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (path, limit) = options()?;
    let directory = AssetDirectory::open(Path::new(&path))?;
    let names = inventory(&directory, limit)?;
    // SAFETY: the standalone helper installs handlers which only store an atomic
    // flag; workers finish or retire staging normally before the process exits.
    unsafe {
        for signal in [libc::SIGINT, libc::SIGTERM] {
            if libc::signal(signal, cancel_signal as *const () as libc::sighandler_t)
                == libc::SIG_ERR
            {
                return Err(io::Error::last_os_error().into());
            }
        }
    }
    let next = AtomicUsize::new(0);
    let totals = Mutex::new((0usize, 0u64, 0u64, Vec::new()));
    println!(
        "Migrating {} local tiles with four bounded workers; no downloads",
        names.len()
    );
    std::thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| {
                while !CANCEL.load(Ordering::Relaxed) {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(name) = names.get(index) else {
                        break;
                    };
                    let result = convert(&directory, name, &CANCEL);
                    let mut totals = totals.lock().unwrap();
                    match result {
                        Ok((raw, stored)) => {
                            totals.0 += 1;
                            totals.1 += raw;
                            totals.2 += stored;
                            println!(
                                "[{}/{}] {}: {} -> {} bytes; saved {:.2} GB total",
                                totals.0,
                                names.len(),
                                name.to_string_lossy(),
                                raw,
                                stored,
                                (totals.1 as f64 - totals.2 as f64) / 1e9
                            );
                        }
                        Err(error) => {
                            eprintln!("{}: {error}", name.to_string_lossy());
                            totals.3.push(error.to_string());
                        }
                    }
                }
            });
        }
    });
    let totals = totals.into_inner().unwrap();
    println!(
        "Finished {} tiles; {} errors; {:.2} GB -> {:.2} GB",
        totals.0,
        totals.3.len(),
        totals.1 as f64 / 1e9,
        totals.2 as f64 / 1e9
    );
    if !totals.3.is_empty() || CANCEL.load(Ordering::Relaxed) {
        return Err("Migration incomplete; rerun to resume".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use little_clip_core::geography::basemap::storage;
    #[test]
    fn migration_verifies_frames_and_resumes_without_touching_other_files() {
        let root = tempfile::tempdir().unwrap();
        let directory = AssetDirectory::open(root.path()).unwrap();
        let name = OsStr::new("N00E000.hgt");
        let raw = 123_i16.to_be_bytes().repeat(4);
        directory.write(name, &raw).unwrap();
        directory.write(OsStr::new("unrelated"), b"keep").unwrap();
        let cancel = AtomicBool::new(true);
        assert!(convert(&directory, name, &cancel).is_err());
        assert!(root.path().join(name).exists());
        cancel.store(false, Ordering::Relaxed);
        let (before, after) = convert(&directory, name, &cancel).unwrap();
        assert_eq!(before, 8);
        assert!(after > 0);
        assert!(!root.path().join(name).exists());
        let bytes = storage::read(root.path().join(hgt::compressed_name(name))).unwrap();
        assert_eq!(hgt::decode(bytes, true).unwrap(), raw);
        directory.write(name, &raw).unwrap();
        convert(&directory, name, &cancel).unwrap();
        assert_eq!(fs::read(root.path().join("unrelated")).unwrap(), b"keep");
    }
    #[test]
    fn retirement_preserves_replaced_source() {
        let root = tempfile::tempdir().unwrap();
        let directory = AssetDirectory::open(root.path()).unwrap();
        let name = OsStr::new("N00E000.hgt");
        let old = directory.write(name, b"old").unwrap();
        let expected = identity(&old.metadata().unwrap());
        directory.write(name, b"replacement").unwrap();
        assert!(retire(&directory, name, expected).is_err());
        assert_eq!(fs::read(root.path().join(name)).unwrap(), b"replacement");
    }
}
