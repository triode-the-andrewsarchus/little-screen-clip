# Projects

A project is an ordinary folder with a `.fmproject` manifest. Its documents stay
in that folder. Home keeps local references; unregistering a project does not
delete its manifest, documents or folder.

Home → Add offers **Create project in current folder…** and **Register current
folder's project**. The Projects menu lists registered folders and can unregister
them or reload the registry. Creation uses the folder retained by the form,
whose path is shown before the Create action.

## Manifest version 1

`.fmproject` is UTF-8 JSON, at most 32 KiB:

```json
{
  "version": 1,
  "id": "4c6e6a0d-5bd3-4c9f-a7d2-bbb029507e1f",
  "title": "Field notes",
  "description": "Research and sketches",
  "readme": "README.md",
  "todo": "TODO.md"
}
```

The UUID is the portable project identity. Title is required and limited to
256 UTF-8 bytes; description is optional, defaults to empty and is limited to
4,096 bytes. Documents must be distinct relative paths with ordinary components,
at most eight components and 1,024 bytes. Absolute paths, parent traversal,
manifest/lock references and NUL bytes are rejected. Document-parent symlinks are
not followed; they cannot redirect a project document outside its project.
Unknown fields, malformed data and unsupported versions fail explicitly and are
left unchanged. The application does not silently rewrite a newer manifest.

Creation generates a UUID and prefers existing `README.md`/`TODO.md`, then
existing `README`/`TODO`. It creates only missing defaults. Existing regular files
are preserved byte-for-byte; symlinks and non-regular destinations fail instead
of being followed or replaced. A cooperating `.fmproject.lock` serializes project
creation. The manifest is installed last with no-overwrite creation, after the
documents. Each installation uses the retained ordinary-directory handle.

Creation is not a multi-file filesystem transaction. A failure can leave newly
created documents in place; the error reports this and no rollback deletes them.
If registry installation fails after manifest creation, the project remains in
its folder and can subsequently be registered. Accepted writes retain their
completion owner and block application close until the result is delivered.

## Local registry and portability

`$XDG_STATE_HOME/little-screen-clip/projects.json` (fallback
`~/.local/state/little-screen-clip/projects.json`) contains version 1 and a
`projects` array of UUID, title hint and lossless Unix folder path records.
It allows 512 projects and 2 MiB. It uses the shared guarded local-state lock,
read/merge/validate and atomic write transport. Malformed/newer registries are
preserved, with a visible error and reload control. Registry data is independent
from Home's layout/drawing history.

Registering an existing ID at its current folder refreshes its title hint. Finding
the same ID at another folder produces an inline choice showing both paths;
**Use this location** explicitly retargets the reference. Confirmation is tied to
the previously observed folder, so a concurrent relocation requires another
decision. The previous folder remains untouched. A copied manifest does not
automatically become a new project, and an unavailable old folder does not prove
that another folder is its replacement. External moves require explicit
registration at the new location. Local registration reads and validates the
manifest through a guarded handle; it does not copy project content into Home.

## Live TODO areas

Home → Add → **Live Projects area** places a movable, resizable area on the desk.
Its membership follows the registry; removing the area does not unregister a
project, and unregistering a project does not remove its files. Each visible row
shows the project title, the first four Markdown tasks, Reload TODO and Open TODO.
Open TODO exposes the complete source document through the ordinary editor.
Arrange moves/resizes/renames the area; Browse enables task checkboxes.

UTF-8 TODO documents are limited to 256 KiB per preview and 2,048 tasks, with
explicit truncation at the task limit. Parsing uses the existing CommonMark
implementation with task-list support and source offsets, so fenced examples are
not interpreted as real tasks. Nested tasks and inline formatting are supported;
the preview labels omit Markdown markup. Extensionless TODO files use the same
manifest-selected reader as TODO.md.

A checkbox changes exactly one marker byte in the real source. It uses the source
snapshot's identity-checked atomic write and held parent directory; both project
manifest and TODO revision must still match. CRLF, UTF-8, comments, formatting and
every other byte remain intact. External changes, renamed/replaced parents,
missing documents or changed project IDs cause explicit errors, never overwrite
an unrelated revision or silently recreate a missing TODO. Drawing/layout undo
only changes the desk, independently from these document writes.

Only visible rows request reads. A separate bounded pool admits two reads, with
at most 24 retained previews. Hidden reads retire; accepted writes retain their
completion owner and block close until delivery even after Home is hidden.
Visible areas refresh every two seconds and keep the previous snapshot during
loading. Read failures retain the old display with disabled checkboxes and an
error. Save failures remain visible in Home recovery even without a live area, until
explicit Reload task previews or Reload TODO; successful
operations add no notification text. Reads, Markdown parsing and writes all run
off the UI thread. No TODO bytes or checkbox state are stored in Home JSON.
