//! Memory pressure and abrupt daemon death run only in the disposable guest.
use super::*;
use std::{io::Write, os::unix::fs::MetadataExt};

const MIB: u64 = 1024 * 1024;

fn config() -> DriveConfig {
    crate::require_disposable_guest();
    let path = PathBuf::from(std::env::var_os("LITTLE_CLIP_DRIVE_RAM_TEST_BASE").unwrap());
    assert_eq!(path, Path::new("/work/ram-store"));
    let root = Root::open(&path).unwrap();
    let directory = root.directory(Path::new(""), false).unwrap();
    assert_eq!(
        rustix::fs::fstatfs(&directory).unwrap().f_type as u32,
        0x01021994
    );
    let stats = rustix::fs::fstatvfs(&directory).unwrap();
    assert!(stats.f_blocks * stats.f_frsize <= 512 * MIB);
    DriveConfig {
        ram_directory: path,
        ram_min_available: 0,
        sync_interval_seconds: 3600,
        ..Default::default()
    }
}

fn paused_copy(service: &Service) -> RamSession {
    finish(
        service,
        DriveRequest::CopyToRam {
            drive: "part:large".into(),
        },
    );
    let ram = status(service).ram.remove(0);
    finish(
        service,
        DriveRequest::Pause {
            pair: ram.id.clone(),
            paused: true,
        },
    );
    fs::write(ram.path.join("acknowledged"), b"retained RAM edit").unwrap();
    File::open(ram.path.join("acknowledged"))
        .unwrap()
        .sync_all()
        .unwrap();
    File::open(&ram.path).unwrap().sync_all().unwrap();
    wait_ram(service, &ram.id, |session| {
        session.pending.as_ref().unwrap().paths != 0
    });
    ram
}

fn write_chunks(file: &mut File, bytes: u64) -> std::io::Result<()> {
    let chunk = vec![0xa5; MIB as usize];
    for _ in 0..bytes / MIB {
        file.write_all(&chunk)?;
    }
    Ok(())
}

#[test]
#[ignore = "requires the disposable vm_ram.sh guest with bounded tmpfs mounts"]
fn available_memory_pressure_refuses_new_sessions_without_evicting_dirty_ram() {
    let mut config = config();
    let fixture = TestDrives::new(0);
    let service = fixture.start_configured(config.clone(), None, fixture.image_authority());
    let ram = paused_copy(&service);
    let initial_memory = available_memory().unwrap();
    config.ram_min_available = initial_memory.saturating_sub(128 * MIB);
    finish(
        &service,
        DriveRequest::Configure {
            config: config.clone(),
        },
    );
    let mut pressure = tempfile::tempfile_in("/work/pressure").unwrap();
    write_chunks(&mut pressure, 256 * MIB).unwrap();
    assert!(available_memory().unwrap() < config.ram_min_available);
    eprintln!(
        "memory before={initial_memory} threshold={} pressure={}",
        config.ram_min_available,
        available_memory().unwrap()
    );
    failed(
        &service,
        DriveRequest::CopyToRam {
            drive: "part:small".into(),
        },
        "Insufficient available RAM",
    );
    assert_eq!(status(&service).ram.len(), 1);
    assert_eq!(
        fs::read(ram.path.join("acknowledged")).unwrap(),
        b"retained RAM edit"
    );
    fs::write(
        ram.path.join("under-pressure"),
        b"existing session remains writable",
    )
    .unwrap();
    File::open(ram.path.join("under-pressure"))
        .unwrap()
        .sync_all()
        .unwrap();
    assert_eq!(status(&service).ram[0].path, ram.path);
    drop(pressure);
    eprintln!("memory after release={}", available_memory().unwrap());
    // MemAvailable is an estimate with a changing baseline. Restore this
    // fixture's normal minimum explicitly rather than bypassing its admission check.
    config.ram_min_available = 0;
    finish(&service, DriveRequest::Configure { config });
    finish(
        &service,
        DriveRequest::CopyToRam {
            drive: "part:small".into(),
        },
    );
    let second = status(&service)
        .ram
        .into_iter()
        .find(|session| session.id != ram.id)
        .unwrap();
    discard_ram(&service, &second);
    assert_eq!(
        fs::read(ram.path.join("under-pressure")).unwrap(),
        b"existing session remains writable"
    );
    discard_ram(&service, &ram);
    service.shutdown().unwrap();
}

