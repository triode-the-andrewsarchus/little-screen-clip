# Phase 8 — organization and a real Home

Status: started on `feat/file-manager-phase8`, from the Phase 7 merge `bf6bf73e`.
Architecture baseline: `before-phase8-home`.

## First delivery: static Home pins

Implement an explicit virtual Home surface, separate from the filesystem home
folder and from the clipboard Shelf. Users deliberately pin files and folders,
open them through ordinary guarded browser/studio navigation, and remove a pin
without deleting its target. Pins persist in local application state, never in
the browsed content folder. Missing/unavailable targets keep their references;
opening reports a failure rather than dropping them or browsing another path.

Reuse existing pinned-directory/FileSource atomic state writes, cancellable
background delivery and ordinary guarded navigation. No payload scans, new media
cache, search index or plugin framework belong in Home persistence. Accepted
state writes must finish before application close. Malformed/newer state is
reported and preserved, never silently reset and overwritten.

Define the persistence contract before durable pins: versioned state, lossless
Unix paths, stable pin identity independent from display names, bounded records,
explicit duplicate policy, and cooperating-window updates. A pin is a path
reference, not proof that an unavailable mount or replaced file is the original
object. External renames/moves do not magically retarget pins. Browser-managed
retargeting and full navigation-history integration must be tested before being
claimed. Home must never be implemented as a fabricated filesystem path.

- [ ] Define and test the bounded Home state and atomic update contract.
- [ ] Add the Home surface and deliberate pin/open/remove actions.
- [ ] Verify restart, missing paths, filenames with non-UTF-8 bytes, failed writes,
      concurrent updates and source-folder preservation.
- [ ] Native review in both appearances; document the delivered initial limits.

## Remaining Phase 8 roadmap

- Named user-arranged Home sections; dynamic rule-driven sections remain Phase 10.
- Session Mode: red/yellow/green underlines with user-defined meanings and labels.
- Configurable A/B/... sorting sets, filtering and repeated subdivision.
- Keep session state, sorting-set membership and visibility rules distinct.
- Specify `.hide`/`.show` matching, precedence, inheritance and recovery, including
  selectively revealing dotfiles and readable solid-background markings.
- Specify versioned `.fmproject` identity/references/portability/local preferences.
- Create/register projects, README/TODO defaults and a global project list,
  preserving every pre-existing file during conversion.
- Define and verify restart/rename/move behavior across marks and projects.

The current objective requires starting Phase 8 after the Phase 7 closeout;
it does not claim completion of this full roadmap.
