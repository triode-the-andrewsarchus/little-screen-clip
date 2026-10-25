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
lives in Rack, and all dynamics controls live in Output. Velocity override sits below the pad grid.
Velocity override, Mute, Solo, Editor,
Bypass and saved-pack selection share the neutral fill and rounded active-outline
treatment used by the map filters. There is no output footer.
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
Ordinary clicks reveal its layers in round-robin order, independently for each
pad, and select the matching editor tab. The map centers that sample in the
visible canvas without changing zoom. A layer absent from the current folder's
index reports that fact rather than inventing a point; index that location to map
it. Double-clicks behave as repeated pad clicks, with no Studio handoff.
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
Mouse and keyboard hits already use 127. The selected pad's square R button is on
by default: repeated hits stop its previous voices and restart its layers. Turn R
off to let successive hits overlap. Other pads can ring independently in either
mode. Overlap is bounded to 128 layer voices per pad; the oldest voices retire
when that limit is reached. R is saved per pad, including in Undo snapshots;
older packs default to retriggering.

Hold Shift and right-click a pad to start a mute-group chain. Right-click further
pads while keeping Shift held to join them. The chain adopts the first existing
group it encounters or chooses an unused group (up to eight). Release Shift or
press Escape to finish. Hover a grouped pad to see red links to its partners.
Links are drawn behind every pad face, so they never cross pad labels or contents.
G1–G8 mark membership. The hint names the linked keys; hold Alt for detailed pad
information without covering those links during ordinary hover. Right-click →
Leave mute group removes one member. A group with fewer than two
members dissolves automatically, including after moving a member to another group
or clearing a pad; Undo restores the partners together.

New groups assign priority by click order: the first pad is High, the second
Medium, and later pads Low. Newcomers to an existing group start Low; existing
priorities stay as edited. The selected-pad header shows its group and priority in one compact menu, with
High, Medium and Low choices; right-clicking a pad exposes the same priority menu.
Pad badges and group hints show the current level. A ringing
higher-priority pad blocks lower-priority hits. Equal priorities use the latest
hit, which cuts off other ringing members. Muted, solo-gated and completed voices
do not block hits. R determines whether the pad's own previous voices stop too.
Priorities are independent per pad, so multiple pads may share a level. Old packs
without priority metadata default to equal Low priorities, retaining latest-hit
behavior; the brief numeric 0–7 format migrates to the three levels.
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
Attack adjusts the response from 0.1 to 200 ms; Release adjusts recovery from 10
to 2000 ms. These use logarithmic sliders and update active playback without
restarting voices. Older packs retain the 5 ms / 120 ms defaults. Non-finite live
edits retain the last valid timing. Knee adjusts the transition width from 0 to
24 dB; Makeup provides -12 to +24 dB
before the peak ceiling. Lower the threshold
and raise the ratio for more audible compression. A linked peak
ceiling at 0.98 catches transient overloads. A normal full-scale single layer at
default gain stays below the compressor threshold. Layered or boosted samples can
engage it. Output provides a
transfer curve with draggable threshold and ratio handles, editable sliders, input
and output peak meters, and gain reduction. The curve overlays three seconds of
single-band gain-reduction history as an amber trace, without input/output bars. The audio mixer captures peaks every 256 frames,
so short hits remain visible between repaints. The static transfer line includes
the knee and makeup gain before the safety ceiling. Double-click any parameter
slider to restore its default.

Bypass disables compression, makeup gain, and the ceiling while keeping input
headroom unchanged. A 1:1 ratio retains makeup gain and the peak ceiling. Threshold,
ratio, knee, makeup gain, attack, release, choke groups, velocity
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

A compact mixer strip sits to the left of the waveform at every window size. Its
150-pixel vertical Gain fader has dB ticks and a silent lower stop; double-click
resets it to 0 dB. Pan is the only knob beside it, with a center detent and
reset to center on double-click. Retrigger sits below the left edge of the waveform,
with Edit in Audio Studio at the right edge. Independent Speed and Pitch faders
share that strip, separated into logical sections; narrow editors use a second
compact row for the faders. Each fader has a fixed-width editable readout.
Hover Pan to see its value. The editor defaults to 290 pixels high
and remains resizable. The waveform and direct Studio button use the remaining
width without displacing the mixer.

Trim uses only the waveform's IN/OUT markers. Smaller A and R flags above those
rails set attack and release (0–500 ms); drag A right or R left to lengthen a fade,
or double-click the flag to reset it. Crowded flags use separate lanes so both
remain reachable. The envelope guides show the resulting fades, including after
trimming; overlapping fades use the lower envelope. No duplicate numeric trim
fields or selected-duration text occupy the editor.

