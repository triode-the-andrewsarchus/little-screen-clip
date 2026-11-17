use super::*;
use std::{fs, fs::FileTimes, os::unix::fs::MetadataExt};

#[test]
fn verified_current_backup_hashes_are_reused_only_for_the_same_captured_revision() {
    let fixture = reliable_fixture();
    let version = fixture.backup().unwrap();
    let original = fs::metadata(fixture.source.path.join("file.txt")).unwrap();
    let catalog = fixture.capture(&DriveConfig::default()).unwrap();
    let view = filesystem(&fixture.state.path, catalog);
    let (_, entry) = view.lookup_path(Path::new("file.txt")).unwrap().unwrap();
    let manifest = Store::existing(version.manifest.as_ref().unwrap(), true).unwrap();
    let saved = scan::entry(
        &manifest,
        &format!("backup:{}", version.id),
        Path::new("file.txt"),
    )
    .unwrap()
    .unwrap();
    if !fixture.source.queried_change_tracking().unwrap() {
        assert!(entry.hash.is_none());
        return;
    }
    assert!(entry.hash.is_some());
    assert_eq!(entry.hash, saved.hash);
    assert_eq!(entry.mode, original.mode());
    assert_eq!(
        (entry.modified, entry.nanos),
        (original.mtime(), original.mtime_nsec())
    );
    // The imported digest must actually guard reads, even when a modified
    // backup file preserves its original size and modification time.
    let copied = version.path.join("file.txt");
    fs::write(&copied, b"changed payload").unwrap();
    File::open(&copied)
        .unwrap()
        .set_times(FileTimes::new().set_modified(original.modified().unwrap()))
        .unwrap();
    assert!(
        view.resolve(&entry)
            .err()
            .unwrap()
            .to_string()
            .contains("stale")
    );
    drop(view);
    fs::write(fixture.source.path.join("file.txt"), b"changed payload").unwrap();
    File::open(fixture.source.path.join("file.txt"))
        .unwrap()
        .set_times(FileTimes::new().set_modified(original.modified().unwrap()))
        .unwrap();
    let catalog = fixture.capture(&DriveConfig::default()).unwrap();
    let view = filesystem(&fixture.state.path, catalog);
    assert!(
        view.lookup_path(Path::new("file.txt"))
            .unwrap()
            .unwrap()
            .1
            .hash
            .is_none()
    );
}

#[test]
fn optional_hash_reuse_rejects_corrupt_wrong_root_and_oversized_manifests() {
    for case in [
        "checksum",
        "source-root",
        "evidence",
        "size",
        "legacy-tracking",
    ] {
        let fixture = reliable_fixture();
        let mut version = fixture.backup().unwrap();
        match case {
            "checksum" => {
                Store::existing(version.manifest.as_ref().unwrap(), false)
                    .unwrap()
                    .put("unverified", "change", &true)
                    .unwrap();
            }
            "source-root" => fixture.rewrite_manifest(&mut version, |manifest| {
                let mut root: Entry = manifest.get("source", "root").unwrap().unwrap();
                root.inode += 1;
                manifest.put("source", "root", &root).unwrap();
            }),
            "evidence" => {
                version.evidence = "different-media".into();
                fixture.db.put("backup", &version.id, &version).unwrap();
            }
            "size" => fixture.rewrite_manifest(&mut version, |manifest| {
                manifest
                    .put("unused", "large", &"x".repeat(512 * 1024))
                    .unwrap();
            }),
            "legacy-tracking" => fixture.rewrite_manifest(&mut version, |manifest| {
                manifest
                    .remove("collection", "source_queried_change_tracking")
                    .unwrap();
            }),
            _ => unreachable!(),
        }
        let config = DriveConfig {
            catalog_max_bytes: 256 * 1024,
            ..Default::default()
        };
        let catalog = fixture.capture(&config).unwrap();
        assert!(catalog.complete, "{case}: {}", catalog.coverage);
        let view = filesystem(&fixture.state.path, catalog);
        assert!(
            view.lookup_path(Path::new("file.txt"))
                .unwrap()
                .unwrap()
                .1
                .hash
                .is_none(),
            "{case}"
        );
    }
}

#[test]
fn a_late_invalid_manifest_row_rolls_back_previously_imported_hashes() {
    let fixture = reliable_fixture();
    fs::write(
        fixture.source.path.join("z-broken"),
        b"valid source payload",
    )
    .unwrap();
    let mut version = fixture.backup().unwrap();
    fixture.rewrite_manifest(&mut version, |manifest| {
        manifest
            .0
            .execute(
                "UPDATE entries SET json='invalid json' WHERE path=?1",
                [b"z-broken".as_slice()],
            )
            .unwrap();
    });
    fixture.assert_metadata_only(&["file.txt", "z-broken"]);
}

#[test]
fn verified_manifest_reads_keep_the_descriptor_and_refuse_name_replacement() {
    let fixture = reliable_fixture();
    let version = fixture.backup().unwrap();
    let path = version.manifest.as_ref().unwrap();
    let mut bytes = u64::MAX;
    let result = backup::read_manifest(
        &version,
        path,
        &AtomicBool::new(false),
        &mut bytes,
        Duration::MAX,
        |manifest| {
            fs::rename(path, path.with_extension("retained")).unwrap();
            Store::open(path)
                .unwrap()
                .put("source", "root", &"replacement")
                .unwrap();
            let original: Entry = manifest.get("source", "root")?.unwrap();
            assert_eq!(original.kind, "directory");
            Ok(())
        },
    );
    assert!(result.is_err());
}

