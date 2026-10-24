# Constellation drum rack

The map uses the file manager canvas and keeps its existing acoustic grouping.
Its default scale fits the observed coordinate extent, including samples outside
the unit square. Category colors identify samples, rather than tinting the panels.
Indexing uses the same themed progress-bar treatment as Image Studio depth work.

Rack, Packs, and Output are tabs in the existing-width right panel. Packs stays inline.
Selecting a pad exposes its layer editor below the map; resize or hide that editor
to reveal more of the map. Docks obscure the full map canvas without moving or
rescaling its points, including at zoom levels with point relaxation. The editor
shows one layer at a time. Its layer tabs show truncated sample filenames, each
with its own close button. Closing an inactive layer keeps the current layer
selected. The Editor toggle sits beside Undo/Redo; Override MIDI velocity
lives in Rack, and all dynamics controls live in Output. There is no output footer.
Pads become square when height permits and compact vertically in small windows.
The sidebar retains its existing default and width limits, and only the user's
divider drag changes its width. The full-height minus rail on the right collapses
both rack and editor to a thin rail. Click anywhere on that rail to reopen them; editor visibility
and rack width survive collapse.

## Keyboard configuration

The default rack is arranged as follows (the configuration stores rows bottom to top):

```text
1 2 3 4
Q W E R
A S D F
Z X C V
```

Press a key to play a pad; Shift+key adds the selected map sample as a layer.
Click a pad to select and play it, or Shift+click to assign the map selection.
Text fields keep their normal keys. Ctrl/Alt/Command combinations do not play pads.

Space retriggers the active editor layer when its editor is visible, otherwise the
selected map sample. Shift+Space, Next unplayed, and N (when N has no pad binding)
select and play
the nearest filtered sample not auditioned since the last map click. Hover and
Random auditions also mark samples visited. Once all matching samples have been
visited, advancing leaves the selection unchanged. Clicking a sample starts a new
exploration; folder changes and index reloads clear it too. This visited set is independent of the
short, fading playback trail.

Set four strings of four characters under `[file_manager]` in `config.toml`:

```toml
[file_manager]
# Rows are stored bottom to top, left to right.
constellation_pad_keys = ["zxcv", "asdf", "qwer", "1234"]
```

For the hardware-remapped Dvorak arrangement described in the review:

```toml
[file_manager]
constellation_pad_keys = [";qjk", "aoeu", "',.p", "1234"]
```

Bindings use logical characters before Shift/Caps Lock, including punctuation.
They do not assume physical QWERTY key positions or select a named layout preset.
Use 16 distinct ASCII letters, digits, or punctuation characters; letters are
case-insensitive. Invalid row lengths and duplicate bindings reject that config
with the application's normal configuration diagnostic. Restart Files after
editing the file. Local `./config.toml` takes precedence over the user config in
`$XDG_CONFIG_HOME/little-screen-clip/config.toml`.

Bindings are local preferences, independent of saved packs. Older ten-slot packs
retain their sample positions and acquire six empty pads when loaded. Their old
`key_char` metadata does not override local bindings.

## MIDI and pad behavior

