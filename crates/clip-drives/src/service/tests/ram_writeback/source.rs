//! Owned source storage for shared FUSE and native-filesystem failure checks.
use super::*;

pub(super) struct Portable {
    pub path: PathBuf,
    pub reservation_root: PathBuf,
    backing: Option<(RamBacking, String, crate::ram::Mounted)>,
    _media: Option<tempfile::TempDir>,
}
impl Portable {
    pub fn new(fixture: &TestDrives) -> Self {
        if let Some(media) = fixture.guest_source() {
            let path = media.path().to_owned();
            fs::write(path.join("edit"), b"original portable content").unwrap();
            let mut drives = fixture.drives.lock().unwrap();
            let drive = &mut drives[1];
            drive.capacity = std::env::var("LITTLE_CLIP_DRIVE_TEST_CAPACITY")
                .unwrap()
                .parse()
                .unwrap();
            drive.drive_capacity = drive.capacity;
            eprintln!(
                "NATIVE_WRITEBACK_SOURCE={} capacity={}",
                drive.filesystem, drive.capacity
            );
            Self {
                reservation_root: path.clone(),
                path,
                backing: None,
                _media: Some(media),
            }
        } else {
            let (backing, id, server, path) = fixture.native_source("partition");
            Self {
                path,
                reservation_root: backing.directory.join("contents"),
                backing: Some((backing, id, server)),
                _media: None,
            }
        }
    }
    pub fn finish(mut self) {
        if let Some((backing, id, server)) = self.backing.take() {
            drop(server);
            crate::ram::discard(&backing, &id).unwrap();
        }
    }
}

pub(super) enum Failure {
    Full(PathBuf),
    ReadOnly(crate::sync_edge_tests::native_fs::GuestMount),
}
impl Failure {
    pub fn reservation(&self) -> Option<&Path> {
        match self {
            Self::Full(path) => Some(path),
            Self::ReadOnly(_) => None,
        }
    }
    pub fn assert_refused(&self, service: &Service, request: DriveRequest) {
        let operation = completed(service, request);
        assert_eq!(operation.state, "failed", "{}", operation.message);
        let reason = match self {
            Self::Full(_) => "no space left",
            Self::ReadOnly(_) => "read-only",
        };
        assert!(
            operation.message.to_lowercase().contains(reason),
            "{}",
            operation.message
        );
        eprintln!("WRITEBACK_REFUSAL={reason} {}", operation.message);
    }
    pub fn clear(self) {
        match self {
            Self::Full(path) => fs::remove_file(path).unwrap(),
            Self::ReadOnly(mut mount) => mount.writable(),
        }
    }
}

/// Distinguish a failed retry from a confirmation in the same wall-clock second.
pub(super) fn seed_confirmation(service: &Service, ram: &RamSession) {
    let db = Store::existing(&service.state.join("drives.sqlite"), false).unwrap();
    let mut saved: RamSession = db.get("ram", &ram.id).unwrap().unwrap();
    saved.last_flushed = Some(1);
    db.put("ram", &ram.id, &saved).unwrap();
}

/// Confirm the fixture's application write before removing native storage.
pub(super) fn write_edit(ram: &RamSession, bytes: &[u8]) {
    let path = ram.path.join("edit");
    fs::write(&path, bytes).unwrap();
    File::open(path).unwrap().sync_all().unwrap();
}
