//! Resolve saved spellings through one verified original, never a guessed collation.
use super::*;
use little_clip_core::kernel_interface::MountLifetime;

#[derive(Serialize, Deserialize)]
struct Source {
    #[serde(with = "little_clip_core::path_serde")]
    path: PathBuf,
    lifetime: MountLifetime,
}

pub(crate) fn record_source(db: &Store, root: &Root) -> Result<()> {
    let pin = root.directory(Path::new(""), false)?;
    // Older kernels can still save metadata. They cannot authorize this optional
    // live alias resolution, and exact saved spellings remain usable offline.
    if let Ok(lifetime) = MountLifetime::capture(&pin) {
        db.put(
            "source",
            "redaction-context",
            &Source {
                path: root.path.clone(),
                lifetime,
            },
        )?;
    }
    Ok(())
}

impl Source {
    fn open(&self) -> Result<Root> {
        use rustix::fs::{CWD, Mode, OFlags, ResolveFlags, openat2};
        // Do not let a replaced ancestor redirect this observation through a
        // symlink. The retained pin is checked before generic Root construction.
        let pin = File::from(openat2(
            CWD,
            &self.path,
            OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::NO_SYMLINKS,
        )?);
        self.lifetime.validate(&pin)?;
        let root = Root::open(&crate::root::anchor(&pin))?;
        if root.path != self.path {
            return Err("Catalog source directory moved".into());
        }
        Ok(root)
    }
    fn visit(
        &self,
        db: &Store,
        relative: &Path,
        cancel: &AtomicBool,
        boundary: &mut impl FnMut(&Path) -> Result<()>,
    ) -> Result<()> {
        let root = self.open().map_err(|error| format!(
            "Catalog metadata cleanup remains pending: original source is unavailable or changed; no name alias was assumed: {error}"
        ))?;
        let identity = |metadata: &std::fs::Metadata| (metadata.dev(), metadata.ino());
        let metadata = root.metadata(relative)?;
        let target = metadata.as_ref().map(identity);
        if !visit_directory(db, &root, relative, metadata.as_ref(), boundary)? {
            visit_live(db, &root, relative, target, cancel, boundary)?;
        }
        if root.metadata(relative)?.as_ref().map(identity) != target {
            return Err("Catalog exclusion changed during name resolution; metadata cleanup remains pending".into());
        }
        let pin = root.directory(Path::new(""), false)?;
        self.lifetime.validate(&pin)?;
        Ok(())
    }
}

fn visit_directory(
    db: &Store,
    root: &Root,
    relative: &Path,
    metadata: Option<&std::fs::Metadata>,
    boundary: &mut impl FnMut(&Path) -> Result<()>,
) -> Result<bool> {
    let Some(metadata) = metadata.filter(|metadata| metadata.is_dir()) else {
        return Ok(false);
    };
    let pin = root.directory(relative, false)?;
    let current = pin.metadata()?;
    if (current.dev(), current.ino()) != (metadata.dev(), metadata.ino()) {
        return Err("Catalog directory changed during name resolution".into());
    }
    let canonical = std::fs::read_link(crate::root::anchor(&pin))?;
    let relative = canonical
        .strip_prefix(&root.path)
        .map_err(|_| "Catalog boundary left the observed source root")?;
    if scan::entry(db, "catalog", relative)?.is_none() {
        return Ok(false);
    }
    // Ordinary directories have no hard-link aliases. A pinned canonical saved
    // name avoids probing every sibling or any child of a million-file folder.
    boundary(relative)?;
    Ok(true)
}

pub(in crate::catalog) fn visit(
    db: &Store,
    relative: &Path,
    cancel: &AtomicBool,
    mut boundary: impl FnMut(&Path) -> Result<()>,
) -> Result<()> {
    crate::cancelled(cancel)?;
    if relative.as_os_str().is_empty() || scan::entry(db, "catalog", relative)?.is_some() {
        return boundary(relative);
    }
    let has_entries: bool = db.0.query_row(
        "SELECT EXISTS(SELECT 1 FROM entries WHERE scan='catalog')",
        [],
        |row| row.get(0),
    )?;
    if !has_entries {
        return Ok(());
    }
    let observed: Source = db.get_bounded("source", "redaction-context", 16384)?
        .ok_or("Saved catalog has no verified name-resolution context; alias cleanup remains pending. Exact saved names or an explicit whole-drive catalog exclusion can remove retained metadata")?;
    observed.visit(db, relative, cancel, &mut boundary)
}

fn visit_live(
    db: &Store,
    root: &Root,
    relative: &Path,
    target: Option<(u64, u64)>,
    cancel: &AtomicBool,
    boundary: &mut impl FnMut(&Path) -> Result<()>,
) -> Result<()> {
    let depth = relative.components().count();
    // The existing cursor bounds decoding and remains valid while the caller
    // marks boundaries and deletes descendants in this same transaction.
    for entry in scan::Cursor::new(db, "catalog") {
        crate::cancelled(cancel)?;
        let entry = entry?;
        if entry.path.components().count() != depth {
            continue;
        }
        let current = root
            .metadata(&entry.path)?
            .map(|m| (m.dev(), m.ino()))
            .ok_or(
                "Saved catalog names are no longer resolvable; metadata cleanup remains pending",
            )?;
        if Some(current) == target {
            boundary(&entry.path)?;
        }
    }
    Ok(())
}