#[test]
fn manifest_reuse_observes_cancellation_and_time_admission_before_querying_rows() {
    let fixture = reliable_fixture();
    let version = fixture.backup().unwrap();
    for (cancelled, timeout) in [(true, Duration::MAX), (false, Duration::ZERO)] {
        let mut bytes = u64::MAX;
        let result: Result<()> = backup::read_manifest(
            &version,
            version.manifest.as_ref().unwrap(),
            &AtomicBool::new(cancelled),
            &mut bytes,
            timeout,
            |_| panic!("Unadmitted manifest read"),
        );
        assert!(result.is_err());
        assert_eq!(
            bytes,
            u64::MAX,
            "unadmitted checks consume no verification allowance"
        );
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Probe {
    #[serde(with = "little_clip_core::path_serde")]
    state: PathBuf,
    #[serde(with = "little_clip_core::path_serde")]
    source: PathBuf,
    drive: crate::Drive,
    read_payload: bool,
}

#[test]
#[ignore = "child probe requires an explicit owned LITTLE_CLIP_HASH_CAPTURE_PROBE"]
fn hash_capture_probe() {
    let path = std::env::var_os("LITTLE_CLIP_HASH_CAPTURE_PROBE")
        .expect("Select an owned capture fixture");
    let probe: Probe = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    let state = Root::open(&probe.state).unwrap();
    let source = Root::open(&probe.source).unwrap();
    let db = Store::existing(&probe.state.join("drives.sqlite"), false).unwrap();
    if probe.read_payload {
        source
            .input(Path::new("file.txt"))
            .unwrap()
            .read_exact_at(&mut [0; 15], 0)
            .unwrap();
    }
    let catalog = capture(
        &db,
        &state,
        &probe.drive,
        &source,
        &DriveConfig::default(),
        &AtomicBool::new(false),
        |_, _| {},
    )
    .unwrap();
    let view = filesystem(&probe.state, catalog);
    assert!(
        view.lookup_path(Path::new("file.txt"))
            .unwrap()
            .unwrap()
            .1
            .hash
            .is_some()
    );
}

#[test]
#[ignore = "requires Linux 7+, strace and writable /dev/shm; traces only an owned child"]
fn real_capture_reuses_hashes_without_source_payload_reads_and_the_trace_detects_a_control_read() {
    use little_clip_core::process::CommandRequest;
    for source in ["backup", "sync", "history"] {
        let fixture = reliable_fixture();
        let mut unused_history = None;
        match source {
            "backup" => {
                unused_history = fixture.backup().unwrap().manifest;
                fixture.backup().unwrap();
            }
            "sync" => {
                fixture.pair(Path::new(""));
            }
            "history" => {
                fixture.backup().unwrap();
                let current = fixture.backup().unwrap();
                Store::existing(current.manifest.as_ref().unwrap(), false)
                    .unwrap()
                    .put("unverified", "trace-control", &true)
                    .unwrap();
            }
            _ => unreachable!(),
        }
        for read_payload in [false, true] {
            let input = fixture
                .temp
                .path()
                .join(format!("probe-{read_payload}.json"));
            let trace = fixture
                .temp
                .path()
                .join(format!("trace-{read_payload}.txt"));
            fs::write(
                &input,
                serde_json::to_vec(&Probe {
                    state: fixture.state.path.clone(),
                    source: fixture.source.path.clone(),
                    drive: fixture.drive.clone(),
                    read_payload,
                })
                .unwrap(),
            )
            .unwrap();
            let mut environment = OsString::from("LITTLE_CLIP_HASH_CAPTURE_PROBE=");
            environment.push(&input);
            let output = CommandRequest::new("strace")
                .args([
                    "-f",
                    "-yy",
                    "-e",
                    "trace=read,pread64,readv,preadv,preadv2",
                    "-o",
                ])
                .arg(&trace)
                .arg("/usr/bin/env")
                .arg(environment)
                .arg(std::env::current_exe().unwrap())
                .args([
                    "catalog::tests::hash_tests::hash_capture_probe",
                    "--ignored",
                    "--exact",
                    "--nocapture",
                ])
                .timeout(Duration::from_secs(20))
                .own_process_group()
                .die_with_parent()
                .run()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let text = fs::read_to_string(&trace).unwrap();
            let prefix = format!("{}/", fixture.source.path.display());
            let reads: Vec<_> = text
                .lines()
                .filter(|line| {
                    // Look only at the syscall's FD label, not strings inside SQLite or
                    // the probe JSON read buffers, which also contain source paths.
                    let Some((_, argument)) = line.split_once('(') else {
                        return false;
                    };
                    let Some((_, label)) = argument.split_once('<') else {
                        return false;
                    };
                    label
                        .split('>')
                        .next()
                        .is_some_and(|label| label.starts_with(&prefix))
                })
                .collect();
            assert_eq!(
                !reads.is_empty(),
                read_payload,
                "source payload reads: {reads:?}"
            );
            if let Some(path) = &unused_history {
                let label = format!("<{}>", path.display());
                assert!(
                    !text
                        .lines()
                        .any(|line| line.split_once('(').is_some_and(|(_, args)| args
                            .split_once(',')
                            .map_or(args, |(fd, _)| fd)
                            .contains(&label))),
                    "already-covered history must not be read"
                );
            }

            fs::copy(
                &trace,
                Path::new("/tmp").join(format!(
                    "portable-drive-catalog-hash-source-trace-{source}-{read_payload}.log"
                )),
            )
            .unwrap();
        }
    }
}
