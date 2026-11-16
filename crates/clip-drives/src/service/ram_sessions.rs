//! RAM helper lifetime and durable sessions remain separate from catalog mounts.
use super::*;

impl Owner {
    pub(super) fn mount_ram(&mut self, id: &str) -> Result<()> {
        let mut session: RamSession = self.db.get("ram", id)?.ok_or("Unknown RAM session")?;
        if self.ram_children.contains_key(id) {
            return self.ram_available(&session);
        }
        let Some(backing) = &mut session.backing else {
            return Ok(());
        };
        if session.path != backing.directory.join("contents") {
            return Err("RAM view path differs from its retained backing".into());
        }
        let helper = crate::ram::Mounted::start(
            backing,
            id,
            !backing.initialized,
            backing.initialized.then_some(session.identity.mount),
        )?;
        self.ram_children.insert(id.into(), helper);
        backing.mounted = true;
        backing.initialized = true;
        session.identity = kernel_interface::MountIdentity::directory(&session.path)?;
        session.status = if session.ready {
            "RAM view recovered; reconciling connected media"
        } else {
            "Initial RAM copy incomplete; use Synchronize to resume"
        }
        .into();
        self.db.put("ram", id, &session)?;
        if let Some(mut pair) = self.db.get::<SyncPair>("pair", id)? {
            if !pair.ram || pair.local != session.path || pair.drive != session.drive {
                return Err(
                    "RAM enrollment differs from retained session; refusing recovery".into(),
                );
            }
            pair.local_identity = Some(Root::open(&session.path)?.storage_identity()?);
            self.db.put("pair", id, &pair)?;
        }
        self.sync_roots.remove(id);
        Ok(())
    }
    pub(super) fn recover_ram(&mut self) {
        let Ok(sessions) = self.db.list::<RamSession>("ram", 0, 1000) else {
            return;
        };
        for mut session in sessions {
            if session.backing.is_some()
                && let Err(error) = self.mount_ram(&session.id)
            {
                session.status = format!("RAM image retained; recovery unavailable: {error}");
                let _ = self.db.put("ram", &session.id, &session);
            }
        }
    }
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
    pub(super) fn ram_available(&self, session: &RamSession) -> Result<()> {
        if let Some(backing) = &session.backing {
            if !backing.mounted
                || !self.ram_children.contains_key(&session.id)
                || crate::mounts::owned_mount(&session.path, "little-clip-ram", &session.id)?
                    .is_none()
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
