# Phase 1: standalone folder browser

The Phase 1 browser is delivered as `clip-file-manager`, a separate executable in the `clip-workbench` package. Launch it with:

```bash
./launch.sh files
./launch.sh files /path/to/folder
cargo run -p clip-workbench --bin clip-file-manager -- /path/to/folder
```

It defaults to the user's home directory. It uses an ordinary resizable window with its own application ID and lifecycle and works without the clipboard daemon or USB display. The workbench retains its existing fixed popup and workspace tabs.

## Delivered behavior

- Back, Forward, parent navigation, refresh, and an editable address bar.
- A lazy folder tree with independent disclosure controls and current-location highlighting.
- A details list showing folders first, filenames, sizes, and local modification times.
- Selection with single click, folder opening with double click or Enter, and keyboard tree navigation.
- Hidden-file visibility, distinct loading/empty/error states, retry, and cancellation of pending navigation.
- History that restores selection and the rendered scroll offset; failed navigation preserves the current listing.
- Bounded background reads, queue admission retry, and rejection of replaced completions.
- Virtualized list and tree rendering. Collapsed tree descendants release their cached listings and pending jobs.
- Known ancestor paths remain representable while loading or when a parent denies directory enumeration.

| Key | Action |
| --- | --- |
| Alt+Left / Alt+Right | Back / Forward |
| Alt+Up | Parent directory |
| Ctrl+L | Focus and select the address |
| Enter | Submit an address or open a selected folder |
| Escape in the address bar | Discard the edit and return focus to the list |
| F5 | Refresh the listing and current tree branch |
| Ctrl+H | Toggle hidden entries |
| Up / Down in the list or tree | Move selection |
| Left / Right in the tree | Collapse / expand the selected branch |
| Ctrl+Q | Close the application |

## Integration decisions

The shared desktop library exposes the existing controls, SVG icons, semantic appearance, tabs, and font setup. Both executables compile against that implementation. Directory browsing is isolated in `src/file_manager`; it reuses core's `BackgroundJobs` instead of adding another executor or depending on daemon availability. Existing viewers and operation pipelines remain available for integration in the later phases.

Paths are stored as `PathBuf`, with a reversible address representation: valid Unicode is displayed directly, backslashes become `\\`, and invalid bytes/control characters become `\xHH`. Spaces require no shell quoting. Address entry accepts absolute paths, `~`, `~/...`, and paths relative to the current folder; input never executes as a shell command.

Successful navigation resolves symlinks to their physical destination. The address bar, history, and current tree path then use that destination. Broken links remain visible in the listing. Tree expansion cannot recurse indefinitely through aliases, and links to ancestors remain directly navigable.

Appearance and fonts come from the workbench's existing shared setup and configured Light/Dark preference at startup. Phase 1 does not add a separate persisted appearance preference.

## Validation

The implementation includes behavior tests for navigation history, rendered scroll restoration, focus and keyboard routing, path encoding, hidden entries, missing metadata, permissions, symlinks, known ancestry, queue saturation, stalled reads, and stale completions. A real 10,000-file fixture verifies that drawing follows visible rows.

The standalone UI acceptance script reuses the private Weston/Niri runner and its socket-scoped pointer helper. It captures screenshots and reads rendered text through Tesseract, including pointer selection, tree expansion/collapse, keyboard tree selection, and double-click opening. OCR images are enlarged for reliable recognition; screenshots retain their original size.

```bash
cargo build -p clip-workbench --bins
cargo test --workspace -- --test-threads=2
cargo fmt --all --check
cargo clippy -p clip-workbench --all-targets --no-deps
python3 scripts/file_manager_acceptance.py --output-dir /tmp/files-dark --appearance dark
python3 scripts/file_manager_acceptance.py --output-dir /tmp/files-light --appearance light
```

The UI runner requires Niri, Weston, wtype, Tesseract with English language data, and Pillow. Set `TESSDATA_PREFIX` if the language data is installed outside Tesseract's default directory. Output directories must be new. The separate workbench UI runner continues to check its fixed popup, singleton focus, and dismissal behavior.

Clippy with `-D warnings` is blocked by existing warnings in `little-clip-core` on the installed Rust 1.98.1 toolchain. Clippy without treating existing warnings as errors completes; the new browser modules have no reported warnings.

Architecture comparison found no new functions above the complexity threshold and no new dependency cycles. Maximum complexity remains 196. The comparison still labels the change as a regression because it flags pre-existing clone snippets in touched files and a shared fixture-setup snippet in independent tests; production browser code introduces no clone groups. These findings do not justify changing unrelated existing code or building a fixture abstraction solely for a metric.

## Practical limits

The 10,000-file fixture on local `/tmp` storage measured approximately 31–35 ms for directory loading and 1–2 ms for the first headless egui frame in the development build, with roughly 72 text shapes. These are observations on warm local storage, not network-mount timing guarantees or an optimized end-to-end desktop benchmark.

Each loaded directory currently retains all of its entry metadata. Rendering is virtualized; directory storage is not paged. The expanded tree materializes its visible row model. There is no recursive size calculation, filesystem index, or automatic whole-tree scan.

Cancellation is checked between entries and suppresses publication of obsolete results. It cannot interrupt an operating-system filesystem call that is already blocked. Two tree reads may run concurrently through the shared bounded executor. Slow mounts can delay a completion while the application remains interactive.

Filesystem watching, file mutation, file launching, thumbnails, previews, playback, search, Shelf integration, and persistence of navigation history remain scheduled for the subsequent phases in [plan.md](plan.md).
