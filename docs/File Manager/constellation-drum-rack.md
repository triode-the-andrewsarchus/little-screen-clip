# Constellation drum rack

The map uses the file manager canvas and keeps its existing acoustic grouping.
Its default scale fits the observed coordinate extent, including samples outside
the unit square. Category colors identify samples, rather than tinting the panels.
Indexing uses the same themed progress-bar treatment as Image Studio depth work.

Rack and Packs are tabs in the right panel. Packs never opens a floating browser.
Selecting a pad exposes its layer editor below the map; resize or hide that editor
to give the map more room. Only this pad's layers are expanded. The footer holds
MIDI input, velocity override, and shared output dynamics. The Editor toggle sits
beside Undo/Redo. Pad heights adapt to keep all sixteen visible in a compact rack.

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

Choose an input from MIDI input in the footer. Enumeration and connection teardown
run on a dedicated worker. Ready pads trigger directly from MIDI callbacks using
a shared playback state; repaint timing does not schedule their audio. Refresh inputs after plugging in or removing a device.
Notes 36–51 map consecutively left to right, bottom to top; the lower-left pad is
note 36 and the upper-right pad is note 51. All MIDI channels are accepted.

Pads are one-shots. Note-off and note-on with velocity zero do not start or choke
anything. Positive velocities scale layer gain; Override velocity forces 127.
Mouse and keyboard hits already use 127. Repeated hits replace that pad's previous
voices; different pads can ring together.

Choose Choke: 1–8 in the selected pad editor to put pads into a shared choke group.
Triggering a grouped pad stops its own old voices and the other pads in that group.
Choke: Off allows unrelated pads to overlap. Navigation, Escape, studio entry, and
application close stop previews; queued MIDI hits from earlier view epochs cannot
restart them. The MIDI connection stays selected while its note delivery is gated.

## Pad output

Ready layers feed one stereo 48 kHz software bus connected to the application's
existing shared device sink. This is an internal mixing rate; hardware buffer
sizes and backend selection remain unchanged. Playback shares cached PCM rather
than copying it when a pad is hit.

Every audition and pad layer has 2 dB of input headroom. Glue uses linked stereo 2:1 compression,
a threshold of 0.85, 5 ms attack, 120 ms release, and no makeup gain. A linked peak
ceiling at 0.98 catches transient overloads. A normal full-scale single layer at
default gain stays below the compressor threshold. Layered or boosted samples can
engage it. The footer shows gain reduction.

Bypass disables compression and the ceiling while keeping input headroom unchanged.
Choke groups, velocity override, and bypass are saved with the pack. The bus also
protects map, individual-layer, and saved-pack auditions. Map auditions retain the
guarded streaming loader, so long samples are not truncated to pad-cache length.

## Editing and persistence

Drop samples onto pads to replace their layers; Shift+drop adds layers instead.
A pad holds up to eight layers. Drops collect references, never move the source.
Worker decoding must succeed for every dropped sample before assignment, and a
pad edited during that load rejects its stale completion. Unavailable sources show
a compact error dot and offer Retry audio; loading and playing have transient cues.
Choke labels appear only for nonzero groups.

The selected pad exposes numeric trim, gain, stereo balance, and linear attack and
release fades (0–500 ms). Mute and Solo gate active pad voices without restarting
them. Layer changes affect subsequent hits. Fades follow the trimmed boundaries;
overlapping fades use the lower envelope. Center balance preserves existing stereo
level; mono is duplicated before balance. These settings also apply to exports.

Undo/Redo keep 32 metadata snapshots and coalesce pointer gestures/text editing.
Ctrl+Z undoes, Ctrl+Y or Ctrl+Shift+Z redoes; text editors retain their own undo.
The rack indicates Saved/Unsaved changes. Loading another pack or closing Files
with unsaved edits opens an inline Save and continue/Discard/Keep editing bar.
Saving records the submitted snapshot; edits made during a save stay unsaved.

Save pack retains source references and edit metadata in a versioned folder.
Export → WAV kit renders each layer's trim, gain, balance, and fades into stereo
32-bit float WAV files with a relative-path bank manifest. WAV kit + originals
also copies the complete unmodified input files. The exported kit can be moved to
another machine without the original paths. Glue is a live bus effect and is not
baked into individual layers. Exports do not change the current pack's Saved state.
Both formats stage under a pinned output directory and install atomically without
overwriting existing names. Accepted writes survive navigation and defer close.

## Indexing and filtering

Index folder again inventories the complete tree and reuses classified observations
whose device/inode, size, and nanosecond mtime/ctime still match. New or changed
files are analyzed; disappeared files leave the next committed snapshot. Progress
and completion show reused and newly analyzed counts. Legacy rows without precise
revision keys, and previously unclassified files, are analyzed again. Discovery or
analysis failures and cancellation retain the prior snapshot.

Duration limits, channel count, category, and filename filtering share one predicate
for painting, picking, Random, and Neighbor. A maximum duration of zero means no
upper limit. Filtered stars remain dim for spatial context. Reset filters clears
all four filter types.

## Verification

Regression coverage includes ten-to-sixteen-pad migration, settings persistence,
duplicate/invalid keyboard bindings, layout stability across frames, inline pack
browsing, logical punctuation and expired input, MIDI mapping and stale epochs,
velocity override, independent retriggering, choke groups, and linked dynamics.
Device-free mixer tests check actual output samples without opening hardware.

The private Weston/Niri acceptance scenario exercises indexing, actual star clicks,
keyboard assignment/retriggering, footer settings, versioned saves, inline browsing,
and navigation in both appearances. `--hardware-keys` installs the hardware-remap
fixture and verifies the apostrophe pad's saved assignment. Its audio sink is ALSA
null. Physical MIDI-controller interoperability, physical ALSA behavior, and
perceptual compressor quality still require listening with the user's equipment.
