# Constellation drum rack

The map uses the file manager canvas and keeps its existing acoustic grouping.
Its default scale fits the observed coordinate extent, including samples outside
the unit square. Category colors identify samples, rather than tinting the panels.
Indexing uses the same themed progress-bar treatment as Image Studio depth work.

Rack and Packs are tabs in the right panel. Packs never opens a floating browser.
Selecting a pad exposes its layer editor below the map; resize or hide that editor
to give the map more room. Only this pad's layers are expanded. The footer holds
MIDI input, velocity override, and pad-output dynamics.

## Keyboard configuration

The default keyboard rows are, from bottom to top:

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
run on a dedicated worker. Refresh inputs after plugging in or removing a device.
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

Each pad layer has 2 dB of input headroom. Glue uses linked stereo 2:1 compression,
a threshold of 0.85, 5 ms attack, 120 ms release, and no makeup gain. A linked peak
ceiling at 0.98 catches transient overloads. A normal full-scale single layer at
default gain stays below the compressor threshold. Layered or boosted samples can
engage it. The footer shows gain reduction.

Bypass disables compression and the ceiling while keeping input headroom unchanged.
Choke groups, velocity override, and bypass are saved with the pack. The bus applies
to rack playback; the map's browser audition and individual layer/library auditions
retain their existing preview players. Pack saves remain source references and
trim/gain metadata, not processed audio exports.

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
