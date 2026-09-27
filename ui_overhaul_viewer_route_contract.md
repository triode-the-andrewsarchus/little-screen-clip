# Viewer Route Contract

This document records the routing behavior implemented by `clip-workbench` and the constraints it establishes for UI work. Line anchors refer to the current source and should be refreshed if routing code moves.

## Route ownership

`WorkbenchApp` owns one active modal viewer in `viewer: Option<ViewerSession>` and a FIFO `startup_viewers` queue. `ViewerSession` enumerates Diff, BinaryDiff, Audio, Image, Spreadsheet, PDF, MIDI, 3D Model, Text, and Component Gallery routes ([`main.rs:249-260`](crates/clip-workbench/src/main.rs#L249)). The workbench view (`Palette`, `NodeMachine`, or `AutoActions`) is a separate state field, so viewer routing overlays the selected workbench view rather than replacing it ([`main.rs:272-300`](crates/clip-workbench/src/main.rs#L272)).

`render_modal_viewers` is the central renderer and close owner. When `viewer` is present it renders the matching state inside a `CentralPanel`, returns `true`, and the `update` loop returns before drawing the workbench underneath. After a close request it takes the next queued startup viewer, if any ([`main.rs:1063-1077`](crates/clip-workbench/src/main.rs#L1063), [`main.rs:1584-1688`](crates/clip-workbench/src/main.rs#L1584)). Direct preview actions assign `viewer` from the palette; this includes audio/image/PDF/text/model previews and diff routes ([`main.rs:903-915`](crates/clip-workbench/src/main.rs#L903), [`main.rs:1175-1201`](crates/clip-workbench/src/main.rs#L1175), [`main.rs:1289-1357`](crates/clip-workbench/src/main.rs#L1289), [`main.rs:1429-1436`](crates/clip-workbench/src/main.rs#L1429)).

## Close and return behavior

Viewer closure is a local route transition: `render_modal_viewers` sets `viewer` to the next queued startup session or `None`. When the queue is empty, the next update resumes rendering the previously selected workbench view. Closing a PDF also marks the mirror dirty if the viewer had unsaved edits; closing Audio or MIDI stops playback ([`main.rs:1621-1628`](crates/clip-workbench/src/main.rs#L1621), [`main.rs:1639-1660`](crates/clip-workbench/src/main.rs#L1639), [`main.rs:1683-1687`](crates/clip-workbench/src/main.rs#L1683)).

`update` applies the global appearance toggle, then renders modal routes before polling jobs or dispatching workbench shortcuts ([`main.rs:1030-1095`](crates/clip-workbench/src/main.rs#L1030)). `render_modal_viewers` gives the recipe prompt precedence; otherwise it renders the active viewer, and either modal route returns early from `update`. Thus the active modal receives Escape before workbench-level routing. MIDI Escape clears its selected region first and closes only when no region is selected; Component Gallery Escape closes immediately, then the route manager advances its startup queue or returns to the previous workbench view ([`main.rs:1595-1694`](crates/clip-workbench/src/main.rs#L1595)). With no active modal route, workbench Escape first closes Node Machine subdialogs/drags, then stops active rolling audio, returns Node Machine or Auto Actions to Palette, and otherwise closes the popup ([`main.rs:303-339`](crates/clip-workbench/src/main.rs#L303), [`main.rs:2025-2060`](crates/clip-workbench/src/main.rs#L2025)). F4/F6 appearance toggling is handled before the modal check and remains available in viewers ([`main.rs:1039-1041`](crates/clip-workbench/src/main.rs#L1039)).

The first Escape action is route-specific:

| Active route | First Escape action |
| --- | --- |
| In-app recipe prompt | Cancel the prompt and return to its workbench route. |
| Audio viewer with save dialog | Close the nested save dialog; keep Audio Studio open. |
| Image viewer with save dialog | Close the nested save dialog and consume Escape before image shortcuts. |
| Text viewer with Find open | Close Find; a later Escape closes the viewer. |
| MIDI viewer with a selected region | Clear the region; a later Escape closes the viewer. |
| Node Machine catalog, save dialog, or cable drag | Close/cancel one active substate; a later Escape advances to the next route. |
| Rolling-audio region active | Stop playback and clear the selection. |
| Any other active viewer | Close that viewer, stopping Audio/MIDI playback and refreshing the mirror after dirty PDF edits. |
| Node Machine or Auto Actions | Return to Quick Palette and restore search focus. |
| Quick Palette | Close the popup. |

The table describes Escape only. Enter/Space and letter shortcuts have separate focus and domain-control rules and remain under H1.3b2-b6.

An in-app recipe prompt is also modal. `render_modal_viewers` gives it precedence over the viewer and returns while it is visible. Confirming a prompt executes the recipe and closes the workbench; cancelling clears the prompt ([`main.rs:1601-1604`](crates/clip-workbench/src/main.rs#L1601), [`main.rs:1690-1795`](crates/clip-workbench/src/main.rs#L1690)).

## Startup precedence

The command line can request Component Gallery, Binary Diff, Diff, Audio, and Image routes. Audio and Image are put in `startup_viewers` in that order. The synchronous `WorkbenchApp::new` precedence is Component Gallery, then Binary Diff, then Diff; if none of these establishes `viewer`, the first queued startup preview becomes active ([`main.rs:496-515`](crates/clip-workbench/src/main.rs#L496), [`main.rs:543-555`](crates/clip-workbench/src/main.rs#L543)). Consequently, with both direct audio and image arguments and no higher-precedence route, Audio opens first and Image opens after Audio closes. A higher-precedence explicit viewer leaves those queued previews to follow it.

`--last` is handled before instance acquisition and opens its own prompt or sends the action directly; it does not construct the workbench route queue ([`main.rs:2435-2506`](crates/clip-workbench/src/main.rs#L2435)).

## Single-instance implications

The popup acquires a runtime file lock before reading daemon state or creating the native window. If another instance holds the lock, the second invocation tries to focus the existing Niri window and exits; this path does not forward its command-line route request into the existing `WorkbenchApp` ([`popup.rs:38-79`](crates/clip-workbench/src/popup.rs#L38), [`popup.rs:134-140`](crates/clip-workbench/src/popup.rs#L134), [`main.rs:2504-2513`](crates/clip-workbench/src/main.rs#L2504)). The fixed popup identity and native size constraints are configured separately ([`main.rs:2513-2525`](crates/clip-workbench/src/main.rs#L2513), [`popup.rs:183-205`](crates/clip-workbench/src/popup.rs#L183)).

Therefore repeated launches focus the existing popup but do not replace its active viewer, enqueue a new preview, or replay new arguments into it. Any future route-forwarding feature would need an explicit IPC or other handoff mechanism; none is present in these source paths.

## Async-load interactions

Spreadsheet and MIDI requests are represented by separate optional job/receiver fields. Each start function cancels any previous spreadsheet or MIDI request before starting the new load; Spreadsheet text carries a fallback payload so failed CSV detection can open a Text viewer ([`main.rs:291-296`](crates/clip-workbench/src/main.rs#L291), [`main.rs:1798-1819`](crates/clip-workbench/src/main.rs#L1798), [`main.rs:1861-1874`](crates/clip-workbench/src/main.rs#L1861)). `update` renders an active prompt/viewer and returns before polling these jobs. Their receivers remain stored, and a result is consumed only after the modal route and any queued startup viewers close, so a completed load is neither installed behind nor discarded because another viewer is active ([`main.rs:1063-1077`](crates/clip-workbench/src/main.rs#L1063)).

After modal routes have cleared, a successful Spreadsheet or MIDI result becomes the active viewer; a text-table load failure falls back to Text, while other failures become status text ([`main.rs:1812-1858`](crates/clip-workbench/src/main.rs#L1812), [`main.rs:1850-1898`](crates/clip-workbench/src/main.rs#L1850)). A newer request cancels its predecessor, and app drop cancels outstanding tickets ([`main.rs:1798-1810`](crates/clip-workbench/src/main.rs#L1798), [`main.rs:1024-1032`](crates/clip-workbench/src/main.rs#L1024)).

## Unresolved or source-unspecified behavior

- If a startup audio preview cannot be constructed, the failed image preview is omitted, or both command-line preview arguments are supplied with a higher-precedence direct route, source establishes which session is selected but does not define a user-facing notice for skipped/failed paths ([`main.rs:504-515`](crates/clip-workbench/src/main.rs#L504), [`main.rs:543-555`](crates/clip-workbench/src/main.rs#L543)).
- A duplicate process invocation only focuses the existing popup; source does not specify whether users expect its new arguments to be ignored permanently or eventually forwarded ([`popup.rs:68-73`](crates/clip-workbench/src/popup.rs#L68), [`main.rs:2504-2506`](crates/clip-workbench/src/main.rs#L2504)).
- Async loading currently applies to Spreadsheet and MIDI routes; it is a pair of host-owned jobs rather than a general route queue. Results wait through active viewers, prompts, and the startup viewer queue, then become the next viewer after that queue drains. They do not wait behind a nonmodal workbench view because `update` polls them after the modal check even when Node Machine or Auto Actions is selected ([`main.rs:1063-1095`](crates/clip-workbench/src/main.rs#L1063), [`main.rs:1683-1687`](crates/clip-workbench/src/main.rs#L1683), [`main.rs:1812-1858`](crates/clip-workbench/src/main.rs#L1812), [`main.rs:1850-1898`](crates/clip-workbench/src/main.rs#L1850)).
- Per-viewer Escape, save, and discard semantics are owned by each viewer implementation and are not uniform. The route manager only observes a close flag and performs route cleanup described above ([`main.rs:1605-1688`](crates/clip-workbench/src/main.rs#L1605)).
