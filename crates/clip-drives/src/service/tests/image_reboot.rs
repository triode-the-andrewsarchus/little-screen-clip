//! Two real kernel boots share only their explicitly owned virtual state disk.
use super::*;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct Saved {
    origin: backup::ImageOrigin,
    uncertain: DriveBackup,
    ready: DriveBackup,
    partial: DriveBackup,
    unrecorded_object: String,
}

#[test]
#[ignore = "requires two isolated guest boots and an owned persistent state disk"]
fn native_image_state_recovers_after_a_real_guest_reboot() {
    crate::require_disposable_guest();
    let filesystem = std::env::var("LITTLE_CLIP_DRIVE_TEST_FILESYSTEM").unwrap();
    assert!(matches!(filesystem.as_str(), "vfat" | "exfat" | "ntfs3"));
    let directory = PathBuf::from(std::env::var_os("LITTLE_CLIP_DRIVE_TEST_REBOOT_DIR").unwrap())
        .join(filesystem);
    fs::create_dir_all(&directory).unwrap();
    match std::env::var("LITTLE_CLIP_DRIVE_TEST_REBOOT_PHASE")
        .unwrap()
        .as_str()
    {
        "prepare" => prepare(&directory),
        "recover" => recover(&directory),
        _ => panic!("Select the owned reboot phase"),
    }
}

fn start(directory: &Path, authority: partition::Authority) -> Service {
    Service::start_with_authority(
        DriveConfig {
            backup_directory: Some(directory.join("backups")),
            ram_min_available: u64::MAX,
            ..Default::default()
        },
        directory.join("state"),
        Arc::new(discovery::discover),
        None,
        Arc::new(authority),
        Default::default(),
    )
    .unwrap()
}

fn prepare(directory: &Path) {
    let origin = backup::ImageOrigin::capture().unwrap();
    assert!(
        origin.host.is_some(),
        "Fixture must retain its installed host identity"
    );
    let original = native_images::selected(&crate::sync_edge_tests::native_fs::guest_drive());
    fs::write(
        original.mount.as_ref().unwrap().join("reboot-image.txt"),
        b"retained across a real reboot",
    )
    .unwrap();
    let first = Arc::new(AtomicBool::new(true));
    let created = Arc::new(Mutex::new(None));
    let saved_object = created.clone();
    let mut authority = partition::Authority::default();
    let mount = authority.images.mount.clone();
    authority.images.mount = Arc::new(move |db, id, phase| {
        if first.swap(false, Ordering::AcqRel) {
            *saved_object.lock().unwrap() = Some(backup::unrecorded_loop_for_test(db, id, phase)?);
            Err("owned lost LoopSetup reply".into())
        } else {
            mount(db, id, phase)
        }
    });
    let service = start(directory, authority);
    let request = DriveRequest::Backup {
        drive: original.id,
        kind: BackupKind::Image,
        restore_mounted: true,
    };
    finish(&service, request.clone());
    let uncertain = status(&service).backups.pop().unwrap();
    failed(
        &service,
        DriveRequest::MountImage {
            backup: uncertain.id.clone(),
        },
        "lost LoopSetup reply",
    );
    finish(&service, request.clone());
    let id = status(&service)
        .backups
        .into_iter()
        .find(|version| version.id != uncertain.id)
        .unwrap()
        .id;
    finish(&service, DriveRequest::MountImage { backup: id.clone() });
    let db = Store::existing(&service.state.join("drives.sqlite"), false).unwrap();
    let ready: DriveBackup = db.get("backup", &id).unwrap().unwrap();
    assert!(
        db.get::<serde_json::Value>("image-control", &uncertain.id)
            .unwrap()
            .is_some()
    );
    assert!(
        db.get::<serde_json::Value>("image-loop", &ready.id)
            .unwrap()
            .is_some()
    );
    assert!(ready.mount.is_some());
    finish(&service, request);
    let id = status(&service)
        .backups
        .into_iter()
        .find(|version| version.id != uncertain.id && version.id != ready.id)
        .unwrap()
        .id;
    finish(&service, DriveRequest::MountImage { backup: id.clone() });
    let mounted: DriveBackup = db.get("backup", &id).unwrap().unwrap();
    let device = native_images::loop_device(mounted.loop_object.as_deref().unwrap());
    let reader = File::open(device).unwrap();
    failed(
        &service,
        DriveRequest::UnmountImage { backup: id.clone() },
        "deferred by open device readers",
    );
    let partial: DriveBackup = db.get("backup", &id).unwrap().unwrap();
    assert!(partial.mount.is_none() && partial.loop_object.is_some());
    let saved = Saved {
        origin,
        uncertain,
        ready,
        partial,
        unrecorded_object: created.lock().unwrap().clone().unwrap(),
    };
    fs::write(
        directory.join("images.json"),
        serde_json::to_vec(&saved).unwrap(),
    )
    .unwrap();
    File::open(directory.join("images.json"))
        .unwrap()
        .sync_all()
        .unwrap();
    service.shutdown().unwrap();
    for image in [&saved.uncertain, &saved.ready, &saved.partial] {
        let loops = CommandRequest::new("losetup")
            .arg("-j")
            .arg(&image.path)
            .run()
            .unwrap();
        assert!(
            loops.status.success() && !loops.stdout.is_empty(),
            "First boot must leave a real mapping"
        );
    }
    drop(reader);
    eprintln!("REBOOT_IMAGE_STATE_PREPARED boot={}", saved.origin.boot);
}

