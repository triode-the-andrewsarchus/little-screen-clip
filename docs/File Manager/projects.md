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

Live TODO content and checkbox editing are the next Phase 8 delivery. At this
point the Projects menu opens the registered folder through ordinary guarded
browser navigation; it does not scan project content on the UI thread.
