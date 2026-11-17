//! Real mount selection runs only in disposable guests, with child-local environments.
use super::*;
use crate::sync_edge_tests::native_fs::GuestMount;
use std::ffi::OsString;
const PROBE_COMPLETED: &str = "RAM_DIRECTORY_PROBE_COMPLETED";

#[test]
#[ignore = "child probe requires an explicit owned RAM-directory expectation in a disposable guest"]
fn directory_probe() {
    crate::require_disposable_guest();
    let expected = PathBuf::from(std::env::var_os("LITTLE_CLIP_RAM_DIRECTORY_EXPECT").unwrap());
    let config = DriveConfig {
        ram_directory: PathBuf::from(
            std::env::var_os("LITTLE_CLIP_RAM_DIRECTORY_OVERRIDE").unwrap(),
        ),
        ..Default::default()
    };
    let selected = directory(&config);
    if expected == Path::new("error") {
        assert!(
            selected.is_err(),
            "An unusable explicit override must not silently fall back"
        );
        println!("\n{PROBE_COMPLETED}");
        return;
    }
    let base = selected.unwrap();
    assert_eq!(base.path, expected);
    assert_eq!(
        rustix::fs::fstatfs(base.directory(Path::new(""), false).unwrap())
            .unwrap()
            .f_type as u32,
        0x01021994
    );
    let backing = prepare(
        &base,
        &Budget {
            bytes: 128 * MIB,
            inodes: 4096,
        },
        |_| Ok(()),
    )
    .unwrap();
    assert_eq!(backing.directory.parent(), Some(base.path.as_path()));
    assert_eq!(
        fs::metadata(&backing.directory).unwrap().mode() & 0o777,
        0o700
    );
    {
        let (_, image) = open(&backing).unwrap();
        assert_eq!(
            rustix::fs::fstatfs(&image).unwrap().f_type as u32,
            0x01021994
        );
        assert_eq!(image.metadata().unwrap().mode() & 0o777, 0o600);
    }
    let id = uuid::Uuid::new_v4().to_string();
    let mounted = Mounted::start(&backing, &id, true, None).unwrap();
    let payload = backing.directory.join("contents/ram-write");
    fs::write(&payload, b"verified RAM backing").unwrap();
    File::open(&payload).unwrap().sync_all().unwrap();
    assert_eq!(fs::read(&payload).unwrap(), b"verified RAM backing");
    drop(mounted);
    discard(&backing, &id).unwrap();
    assert!(!backing.directory.exists());
    println!("\n{PROBE_COMPLETED}");
}

#[test]
#[ignore = "requires the removable-filesystem guest, bounded tmpfs, and packaged RAM helper"]
fn guest_default_ram_location_falls_back_from_non_tmpfs_and_read_only_mounts() {
    crate::require_disposable_guest();
    let media = Path::new("/work/media");
    assert_ne!(
        rustix::fs::fstatfs(File::open(media).unwrap())
            .unwrap()
            .f_type as u32,
        0x01021994
    );
    let work = tempfile::tempdir_in("/work").unwrap();
    let runtime = work.path().join("runtime");
    let read_only = work.path().join("read-only-runtime");
    let missing = work.path().join("missing-runtime");
    let alias = work.path().join("runtime-alias");
    for path in [&runtime, &read_only] {
        fs::create_dir(path).unwrap();
    }
    std::os::unix::fs::symlink(&runtime, &alias).unwrap();
    fs::create_dir_all("/dev/shm").unwrap();
    let shm = GuestMount::start(
        CommandRequest::new("mount").args([
            "-t",
            "tmpfs",
            "-o",
            "size=256m,nr_inodes=8192,mode=700",
            "owned-ram-fallback",
        ]),
        Path::new("/dev/shm"),
    );
    let disk_tmp = GuestMount::attach(media, Path::new("/tmp"));
    let run =
        |label: &str, xdg: Option<&Path>, configured: Option<&Path>, expected: Option<&Path>| {
            let mut request = CommandRequest::new("/usr/bin/env").args(["-u", "XDG_RUNTIME_DIR"]);
            let settings = [
                (
                    "LITTLE_CLIP_RAM_DIRECTORY_EXPECT",
                    expected.unwrap_or(Path::new("error")),
                ),
                (
                    "LITTLE_CLIP_RAM_DIRECTORY_OVERRIDE",
                    configured.unwrap_or(Path::new("/tmp")),
                ),
            ];
            for (key, value) in settings
                .into_iter()
                .chain(xdg.map(|path| ("XDG_RUNTIME_DIR", path)))
            {
                let mut entry = OsString::from(key);
                entry.push("=");
                entry.push(value);
                request = request.arg(entry);
            }
            let output = request
                .arg(std::env::current_exe().unwrap())
                .args([
                    "ram::tests::directory_tests::directory_probe",
                    "--exact",
                    "--ignored",
                    "--nocapture",
                ])
                .timeout(Duration::from_secs(45))
                .own_process_group()
                .die_with_parent()
                .run()
                .unwrap();
            assert!(
                output.status.success(),
                "{label}: {}{}",
                output.stdout_lossy(),
                output.stderr_lossy()
            );
            assert_eq!(
                output
                    .stdout_lossy()
                    .lines()
                    .filter(|line| *line == PROBE_COMPLETED)
                    .count(),
                1,
                "{label}: the selected child probe must actually execute: {}",
                output.stdout_lossy()
            );
            eprintln!("RAM directory case passed: {label}");
        };
    let shared = Some(Path::new("/dev/shm"));
    run("verified XDG", Some(&runtime), None, Some(&runtime));
    run("unset XDG", None, None, shared);
    run("disk-backed XDG", Some(media), None, shared);
    run("missing XDG", Some(&missing), None, shared);
    run(
        "symlink to verified tmpfs",
        Some(&alias),
        None,
        Some(&runtime),
    );
    run(
        "explicit disk-backed override",
        Some(&runtime),
        Some(media),
        None,
    );
    run(
        "explicit tmpfs override",
        shared,
        Some(&runtime),
        Some(&runtime),
    );
    let read_only_runtime = GuestMount::attach(&runtime, &read_only);
    read_only_runtime.remount("remount,bind,ro");
    run("read-only XDG", Some(&read_only), None, shared);
    run(
        "explicit read-only override",
        shared,
        Some(&read_only),
        None,
    );
    drop(read_only_runtime);
    {
        let unavailable_shm = GuestMount::attach(media, Path::new("/dev/shm"));
        run("no verified tmpfs candidate", Some(media), None, None);
        drop(unavailable_shm);
    }
    {
        let ram_tmp = GuestMount::start(
            CommandRequest::new("mount").args([
                "-t",
                "tmpfs",
                "-o",
                "size=256m,nr_inodes=8192,mode=700",
                "owned-tmp-preference",
            ]),
            Path::new("/tmp"),
        );
        run(
            "prefer writable tmpfs tmp",
            Some(&runtime),
            None,
            Some(Path::new("/tmp")),
        );
        ram_tmp.remount("remount,ro");
        run("read-only tmpfs tmp", Some(&runtime), None, Some(&runtime));
        drop(ram_tmp);
    }
    drop(disk_tmp);
    drop(shm);
}