All available MIDI inputs connect automatically and non-exclusively. A dedicated
worker reconciles port IDs every two seconds, retaining unchanged connections and
closing removed ports off the UI thread. Ready pads trigger directly from callbacks
using shared playback state; repaint timing does not schedule audio. Output shows
a compact MIDI-offline hint with backend details on hover when input is unavailable.
Pad labels start as configured keyboard keys. MIDI note-on switches them to note
names (C2 through D#3); the next keyboard audition switches back to keyboard keys.
Notes 36–51 map consecutively left to right, bottom to top; the lower-left pad is
note 36 and the upper-right pad is note 51. All MIDI channels are accepted.

Pads are one-shots. Note-off and note-on with velocity zero do not start or choke
anything. Positive velocities scale layer gain; Override MIDI velocity forces 127.
Mouse and keyboard hits already use 127. Repeated hits replace that pad's previous
voices; different pads can ring together.

Hold Shift and right-click a pad to start a mute-group chain. Right-click further
pads while keeping Shift held to join them. The chain adopts the first existing
group it encounters or chooses an unused group (up to eight). Release Shift or
press Escape to finish. Hover a grouped pad to see red links to its partners;
G1–G8 mark membership. The hint names the linked keys; hold Alt for detailed pad
information without covering those links during ordinary hover. Right-click → Leave mute group removes one member.
Triggering a grouped pad stops its own old voices and the other pads in that group.
Ungrouped pads overlap independently. Navigation, Escape, studio entry, and
application close stop previews; queued MIDI hits from earlier view epochs cannot
restart them. MIDI connections stay open while their note delivery is gated.

## Pad output

Ready layers feed one stereo 48 kHz software bus connected to the application's
existing shared device sink. This is an internal mixing rate; hardware buffer
sizes and backend selection remain unchanged. Playback shares cached PCM rather
than copying it when a pad is hit.

Every audition and pad layer has 2 dB of input headroom. Glue uses linked stereo
compression with an adjustable threshold (-48 to 0 dBFS) and ratio (1:1 to 12:1).
Defaults preserve the previous subtle behavior: about -1.4 dBFS (0.85 linear),
2:1 ratio, hard knee, 5 ms attack, 120 ms release, and 0 dB makeup gain.
Knee adjusts the transition width from 0 to 24 dB; Makeup provides -12 to +24 dB
before the peak ceiling. Lower the threshold
and raise the ratio for more audible compression. A linked peak
ceiling at 0.98 catches transient overloads. A normal full-scale single layer at
default gain stays below the compressor threshold. Layered or boosted samples can
engage it. Output provides a
transfer curve with draggable threshold and ratio handles, editable sliders, input
and output peak meters, and gain reduction. The live dot includes attack/release
and the safety ceiling; the static line includes the knee and makeup gain before
the ceiling. Double-click any parameter slider to restore its default.

Bypass disables compression, makeup gain, and the ceiling while keeping input
headroom unchanged. A 1:1 ratio retains makeup gain and the peak ceiling. Threshold,
ratio, knee, makeup gain, choke groups, velocity
override, and bypass are saved with the pack; older packs inherit the defaults. The bus also
protects map, individual-layer, and saved-pack auditions. Map auditions retain the
guarded streaming loader, so long samples are not truncated to pad-cache length.

## Editing and persistence

Drop samples onto pads to replace their layers; Shift+drop adds layers instead.
A pad holds up to eight layers. Drops collect references, never move the source.
Worker decoding must succeed for every dropped sample before assignment, and a
pad edited during that load rejects its stale completion. Unavailable sources show
a compact error dot and offer Retry audio; loading and playing have transient cues.
Adding the same file to multiple layers of a pad asks for confirmation. Add anyway
keeps the duplication; Cancel leaves the pad untouched. Drops retain their native
completion until that decision and reject changes to the target while waiting.
Pads list layer-name prefixes, with an ellipsis when space cannot show all layers.
Group labels appear only for nonzero groups. Drag a pad onto another pad to swap
complete instruments, including layers, mute/solo and group membership. Keyboard
and MIDI bindings stay at their grid positions. Swaps stop affected voices, reject
stale drags, and participate in Undo.

The frequency-colored waveform shares the app's bass/mid/treble crossover. Drag
IN or OUT to trim, or double-click the waveform to reset its range. Handles are
inset from the edges; any overflow scrollbar occupies a separate gutter. Audio
preparation computes up to 16,384 waveform buckets off the UI thread, and their
storage counts toward the existing cache budget. The canvas reduces these to the
actual displayed columns. Wheel zooms around the pointer; Shift+wheel pans. The
canvas shares Audio Studio's viewport model and time ruler. Tick spacing
also accounts for available width; envelope guides use the playback fade model.

Sound controls sit above the waveform at every editor width; a compact grid keeps
gain, pan, attack and release visible in both small and large windows. Trim uses
the waveform's IN/OUT markers, without duplicate numeric endpoints or hint text.
Gain is a dB fader with a silent lower stop;
pan shows Center/L/R. Attack/release sliders have finer logarithmic control near
zero; values remain editable, and double-click resets controls to neutral.
The selected pad exposes marker-based trim, gain, stereo balance, and linear attack and
release fades (0–500 ms). Mute and Solo gate active pad voices without restarting
them. Layer changes affect subsequent hits. Fades follow the trimmed boundaries;
overlapping fades use the lower envelope. Center balance preserves existing stereo
level; mono is duplicated before balance. These settings also apply to exports.

Double-click a populated pad to open its first layer in Audio Studio with the pad's
trim selected. The direct Edit in Audio Studio button opens the selected layer.
Returning offers Save copy and replace pad, Keep editing, or Return without
replacing. Saves use the existing guarded audio export worker and publish a new
`name.edited.wav` beside the original; collisions become `name.edited-1.wav`, etc.
Publication is atomic and never overwrites an existing sibling. The grid changes
only after successful publication, and only if the originating bank/layer still
matches. Other layers and pad gain/pan/fades remain intact; the new file starts with
full-range trim. Failed or stale handoffs retain the saved file and prior pad.

The top toolbar always shows a two-ended duration slider, bounded by the shortest
and longest indexed samples, and an outlined Mono samples only toggle. Double-click
the duration track to reset its range. Category buttons retain subtle outlines and
use the short labels CH, OH, and Bass. Active categories keep their normal fill
and text, with their category color on the border. Hover audition, All and Mono
also use rounded outlines rather than bright selected fills. The bottom status bar reports active / total
samples. Re-indexing remains in File; there is no Map menu.

Every visible sample gets a mesh marker of the same size at any given zoom.
The radius depends on the whole library size and camera zoom, so sparse folders
remain readable without turning large libraries into a solid wall. Neither local
density nor opening a dock changes point size. Ordinary zoom levels preserve the
original projection. Only the final four zoom steps allow bounded local spacing:
each colliding point checks at most 48 nearby cells and moves at most 64 logical
pixels. Saturated neighborhoods retain overlaps instead of pushing clusters into
holes. Saved acoustic coordinates remain unchanged; no samples are dropped.
Empty searches avoid per-sample lowercase allocation on the render path.

Undo/Redo keep 32 metadata snapshots and coalesce pointer gestures/text editing.
Ctrl+Z undoes, Ctrl+Y or Ctrl+Shift+Z redoes; text editors retain their own undo.
The rack indicates Saved/Unsaved changes. Loading another pack or closing Files
with unsaved edits opens an inline Save and continue/Discard/Keep editing bar.
Saving records the submitted snapshot; edits made during a save stay unsaved.

Save pack writes a portable, versioned folder automatically. Edited layers render
their trim, gain, balance, and fades into stereo 32-bit float WAVs; their complete
unmodified inputs are copied into originals/. Unedited layers retain one
byte-identical source copy. The relative-path bank manifest makes the kit movable
without source paths or symlink support. Bus compression is not baked into layers.
Saving records the submitted rack snapshot as Saved; later edits remain unsaved.
Staging retains pinned inputs and output directories and installs atomically
without overwriting existing versions. Accepted writes survive navigation and
defer application close. Discard approval survives the backend's final close event.

## Indexing and filtering

File → Re-index folder into sample constellation inventories the complete tree and reuses classified observations
whose device/inode, size, and nanosecond mtime/ctime still match. New or changed
files are analyzed; disappeared files leave the next committed snapshot. Progress
and completion show reused and newly analyzed counts. Legacy rows without precise
revision keys, and previously unclassified files, are analyzed again. Discovery or
analysis failures and cancellation retain the prior snapshot.

Duration range, mono-only, category, and filename filtering share one predicate
for painting, picking, Random, Next unplayed, and the status count. A maximum
duration of zero internally means no upper limit. Duration endpoints clamp to the
observed library bounds when folders change. Filtered stars remain dim for context.

## Verification

Regression coverage includes ten-to-sixteen-pad migration, settings persistence,
duplicate/invalid keyboard bindings, layout stability across frames, inline pack
browsing, logical punctuation and expired input, MIDI mapping and stale epochs,
velocity override, independent retriggering, choke groups, and linked dynamics.
Device-free mixer tests check actual output samples without opening hardware.

The private Weston/Niri acceptance scenario exercises indexing, actual star clicks,
keyboard assignment/retriggering, duplicate confirmation, pad swaps and mute links,
Studio return, rack collapse/reopen, output controls, portable versioned saves,
inline browsing, and quitting after discard in both appearances. `--hardware-keys` installs the hardware-remap
fixture and verifies remapped logical punctuation during pad assignment. Its audio sink is ALSA
null. Physical MIDI-controller interoperability, physical ALSA behavior, and
perceptual compressor quality still require listening with the user's equipment.
