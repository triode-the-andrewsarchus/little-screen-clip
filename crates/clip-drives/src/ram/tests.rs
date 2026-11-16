use super::*;
use std::io::Write;

#[test]
fn reservations_reject_aggregate_and_expanded_content_limits() {
    let mut config = DriveConfig::default();
    assert!(budget(&config, 2_000_000, 10, 1_000_000).unwrap().bytes >= 128 * MIB);
    assert!(budget(&config, 2_000_000, 10, u64::MAX).is_err());
    assert!(budget(&config, 2_000_000, u64::MAX, 1).is_err());
    let temp = tempfile::tempdir().unwrap();
    let mut stats = rustix::fs::fstatvfs(File::open(temp.path()).unwrap()).unwrap();
    stats.f_bavail = 1_000_000_000;
    stats.f_frsize = 4096;
    stats.f_favail = 100_000;
    let required = Budget {
        bytes: 128 * MIB,
        inodes: 4096,
    };
    assert!(admit(&config, &required, &[], &stats, u64::MAX).is_ok());
    config.ram_total_max_bytes = 127 * MIB;
    assert!(
        admit(&config, &required, &[], &stats, u64::MAX)
            .unwrap_err()
            .to_string()
            .contains("Aggregate")
    );
    config.ram_total_max_bytes = 16_000_000_000;
    config.ram_session_max_bytes = 127 * MIB;
    assert!(
        validate_quotas(
            &config,
            &[Budget {
                bytes: 128 * MIB,
                inodes: 4096
            }]
        )
        .is_err()
    );
    config.ram_session_max_bytes = 6_000_000_000;
    assert!(
        admit(
            &config,
            &required,
            &[],
            &stats,
            config.ram_min_available - 1
        )
        .is_err()
    );
}

