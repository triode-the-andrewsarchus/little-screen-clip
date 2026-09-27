# Space Key Contract (H1.3b3)

This note records Space-key behavior as implemented in the workbench and viewer routes. It describes source behavior, not a proposed global shortcut policy. Source references are relative to the repository root and include current line numbers.

## Routing and precedence

`WorkbenchApp::update` handles appearance switching first, then calls `render_modal_viewers` before workbench shortcuts. If a prompt or viewer is active, that route renders and returns from `update`; workbench shortcuts and the rolling-audio strip are not reached in that frame. The same modal check runs again after asynchronous Spreadsheet and MIDI polling, because either poll can open a viewer (`crates/clip-workbench/src/main.rs:1043-1079`).

Within `render_modal_viewers`, the prompt modal is tested before the active `ViewerSession`. Viewer routes then call their own `render_ui`; MIDI has an outer Escape check, and the component gallery has its own Escape check (`crates/clip-workbench/src/main.rs:1584-1665`). This means Space ownership belongs to the active viewer while a viewer is open; the background Quick Palette and rolling strip do not also process it.

When there is no active modal viewer, workbench shortcuts run before the rolling-audio strip is rendered (`crates/clip-workbench/src/main.rs:1081-1095`). The central Space shortcut checks `ctx.wants_keyboard_input()` before toggling rolling-strip playback, while Quick Palette's empty-search Space selection checks the same focus condition (`crates/clip-workbench/src/main.rs:2049-2058`, `2130-2142`).

## Verified Space behavior by route

| Route / state | Space behavior | Focus and modal conditions | Source |
| --- | --- | --- | --- |
| Quick Palette, empty search, no rolling-audio selection | Selects Left Hand + Right Hand and resets selected action index to zero. | Only when the Palette view is active and `ctx.wants_keyboard_input()` is false. | `crates/clip-workbench/src/main.rs:2116-2127`, `2130-2142`, `2162-2165` |
| Quick Palette, empty search, rolling-audio region selected | Does not change slot selection; central workbench handler toggles rolling-strip playback instead, unless keyboard input is wanted. | The central playback handler is usable from any workbench view, but requires an active region and no keyboard input. | `crates/clip-workbench/src/main.rs:2049-2058`, `2141-2142` |
| Quick Palette, search text begins with a literal space | On the subsequent handler pass, a leading space selects Left Hand + Right Hand if no rolling region is selected, then all leading spaces are trimmed from the query. | This prefix path is chosen whenever `search_query` is non-empty and does not itself check `ctx.wants_keyboard_input()`. The preceding empty-query path returns while a text widget wants keyboard input, allowing the edit to receive Space first. | `crates/clip-workbench/src/main.rs:2116-2127`, `2135-2139`, `2170-2177` |
| Rolling Audio strip, selected region | Toggles strip playback: when playing, `toggle_playback` stops; otherwise it calls `play_selection`. Playback requires a selected region longer than 0.05 seconds, a non-silent buffer, and a source path. | Workbench-level handler requires a selected region and no keyboard input. The strip itself has no Space key handler; its `[Space] Play` footer is a hint. | `crates/clip-workbench/src/main.rs:2049-2058`; `crates/clip-workbench/src/rolling_audio_strip.rs:291-313`, `402-409`, `586`; footer text is not a separate handler. |
| Audio Preview viewer, Inspect mode | Space calls `toggle_play` (play/pause). | The handler does not gate on `wants_keyboard_input`; the code clears focused widget state immediately before its tool shortcuts, and the Space check itself is unconditional. The modal viewer route prevents the workbench handler from also running. | `crates/clip-workbench/src/audio_preview/tools/inspect.rs:321-347`; route `crates/clip-workbench/src/main.rs:1605-1611` |
| Audio Preview viewer, HF Analysis mode | Space pauses if playing, otherwise starts playback. | Explicitly gated by `!ctx.wants_keyboard_input()`. | `crates/clip-workbench/src/audio_preview/tools/hf_analysis.rs:294-301` |
| Audio Preview viewer, Digital Decode mode | Space pauses if playing, otherwise starts playback. | Explicitly gated by `!ctx.wants_keyboard_input()`. | `crates/clip-workbench/src/audio_preview/tools/digital_decode.rs:285-292` |
| Audio Studio, Stem Separation mode | Starts stem separation if no stems exist and no separation is in progress. Space does not pause playback here. | Requires `!ctx.wants_keyboard_input()` and no save dialog; it does nothing when stems already exist or separation is in progress. | `crates/clip-workbench/src/audio_preview.rs:3160-3189` |
| MIDI viewer | If playing, pauses; otherwise plays the selected region (when over 0.05 seconds) or plays from the current playhead through the rest of the MIDI. | Space handler has no `wants_keyboard_input` check. The viewer is modal, so workbench Space handlers are preempted. Outer Escape handling runs before the MIDI UI renderer. | `crates/clip-workbench/src/midi_view.rs:568-575`, `257-273`; route `crates/clip-workbench/src/main.rs:1629-1644` |
| Image Studio and image tools | No Space handler was found. | Image keyboard shortcuts are generally gated by `image_shortcuts_enabled` (no save modal and egui does not want keyboard input); tool switching uses letter keys. | `crates/clip-workbench/src/image_preview.rs:816-824`, `914-1034`; full source search for `Key::Space` found no image-preview handler. |
| Other viewer routes | No Space handler was found for PDF, Text, Spreadsheet, Diff, Binary Diff, Model 3D, or Component Gallery. | Active viewers still preempt workbench shortcuts. | `crates/clip-workbench/src/main.rs:1594-1665`; source search for `Key::Space` across `crates/clip-workbench/src`. |

