use super::*;
use std::{io::Write, sync::atomic::AtomicBool};

fn entry(root: &std::path::Path, name: &str) -> Entry {
    super::super::filesystem::read_directory(root, &AtomicBool::new(false))
        .unwrap()
        .entries
        .into_iter()
        .find(|e| e.name == name)
        .unwrap()
}
#[test]
fn folder_inventory_is_bounded_and_reports_incomplete_without_following_links() {
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join("small");
    std::fs::create_dir(&folder).unwrap();
    for i in 0..40 {
        std::fs::write(folder.join(format!("{i}.txt")), "ink").unwrap();
    }
    let source = entry(root.path(), "small");
    let result = inventory::load(&source, &AtomicBool::new(false)).unwrap();
    assert_eq!(result.children.len(), 32);
    assert!(!result.complete);
    assert_eq!(result.observed, 33);
    std::os::unix::fs::symlink(&folder, root.path().join("alias")).unwrap();
    let alias = entry(root.path(), "alias");
    assert!(!candidate(&alias));
    assert!(inventory::load(&alias, &AtomicBool::new(false)).is_err());
}
#[test]
fn changed_cancelled_and_protected_folder_sources_do_not_publish() {
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join("folder");
    std::fs::create_dir(&folder).unwrap();
    let old = entry(root.path(), "folder");
    std::fs::rename(&folder, root.path().join("old")).unwrap();
    std::fs::create_dir(&folder).unwrap();
    assert!(inventory::load(&old, &AtomicBool::new(false)).is_err());
    let fresh = entry(root.path(), "folder");
    assert!(inventory::load(&fresh, &AtomicBool::new(true)).is_err());
    let protected = entry(std::path::Path::new("/"), "proc");
    assert!(inventory::load(&protected, &AtomicBool::new(false)).is_err());
}
#[test]
fn zip_metadata_never_extracts_or_follows_member_paths() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("example.zip");
    let mut writer = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
    writer
        .start_file(
            "images/picture.txt",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
    writer.write_all(b"contents").unwrap();
    writer.finish().unwrap();
    let original = std::fs::read(&path).unwrap();
    let result =
        inventory::load(&entry(root.path(), "example.zip"), &AtomicBool::new(false)).unwrap();
    assert!(result.archive && result.complete);
    assert_eq!(result.children[0].name(), "images/picture.txt");
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert!(!root.path().join("images").exists());
    let mut writer = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
    writer
        .start_file("../escape", zip::write::SimpleFileOptions::default())
        .unwrap();
    writer.write_all(b"bad").unwrap();
    writer.finish().unwrap();
    assert!(inventory::load(&entry(root.path(), "example.zip"), &AtomicBool::new(false)).is_err());
}
#[test]
fn tar_inventory_reads_regular_headers_without_member_payload_allocation() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("example.tar");
    let mut tar = tar::Builder::new(std::fs::File::create(&path).unwrap());
    let mut header = tar::Header::new_gnu();
    header.set_size(4);
    header.set_mode(0o644);
    header.set_cksum();
    tar.append_data(&mut header, "nested/file.txt", &b"data"[..])
        .unwrap();
    tar.finish().unwrap();
    drop(tar);
    let result =
        inventory::load(&entry(root.path(), "example.tar"), &AtomicBool::new(false)).unwrap();
    assert_eq!(result.children[0].name(), "nested/file.txt");
    assert!(result.complete);
}

#[test]
fn visible_inventory_admission_is_bounded_and_retirement_cancels_owned_tasks() {
    let root = tempfile::tempdir().unwrap();
    for index in 0..40 {
        std::fs::create_dir(root.path().join(format!("folder-{index:02}"))).unwrap();
    }
    let directory =
        super::super::filesystem::read_directory(root.path(), &AtomicBool::new(false)).unwrap();
    let mut previews = Previews::default();
    for entry in &directory.entries {
        previews.request(entry);
    }
    assert_eq!(previews.nodes.len(), CACHE);
    assert!(
        previews
            .nodes
            .values()
            .filter(|node| node.task.is_some())
            .count()
            <= READS
    );
    let tokens: Vec<_> = previews
        .nodes
        .values()
        .filter_map(|node| node.task.as_ref().map(|task| task.token.clone()))
        .collect();
    assert!(!tokens.is_empty());
    previews.retain(&BTreeSet::new());
    assert!(previews.nodes.is_empty());
    assert!(
        tokens
            .iter()
            .all(|token| token.load(std::sync::atomic::Ordering::Acquire))
    );
}

#[test]
fn automatic_small_folder_expansion_and_manual_override_have_separate_meanings() {
    let mut node = Node::new(1);
    node.ready = Some(Arc::new(inventory::Inventory {
        children: Vec::new(),
        observed: 9,
        complete: true,
        archive: false,
    }));
    assert!(
        !node.expanded(Options::default()),
        "Larger folders require explicit expansion"
    );
    node.expanded = Some(true);
    assert!(node.expanded(Options::default()));
    node.ready = Some(Arc::new(inventory::Inventory {
        children: Vec::new(),
        observed: 2,
        complete: true,
        archive: false,
    }));
    node.expanded = Some(false);
    assert!(
        !node.expanded(Options::default()),
        "Manual collapse wins over the small-folder default"
    );
    node.expanded = None;
    assert!(node.expanded(Options::default()));
}

#[test]
fn disabling_automatic_previews_starts_no_reads_but_explicit_expansion_still_works() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("folder")).unwrap();
    let source = entry(root.path(), "folder");
    let mut previews = Previews::default();
    previews.configure(false, 0);
    assert_eq!(previews.options.small_items, 1);
    assert_eq!(previews.height(&source), 28.0);
    previews.request(&source);
    assert!(previews.nodes[&source.path].task.is_none());
    previews.nodes.get_mut(&source.path).unwrap().expanded = Some(true);
    previews.request(&source);
    assert!(previews.nodes[&source.path].task.is_some());
    assert_eq!(previews.height(&source), 108.0);
    previews.configure(true, usize::MAX);
    assert_eq!(previews.options.small_items, inventory::LIMIT);
    assert_eq!(previews.nodes[&source.path].expanded, Some(true));
}

#[test]
fn disabling_automatic_previews_retires_automatic_reads_and_preserves_manual_choices() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("folder")).unwrap();
    let source = entry(root.path(), "folder");
    let mut previews = Previews::default();
    previews.request(&source);
    let token = previews.nodes[&source.path]
        .task
        .as_ref()
        .unwrap()
        .token
        .clone();
    previews.configure(false, 8);
    assert!(token.load(std::sync::atomic::Ordering::Acquire));
    assert!(previews.nodes[&source.path].task.is_none());
    previews.nodes.get_mut(&source.path).unwrap().expanded = Some(true);
    previews.request(&source);
    previews.configure(false, 2);
    assert_eq!(previews.nodes[&source.path].expanded, Some(true));
    assert_eq!(previews.height(&source), 108.0);
    std::fs::write(source.path.join("changed.txt"), "new child").unwrap();
    previews.request(&entry(root.path(), "folder"));
    assert_eq!(previews.nodes[&source.path].expanded, Some(true));
}
