# Phase 7 — views, container previews and meaningful sorting

Status: complete after the six-task closeout and recorded technical review on
`feat/file-manager-phase7`. The branch started at `d0298ebd`.
The authoritative remaining tasks and current-head verification are in
[phase7_closeout.md](phase7_closeout.md).

## Current requirements and behavior

- List and Columns share bounded image/text/audio/MIDI/video/PDF and other
  existing preview owners. Redundant Music and Videos views are removed;
  persisted legacy choices migrate to List. Photos and Constellation remain.
- Folder/archive entry targets are full squares, without +/− disclosures.
  Inline strips reserve direct-child capacity first, then use spare space for
  at most three progressively smaller layers (64/44/32px icons). Reads, depth,
  observations and visible work are bounded; incomplete coverage stays explicit.
- Columns uses a horizontally scrollable chain of actual folder listings,
  resizable dividers and a one-line bottom breadcrumb bar with ancestor overflow.
  The final column uses the shared rich List, not a separate preview pane.
- List/Columns arrows traverse current-folder rows, skipping inline descendants.
  Descendants remain clickable, selectable and actionable. Keyboard movement
  from a clicked descendant resumes relative to its owning row. Columns
  Left/Right navigate the parent/selected folder. History retains selection,
  the view and its saved scroll position.
- Browsing is the default arrow-key target on opening either view and returning
  from a studio. Explicit sidebar, text, media-strip, popup and modal focus owns
  its keys. Pending focus handoffs settle without dropping the first arrow.
- Moving within the viewport leaves it stationary; crossing an edge reveals
  only the necessary row area. Overlapping preview textures stay painted and
  retained. Header sorting preserves the viewport instead of revealing a remote
  selection or jumping to the top.
- Type groups have pinned family/column headers, pushed out by the next group.
  Single-family ordinary folders use one family header instead of a generic bar
  plus a duplicate section header, and omit the redundant Type button.
- Family metadata includes audio artist/album/genre/duration, image dimensions
  and pixels, video duration/dimensions/codec, and document title/author/pages.
  Size and Modified remain present for every family. Missing and failed values
  remain last in both directions; metadata completion preserves file identity.
- Optional row summaries yield space to filenames. Filename order is an explicit
  ascending natural-name reset, separate from reversible header clicks.
- Explicit folder-view overrides and descendant inheritance persist in local
  application state; browsing creates no sidecars in content folders.
- Expanded `here` search shares the existing scope, provenance and file actions.
  Image dHash/reference sorting reuses Phase 6 and needs no embedding runtime.
- Unconsumed Escape uses the common guarded studio close, preserving nested
  popup/tool cancellation, dirty-edit confirmation and accepted write completion.

## Archive boundaries

ZIP, TAR and gzip/bzip2/xz/zstd TAR browsing uses validated private snapshots,
ordinary List/Columns navigation, logical archive breadcrumbs and a persistent
compressed-archive warning. Member cards reveal the selected member in that
browser; ordinary studios open its captured contents. Passive inventories never
extract into the source folder. Archive browsing never edits the source archive;
retaining changes requires saving/copying to an ordinary destination.

Snapshot limits are 4,096 members, 32 path levels, 512 MiB encoded source,
64 MiB per workspace and a 256 MiB retained-history budget including directory
overhead. Links/special members, unsafe or duplicate paths, prefix collisions,
failed checksums and incomplete scans cannot publish a partial browseable folder.
Passive card inventories have their own smaller bounded display/decoding limits.

## Acceptance

The approved feature deliveries are complete. Current-head closeout checks,
fix commits and the final review checkpoint are tracked in
[phase7_closeout.md](phase7_closeout.md), including workspace tests, strict
all-target Clippy, formatting, canonical build and native workflows in both
appearances and narrow/wide windows. A result from an earlier checkpoint does
not substitute for the final gate.

Warm/cool ordering is optional follow-up. New automatic view selection and audio
sample similarity are excluded. Home, session/sorting marks, visibility rules
and project organization belong to Phase 8.

## Historical implementation checkpoints

[phase7_history.md](phase7_history.md) preserves the original chronological
ledger, intermediate failures, superseded layouts and earlier test counts.
Those descriptions are history, not additional unfinished requirements.