## Conflicts and gaps in the current contract

- Space is context-dependent: with an empty, unfocused Quick Palette search it selects both hands, while an active rolling-audio selection routes it to playback. In the focused search case, a typed leading space is later interpreted as the same slot-selection prefix (if no audio region is active) and stripped from the query. The code therefore gives Space both text-entry and command meaning across frames.
- Focus policy differs between viewer modes. HF Analysis and Digital Decode suppress Space while egui wants keyboard input; Inspect and MIDI do not. The Audio Studio stem action does suppress it while text input or its save dialog is active.
- Audio Studio uses Space to start a stem job only in the empty, idle stem state. The source does not define Space as a general start/retry/pause shortcut for that studio.
- Image Studio has no Space action despite many other tool shortcuts. This is an observed absence, not evidence that a future global Space meaning should apply there.
- The rolling-audio footer advertises `[Space] Play` when a region is selected, but workbench handling is the actual implementation. It toggles playback and is disabled while keyboard input is wanted; very short/invalid selections can make the attempted play a no-op.

## Precedence decisions for implementation

- Preserve modal route ownership: active viewers handle Space and the workbench underneath never receives the same event. Do not add a global Space action to image, PDF, Text, Spreadsheet, Diff, Binary Diff, Model 3D, or Component Gallery.
- Preserve Quick Palette's leading-space prefix as its command for selecting both hands. The search box is a command filter, and the prefix is an existing shortcut; a leading space intentionally executes the command instead of remaining query content.
- Keep rolling-strip Space limited to an active selection and no focused text input. If a selection is too short or the buffer cannot play, preserve the current no-op behavior; availability remains explained by the strip's state and hint.
- Preserve mode-local behavior where no text entry is active: Audio Inspect toggles playback and MIDI plays/pauses. HF Analysis and Digital Decode yield Space while a text field owns keyboard input; Stem Separation starts only when idle, empty, and outside a save dialog. Add focused regression coverage before changing these rules.

These differences are intentional under the contract: Space remains a local transport or command key and never becomes a workbench-wide action while a viewer owns the frame.