#[test]
fn explicit_disk_backed_ram_directory_is_rejected() {
    let temp =
        tempfile::tempdir_in(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target")).unwrap();
    let pin = kernel_interface::pin_content_directory(temp.path()).unwrap();
    if rustix::fs::fstatfs(&pin).unwrap().f_type as u32 == 0x01021994 {
        return;
    }
    assert!(
        directory(&DriveConfig {
            ram_directory: temp.path().into(),
            ..Default::default()
        })
        .is_err()
    );
}

#[test]
#[ignore = "requires tmpfs, /dev/fuse and the packaged RAM filesystem helper"]
fn retiring_watch_pins_allows_a_real_normal_unmount() {
    let base = directory(&DriveConfig::default()).unwrap();
    let backing = prepare(
        &base,
        &Budget {
            bytes: 128 * MIB,
            inodes: 4096,
        },
    )
    .unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let mount = Mounted::start(&backing, &id, true, None).unwrap();
    let path = backing.directory.join("contents");
    let root = std::sync::Arc::new(Root::open(&path).unwrap());
    let mut watch = crate::watch::Watch::new().unwrap();
    watch.add(root.clone());
    drop(root);
    let unmount = || {
        CommandRequest::new("fusermount3")
            .args(["-u"])
            .arg(&path)
            .timeout(Duration::from_secs(5))
            .max_output_bytes(4096)
            .run()
            .unwrap()
    };
    assert!(
        !unmount().status.success(),
        "live directory pins should keep this owned fixture busy"
    );
    watch.remove_tree(&path);
    let result = unmount();
    assert!(result.status.success(), "{}", result.stderr_lossy());
    drop(mount);
    discard(&backing, &id).unwrap();
}

#[test]
#[ignore = "requires tmpfs, mke2fs/e2fsck, /dev/fuse and the packaged RAM filesystem helper"]
fn native_ram_quotas_atomic_install_and_open_file_restart_preserve_edits() {
    let config = DriveConfig::default();
    let base = directory(&config).unwrap();
    let mut backing = prepare(
        &base,
        &Budget {
            bytes: 128 * MIB,
            inodes: 4096,
        },
    )
    .unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let mount = Mounted::start(&backing, &id, true, None).unwrap();
    backing.initialized = true;
    let view = backing.directory.join("contents");
    assert_eq!(
        fs::metadata(&backing.directory).unwrap().mode() & 0o777,
        0o700
    );
    assert!(
        Mounted::start(&backing, &id, false, None)
            .err()
            .unwrap()
            .to_string()
            .contains("owned")
    );
    assert!(!view.join("lost+found").exists());
    let session = RamSession {
        identity: kernel_interface::MountIdentity::directory(&view).unwrap(),
        id: id.clone(),
        drive: "owned-test".into(),
        path: view.clone(),
        ready: true,
        connected: false,
        status: String::new(),
        backing: Some(backing.clone()),
    };
    let stats = rustix::fs::fstatvfs(base.directory(Path::new(""), false).unwrap()).unwrap();
    let mut limited = config.clone();
    limited.ram_total_max_bytes = 256 * MIB - 1;
    let another = Budget {
        bytes: 128 * MIB,
        inodes: 4096,
    };
    assert!(
        admit(
            &limited,
            &another,
            std::slice::from_ref(&session),
            &stats,
            u64::MAX
        )
        .unwrap_err()
        .to_string()
        .contains("Aggregate")
    );
    limited.ram_total_max_bytes = config.ram_total_max_bytes;
    limited.ram_total_max_entries = 8191;
    assert!(
        admit(
            &limited,
            &another,
            std::slice::from_ref(&session),
            &stats,
            u64::MAX
        )
        .unwrap_err()
        .to_string()
        .contains("Aggregate")
    );

    fs::write(view.join("old"), b"old destination").unwrap();
    fs::write(backing.directory.join("unexpected"), b"retain this").unwrap();
    assert!(check_discard(&backing).is_err());
    assert_eq!(fs::read(view.join("old")).unwrap(), b"old destination");
    fs::remove_file(backing.directory.join("unexpected")).unwrap();
    fs::write(view.join("staged"), b"new bytes").unwrap();
    assert!(
        little_clip_core::file_source::rename_noreplace(&view.join("staged"), &view.join("old"))
            .is_err()
    );
    assert_eq!(fs::read(view.join("old")).unwrap(), b"old destination");
    little_clip_core::file_source::rename_noreplace(&view.join("staged"), &view.join("new"))
        .unwrap();
    let mut held = File::create(view.join("held-open")).unwrap();
    held.write_all(b"acknowledged before daemon restart")
        .unwrap();
    // Do not close/fsync held: normal server retirement must drain the native write cache.
    drop(mount);
    let mount = Mounted::start(&backing, &id, false, None).unwrap();
    assert_eq!(
        fs::read(view.join("held-open")).unwrap(),
        b"acknowledged before daemon restart"
    );
    assert_eq!(fs::read(view.join("new")).unwrap(), b"new bytes");
    drop(held);
    // A parent-directory fsync must durably publish directory creation even if
    // the server is killed before op_destroy. Rename already flushes upstream.
    fs::rename(view.join("new"), view.join("renamed")).unwrap();
    fs::create_dir(view.join("fsync-created-directory")).unwrap();
    File::open(&view).unwrap().sync_all().unwrap();
    let mut mount = mount;
    mount.child.kill().unwrap();
    mount.child.wait().unwrap();
    drop(mount);
    let mount = Mounted::start(&backing, &id, false, None).unwrap();
    assert!(
        !view.join("new").exists(),
        "Directory fsync failed to publish rename in {}",
        backing.directory.display()
    );
    assert_eq!(fs::read(view.join("renamed")).unwrap(), b"new bytes");
    assert!(
        view.join("fsync-created-directory").is_dir(),
        "Directory fsync failed to publish mkdir in {}",
        backing.directory.display()
    );
    let mut filled = File::create(view.join("fill")).unwrap();
    let chunk = vec![0xa5; MIB as usize];
    let mut written = 0;
    loop {
        match filled.write(&chunk) {
            Ok(0) => panic!("unexpected zero write"),
            Ok(n) => written += n as u64,
            Err(e) => {
                assert_eq!(e.raw_os_error(), Some(libc::ENOSPC));
                break;
            }
        }
        assert!(
            written <= backing.capacity,
            "RAM filesystem grew beyond reservation"
        );
    }
    drop(filled);
    fs::remove_file(view.join("fill")).unwrap();
    let stats = rustix::fs::fstatvfs(File::open(&view).unwrap()).unwrap();
    assert!(stats.f_bavail * stats.f_frsize > 64 * MIB);
    let mut created = 0;
    loop {
        match File::create(view.join(format!("inode-{created}"))) {
            Ok(_) => created += 1,
            Err(e) => {
                assert_eq!(e.raw_os_error(), Some(libc::ENOSPC));
                break;
            }
        }
        assert!(created <= backing.inodes);
    }
    assert!(created > 1000);
    assert!(
        rustix::fs::fstatvfs(File::open(&view).unwrap())
            .unwrap()
            .f_bavail
            * stats.f_frsize
            > 64 * MIB
    );
    assert_eq!(
        fs::metadata(backing.directory.join("volume.ext4"))
            .unwrap()
            .len(),
        backing.capacity
    );
    assert!(File::create(view.join("one-too-many")).is_err());
    drop(mount);
    discard(&backing, &id).unwrap();
    assert!(!backing.directory.exists());
}
