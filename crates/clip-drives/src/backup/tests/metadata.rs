use super::*;
use std::os::unix::{ffi::OsStrExt, fs::MetadataExt};

fn facts(version: &Backup, path: &Path) -> serde_json::Value {
    let manifest = Store::existing(version.manifest.as_ref().unwrap(), true).unwrap();
    let json: String = manifest
        .0
        .query_row(
            "SELECT json FROM backup_metadata WHERE path=?1",
            [path.as_os_str().as_bytes()],
            |row| row.get(0),
        )
        .unwrap();
    serde_json::from_str(&json).unwrap()
}
fn attribute(path: &Path, name: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0; 65536];
    let count =
        rustix::fs::getxattr(path, std::ffi::OsStr::from_bytes(name), &mut bytes[..]).unwrap();
    bytes.truncate(count);
    bytes
}

#[test]
fn file_metadata_and_raw_attributes_are_verified_and_recorded_without_omissions() {
    let fixture = FileFixture::new(b"payload");
    let source = fixture.source.path.join("file");
    let name = b"user.raw_\xff";
    rustix::fs::setxattr(
        &source,
        std::ffi::OsStr::from_bytes(name),
        b"\0binary\xff",
        rustix::fs::XattrFlags::empty(),
    )
    .unwrap();
    fs::set_permissions(&source, fs::Permissions::from_mode(0o640)).unwrap();
    let accessed = time(1_500_000_000, 321_000_000).unwrap();
    let modified = time(1_600_000_000, 123_456_789).unwrap();
    File::open(&source)
        .unwrap()
        .set_times(
            fs::FileTimes::new()
                .set_accessed(accessed)
                .set_modified(modified),
        )
        .unwrap();
    let version = fixture.backup(|_, _| {}).unwrap();
    let target = version.path.join("file");
    let observed = fs::metadata(&target).unwrap();
    assert_eq!(observed.mode() & 0o7777, 0o640);
    assert_eq!(observed.accessed().unwrap(), accessed);
    assert_eq!(observed.modified().unwrap(), modified);
    assert_eq!(
        (observed.uid(), observed.gid()),
        (
            fs::metadata(&source).unwrap().uid(),
            fs::metadata(&source).unwrap().gid()
        )
    );
    assert_eq!(attribute(&target, name), b"\0binary\xff");
    assert_eq!(fs::read(&target).unwrap(), b"payload");
    let captured = facts(&version, Path::new("file"));
    assert_eq!(
        captured["accessed"],
        serde_json::json!([1_500_000_000_i64, 321_000_000])
    );
    assert_eq!(
        captured["attributes"][0]["name"],
        serde_json::json!(name.as_slice())
    );
    assert_eq!(
        captured["attributes"][0]["value"],
        serde_json::json!(b"\0binary\xff".as_slice())
    );
    assert_eq!(captured["omissions"], serde_json::json!([]));
    let summary = version.fidelity.as_ref().unwrap();
    assert_eq!(
        (
            summary.entries,
            summary.omitted_fields,
            summary.source_attributes_unavailable
        ),
        (2, 0, 0)
    );
    publication::check_manifest(&version).unwrap();
}

