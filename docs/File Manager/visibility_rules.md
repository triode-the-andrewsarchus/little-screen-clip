# Folder visibility rules

This is the C7 implementation contract. Validation evidence is recorded in
`phase8_completion.md`.

`.hide` and `.show` are folder-owned UTF-8 TOML documents. They never change file
permissions, content or search indexing policy. Each file must declare version 1;
unknown versions, malformed documents and unknown keys are errors, not an invitation
to replace the original file. For example:

```toml
# .hide
version = 1
patterns = ["*.tmp", "cache/", "!keep.tmp"]
```

```toml
# .show
version = 1
patterns = [".env", ".gitignore", "keep.tmp"]
```

## Matching and precedence

Matching is case-sensitive, with literal `/` separators, backslash escapes and
standard glob wildcards/classes/alternatives (`*`, `?`, `**`, `[abc]`, `{a,b}`).
A slashless pattern matches a basename at any descendant level. A pattern with a
slash is relative to the owning folder. An initial `/` anchors even a slashless
pattern to that folder. A trailing `/` limits the match to directories, including
their descendants. A matching directory opinion follows its descendants; it does
not trigger a recursive scan. Parent traversal components are invalid. Patterns
match path bytes through globset, retaining Unix non-UTF-8 path support.

Files are processed from the oldest ancestor down to the currently opened folder.
A deeper matching file overrides an inherited opinion. At the same level `.show`
is processed after `.hide` and wins when both match. Within one file, the last
matching pattern wins. A leading `!` cancels that file's opinion for the matching
path; it does not force the opposite opinion or undo an explicit ancestor rule.
Use a deeper `.show` to override an ancestor hide, or a deeper `.hide` to override
an ancestor show.

No matching opinion uses the ordinary Hidden toggle. An explicit `.hide` match
hides the entry even when Hidden is on. An explicit `.show` match shows it even
when Hidden is off, allowing selective dotfile reveal. Hidden ancestor-column
entries on the active route remain visible so the current path stays navigable.
Sorting-set filters remain independent and can still exclude a shown file.

## Scope and observation

Inheritance follows pinned directory parents and stops at the current mount's
boundary. It never reads rule payloads from protected kernel interfaces. The
browser directory's device/inode and mount identity are checked before collection
and again before publication. Rule sources are guarded ordinary regular files;
symlinked/nonregular rule files are rejected. Rule reads check their same-handle
source identity and pathname before accepting their bytes.

Each source permits 32 KiB; all inherited sources permit 256 KiB, 128 patterns and
32 ancestor directories. Each pattern permits 256 bytes. Exceeding any bound is
an explicit failure. File reads and glob compilation run on a separate bounded
worker. Obsolete requests cancel delivery; a stalled native read cannot create
unbounded replacement workers. No filesystem reads occur during row filtering.

Ordinary folder-preview inventories load the same rules on their existing bounded
worker so each preview can honor its own folder's local overrides. Archive members
are payload previews rather than independently rule-managed ordinary paths.
Source errors in a contained folder retain its inventory and expose a rule-error
notice instead of silently dropping its contents or its inventory coverage.

Folder refresh/navigation reloads the current rules. Ancestor edits should be
followed by Refresh or Reload visibility rules; there is no separate recursive
watch of every ancestor. Rule snapshots are observations, not access controls.

## Presentation and recovery

The existing view toolbar contains Rules: on/off. It exposes Apply .hide / .show,
Reload visibility rules, the observed source documents and Reveal all files for
recovery. Recovery ignores rules and enables ordinary hidden entries in one
operation, so the rule files themselves can be repaired. These viewing choices
are temporary and do not rewrite rule files or configuration.

Matched rows use solid tinted backgrounds and explicit Shown/Hidden labels, with
the source path on hover. Ignored opinions are identified in the label when space
permits and always in the hover explanation. Session underlines and sorting-set
bordered badges keep their own meanings. Invalid or unavailable rules fail open
to ordinary visibility and display an error with recovery guidance. They do not
retain a previous hide snapshot after collection fails.
