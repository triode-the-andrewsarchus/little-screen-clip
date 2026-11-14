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
        notice: None,
        source: None,
        archive_source: None,
        verified: true,
        payload: None,
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
        notice: None,
        source: None,
        archive_source: None,
        verified: true,
        payload: None,
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
    assert_eq!(previews.height(&source, false), 108.0);
    previews.request(&source);
    assert!(previews.nodes[&source.path].task.is_none());
    previews.nodes.get_mut(&source.path).unwrap().expanded = Some(true);
    previews.request(&source);
    assert!(previews.nodes[&source.path].task.is_some());
    assert_eq!(previews.height(&source, false), 108.0);
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
    assert_eq!(previews.height(&source, false), 108.0);
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
    assert_eq!(previews.height(&source, false), 108.0);
    assert!(previews.nodes[&source.path].task.is_none());
    previews.retain(&BTreeSet::new());
    assert!(previews.nodes.is_empty());
    assert_eq!(previews.height(&source, false), 108.0);
    previews.request(&source);
    assert!(previews.nodes[&source.path].task.is_none());
    previews.clear(); // Folder change, studio entry or view switch.
    previews.configure(true, 32);
    previews.request(&source);
    assert_eq!(previews.height(&source, false), 108.0);
    assert!(previews.nodes[&source.path].task.is_none());

    previews.clear();
    previews.choices.insert(source.path.clone(), true);
    previews.configure(false, 1);
    previews.request(&source);
    assert_eq!(previews.height(&source, false), 108.0);
    assert!(previews.nodes[&source.path].task.is_some());
    let token = previews.nodes[&source.path]
        .task
        .as_ref()
        .unwrap()
        .token
        .clone();
    previews.clear();
    assert!(token.load(std::sync::atomic::Ordering::Acquire));
    assert_eq!(previews.height(&source, false), 108.0);
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

fn observe_container(previews: &mut Previews, source: &Entry) {
    let mut node = Node::new(source.revision);
    node.expanded = previews.choices.get(&source.path).copied();
    node.ready = Some(Arc::new(
        inventory::load(source, &AtomicBool::new(false)).unwrap(),
    ));
    previews.nodes.insert(source.path.clone(), node);
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
    observe_container(&mut browser.containers, &folder);
    browser
}