#[test]
fn access_and_default_acls_survive_without_accidental_inheritance() {
    let fixture = FileFixture::new(b"ordinary");
    let folder = fixture.source.path.join("folder");
    fs::create_dir(&folder).unwrap();
    let mut acl = 2_u32.to_le_bytes().to_vec();
    // Linux POSIX ACL xattr: owner, named user, owning group, mask, other.
    for (tag, permissions, id) in [
        (1_u16, 7_u16, u32::MAX),
        (2, 5, 65534),
        (4, 0, u32::MAX),
        (16, 5, u32::MAX),
        (32, 0, u32::MAX),
    ] {
        acl.extend(tag.to_le_bytes());
        acl.extend(permissions.to_le_bytes());
        acl.extend(id.to_le_bytes());
    }
    rustix::fs::setxattr(
        &folder,
        "system.posix_acl_default",
        &acl,
        rustix::fs::XattrFlags::empty(),
    )
    .unwrap();
    fs::write(folder.join("child"), b"inherited ACL").unwrap();
    let access = attribute(&folder.join("child"), b"system.posix_acl_access");
    let accessed = time(1_400_000_000, 123_000_000).unwrap();
    for directory in [&folder, &fixture.source.path] {
        File::open(directory)
            .unwrap()
            .set_times(fs::FileTimes::new().set_accessed(accessed))
            .unwrap();
    }
    let version = fixture.backup(|_, _| {}).unwrap();
    assert_eq!(
        fs::metadata(&version.path).unwrap().accessed().unwrap(),
        accessed
    );
    assert_eq!(
        fs::metadata(version.path.join("folder"))
            .unwrap()
            .accessed()
            .unwrap(),
        accessed
    );
    assert_eq!(
        attribute(&version.path.join("folder"), b"system.posix_acl_default"),
        acl
    );
    assert_eq!(
        attribute(
            &version.path.join("folder/child"),
            b"system.posix_acl_access"
        ),
        access
    );
    assert_eq!(
        fs::metadata(version.path.join("folder/child"))
            .unwrap()
            .mode()
            & 0o7777,
        fs::metadata(folder.join("child")).unwrap().mode() & 0o7777
    );
    assert_eq!(version.fidelity.as_ref().unwrap().omitted_fields, 0);
    assert_eq!(
        facts(&version, Path::new("folder"))["omissions"],
        serde_json::json!([])
    );
}

#[test]
fn symlink_metadata_never_changes_or_reads_its_outside_referent() {
    use little_clip_core::kernel_interface;
    let fixture = FileFixture::new(b"payload");
    let foreign = fixture.temp.path().join("outside");
    fs::write(&foreign, b"outside payload").unwrap();
    let before = fs::metadata(&foreign).unwrap();
    let link = fixture.source.path.join("reference");
    std::os::unix::fs::symlink(&foreign, &link).unwrap();
    std::os::unix::fs::symlink("missing-target", fixture.source.path.join("dangling")).unwrap();
    let pin = kernel_interface::pin(&link, false).unwrap();
    rustix::fs::utimensat(
        rustix::fs::CWD,
        anchor(&pin),
        &rustix::fs::Timestamps {
            last_access: rustix::fs::Timespec {
                tv_sec: 1_500_000_000,
                tv_nsec: 0,
            },
            last_modification: rustix::fs::Timespec {
                tv_sec: 1_600_000_000,
                tv_nsec: 123_456_789,
            },
        },
        rustix::fs::AtFlags::empty(),
    )
    .unwrap();
    let version = fixture.backup(|_, _| {}).unwrap();
    let original_link_access = fs::symlink_metadata(version.path.join("reference")).unwrap();
    assert_eq!(
        (
            original_link_access.atime(),
            original_link_access.atime_nsec()
        ),
        (1_500_000_000, 0)
    );
    assert_eq!(
        fs::read_link(version.path.join("reference")).unwrap(),
        foreign
    );
    assert_eq!(
        fs::read_link(version.path.join("dangling")).unwrap(),
        Path::new("missing-target")
    );
    let link_metadata = fs::symlink_metadata(version.path.join("reference")).unwrap();
    assert_eq!(
        (link_metadata.mtime(), link_metadata.mtime_nsec()),
        (1_600_000_000, 123_456_789)
    );
    let after = fs::metadata(&foreign).unwrap();
    assert_eq!(
        (
            after.atime(),
            after.atime_nsec(),
            after.mtime(),
            after.mtime_nsec(),
            after.mode()
        ),
        (
            before.atime(),
            before.atime_nsec(),
            before.mtime(),
            before.mtime_nsec(),
            before.mode()
        )
    );
    assert_eq!(
        facts(&version, Path::new("reference"))["omissions"],
        serde_json::json!([])
    );
}

