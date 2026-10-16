# Phase 6 — Initial type-to-navigate checkpoint

This checkpoint adds local filename navigation only. It does not complete Phase 6.

With focus in the file listing (or no focused control), typing selects and scrolls
to a matching visible entry in the current directory. Matching ignores case and
prefers an exact name, then a name prefix, then an ordinary substring. Ties follow
the current display order. The listing stays intact; typing does not open files
or enter folders. Enter retains its normal action on the selected item.

The footer displays `Find: <text>` and reports `No match` when appropriate. A query
with no match leaves the existing selection intact. Backspace edits the query;
Escape dismisses it while preserving selection. After two seconds without typing,
the query expires and the next text starts a new query. A second Escape retains
the usual clear-selection behavior. Folder navigation, manual selection, view
changes, focus changes and naming dialogs also end the query.

Queries are literal text, including spaces within names. Hidden entries participate
only when shown. The address bar, editors, dialogs, folder tree and media-strip
focus targets keep their normal input. A bare Space retains playback behavior.
The same browser selection/reveal paths serve List, Photos, Music and Videos.

`type_navigation.rs` keeps transient input separate from local name ranking. Later
Phase 6 work can extend the matcher and introduce explicit scopes without changing
the current selection and focus routing. No recursive search, subsequence/fuzzy
matching, query operators, content extraction, indexes or background search jobs
are included here.

Verification covers matching order, Unicode input/backspace, query expiry,
off-screen selection/reveal, no-match behavior, hidden-file exclusion, folder
activation and address/rename focus isolation.

Checks run:

- `cargo test -p clip-workbench --lib file_manager:: -- --test-threads=1`
- `cargo build --release -p clip-workbench --bin clip-file-manager`
- `cargo fmt --all --check` and `git diff --check`
- `scripts/file_manager_type_navigation_acceptance.py` in Light and Dark private
  Weston/Niri sessions: off-screen matching, Backspace, Enter, substring matching,
  expiry and rename/address input isolation all pass.

The architecture comparison against `files-type-navigation` has no new clone
groups or high-complexity functions. Its remaining regression report is a cycle
member-order difference in unchanged video benchmark scripts: the same members
appear as both a removed cycle and an added cycle.
