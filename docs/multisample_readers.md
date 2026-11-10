# Multisample reader choices

The inspector needs source mappings and a useful audio preview, rather than a
converter or a complete sampler workstation. Format readers must retain the
file manager's guarded input handling, cancellation and bounded worker budgets.
Instrument scripts and external resources are never executed automatically.

## Playing the previews

List rows use equal-width chromatic keys. Pitch-class colors repeat each octave;
recorded roots are saturated, repitched coverage is muted, and unmapped keys are
gray. Click or drag high on a key for higher velocity, low for lower velocity.
Velocity selects the matching recording and applies a quadratic amplitude curve,
so a single-layer instrument still responds audibly. Escape, Space, navigation,
hiding the row and studio entry stop inline audition.

Mapping is the default inspector view, plotting key against velocity alongside a
conventional piano reference. Harmony and Pads remain available; instrument-type
hints do not override the default. The Keys view has been removed. Pads audition
named recordings at their original pitch. Clicking a velocity layer auditions its
midpoint; repeated single notes cycle overlapping recordings in source order.
This exposes source recordings without reproducing articulation scripts.

Harmony is a pannable fifth/third lattice, drawn only within the viewport. Wheel
zoom anchors at the pointer; right or middle drag pans. Unmapped pitches and cells
outside MIDI's range are gray. Hover outlines every exact pitch in the chosen
chord; the piano highlights sounding notes. Number shortcuts select major (1),
minor (2), diminished (3), augmented (4), or half-diminished (5). 6 cycles inversions,
7 adds the quality's seventh, 8 toggles open voicing, 9 adds a lower root octave,
and 0 adds an upper root octave. Choosing half-diminished enables its minor seventh; 7 can subsequently toggle it.
Missing mappings are reported rather than silently omitting chord voices.

MIDI input follows the last clicked multisample strip, and disconnects when a
click lands elsewhere. The open inspector receives MIDI automatically. Controllers
are connected non-exclusively (an optional `LITTLE_CLIP_MIDI_INPUT_PREFIX` limits
input to a device-name prefix) and reconciled for hotplug on a worker shared in
implementation with the constellation. Note-on velocity, note-off, sustain and
all-notes-off are channel-aware. Each focused instrument has at most 16 voices and
eight cached decoded regions; pending notes cancel on release or loss of focus.
Queue overflow or a lost connection retires held voices instead of leaving notes
stuck. Cold recordings decode on the existing bounded jobs; subsequent strikes
share the decoded PCM. Audio output reserves headroom for polyphony.

Pitch preview uses playback-rate transposition from the recorded root. Mapping
can disable it to hear original recordings. Effects, envelopes, loops, fine tuning
and instrument scripts are not synthesized; related source metadata remains
inspectable. Decoded input and playback are bounded to ten seconds. Chords reuse
each distinct decoded region once and mix on the cancellable worker.

## Implemented readers