#[test]
#[ignore = "requires the disposable vm_ram.sh guest with bounded tmpfs mounts"]
fn backing_tmpfs_exhaustion_retains_acknowledged_edits_through_restart() {
    let config = config();
    let fixture = TestDrives::new(0);
    let service = fixture.start_configured(config.clone(), None, fixture.image_authority());
    let ram = paused_copy(&service);
    let backing = ram.backing.clone().unwrap();
    let before = fs::metadata(backing.directory.join("volume.ext4")).unwrap();
    eprintln!(
        "image allocation {} / {}",
        before.blocks() * 512,
        before.len()
    );
    let mut fill = tempfile::tempfile_in(&config.ram_directory).unwrap();
    assert_eq!(
        write_chunks(&mut fill, 512 * MIB)
            .unwrap_err()
            .raw_os_error(),
        Some(libc::ENOSPC)
    );
    let logical = rustix::fs::fstatvfs(File::open(&ram.path).unwrap()).unwrap();
    let underlying = rustix::fs::fstatvfs(File::open(&config.ram_directory).unwrap()).unwrap();
    eprintln!(
        "underlying free={} logical free={} filled={}",
        underlying.f_bavail * underlying.f_frsize,
        logical.f_bavail * logical.f_frsize,
        fill.metadata().unwrap().len()
    );
    assert!(logical.f_bavail * logical.f_frsize > 64 * MIB);
    let mut growth = File::create(ram.path.join("unacknowledged-growth")).unwrap();
    let failure = write_chunks(&mut growth, 64 * MIB).unwrap_err();
    eprintln!("RAM growth failed with {failure}");
    assert!(
        matches!(failure.raw_os_error(), Some(libc::ENOSPC | libc::EIO)),
        "{failure}"
    );
    drop(growth);
    assert!(backing.directory.join("volume.ext4").exists());
    assert_eq!(status(&service).ram[0].path, ram.path);
    drop(fill);
    service.shutdown().unwrap();
    drop(service);
    let service = fixture.start_configured(config, None, fixture.image_authority());
    let recovered = wait_ram(&service, &ram.id, RamSession::browsable);
    assert_eq!(recovered.path, ram.path);
    assert_eq!(
        recovered.backing.as_ref().unwrap().directory,
        backing.directory
    );
    let after = fs::metadata(backing.directory.join("volume.ext4")).unwrap();
    assert_eq!((after.dev(), after.ino()), (before.dev(), before.ino()));
    assert_eq!(
        fs::read(ram.path.join("acknowledged")).unwrap(),
        b"retained RAM edit"
    );
    assert!(recovered.pending.as_ref().unwrap().full_check_required);
    fs::write(
        ram.path.join("after-recovery"),
        b"writable after backing space returned",
    )
    .unwrap();
    finish(
        &service,
        DriveRequest::FlushRam {
            session: ram.id.clone(),
        },
    );
    assert_eq!(
        fs::read(fixture.temp.path().join("large/acknowledged")).unwrap(),
        b"retained RAM edit"
    );
    assert_eq!(
        fs::read(fixture.temp.path().join("large/after-recovery")).unwrap(),
        b"writable after backing space returned"
    );
    discard_ram(&service, &ram);
    service.shutdown().unwrap();
}

#[test]
#[ignore = "internal child of abrupt_daemon_death_recovers_an_external_unflushed_write"]
fn crash_owner_probe() {
    let config = config();
    let root = PathBuf::from(std::env::var_os("LITTLE_CLIP_RAM_CRASH_FIXTURE").unwrap());
    assert!(root.starts_with("/work/tmp"));
    let drives: Vec<Drive> =
        serde_json::from_slice(&fs::read(root.join("drives.json")).unwrap()).unwrap();
    let service = Service::start_with_inventory(
        config,
        root.join("state"),
        Arc::new(move || Ok(drives.clone())),
    )
    .unwrap();
    let ram = paused_copy(&service);
    fs::write(root.join("ready.json"), serde_json::to_vec(&ram).unwrap()).unwrap();
    // This child owns the service, but never owns the external writer's file.
    // Bound its lifetime even if its test parent fails before terminating it.
    std::thread::sleep(Duration::from_secs(30));
    panic!("crash fixture owner was not terminated by its parent");
}

