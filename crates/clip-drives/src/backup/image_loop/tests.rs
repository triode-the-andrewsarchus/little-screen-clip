//! Guest authority probes for descriptor-bound loop cleanup.
use super::super::image_control::setup_loop;
use super::*;

#[test]
fn loop_uapi_layout_and_ordinary_descriptor_refusal_are_explicit() {
    assert_eq!(std::mem::size_of::<Info>(), 232);
    assert_eq!(std::mem::offset_of!(Info, flags), 52);
    let file = tempfile::tempfile().unwrap();
    assert_eq!(
        Info::read(&file).err().unwrap().raw_os_error(),
        Some(libc::ENOTTY)
    );
    assert!(crate::device::disk_sequence(&file).is_err());
}

#[test]
fn attachment_status_rejects_other_generations_and_backing_layouts() {
    let temp = tempfile::tempdir().unwrap();
    let image = tempfile::tempfile().unwrap();
    let metadata = image.metadata().unwrap();
    let attachment = Attachment {
        host: None,
        backup: "owned".into(),
        image: temp.path().join("owned.img"),
        identity: StorageIdentity::pinned(&image).unwrap(),
        object: "/owned".into(),
        device: "/dev/owned-fixture".into(),
        // Only the pure status comparison runs; this fixture never admits a device.
        node: Lifetime::capture(&kernel_interface::pin(temp.path(), true).unwrap()).unwrap(),
        rdev: libc::makedev(7, 1),
        sequence: 17,
        backing_device: metadata.dev(),
        backing_inode: metadata.ino(),
    };
    // SAFETY: the UAPI structure contains only integer and byte-array fields.
    let mut valid: Info = unsafe { std::mem::zeroed() };
    valid.device = metadata.dev();
    valid.inode = metadata.ino();
    valid.flags = 1;
    attachment.check_status(17, &valid).unwrap();
    for field in [
        "sequence",
        "device",
        "inode",
        "rdevice",
        "offset",
        "size",
        "read-only",
    ] {
        let mut changed = valid;
        let sequence = match field {
            "sequence" => 18,
            "device" => {
                changed.device += 1;
                17
            }
            "inode" => {
                changed.inode += 1;
                17
            }
            "rdevice" => {
                changed.rdevice = 1;
                17
            }
            "offset" => {
                changed.offset = 512;
                17
            }
            "size" => {
                changed.size_limit = 4096;
                17
            }
            "read-only" => {
                changed.flags = 0;
                17
            }
            _ => unreachable!(),
        };
        assert!(
            attachment.check_status(sequence, &changed).is_err(),
            "{field}"
        );
    }
}
#[test]
#[ignore = "requires an inherited authorized descriptor in the isolated authority guest"]
fn unprivileged_descriptor_retirement_probe() {
    crate::require_disposable_guest();
    assert_eq!(rustix::process::geteuid().as_raw(), 65534);
    let number: i32 = std::env::var("LITTLE_CLIP_OWNED_LOOP_FD")
        .unwrap()
        .parse()
        .unwrap();
    let file = little_clip_core::process::duplicate_inherited_handle(number).unwrap();
    assert_eq!(
        Info::read(&file).unwrap().flags & 1,
        1,
        "owned image is read-only"
    );
    // SAFETY: LOOP_CLR_FD acts on the explicitly inherited authorized loop only.
    let result = unsafe { libc::ioctl(file.as_raw_fd(), 0x4c01 as libc::c_ulong) };
    assert_eq!(result, 0, "{}", std::io::Error::last_os_error());
    println!("OWNED_LOOP_DESCRIPTOR_CLEARED");
}

#[test]
#[ignore = "requires the isolated authority guest; creates only private test loops"]
fn native_authorized_descriptor_retirement_distinguishes_deferred_cleanup() {
    crate::require_disposable_guest();
    assert_eq!(
        std::env::var("LITTLE_CLIP_DRIVE_TEST_AUTHORITY").unwrap(),
        "1"
    );
    for deferred in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("owned.img");
        let image = File::create(&path).unwrap();
        image.set_len(1024 * 1024).unwrap();
        drop(image);
        let image = File::open(&path).unwrap();
        let connection = system_connection().unwrap();
        let object = setup_loop(&connection, &image, &mut |_| Ok(())).unwrap();
        let file = open(&connection, object.as_str()).unwrap().0;
        let info = Info::read(&file).unwrap();
        assert_eq!(
            (info.device, info.inode),
            (
                image.metadata().unwrap().dev(),
                image.metadata().unwrap().ino()
            )
        );
        assert_eq!((info.offset, info.size_limit, info.flags & 1), (0, 0, 1));
        let reader = deferred.then(|| open(&connection, object.as_str()).unwrap().0);
        let (child, fd) = CommandRequest::new("env").inherit_handle(&file).unwrap();
        let child = child
            .arg(format!("LITTLE_CLIP_OWNED_LOOP_FD={fd}"))
            .arg("setpriv")
            .args(["--reuid=65534", "--regid=65534", "--clear-groups"])
            .arg(std::env::current_exe().unwrap())
            .args([
                "backup::image_loop::tests::unprivileged_descriptor_retirement_probe",
                "--exact",
                "--ignored",
                "--nocapture",
            ]);
        let result = child
            .timeout(Duration::from_secs(10))
            .own_process_group()
            .run()
            .unwrap();
        assert!(
            result.status.success(),
            "{}{}",
            result.stdout_lossy(),
            result.stderr_lossy()
        );
        assert!(
            result
                .stdout_lossy()
                .contains("OWNED_LOOP_DESCRIPTOR_CLEARED")
        );
        if deferred {
            assert_ne!(
                Info::read(&file).unwrap().flags & 4,
                0,
                "other readers defer kernel retirement"
            );
            drop(file);
            assert_ne!(Info::read(reader.as_ref().unwrap()).unwrap().flags & 4, 0);
        } else {
            assert_eq!(
                Info::read(&file).err().unwrap().raw_os_error(),
                Some(libc::ENXIO)
            );
            drop(file);
        }
        drop(reader);
        let loops = CommandRequest::new("losetup")
            .arg("-j")
            .arg(&path)
            .run()
            .unwrap();
        assert!(loops.status.success());
        assert!(loops.stdout.is_empty(), "{}", loops.stdout_lossy());
        let detached = open(&connection, object.as_str()).unwrap().0;
        assert_eq!(
            Info::read(&detached).err().unwrap().raw_os_error(),
            Some(libc::ENXIO)
        );
        eprintln!("Authorized read-only descriptor retirement: deferred={deferred}");
    }
}
