//! RAM helper lifetime and durable sessions remain separate from catalog mounts.
use super::*;
pub(super) mod recovery;

#[derive(Clone)]
pub(super) struct Reconciliation {
    session: Option<RamSession>,
    live: bool,
    unidentified: Option<Arc<Root>>,
}
impl Reconciliation {
    pub fn capture(owner: &Owner, id: &str) -> Result<Self> {
        Ok(Self {
            session: owner.db.get("ram", id)?,
            live: owner.ram_children.contains_key(id),
            unidentified: owner.unidentified_ram.get(id).cloned(),
        })
    }
    pub fn current(&self, owner: &Owner, id: &str) -> Result<bool> {
        let now = Self::capture(owner, id)?;
        Ok(self.session == now.session
            && self.live == now.live
            && match (&self.unidentified, &now.unidentified) {
                (None, None) => true,
                (Some(a), Some(b)) => a.same_root(b),
                _ => false,
            })
    }
    pub fn validate(&self, pair: &SyncPair, drive: &Drive) -> Result<bool> {
        if pair.ram && self.session.is_none() {
            return Err("RAM session metadata is missing; retained contents need recovery before synchronization".into());
        }
        if pair.ram && drive.part_uuid.is_none() && drive.filesystem_uuid.is_none() {
            self.unidentified
                .as_ref()
                .ok_or("No persistent media identity; writeback requires a new explicit session")?
                .validate()?;
        }
        if let Some(session) = &self.session {
            available(session, self.live)?;
        }
        Ok(pair.ram && self.session.as_ref().is_some_and(|s| !s.ready))
    }
}

pub(super) fn same_source(a: &RamSession, b: &RamSession) -> bool {
    a.id == b.id
        && a.drive == b.drive
        && a.path == b.path
        && a.identity == b.identity
        && a.ready == b.ready
        && a.backing == b.backing
}

pub(super) fn recover_view(
    db: &Store,
    expected: &RamSession,
    pair: Option<&SyncPair>,
    live: bool,
    helper: &mut Option<crate::ram::Mounted>,
) -> Result<(RamSession, Option<SyncPair>)> {
    let mut session: RamSession = db.get("ram", &expected.id)?.ok_or("Unknown RAM session")?;
    if !same_source(&session, expected) || db.get::<SyncPair>("pair", &session.id)?.as_ref() != pair
    {
        return Err("RAM ownership changed before preparation; retry".into());
    }
    if !live {
        *helper = start_view(db, &mut session)?;
    }
    available(&session, live || helper.is_some())?;
    let pair = db.get("pair", &session.id)?;
    Ok((session, pair))
}

/// Starts the same identity-checked view for initial preparation and restart recovery.
pub(super) fn start_view(
    db: &Store,
    session: &mut RamSession,
) -> Result<Option<crate::ram::Mounted>> {
    let Some(backing) = &mut session.backing else {
        return Ok(None);
    };
    if session.path != backing.directory.join("contents") {
        return Err("RAM view path differs from its retained backing".into());
    }
    let mut pair = db.get::<SyncPair>("pair", &session.id)?;
    if pair
        .as_ref()
        .is_some_and(|pair| !pair.ram || pair.local != session.path || pair.drive != session.drive)
    {
        return Err("RAM enrollment differs from retained session; refusing recovery".into());
    }
    let helper = crate::ram::Mounted::start(
        backing,
        &session.id,
        // A crash can precede the first ready-record commit after mounting.
        // Check even a newly formatted image before exposing its view.
        false,
        backing.initialized.then_some(session.identity.mount),
    )?;
    backing.mounted = true;
    backing.initialized = true;
    session.identity = kernel_interface::MountIdentity::directory(&session.path)?;
    session.status = if session.ready {
        "RAM view recovered; reconciling connected media"
    } else {
        "Initial RAM copy incomplete; use Synchronize to resume"
    }
    .into();
    if let Some(pair) = &mut pair {
        pair.local_identity = Some(Root::open(&session.path)?.storage_identity()?);
    }
    let transaction = db.0.unchecked_transaction()?;
    db.put("ram", &session.id, session)?;
    if let Some(pair) = &pair {
        db.put("pair", &pair.id, pair)?;
    }
    transaction.commit()?;
    Ok(Some(helper))
}

impl Owner {
    pub(super) fn mark_ram_unmounted(&self) {
        if let Ok(sessions) = self.db.list::<RamSession>("ram", 0, 1000) {
            for mut session in sessions {
                if let Some(backing) = &mut session.backing {
                    backing.mounted = false;
                    let _ = self.db.put("ram", &session.id, &session);
                }
            }
        }
    }
    pub(super) fn reap_ram(&mut self) {
        let exited = self
            .ram_children
            .iter_mut()
            .filter_map(|(id, helper)| helper.exited().then(|| id.clone()))
            .collect::<Vec<_>>();
        for id in exited {
            self.ram_children.remove(&id);
            self.sync_roots.remove(&id);
            if let Ok(Some(mut session)) = self.db.get::<RamSession>("ram", &id) {
                if let Some(backing) = &mut session.backing {
                    backing.mounted = false;
                }
                session.status =
                    "RAM filesystem stopped; image retained. Synchronize to retry recovery".into();
                let _ = self.db.put("ram", &id, &session);
            }
        }
    }
    pub(super) fn discard_ram(&mut self, session: &RamSession, cancel: &AtomicBool) -> Result<()> {
        self.sync_roots.remove(&session.id);
        if let Some(backing) = &session.backing {
            crate::ram::check_discard(backing)?;
            self.ram_children.remove(&session.id);
            let mut retained = session.clone();
            if let Some(backing) = &mut retained.backing {
                backing.mounted = false;
            }
            retained.status =
                "RAM view stopped for discard; backing retained until removal completes".into();
            self.db.put("ram", &session.id, &retained)?;
            crate::ram::discard(backing, &session.id)?;
            return Ok(());
        }
        let root = Root::open(&session.path)?;
        if kernel_interface::MountIdentity::directory(&root.path)? != session.identity {
            return Err("RAM directory identity changed; refusing discard".into());
        }
        let base = Root::open(root.path.parent().ok_or("Missing RAM parent")?)?;
        if !root
            .path
            .file_name()
            .is_some_and(|n| n.as_encoded_bytes().starts_with(b"little-clip-ram-"))
            || rustix::fs::fstatfs(&base.directory(Path::new(""), false)?)?.f_type as u32
                != 0x01021994
        {
            return Err("RAM session path is no longer owned".into());
        }
        let coverage = scan::scan(
            &self.db,
            &root,
            "discard-check",
            &scan::Limits {
                hashes: false,
                ..Default::default()
            },
            cancel,
            |_, _| {},
        )?;
        if !coverage.complete {
            return Err("Cannot validate RAM tree for discard".into());
        }
        root.validate()?;
        fs::remove_dir_all(&root.path)?;
        Ok(())
    }
}

pub(super) fn available(session: &RamSession, live: bool) -> Result<()> {
    if let Some(backing) = &session.backing {
        if !backing.mounted
            || !live
            || crate::mounts::owned_mount(&session.path, "little-clip-ram", &session.id)?.is_none()
        {
            return Err(
                "RAM filesystem is unavailable; retained image must be recovered first".into(),
            );
        }
        crate::ram::open(backing)?;
    }
    if kernel_interface::MountIdentity::directory(&session.path)? != session.identity {
        return Err("RAM view identity changed; refusing access".into());
    }
    Ok(())
}
