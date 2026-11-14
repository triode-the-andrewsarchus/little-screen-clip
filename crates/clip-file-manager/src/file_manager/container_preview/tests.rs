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
        omitted: 0,
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
        omitted: 0,
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

#[test]
fn explicit_choices_survive_scrolling_view_changes_and_folder_history() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("folder")).unwrap();
    let source = entry(root.path(), "folder");
    let mut previews = Previews::default();
    // A click records intent separately from the visible observation cache.
    previews.choices.insert(source.path.clone(), false);
    previews.request(&source);
    assert_eq!(previews.height(&source), 28.0);
    assert!(previews.nodes[&source.path].task.is_none());
    previews.retain(&BTreeSet::new());
    assert!(previews.nodes.is_empty());
    assert_eq!(previews.height(&source), 28.0);
    previews.request(&source);
    assert!(previews.nodes[&source.path].task.is_none());
    previews.clear(); // Folder change, studio entry or view switch.
    previews.configure(true, 32);
    previews.request(&source);
    assert_eq!(previews.height(&source), 28.0);
    assert!(previews.nodes[&source.path].task.is_none());

    previews.clear();
    previews.choices.insert(source.path.clone(), true);
    previews.configure(false, 1);
    previews.request(&source);
    assert_eq!(previews.height(&source), 108.0);
    assert!(previews.nodes[&source.path].task.is_some());
    let token = previews.nodes[&source.path]
        .task
        .as_ref()
        .unwrap()
        .token
        .clone();
    previews.clear();
    assert!(token.load(std::sync::atomic::Ordering::Acquire));
    assert_eq!(previews.height(&source), 108.0);
    previews.request(&source);
    assert_eq!(previews.nodes[&source.path].expanded, Some(true));
}

#[test]
fn empty_zip_inventory_is_complete_without_extracting_anything() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("empty.zip");
    zip::ZipWriter::new(std::fs::File::create(&path).unwrap())
        .finish()
        .unwrap();
    let result =
        inventory::load(&entry(root.path(), "empty.zip"), &AtomicBool::new(false)).unwrap();
    assert!(result.archive && result.complete);
    assert!(result.children.is_empty());
    assert_eq!((result.observed, result.omitted), (0, 0));
}

#[test]
fn archive_links_are_disclosed_as_omitted_instead_of_an_empty_inventory() {
    let root = tempfile::tempdir().unwrap();
    let mut zip =
        zip::ZipWriter::new(std::fs::File::create(root.path().join("links.zip")).unwrap());
    zip.add_symlink(
        "alias",
        "../outside",
        zip::write::SimpleFileOptions::default(),
    )
    .unwrap();
    zip.finish().unwrap();
    let mut tar = tar::Builder::new(std::fs::File::create(root.path().join("links.tar")).unwrap());
    for kind in [tar::EntryType::Symlink, tar::EntryType::Fifo] {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(kind);
        header.set_size(0);
        header.set_mode(0o644);
        header.set_link_name("../outside").unwrap();
        header.set_cksum();
        tar.append_data(&mut header, format!("kind-{}", kind.as_byte()), &[][..])
            .unwrap();
    }
    tar.finish().unwrap();
    drop(tar);
    for (name, count) in [("links.zip", 1), ("links.tar", 2)] {
        let result = inventory::load(&entry(root.path(), name), &AtomicBool::new(false)).unwrap();
        assert!(!result.complete);
        assert!(result.children.is_empty());
        assert_eq!((result.observed, result.omitted), (count, count));
    }
    assert!(!root.path().join("alias").exists());
}

#[test]
fn duplicate_tar_members_do_not_publish_an_ambiguous_inventory() {
    let root = tempfile::tempdir().unwrap();
    let mut tar =
        tar::Builder::new(std::fs::File::create(root.path().join("duplicate.tar")).unwrap());
    for value in [b"old", b"new"] {
        let mut header = tar::Header::new_gnu();
        header.set_size(3);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(&mut header, "same.txt", &value[..])
            .unwrap();
    }
    tar.finish().unwrap();
    drop(tar);
    let error = inventory::load(
        &entry(root.path(), "duplicate.tar"),
        &AtomicBool::new(false),
    )
    .unwrap_err();
    assert!(error.contains("Duplicate archive member path"), "{error}");
}

fn browser_with_expanded_container(
    root: &std::path::Path,
    name: &str,
) -> super::super::FileManager {
    let mut browser = super::super::FileManager::default();
    browser.commit(
        super::super::filesystem::read_directory(root, &AtomicBool::new(false)).unwrap(),
        super::super::Navigation::New,
    );
    let folder = entry(root, name);
    let mut node = Node::new(folder.revision);
    node.ready = Some(Arc::new(
        inventory::load(&folder, &AtomicBool::new(false)).unwrap(),
    ));
    browser.containers.nodes.insert(folder.path, node);
    browser
}

