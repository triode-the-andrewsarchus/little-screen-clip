//! Disk-backed source-inode groups; only equal captured revisions share an inode.
use super::*;

pub(super) fn finish(
    report: &Report<'_>,
    target: &Root,
    scan: &str,
    cancel: &AtomicBool,
) -> Result<()> {
    let mut statement = report.db.0.prepare(
        "SELECT identity,count(*) FROM backup_links GROUP BY identity ORDER BY identity",
    )?;
    let groups = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?))
    })?;
    for group in groups {
        cancelled(cancel)?;
        let (identity, count) = group?;
        let mut group = Group::collect(report, target, scan, &identity, cancel)?;
        visit(report.db, scan, &identity, |entry, facts| {
            group.record(report, target, &entry, facts, count, cancel)
        })?;
    }
    Ok(())
}

fn visit(
    db: &Store,
    scan: &str,
    identity: &str,
    mut visit: impl FnMut(Entry, Facts) -> Result<()>,
) -> Result<()> {
    // Inventory insertion order selects the first access-time observation.
    // readlink may update that shared timestamp before observing another alias.
    let mut statement = db.0.prepare(
        "SELECT e.json,m.json FROM backup_links l
        JOIN entries e ON e.path=l.path AND e.scan=?1
        JOIN backup_metadata m ON m.path=l.path
        WHERE l.identity=?2 ORDER BY e.rowid",
    )?;
    let rows = statement.query_map(rusqlite::params![scan, identity], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (entry, facts) = row?;
        visit(serde_json::from_str(&entry)?, serde_json::from_str(&facts)?)?;
    }
    Ok(())
}

struct Group {
    entry: Entry,
    facts: Facts,
    output: Object,
    incomplete: bool,
}
impl Group {
    fn collect(
        report: &Report<'_>,
        target: &Root,
        scan: &str,
        identity: &str,
        cancel: &AtomicBool,
    ) -> Result<Self> {
        let mut group: Option<Self> = None;
        visit(report.db, scan, identity, |entry, mut facts| {
            cancelled(cancel)?;
            if let Some(group) = group.as_mut() {
                group.install(report, target, &entry, &mut facts, cancel)?;
            } else {
                let output = Object::open(target, &entry)?;
                verify_contents(&output, &entry, cancel)?;
                group = Some(Self {
                    entry,
                    facts,
                    output,
                    incomplete: false,
                });
            }
            Ok(())
        })?;
        let group = group.ok_or("Missing hard-link group")?;
        group.verify_representative(target, cancel)?;
        Ok(group)
    }
    fn install(
        &mut self,
        report: &Report<'_>,
        target: &Root,
        entry: &Entry,
        facts: &mut Facts,
        cancel: &AtomicBool,
    ) -> Result<()> {
        if !same_capture(&self.entry, &self.facts, entry, facts) {
            return Err(crate::Error::SourceChanged(format!(
                "Hard-linked source revisions differ between {} and {}; restart backup collection; previous backup retained",
                little_clip_core::path_address::display_path(&self.entry.path),
                little_clip_core::path_address::display_path(&entry.path)
            )));
        }
        self.verify_representative(target, cancel)?;
        let previous = Object::open(target, entry)?;
        verify_contents(&previous, entry, cancel)?;
        if same_inode(&self.output, &previous)? {
            return Ok(());
        }
        if let Some(error) = install(target, &self.output, &previous, entry, cancel)? {
            facts.note(
                Field::HardLinks,
                None,
                Some(&error),
                "Destination could not create a hard link; complete independent payload retained",
            );
            report.save_facts(entry, facts)?;
            self.incomplete = true;
        }
        Ok(())
    }
    fn verify_representative(&self, target: &Root, cancel: &AtomicBool) -> Result<()> {
        self.output.validate_name(target, &self.entry, cancel)?;
        verify_contents(&self.output, &self.entry, cancel)
    }
    fn record(
        &mut self,
        report: &Report<'_>,
        target: &Root,
        entry: &Entry,
        mut facts: Facts,
        count: u64,
        cancel: &AtomicBool,
    ) -> Result<()> {
        cancelled(cancel)?;
        let output = Object::open(target, entry)?;
        if same_inode(&self.output, &output)? {
            facts = self.facts.clone();
            if entry.path != self.entry.path {
                facts.hard_link_to = Some(self.entry.path.clone());
            }
        }
        if count < facts.links {
            facts.note(Field::HardLinks, None, None, &format!(
                "Only {count} of {} source links were captured; names outside the selected tree are excluded", facts.links));
        }
        if self.incomplete
            && !facts
                .omissions
                .iter()
                .any(|item| item.field == Field::HardLinks)
        {
            facts.note(
                Field::HardLinks,
                None,
                None,
                "Destination did not reproduce every captured alias; see per-path link failures",
            );
        }
        verify_contents(&output, entry, cancel)?;
        facts.restore_times(&output, entry)?;
        output.validate_name(target, entry, cancel)?;
        output.sync()?;
        report.save_facts(entry, &facts)
    }
}

fn same_capture(a: &Entry, af: &Facts, b: &Entry, bf: &Facts) -> bool {
    // Access time is observational and can change from our own inventory reads.
    // The group shares its first inventory observation instead of rejecting it.
    a.same_revision(b)
        && a.hash == b.hash
        && a.link == b.link
        && af.created == bf.created
        && af.links == bf.links
        && af.attributes_complete == bf.attributes_complete
        && af.attribute_names_complete == bf.attribute_names_complete
        && af.attribute_list_errno == bf.attribute_list_errno
        && af.attributes == bf.attributes
}

fn same_inode(a: &Object, b: &Object) -> Result<bool> {
    let a = a.metadata()?;
    let b = b.metadata()?;
    Ok((a.dev(), a.ino(), a.mode() & libc::S_IFMT) == (b.dev(), b.ino(), b.mode() & libc::S_IFMT))
}

/// A link(2) capability failure preserves the independent payload. Guard,
/// cancellation and installation failures stop publication instead of falling back.
fn install(
    target: &Root,
    original: &Object,
    previous: &Object,
    entry: &Entry,
    cancel: &AtomicBool,
) -> Result<Option<std::io::Error>> {
    let (parent, destination) = target.parent(&entry.path, false)?;
    let scratch = tempfile::Builder::new()
        .prefix(".hard-link-")
        .tempdir_in(anchor(&parent))?;
    let staged = scratch.path().join("link");
    if let Err(error) = original.link_to(&staged) {
        return Ok(Some(error));
    }
    previous.validate_name(target, entry, cancel)?;
    fs::rename(&staged, &destination)?;
    File::open(anchor(&parent))?.sync_all()?;
    let installed = Object::open(target, entry)?;
    if !same_inode(original, &installed)? {
        return Err("Installed backup hard link was replaced".into());
    }
    target.validate()?;
    Ok(None)
}
