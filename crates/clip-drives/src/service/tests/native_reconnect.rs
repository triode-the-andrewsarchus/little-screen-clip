//! Actual device/mount relocation and cloned identities without a GUI process.
use super::*;
use crate::sync_edge_tests::native_fs::{self, GuestMount};
use std::fs::FileTimes;

struct Loop(Option<PathBuf>);
impl Loop {
    fn attach(image: &Path) -> Self {
        crate::require_disposable_guest();
        let output = CommandRequest::new("losetup")
            .args(["--find", "--show", "--partscan"])
            .arg(image)
            .run()
            .unwrap();
        assert!(output.status.success(), "{}", output.stderr_lossy());
        let name = String::from_utf8(output.stdout).unwrap();
        let owner = Self(Some(native_images::loop_device(name.trim())));
        for arguments in [
            vec!["trigger", "--action=add", "--subsystem-match=block"],
            vec!["settle", "--timeout=10"],
        ] {
            let output = CommandRequest::new("udevadm")
                .args(arguments)
                .run()
                .unwrap();
            assert!(output.status.success(), "{}", output.stderr_lossy());
        }
        owner
    }
    fn partition(&self) -> PathBuf {
        PathBuf::from(format!("{}p1", self.0.as_ref().unwrap().display()))
    }
    fn detach(&mut self) {
        let output = CommandRequest::new("losetup")
            .arg("--detach")
            .arg(self.0.as_ref().unwrap())
            .run()
            .unwrap();
        assert!(output.status.success(), "{}", output.stderr_lossy());
        self.0 = None;
    }
}
impl Drop for Loop {
    fn drop(&mut self) {
        if let Some(device) = self.0.take() {
            let _ = CommandRequest::new("losetup")
                .arg("--detach")
                .arg(device)
                .run();
        }
    }
}

struct Media {
    mounted: Option<GuestMount>,
    device: Loop,
    temp: tempfile::TempDir,
    image: PathBuf,
    root: PathBuf,
    filesystem: String,
    id: String,
}
impl Media {
    fn new() -> Self {
        let source = native_fs::guest_partition();
        assert!(matches!(
            source.filesystem.as_str(),
            "vfat" | "exfat" | "ntfs3"
        ));
        let temp = tempfile::tempdir().unwrap();
        let image = temp.path().join("owned.img");
        // The runner owns this source image. Quiesce it before copying the
        // entire GPT backing file; mounted NTFS copies retain the dirty flag.
        let output = CommandRequest::new("/usr/bin/umount")
            .arg(source.mount.as_ref().unwrap())
            .run()
            .unwrap();
        assert!(output.status.success(), "{}", output.stderr_lossy());
        let copied = fs::copy(
            Path::new("/work").join(format!("{}.img", source.filesystem)),
            &image,
        );
        let output = CommandRequest::new("/usr/bin/mount")
            .args(["--internal-only", "-t", &source.filesystem])
            .arg(&source.device)
            .arg(source.mount.as_ref().unwrap())
            .run()
            .unwrap();
        assert!(output.status.success(), "{}", output.stderr_lossy());
        copied.unwrap();
        let uuid = uuid::Uuid::new_v4();
        let output = CommandRequest::new("sfdisk")
            .arg("--part-uuid")
            .arg(&image)
            .arg("1")
            .arg(uuid.to_string())
            .run()
            .unwrap();
        assert!(output.status.success(), "{}", output.stderr_lossy());
        let mut media = Self {
            mounted: None,
            device: Loop::attach(&image),
            root: PathBuf::new(),
            temp,
            image,
            filesystem: source.filesystem,
            id: format!("part:{uuid}"),
        };
        media.mount("first-mount");
        media
    }
    fn mount(&mut self, name: &str) {
        assert!(self.mounted.is_none());
        self.root = self.temp.path().join(name);
        fs::create_dir_all(&self.root).unwrap();
        self.mounted = Some(GuestMount::start(
            CommandRequest::new("/usr/bin/mount")
                .args(["--internal-only", "-t", &self.filesystem])
                .arg(self.device.partition()),
            &self.root,
        ));
    }
    fn eject(&mut self, service: &Service) {
        finish(
            service,
            DriveRequest::Eject {
                drive: self.id.clone(),
            },
        );
        drop(self.mounted.take());
    }
    fn relocate_and_edit(&mut self, service: &Service) -> Loop {
        self.eject(service);
        let previous = self.device.0.clone().unwrap();
        self.device.detach();
        let blank = self.temp.path().join("old-number.img");
        File::create(&blank).unwrap().set_len(1024 * 1024).unwrap();
        let reservation = Loop::attach(&blank);
        assert_eq!(reservation.0.as_ref(), Some(&previous));
        self.device = Loop::attach(&self.image);
        assert_ne!(self.device.0.as_ref(), Some(&previous));
        // Another mount namespace models edits on another computer. Discovery
        // in the live owner's namespace must still see an unmounted partition.
        let output = CommandRequest::new("unshare")
            .args(["--mount", "--propagation", "private", "--", "/bin/sh", "-c"])
            .arg("set -eu; /usr/bin/mount --internal-only -t \"$1\" \"$2\" \"$3\"; /usr/bin/touch -r \"$3/report.txt\" \"$3/stamp\"; /usr/bin/printf drive-side > \"$3/report.txt\"; /usr/bin/touch -r \"$3/stamp\" \"$3/report.txt\"; /usr/bin/rm \"$3/stamp\"; /usr/bin/printf portable-only > \"$3/portable-only.txt\"; /usr/bin/umount \"$3\"")
            .arg("owned-offline-edit")
            .arg(&self.filesystem)
            .arg(self.device.partition())
            .arg(&self.root)
            .run()
            .unwrap();
        assert!(output.status.success(), "{}", output.stderr_lossy());
        reservation
    }
}