#[test]
fn expanded_children_share_selection_keyboard_navigation_and_mutability() {
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join("folder");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("a.txt"), b"first").unwrap();
    std::fs::write(folder.join("b.txt"), b"second").unwrap();
    std::fs::write(folder.join(".hidden.txt"), b"hidden").unwrap();
    let mut browser = browser_with_expanded_container(root.path(), "folder");
    assert_eq!(
        browser
            .displayed_entries()
            .iter()
            .map(|e| e.name.as_str())
            .collect::<Vec<_>>(),
        ["folder", "a.txt", "b.txt"]
    );
    browser.select(folder.clone());
    browser.move_selection_by(1, false);
    assert_eq!(browser.selected_paths(), [folder.join("a.txt")]);
    assert!(browser.selection_is_mutable());
    browser.select_with(folder.join("b.txt"), false, true);
    assert_eq!(
        browser.selected_paths(),
        [folder.join("a.txt"), folder.join("b.txt")]
    );
    browser.containers.retire();
    browser.reconcile_selection();
    assert_eq!(browser.selection_count(), 2);
    assert!(
        browser.selection_is_mutable(),
        "offscreen snapshots still own the selection"
    );
    browser.containers.nodes.get_mut(&folder).unwrap().expanded = Some(false);
    browser.reconcile_selection();
    assert_eq!(
        browser.selection_count(),
        0,
        "collapsed children leave the active location model"
    );
}

#[test]
fn refresh_keeps_unknown_child_selection_until_a_complete_observation_proves_absence() {
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join("folder");
    std::fs::create_dir(&folder).unwrap();
    let child = folder.join("selected.txt");
    std::fs::write(&child, b"selected bytes").unwrap();
    let mut browser = browser_with_expanded_container(root.path(), "folder");
    browser.select(child.clone());
    std::fs::remove_file(&child).unwrap();
    browser.commit(
        super::super::filesystem::read_directory(root.path(), &AtomicBool::new(false)).unwrap(),
        super::super::Navigation::Refresh,
    );
    assert_eq!(browser.selected_paths(), [child]);
    assert!(
        !browser.selection_is_mutable(),
        "a pending observation cannot authorize mutation"
    );
    let source = entry(root.path(), "folder");
    browser.containers.nodes.get_mut(&folder).unwrap().ready = Some(Arc::new(
        inventory::load(&source, &AtomicBool::new(false)).unwrap(),
    ));
    browser.reconcile_selection();
    assert_eq!(browser.selection_count(), 0);
}

#[test]
fn snapshot_admission_keeps_selected_children_and_evicts_old_unselected_observations() {
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join("folder");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("selected.txt"), b"selected").unwrap();
    let mut browser = browser_with_expanded_container(root.path(), "folder");
    browser.select(folder.join("selected.txt"));
    browser
        .containers
        .protect(Some(&browser.location().unwrap().selection.clone()));
    for i in 0..CACHE {
        let name = format!("other-{i:02}");
        let path = root.path().join(&name);
        std::fs::create_dir(&path).unwrap();
        if i < CACHE - 1 {
            let source = entry(root.path(), &name);
            let mut node = Node::new(source.revision);
            node.ready = Some(Arc::new(
                inventory::load(&source, &AtomicBool::new(false)).unwrap(),
            ));
            browser.containers.nodes.insert(path, node);
        }
    }
    browser.containers.request(&entry(root.path(), "other-31"));
    assert_eq!(browser.containers.nodes.len(), CACHE);
    assert!(browser.containers.nodes.contains_key(&folder));
    assert_eq!(
        browser
            .displayed_entries()
            .iter()
            .filter(|e| e.name == "selected.txt")
            .count(),
        1
    );
    assert!(browser.selection_is_mutable());
}

#[test]
fn archive_members_never_become_selectable_or_mutable_host_entries() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("package.zip");
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
    zip.start_file("assets/a.txt", zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(b"archived bytes").unwrap();
    zip.finish().unwrap();
    let mut browser = browser_with_expanded_container(root.path(), "package.zip");
    assert_eq!(
        browser
            .displayed_entries()
            .iter()
            .map(|e| &e.path)
            .collect::<Vec<_>>(),
        [&path]
    );
    browser.select(root.path().join("assets/a.txt"));
    assert!(!browser.selection_is_mutable());
    browser.begin_rename();
    assert!(browser.edits.dialog.is_none());
    browser.reconcile_selection();
    assert_eq!(browser.selection_count(), 0);
    assert!(!root.path().join("assets").exists());
}