Retrigger controls restarting for the entire selected pad.
Speed and Pitch apply independently to the selected layer. Speed ranges from
−2× to +2×: negative rates play the trimmed sample backwards, zero silences it,
and +1× is normal playback. Its center detent is zero; double-click resets to +1×.
Speed changes duration and pitch together. Pitch shifts the sound by up to two
octaves in either direction while preserving the frame count. It adjusts in single
cents (1200 per octave), with a center detent and double-click reset to zero.
Readouts show cents between octave stops and octaves at −2, −1, 0, +1 and +2.
Drag a readout for fine adjustment or type a value with `ct` or `oct` for pitch.
Changing pitch shows Preparing pitch until a bounded worker has prepared its PCM;
that layer does not play stale pitch data while preparation is pending. Large
shifts can color transients; this preview pitch shifter has no formant controls.

Old P/T packs retain their sound: Time settings migrate to playback speed with
zero independent pitch. Previously saved rates above +2× (up to +4×) stay intact
until edited; new speed adjustments are restricted to ±2×.

Prepared pitch PCM shares the existing bounded audio cache and is invalidated by
source or pitch changes. Preparation uses the existing FFT dependency and runs off
the UI and audio callback threads. Gain, balance and fades apply after source
preparation. Mute and Solo gate active pad voices without restarting them. Center
balance preserves existing stereo level; mono is duplicated before balance. These
settings also apply to exports.

The direct Edit in Audio Studio button opens the selected layer with its trim
selected. Pad clicks stay in the constellation.
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

Every visible sample gets a circular dot of the same size at any given zoom.
One shared antialiased disk texture keeps the batched mesh at four vertices and
six indices per sample; circles do not add per-point CPU tessellation.
The radius depends on the whole library size and camera zoom, so sparse folders
remain readable without turning large libraries into a solid wall. Neither local
density nor opening a dock changes point size. Ordinary zoom levels preserve the
original projection. Only the final four zoom steps allow bounded local spacing:
each colliding point checks at most 48 nearby cells and moves at most 64 logical
pixels. Saturated neighborhoods retain overlaps instead of pushing clusters into
holes. Saved acoustic coordinates remain unchanged; no samples are dropped.
Empty searches avoid per-sample lowercase allocation on the render path.
A 28-pixel edge vignette fades map content into the themed canvas. It is painted
after map points/trails and before hover cards, within the visible map boundaries.
It neither moves points nor changes picking or panning. The center remains clear;
tiny viewports cap the fade to a quarter of their width/height.
Middle-drag pans even when it starts over a sample. It never begins a file drag,
and hover audition stays suppressed after panning until the next pointer movement.

Undo/Redo keep 32 metadata snapshots and coalesce pointer gestures/text editing.
Ctrl+Z undoes, Ctrl+Y or Ctrl+Shift+Z redoes; text editors retain their own undo.
The rack indicates Saved/Unsaved changes. Loading another pack or closing Files
with unsaved edits opens an inline Save and continue/Discard/Keep editing bar.
Saving records the submitted snapshot; edits made during a save stay unsaved.

Save pack writes a portable, versioned folder automatically. Edited layers render
their pitch or playback-speed adjustment, trim, gain, balance, and fades into
stereo 32-bit float WAVs; their complete
unmodified inputs are copied into originals/. Unedited layers retain one
byte-identical source copy. The relative-path bank manifest makes the kit movable
without source paths or symlink support. Rendered layer parameters reset to neutral
to avoid applying edits twice. Pitch resets to zero and playback speed resets to +1× after rendering. Zero-speed
layers must be given a nonzero speed before export; there is no finite playback
duration for a stopped layer. Legacy
layers default to zero pitch and +1× speed. Bus compression is not baked into layers.
Saving records the submitted rack snapshot as Saved; later edits remain unsaved.
Staging retains pinned inputs and output directories and installs atomically
without overwriting existing versions. Accepted writes survive navigation and
defer application close. Discard approval survives the backend's final close event.

## Indexing and filtering

When Constellation is active, changing folders retains it if ordinary audio exists
in the destination or its subfolders. An existing nonempty snapshot proves cached
audio presence; otherwise the existing guarded inventory checks recursively on a
background worker and stops at the first audio filename. It does not decode or
automatically index the folder. Hidden, symlink and generated-pack trees follow
the indexer's existing exclusion rules. The view stays put while checking; a
completed empty inventory permits normal view fallback. Errors do not prove
absence. Explicit mode changes still win, and superseded checks are canceled.

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

## Shared canvas vignette

`ui_ext::paint_canvas_edge_fade` is the shared desktop painter. It keeps the
28-pixel edge treatment from photo-similarity commit
`8826c084a6e1e6d5892da2f9e036223a3b234b30`, with eight gradient quads sharing 16
vertices. It clips to the painter and visible canvas; the clear center has no
triangles. It adds no widget or input target.

When updating the photo-similarity branch, remove its local
`file_manager::ui::gallery::paint_canvas_edge_fade` implementation and replace its
call with:

```rust
crate::ui_ext::paint_canvas_edge_fade(ui.painter(), clip_rect, colors.canvas, 28.0);
```

Keep that call after tiles/dividers/headers and before the search HUD. Keep the
photo canvas's own clipping tests; use the shared painter's geometry/bounds test
rather than retaining a second helper implementation or its old 32-vertex
assertion. The shared helper has the same arguments as the original, so no photo
layout or camera change is needed.
