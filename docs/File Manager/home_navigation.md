# Home navigation

Home is an explicit virtual kind of browser-history visit. Each visit retains a
real folder context for project creation, pins and returning to browsing. That
context is not a fabricated pathname for Home. The drawing/camera belongs to one
persistent personal desk and is shared by all Home visits; visiting Home does not
create another copy of the board or its undo history.

A new window with no directory argument starts at Home. An explicit directory
argument opens that ordinary directory. A startup Home with no prior listing uses
the user's real filesystem home folder as its context, loading it only when the
user returns to browsing. Navigation history is local to the window; the desk,
project registry and organization metadata use their existing persistent stores.

The folder toolbar and Home header share Back/Forward controls. Alt+Left and
Alt+Right navigate Home and folder visits in order. Home's Back to folder and
Escape restore its preceding folder visit, including selection and scroll. With
no earlier folder visit they open its real folder context. Forward history remains
available. Opening a shortcut from Home creates an ordinary folder visit even if
that folder has the same path as the retained context. Reopening the current Home
visit is idempotent.

A Home visit does not invent a filesystem read. Folder visits use ordinary guarded
navigation. Home stays visible with a loading indicator until a requested folder
has been accepted, preventing a different cached listing from being shown under
the context's address. A failed return keeps Home visible with the navigation
error. Unavailable folders do not erase the desk or cause another path to be used.

Unapplied drawing text blocks leaving through history, just as it blocks ordinary
Home exit. Accepted drawing/layout writes retain their owners and continue while
browsing. Title/card-label drafts are committed before a permitted transition.
Home preserves its camera and editor history independently from folder selection
history. Folder-only shortcuts and drop targets remain inactive while Home is
visible.

Reference retargeting after confirmed mutations is described in
`reference_updates.md`; its native acceptance remains tracked in the completion
ledger. External moves are not inferred from a navigation history visit.