#[test]
fn expanded_children_share_pointer_selection_and_mutability() {
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
    browser.select(folder.join("a.txt"));
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
fn list_and_columns_arrow_navigation_skips_inline_children_and_uses_their_parent_row() {
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join("a-folder");
    let next = root.path().join("b-folder");
    std::fs::create_dir(&folder).unwrap();
    std::fs::create_dir(&next).unwrap();
    let child = folder.join("nested");
    std::fs::create_dir(&child).unwrap();
    for view in [
        super::super::BrowserView::List,
        super::super::BrowserView::Columns,
    ] {
        let mut browser = browser_with_expanded_container(root.path(), "a-folder");
        browser.set_view(view);
        browser.select(folder.clone());
        browser.move_selection_by(1, false);
        assert_eq!(browser.selected_paths(), std::slice::from_ref(&next));
        browser.move_selection_by(-1, false);
        assert_eq!(browser.selected_paths(), std::slice::from_ref(&folder));
        // Clicking a preview still selects it. The next arrow resumes relative
        // to the preview's owning row, rather than jumping to the first row.
        browser.select(child.clone());
        assert_eq!(browser.selected_paths(), std::slice::from_ref(&child));
        browser.move_selection_by(1, false);
        assert_eq!(browser.selected_paths(), std::slice::from_ref(&next));
        browser.move_selection_by(-1, false);
        browser.move_selection_by(-1, false);
        assert_eq!(browser.selected_paths(), std::slice::from_ref(&folder));
    }
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

fn nested_fixture(name: &str) -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join("folder");
    let nested = folder.join("nested");
    std::fs::create_dir_all(&nested).unwrap();
    let child = nested.join(name);
    std::fs::write(&child, b"nested bytes").unwrap();
    (root, folder, nested, child)
}

#[test]
fn explicit_nested_sections_share_layout_selection_and_collapse() {
    let (root, folder, nested, child) = nested_fixture("inside.txt");
    let mut browser = browser_with_expanded_container(root.path(), "folder");
    let source = entry(root.path(), "folder");
    assert_eq!(browser.containers.height(&source, false), 108.0);
    assert!(!browser.displayed_entries().iter().any(|e| e.path == child));
    browser.containers.choose(nested.clone(), true);
    assert_eq!(browser.containers.height(&source, false), 108.0);
    observe_container(&mut browser.containers, &entry(&folder, "nested"));
    browser.select(child.clone());
    assert!(browser.selection_is_mutable());
    assert_eq!(
        browser.containers.selection_offset(&source, &child, false),
        30.0
    );
    browser.commit(
        super::super::filesystem::read_directory(root.path(), &AtomicBool::new(false)).unwrap(),
        super::super::Navigation::Refresh,
    );
    assert!(
        browser.selection_is_mutable(),
        "unchanged nested snapshots survive a refresh"
    );
    browser
        .containers
        .protect(Some(&browser.location().unwrap().selection.clone()));
    assert!(browser.containers.protected.contains(&folder));
    assert!(browser.containers.protected.contains(&nested));
    browser.containers.retire();
    browser.reconcile_selection();
    assert_eq!(browser.selected_paths(), [child]);
    browser.containers.choose(nested, false);
    browser.reconcile_selection();
    assert_eq!(browser.containers.height(&source, false), 108.0);
    assert_eq!(browser.selection_count(), 0);
}

#[test]
fn nested_layout_and_entries_stop_at_the_documented_depth() {
    let root = tempfile::tempdir().unwrap();
    let mut parent = root.path().to_path_buf();
    let mut previews = Previews::default();
    let names = ["folder", "level1", "level2", "level3", "level4", "level5"];
    for name in names {
        parent = parent.join(name);
        std::fs::create_dir(&parent).unwrap();
    }
    parent = root.path().to_path_buf();
    for name in names {
        let source = entry(&parent, name);
        previews.choose(source.path.clone(), true);
        observe_container(&mut previews, &source);
        parent = source.path;
    }
    let source = entry(root.path(), "folder");
    let (sections, truncated) = previews.sections(&source, false);
    assert!(!truncated);
    assert_eq!(
        sections.iter().map(|(_, depth)| *depth).collect::<Vec<_>>(),
        [0, 1, 2]
    );
    assert_eq!(previews.height(&source, false), 108.0);
    assert!(
        previews
            .children(&source, false)
            .iter()
            .any(|e| e.name == "level3")
    );
    assert!(
        !previews
            .children(&source, false)
            .iter()
            .any(|e| e.name == "level4")
    );
}

#[test]
fn wide_nested_layout_discloses_truncation_without_unbounded_rows() {
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join("folder");
    std::fs::create_dir(&folder).unwrap();
    let mut previews = Previews::default();
    for i in 0..32 {
        let nested = folder.join(format!("nested-{i:02}"));
        std::fs::create_dir(&nested).unwrap();
        previews.choose(nested, true);
    }
    let source = entry(root.path(), "folder");
    observe_container(&mut previews, &source);
    previews.choose(folder, true);
    let (sections, truncated) = previews.sections(&source, false);
    assert_eq!(sections.len(), CACHE);
    assert!(truncated);
    assert_eq!(previews.height(&source, false), 108.0);
}

#[test]
fn ancestor_refresh_preserves_pending_nested_selection_until_fresh_coverage() {
    let (root, folder, nested, child) = nested_fixture("selected.txt");
    let mut browser = browser_with_expanded_container(root.path(), "folder");
    browser.containers.choose(nested.clone(), true);
    observe_container(&mut browser.containers, &entry(&folder, "nested"));
    browser.select(child.clone());
    std::fs::remove_file(&child).unwrap();
    std::fs::write(folder.join("changed.txt"), b"new parent revision").unwrap();
    browser.commit(
        super::super::filesystem::read_directory(root.path(), &AtomicBool::new(false)).unwrap(),
        super::super::Navigation::Refresh,
    );
    assert_eq!(browser.selected_paths(), std::slice::from_ref(&child));
    assert!(!browser.selection_is_mutable());
    observe_container(&mut browser.containers, &entry(root.path(), "folder"));
    browser.reconcile_selection();
    assert_eq!(browser.selected_paths(), [child]);
    assert!(!browser.selection_is_mutable());
    observe_container(&mut browser.containers, &entry(&folder, "nested"));
    browser.reconcile_selection();
    assert_eq!(browser.selection_count(), 0);
}

#[test]
fn hidden_nested_sections_do_not_reserve_space_or_expose_children_until_enabled() {
    let (root, folder, _, _) = nested_fixture("ordinary.txt");
    let private = folder.join(".private");
    std::fs::create_dir(&private).unwrap();
    let secret = private.join("note.txt");
    std::fs::write(&secret, b"hidden content").unwrap();
    let mut browser = browser_with_expanded_container(root.path(), "folder");
    browser.containers.choose(private, true);
    observe_container(&mut browser.containers, &entry(&folder, ".private"));
    let source = entry(root.path(), "folder");
    assert_eq!(browser.containers.height(&source, false), 108.0);
    assert!(!browser.displayed_entries().iter().any(|e| e.path == secret));
    browser.show_hidden = true;
    assert_eq!(browser.containers.height(&source, true), 108.0);
    assert!(browser.displayed_entries().iter().any(|e| e.path == secret));
    browser.select(secret);
    browser.toggle_hidden();
    assert_eq!(browser.selection_count(), 0);
}

#[test]
fn here_scope_captures_expanded_parents_and_pending_nested_identity_without_archives() {
    let (root, folder, nested, child) = nested_fixture("needle.txt");
    std::fs::write(folder.join(".hidden-value.txt"), b"hidden metadata").unwrap();
    let mut browser = browser_with_expanded_container(root.path(), "folder");
    browser.containers.choose(nested.clone(), true);
    let directory = browser.directory.as_ref().unwrap();
    let (pending, _) = browser
        .containers
        .search_scope(directory, false)
        .unwrap()
        .unwrap();
    pending
        .check_shape(root.path(), clip_search::query::Scope::Here)
        .unwrap();
    assert_eq!(pending.directories.len(), 2);
    assert_eq!(
        pending
            .pending
            .iter()
            .map(|entry| &entry.path)
            .collect::<Vec<_>>(),
        [&nested]
    );
    observe_container(&mut browser.containers, &entry(&folder, "nested"));
    let (ready, entries) = browser
        .containers
        .search_scope(browser.directory.as_ref().unwrap(), false)
        .unwrap()
        .unwrap();
    assert_eq!(ready.directories.len(), 3);
    assert!(ready.pending.is_empty());
    assert!(entries.iter().any(|entry| entry.path == child));
    let mut query = clip_search::query::Query::navigation("needle".into());
    query.resolve_navigation_entries(
        browser
            .directory
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .chain(entries.iter()),
        false,
    );
    assert_eq!(
        query.scope,
        clip_search::query::Scope::Here,
        "known expanded hits must not trigger recursive fallback"
    );
    for hidden in [false, true] {
        let mut navigation = clip_search::query::Query::navigation("hidden-value".into());
        navigation.resolve_navigation_entries(entries.iter(), hidden);
        assert_eq!(
            navigation.scope,
            if hidden {
                clip_search::query::Scope::Here
            } else {
                clip_search::query::Scope::Recursive
            }
        );
    }
    browser.containers.choose(folder, false);
    assert!(
        browser
            .containers
            .search_scope(browser.directory.as_ref().unwrap(), false)
            .unwrap()
            .is_none()
    );
}

#[test]
fn inline_layout_reserves_direct_children_before_complete_nested_groups() {
    let (root, folder, nested, _) = nested_fixture("inside.txt");
    std::fs::write(folder.join("direct.txt"), b"direct").unwrap();
    let source = entry(root.path(), "folder");
    let mut previews = Previews::default();
    observe_container(&mut previews, &source);
    observe_container(&mut previews, &entry(&folder, "nested"));
    // Two direct cards consume 140px. The child requires a further 50px.
    previews.plan_strip(&source, 189.0, false, false);
    assert!(!previews.explicitly_expanded(&nested));
    previews.plan_strip(&source, 190.0, false, false);
    assert!(previews.explicitly_expanded(&nested));
    let mut tiles = Vec::new();
    previews.tiles(&source, false, 0, &mut tiles);
    assert_eq!(
        tiles
            .iter()
            .map(|tile| tile.child.name())
            .collect::<Vec<_>>(),
        ["nested", "inside.txt", "direct.txt"]
    );
    assert_eq!(
        tiles.iter().map(|tile| tile.level).collect::<Vec<_>>(),
        [0, 1, 0]
    );
    previews.plan_strip(&source, 140.0, false, false);
    assert!(!previews.explicitly_expanded(&nested));
    assert_eq!(previews.height(&source, false), 108.0);
}
#[test]
fn inline_layout_stops_after_three_progressively_smaller_layers() {
    let root = tempfile::tempdir().unwrap();
    let mut parent = root.path().to_owned();
    let mut previews = Previews::default();
    for name in ["folder", "one", "two", "three", "four"] {
        std::fs::create_dir(parent.join(name)).unwrap();
        let source = entry(&parent, name);
        // Observe after descendants exist below.
        parent = source.path;
    }
    parent = root.path().to_owned();
    for name in ["folder", "one", "two", "three", "four"] {
        let source = entry(&parent, name);
        observe_container(&mut previews, &source);
        parent = source.path;
    }
    let source = entry(root.path(), "folder");
    previews.plan_strip(&source, 2000.0, false, false);
    let mut tiles = Vec::new();
    previews.tiles(&source, false, 0, &mut tiles);
    assert_eq!(
        tiles
            .iter()
            .map(|tile| tile.child.name())
            .collect::<Vec<_>>(),
        ["one", "two", "three"]
    );
    assert_eq!(
        tiles
            .iter()
            .map(|tile| strip::edge(tile.level))
            .collect::<Vec<_>>(),
        [64.0, 44.0, 32.0]
    );
}