#[test]
fn hard_links_share_a_backup_inode_and_retain_original_relationship_facts() {
    let fixture = FileFixture::new(b"linked bytes");
    fs::create_dir(fixture.source.path.join("nested")).unwrap();
    fs::hard_link(
        fixture.source.path.join("file"),
        fixture.source.path.join("nested/alias"),
    )
    .unwrap();
    let version = fixture.backup(|_, _| {}).unwrap();
    assert_eq!(
        fs::metadata(version.path.join("file")).unwrap().ino(),
        fs::metadata(version.path.join("nested/alias"))
            .unwrap()
            .ino()
    );
    for relative in [Path::new("file"), Path::new("nested/alias")] {
        assert_eq!(
            fs::read(version.path.join(relative)).unwrap(),
            b"linked bytes"
        );
        let captured = facts(&version, relative);
        assert_eq!(captured["links"], 2);
        assert_eq!(captured["omissions"], serde_json::json!([]));
    }
    assert_eq!(fs::metadata(version.path.join("file")).unwrap().nlink(), 2);
    assert_eq!(version.fidelity.as_ref().unwrap().entries_with_omissions, 0);
    let file = facts(&version, Path::new("file"));
    let alias = facts(&version, Path::new("nested/alias"));
    assert!(file["hard_link_to"] == "nested/alias" || alias["hard_link_to"] == "file");
    fs::write(fixture.source.path.join("file"), b"new shared contents").unwrap();
    let next = fixture.backup(|_, _| {}).unwrap();
    assert_eq!(
        fs::metadata(next.path.join("file")).unwrap().ino(),
        fs::metadata(next.path.join("nested/alias")).unwrap().ino()
    );
    assert_ne!(
        fs::metadata(next.path.join("file")).unwrap().ino(),
        fs::metadata(version.path.join("file")).unwrap().ino()
    );
    assert_eq!(
        fs::read(version.path.join("nested/alias")).unwrap(),
        b"linked bytes"
    );
    assert_eq!(
        fs::read(next.path.join("nested/alias")).unwrap(),
        b"new shared contents"
    );
    publication::check_manifest(&version).unwrap();
    publication::check_manifest(&next).unwrap();
}

#[test]
fn hard_link_names_outside_the_selected_tree_are_disclosed_without_being_copied() {
    let fixture = FileFixture::new(b"selected payload");
    let outside = fixture.temp.path().join("outside");
    fs::hard_link(fixture.source.path.join("file"), &outside).unwrap();
    let version = fixture.backup(|_, _| {}).unwrap();
    assert_eq!(fs::metadata(version.path.join("file")).unwrap().nlink(), 1);
    assert!(!version.path.join("outside").exists());
    let captured = facts(&version, Path::new("file"));
    assert_eq!(captured["links"], 2);
    assert!(
        captured["omissions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["field"] == "hard_links"
                && item["reason"].as_str().unwrap().contains("Only 1 of 2"))
    );
    assert_eq!(fs::read(outside).unwrap(), b"selected payload");
    assert_eq!(version.fidelity.as_ref().unwrap().entries_with_omissions, 1);
}

#[test]
fn raw_named_hard_linked_symlinks_preserve_the_link_inode_and_leave_the_referent_alone() {
    use std::os::unix::ffi::OsStringExt;
    let fixture = FileFixture::new(b"ordinary");
    let referent = fixture.temp.path().join("referent");
    fs::write(&referent, b"outside canary").unwrap();
    let before = fs::metadata(&referent).unwrap();
    let first = fixture.source.path.join("symbolic");
    std::os::unix::fs::symlink(&referent, &first).unwrap();
    let alias = PathBuf::from(std::ffi::OsString::from_vec(b"raw-\xff".to_vec()));
    fs::hard_link(&first, fixture.source.path.join(&alias)).unwrap();
    let version = fixture.backup(|_, _| {}).unwrap();
    let a = version.path.join("symbolic");
    let b = version.path.join(&alias);
    assert_eq!(
        fs::symlink_metadata(&a).unwrap().ino(),
        fs::symlink_metadata(&b).unwrap().ino()
    );
    assert_eq!(fs::symlink_metadata(&a).unwrap().nlink(), 2);
    assert_eq!(fs::read_link(&a).unwrap(), referent);
    assert_eq!(fs::read_link(&b).unwrap(), referent);
    let after = fs::metadata(&referent).unwrap();
    assert_eq!(
        (
            before.atime(),
            before.atime_nsec(),
            before.mtime(),
            before.mode()
        ),
        (
            after.atime(),
            after.atime_nsec(),
            after.mtime(),
            after.mode()
        )
    );
    assert_eq!(version.fidelity.as_ref().unwrap().entries_with_omissions, 0);
    let captured = facts(&version, &alias);
    assert_eq!(captured["links"], 2);
    assert_eq!(fs::read(referent).unwrap(), b"outside canary");
    publication::check_manifest(&version).unwrap();
}