struct CrashOwner(std::process::Child);

impl Drop for CrashOwner {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
#[ignore = "requires the disposable vm_ram.sh guest with bounded tmpfs mounts"]
fn abrupt_daemon_death_recovers_an_external_unflushed_write() {
    for (relative, expected) in [
        (
            "external-unflushed",
            b"external write acknowledged without close or fsync".to_vec(),
        ),
        (
            "acknowledged",
            b"replacement of an existing RAM file".to_vec(),
        ),
        ("large-sparse", vec![0x5c; 8 * MIB as usize]),
    ] {
        crash_write(relative, &expected);
    }
}

fn crash_write(relative: &str, expected: &[u8]) {
    use std::os::fd::AsRawFd;

    let config = config();
    let fixture = TestDrives::new(0);
    let root = fixture.temp.path();
    fs::write(
        root.join("drives.json"),
        serde_json::to_vec(&*fixture.drives.lock().unwrap()).unwrap(),
    )
    .unwrap();
    let mut command = CommandRequest::new(std::env::current_exe().unwrap())
        .args([
            "service::tests::guest_ram::crash_owner_probe",
            "--exact",
            "--ignored",
            "--nocapture",
        ])
        .own_process_group()
        .die_with_parent()
        .build_command();
    command.env("LITTLE_CLIP_RAM_CRASH_FIXTURE", root);
    let mut owner = CrashOwner(command.spawn().unwrap());
    let started = Instant::now();
    let ram: RamSession = loop {
        if let Ok(bytes) = fs::read(root.join("ready.json"))
            && let Ok(ram) = serde_json::from_slice(&bytes)
        {
            break ram;
        }
        assert!(owner.0.try_wait().unwrap().is_none(), "owner exited early");
        assert!(started.elapsed() < Duration::from_secs(15));
        std::thread::sleep(Duration::from_millis(10));
    };
    let backing = ram.backing.as_ref().unwrap();
    let (_, image) = crate::ram::open(backing).unwrap();
    let identity = image.metadata().unwrap();
    let mut held = File::create(ram.path.join(relative)).unwrap();
    let writing = Instant::now();
    for chunk in expected.chunks(4096) {
        held.write_all(chunk).unwrap();
    }
    let elapsed = writing.elapsed();
    // No close, fsync, rename, or service request between acknowledgement and kill.
    owner.0.kill().unwrap();
    assert!(!owner.0.wait().unwrap().success());
    eprintln!(
        "crash write {relative}: {} bytes in {elapsed:?}, external fd still open",
        expected.len()
    );
    let started = Instant::now();
    loop {
        // SAFETY: the validated image descriptor is live; flock takes no pointers.
        if unsafe { libc::flock(image.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            break;
        }
        let error = std::io::Error::last_os_error();
        assert_eq!(error.raw_os_error(), Some(libc::EWOULDBLOCK));
        assert!(started.elapsed() < Duration::from_secs(10), "{error}");
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(image);
    drop(held);
    let service = fixture.start_configured(config, None, fixture.image_authority());
    let recovered = wait_ram(&service, &ram.id, |session| {
        // Linux can reuse both FUSE minor numbers and mount IDs immediately.
        // The explicit recovery result distinguishes this from the old ready record.
        session.browsable() && session.status.starts_with("RAM view recovered;")
    });
    assert_eq!(recovered.path, ram.path);
    let (_, image) = crate::ram::open(recovered.backing.as_ref().unwrap()).unwrap();
    let after = image.metadata().unwrap();
    assert_eq!((after.dev(), after.ino()), (identity.dev(), identity.ino()));
    drop(image);
    assert!(
        fs::read(ram.path.join(relative)).unwrap() == expected,
        "recovered contents differ for {relative}"
    );
    assert!(recovered.pending.as_ref().unwrap().full_check_required);
    finish(
        &service,
        DriveRequest::FlushRam {
            session: ram.id.clone(),
        },
    );
    assert!(
        fs::read(root.join("large").join(relative)).unwrap() == expected,
        "portable writeback differs for {relative}"
    );
    discard_ram(&service, &ram);
    service.shutdown().unwrap();
}