fn wait_status(service: &Service, ready: impl Fn(&DriveStatus) -> bool) -> DriveStatus {
    let started = Instant::now();
    loop {
        let observed = status(service);
        if ready(&observed) {
            return observed;
        }
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "Missing native reconnect state: {observed:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
#[ignore = "requires owned GPT loops and mount namespaces inside the disposable guest"]
fn native_reconnect_preserves_offline_edits_and_suspends_cloned_partition_ids() {
    let mut media = Media::new();
    fs::write(media.root.join("report.txt"), b"baseline00").unwrap();
    fs::write(media.root.join("restore.txt"), b"restore by default").unwrap();
    let local = media.temp.path().join("local");
    let service = Service::start_with_inventory(
        DriveConfig {
            sync_interval_seconds: 1,
            ram_min_available: u64::MAX,
            ..Default::default()
        },
        media.temp.path().join("state"),
        Arc::new(discovery::discover),
    )
    .unwrap();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let pair = enroll_pair(&service, &media.id, Path::new(""), &local);
        finish(
            &service,
            DriveRequest::Sync {
                pair: pair.id.clone(),
            },
        );
        let original = discovery::select(discovery::discover().unwrap(), &media.id).unwrap();
        assert_eq!(
            original.drive_capacity,
            fs::metadata(&media.image).unwrap().len()
        );
        assert_eq!(
            original.capacity,
            std::env::var("LITTLE_CLIP_DRIVE_TEST_CAPACITY")
                .unwrap()
                .parse::<u64>()
                .unwrap()
        );
        assert_eq!(
            original.filesystem_uuid,
            Some(std::env::var("LITTLE_CLIP_DRIVE_TEST_UUID").unwrap())
        );
        assert!(original.partition_layout.is_some());
        assert!(!original.read_only);

        reconnect(&mut media, &service, &local, &original);
        cloned_identity(&mut media, &service, &local, &pair.id);
        let observed = status(&service);
        assert_eq!(observed.pairs.len(), 1);
        assert_eq!(observed.pairs[0].id, pair.id);
        assert_eq!(observed.pairs[0].evidence, pair.evidence);
        assert_eq!(
            observed.conflicts.len(),
            1,
            "Repeated reconciliation duplicated a conflict"
        );
    }));
    service.shutdown().unwrap();
    result.unwrap();
}