#[test]
fn differing_hard_link_revisions_stop_publication_and_keep_the_previous_version() {
    let fixture = FileFixture::new(b"before");
    fs::hard_link(
        fixture.source.path.join("file"),
        fixture.source.path.join("alias"),
    )
    .unwrap();
    let first = fixture.backup(|_, _| {}).unwrap();
    let mut edited = false;
    let error = fixture
        .backup(|count, _| {
            if count == 1 && !edited {
                fs::write(fixture.source.path.join("file"), b"edited between aliases").unwrap();
                edited = true;
            }
        })
        .unwrap_err();
    assert!(edited);
    assert!(
        error
            .to_string()
            .contains("Hard-linked source revisions differ"),
        "{error}"
    );
    fixture.assert_only_current(&first);
    assert_eq!(fs::read(first.path.join("file")).unwrap(), b"before");
    assert_eq!(fs::read(first.path.join("alias")).unwrap(), b"before");
}

#[test]
fn attribute_changes_during_copy_are_recollected_and_replaced_in_the_manifest() {
    let fixture = FileFixture::new(b"stable contents");
    let source = fixture.source.path.join("file");
    rustix::fs::setxattr(
        &source,
        "user.revision",
        b"before",
        rustix::fs::XattrFlags::empty(),
    )
    .unwrap();
    let (version, attempts) = fixture.edit_before_copy(|attempt| {
        if attempt == 1 {
            rustix::fs::setxattr(
                &source,
                "user.revision",
                b"after",
                rustix::fs::XattrFlags::empty(),
            )
            .unwrap();
        }
    });
    let version = version.unwrap();
    assert_eq!(attempts, 2);
    assert_eq!(version.bytes_written, b"stable contents".len() as u64);
    assert_eq!(
        attribute(&version.path.join("file"), b"user.revision"),
        b"after"
    );
    let captured = facts(&version, Path::new("file"));
    assert_eq!(
        captured["attributes"][0]["value"],
        serde_json::json!(b"after".as_slice())
    );
    assert_eq!(captured["omissions"], serde_json::json!([]));
}

#[test]
#[ignore = "requires the disposable guest's bounded tmpfs for source attribute limits"]
fn bounded_attribute_collection_preserves_payload_and_reports_uncaptured_metadata() {
    crate::require_disposable_guest();
    let fixture = FileFixture::new(b"complete payload");
    let source = fixture.source.path.join("file");
    let file = File::open(&source).unwrap();
    assert_eq!(
        rustix::fs::fstatfs(&file).unwrap().f_type as u32,
        0x01021994
    );
    for index in 0..17 {
        rustix::fs::fsetxattr(
            &file,
            format!("user.large-{index:02}"),
            &vec![index; 65536],
            rustix::fs::XattrFlags::empty(),
        )
        .unwrap();
    }
    let version = fixture.backup(|_, _| {}).unwrap();
    assert_eq!(
        fs::read(version.path.join("file")).unwrap(),
        b"complete payload"
    );
    let captured = facts(&version, Path::new("file"));
    assert_eq!(captured["attributes_complete"], false);
    assert_eq!(captured["attribute_names_complete"], false);
    assert!(captured["attributes"].as_array().unwrap().len() < 17);
    assert!(
        captured["omissions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["field"] == "extended_attribute_list"
                && item["reason"].as_str().unwrap().contains("budget"))
    );
    assert_eq!(
        version
            .fidelity
            .as_ref()
            .unwrap()
            .source_attributes_unavailable,
        1
    );
    assert!(version.fidelity.as_ref().unwrap().omitted_fields > 0);
    publication::check_manifest(&version).unwrap();
}