fn recover(directory: &Path) {
    let saved: Saved =
        serde_json::from_slice(&fs::read(directory.join("images.json")).unwrap()).unwrap();
    let current = backup::ImageOrigin::capture().unwrap();
    assert_ne!(
        saved.origin.boot, current.boot,
        "A second process is not a kernel reboot"
    );
    assert_eq!(saved.origin.host, current.host);
    let foreign = tempfile::tempdir().unwrap();
    let objects = [
        &saved.unrecorded_object,
        saved.ready.loop_object.as_ref().unwrap(),
        saved.partial.loop_object.as_ref().unwrap(),
    ];
    let mut devices = Vec::new();
    for (index, object) in objects.into_iter().enumerate() {
        let device = native_images::loop_device(object);
        let image = foreign.path().join(format!("foreign-{index}.img"));
        fs::write(&image, b"foreign kernel attachment").unwrap();
        File::options()
            .write(true)
            .open(&image)
            .unwrap()
            .set_len(1024 * 1024)
            .unwrap();
        let output = CommandRequest::new("losetup")
            .arg("--read-only")
            .arg(&device)
            .arg(&image)
            .run()
            .unwrap();
        assert!(output.status.success(), "{}", output.stderr_lossy());
        devices.push((device, image));
    }
    let mut authority = partition::Authority::default();
    authority.images.mount =
        Arc::new(|_, _, _| panic!("Restart must not replay native mount authority"));
    authority.images.unmount = Arc::new(|_, _, _| panic!("Old-boot cleanup must be metadata-only"));
    let service = start(directory, authority);
    let db = Store::existing(&service.state.join("drives.sqlite"), false).unwrap();
    let started = Instant::now();
    loop {
        let unknown = db
            .get::<serde_json::Value>("image-control", &saved.uncertain.id)
            .unwrap()
            .is_some();
        let ready = db
            .get::<serde_json::Value>("image-loop", &saved.ready.id)
            .unwrap()
            .is_some();
        let partial = db
            .get::<serde_json::Value>("image-control", &saved.partial.id)
            .unwrap()
            .is_some();
        if !unknown && !ready && !partial {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "Old-boot image state did not retire"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    for image in [&saved.uncertain, &saved.ready, &saved.partial] {
        let retired: DriveBackup = db.get("backup", &image.id).unwrap().unwrap();
        assert!(retired.mount.is_none() && retired.loop_object.is_none());
        assert_eq!(
            native_images::digest(File::open(&image.path).unwrap()),
            *image.sha256.as_ref().unwrap()
        );
    }
    loop {
        let policy: IndexPolicy =
            serde_json::from_slice(&fs::read(service.state.join("index-policy.json")).unwrap())
                .unwrap();
        if !policy.excluded.contains(&PathBuf::from("/")) {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "Recovered policy did not publish automatically"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    service.shutdown().unwrap();
    for (device, image) in devices {
        let loops = CommandRequest::new("losetup")
            .arg("-j")
            .arg(&image)
            .run()
            .unwrap();
        assert!(
            loops.status.success() && !loops.stdout.is_empty(),
            "Foreign reused loop must survive recovery"
        );
        let mut bytes = [0_u8; 25];
        std::os::unix::fs::FileExt::read_exact_at(&File::open(&image).unwrap(), &mut bytes, 0)
            .unwrap();
        assert_eq!(&bytes, b"foreign kernel attachment");
        assert!(
            CommandRequest::new("losetup")
                .arg("--detach")
                .arg(device)
                .run()
                .unwrap()
                .status
                .success()
        );
    }
    eprintln!(
        "REBOOT_IMAGE_STATE_RECOVERED old={} current={}",
        saved.origin.boot, current.boot
    );
}