fn reconnect(media: &mut Media, service: &Service, local: &Path, original: &Drive) {
    let before = fs::metadata(local.join("report.txt"))
        .unwrap()
        .modified()
        .unwrap();
    let _old_number = media.relocate_and_edit(service);
    wait_status(service, |status| {
        status
            .drives
            .iter()
            .any(|drive| drive.id == media.id && drive.mount.is_none())
    });
    fs::write(local.join("report.txt"), b"local-side").unwrap();
    File::options()
        .write(true)
        .open(local.join("report.txt"))
        .unwrap()
        .set_times(FileTimes::new().set_modified(before))
        .unwrap();
    fs::write(local.join("local-only.txt"), b"local-only").unwrap();
    fs::remove_file(local.join("restore.txt")).unwrap();
    media.mount("second-mount");
    let current = discovery::select(discovery::discover().unwrap(), &media.id).unwrap();
    assert_eq!(current.id, original.id);
    assert_eq!(current.evidence, original.evidence);
    assert_ne!(current.device, original.device);
    assert_ne!(current.disk_sequence, original.disk_sequence);
    assert_ne!(current.mount, original.mount);
    eprintln!(
        "NATIVE_RECONNECTED_DEVICE old={} new={} old_mount={} new_mount={}",
        original.device.display(),
        current.device.display(),
        original.mount.as_ref().unwrap().display(),
        current.mount.as_ref().unwrap().display()
    );
    let merged = wait_status(service, |status| {
        status.conflicts.iter().any(|conflict| conflict.complete)
    });
    let conflict = merged
        .conflicts
        .into_iter()
        .find(|conflict| conflict.complete)
        .unwrap();
    assert!(!conflict.resolved);
    for root in [media.root.as_path(), local] {
        wait_contents(&root.join(&conflict.portable), b"drive-side");
        wait_contents(&root.join(&conflict.local), b"local-side");
        wait_contents(&root.join("portable-only.txt"), b"portable-only");
        wait_contents(&root.join("local-only.txt"), b"local-only");
        wait_contents(&root.join("restore.txt"), b"restore by default");
    }
    assert_eq!(
        fs::read(media.root.join("report.txt")).unwrap(),
        b"drive-side"
    );
    assert_eq!(fs::read(local.join("report.txt")).unwrap(), b"local-side");
}

fn cloned_identity(media: &mut Media, service: &Service, local: &Path, pair: &str) {
    // Copy only after unmount has flushed this entirely owned guest filesystem.
    media.eject(service);
    let image = media.temp.path().join("cloned.img");
    fs::copy(&media.image, &image).unwrap();
    let mut clone = Loop::attach(&image);
    media.mount("second-mount");
    let before = native_images::digest(File::open(&image).unwrap());
    let ambiguous = wait_status(service, |status| {
        status
            .drives
            .iter()
            .any(|drive| drive.id == media.id && drive.ambiguous)
    });
    assert_eq!(
        ambiguous
            .drives
            .iter()
            .filter(|drive| drive.id == media.id && drive.ambiguous)
            .count(),
        2
    );
    fs::write(
        local.join("wait-for-unambiguous.txt"),
        b"queued while cloned",
    )
    .unwrap();
    wait_status(service, |status| {
        status
            .pairs
            .iter()
            .any(|saved| saved.id == pair && saved.status.contains("Multiple drives"))
    });
    failed(
        service,
        DriveRequest::Sync { pair: pair.into() },
        "Multiple drives",
    );
    assert!(!media.root.join("wait-for-unambiguous.txt").exists());
    assert_eq!(native_images::digest(File::open(&image).unwrap()), before);
    clone.detach();
    wait_status(service, |status| {
        status
            .drives
            .iter()
            .any(|drive| drive.id == media.id && !drive.ambiguous)
    });
    wait_contents(
        &media.root.join("wait-for-unambiguous.txt"),
        b"queued while cloned",
    );
}