| Format | Alternatives examined | Current choice and reason |
| --- | --- | --- |
| SFZ | [sfizz](https://github.com/sfztools/sfizz), a C++ parser/synthesizer; [SoFiZa](https://github.com/andamira/sofiza), a Rust parser whose documented coverage targets particular libraries | Keep the original bounded mapping reader. The textual mapping subset is small, retains unknown opcodes for inspection, and shares guarded include resolution with the file manager. A synthesis engine would bring broader playback behavior and a separate resource-loading integration; it remains an option if instrument rendering is added. |
| SoundFont 2 | [RustySynth](https://github.com/sinshu/rustysynth), [OxiSynth](https://github.com/PolyMeilex/OxiSynth), and the [published SoundFont specification](https://www.synthfont.com/sfspec24.pdf) | Keep the original bounded RIFF/table reader for mapping inspection and selected PCM audition. It resolves preset/instrument ranges and sample offsets without constructing a synthesizer. This does not implement the complete generator/modulator or effects model; the native synthesizers are preferable candidates for that larger feature. |
| Kontakt NKI | [ni-file fork](https://github.com/Ma5onic/ni-file), [KoEd](https://github.com/zhangdoa/KoEd), and [ConvertWithMoss](https://github.com/git-moss/ConvertWithMoss) | A separate native LGPL executable adapts only the latter's required read paths. The reviewed ni-file fork (`1b7a518243125857fddec8217167b47a35cb58fa`) has neither a package license declaration nor a root license file; its code was not adopted. KoEd documents NICNT library-container editing rather than instrument-zone reading. The native helper is checked against legacy and modern mapping references. See its [source/license and coverage notes](../crates/little-clip-nki/README.md). |

SFZ/SF2 readers are original application code, not translations of the converter's
readers. The NKI/EXS adaptations live entirely in `little-clip-nki`; application crates
do not depend on or link that crate. JSON is the process boundary. The delivered
helper includes its independently rebuildable source and LGPL notices. Packaging
does not include a JVM, Java libraries or the converter's other format readers.

Current audition plays a bounded excerpt from one selected source recording.
It does not apply Kontakt scripts, group conditions, instrument effects or pitch
processing. Unsupported/protected data must produce a clear limitation or error.

## Additional native readers

The following readers use the same bounded mapping model and explicit audition path.

| Format | Evidence and alternatives | Intended approach |
| --- | --- | --- |
| NCW sample audio | [ncw](https://github.com/monomadic/ncw) is a zero-dependency Rust block decoder with explicit MIT/Apache licenses; the converter also has Java audio readers | Use the native crate with owned mono/stereo PCM fixtures. Preflight headers before decoding; its whole-file API has a 16-million-interleaved-sample preparation limit. Shared prepared WAV snapshots serve playback, waveform analysis and audio exports. No Java codec is required. |
| Bitwig/PreSonus `.multisample` | [Official format](https://github.com/bitwig/multisample) specifies ZIP plus `multisample.xml`; [dot-multisample](https://github.com/g-s-k/auto-sampler/tree/main/dot-multisample) supplies MIT/Apache Serde model bindings; the converter supplies another implementation | The original mapping reader reuses existing bounded XML/ZIP readers and retains raw source attributes for inspection. The bindings provide a domain model rather than archive/resource admission and would require a second representation alongside those source attributes. The small public schema justifies the shared XML reader here. Embedded samples are decoded lazily, with bounded reads and no extraction into user folders. |
| DecentSampler `.dspreset` / `.dslibrary` | [Official developer guide](https://decentsampler-developers-guide.readthedocs.io/en/latest/) specifies the XML preset schema; the converter supplies a Java reader as another implementation | Use an original reader of the documented groups/sample mapping subset, including inherited ranges and archive-local members. UI scripts and effects remain inert. Owned fixtures check inclusive end conversion, inheritance and library-relative sample resolution. |
| Logic `.exs` | [EXS2DS](https://github.com/DecentSamples/EXS2DS) is MIT but delegates parsing to [DSPresetConverter](https://github.com/DecentSamples/DSPresetConverter), whose reviewed checkout has no separate license declaration; the converter has a separate LGPL binary reader | Use the converter's bounded read-only EXS subset inside the separate LGPL executable. Support little/big-endian headers, zone/group/sample indices, source paths and loop metadata. The alternative wrapper's license does not establish its submodule's license. `.exs` also denotes Elixir scripts, so a worker verifies binary framing before selecting this viewer. |
| Renoise `.xrni` | Vendor describes XML plus embedded samples in its [format discussion](https://forum.renoise.com/t/is-there-a-way-i-could-get-a-copy-of-the-file-formats-xrns-xrni/48894/2); [chirashi](https://github.com/g-lok/chirashi) provides a native Go alternative focused on sliced instruments | Use an original bounded XML reader and retained ZIP handle. Bind sample indices to exactly one SampleData member, never an external FileName hint. Slice aliases use the primary sample and marker-relative intervals; ambiguous/missing associations fail. The Go converter targets slice conversion rather than a general source mapping inventory. |

## Detection policy

Specific extensions are candidates for a reader, not proof of valid content.
SF2 must have a SoundFont RIFF form; NKI must have recognized Kontakt/container
framing. Archive readers must verify their particular manifest and root schema.

Generic `.xml`, `.preset`, `.iso`, `.img` and similar names must keep their existing
handling unless a bounded worker positively identifies a supported sampler
structure. A filename such as `preset.xml`, an XML declaration, ZIP magic, or an
ordinary ISO filesystem signature is insufficient. Unknown variants must fall
back to their ordinary viewer; no generic disk image should become an instrument
because its extension appears in a converter's supported-format list.
The same applies to `.exs` (Elixir scripts) and `.xpm` (X PixMap images): sampler
support must positively distinguish those contents before changing their viewer.

## Deferred formats and limits

TX16Wx XML formats remain deferred pending schema/example validation. Generic Akai
`.xpm`, device-specific `.preset`, disk-image `.iso` and other ambiguous containers
are not registered by extension. No hardware-volume probing or conversion is
performed. This batch does not promise all formats from a conversion suite.

XML manifests are capped at 4 MiB, 100,000 events/nodes and depth 128. Archive
indices share the existing ZIP bounds (8,192 entries and 4 MiB directory), with
2 GiB source archives and at most 64 MiB per selected audio member. Library
manifests share a 4 MiB expansion budget. Mapping inventories allow 256 instruments,
8,192 regions and 16 MiB of source metadata. Selected auditions retain at most ten
seconds or two million interleaved samples. Archive members stay inside the held
archive and never become filesystem extraction paths.
