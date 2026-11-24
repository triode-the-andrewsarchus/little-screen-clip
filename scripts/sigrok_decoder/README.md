# Saved logic decoder

Files can use this optional standalone libsigrokdecode adapter for saved logic
captures. Build it with `scripts/build_sigrok_decoder.sh` on a system with
libsigrokdecode development headers and pkg-config. Install the matching decoder
catalog with the runtime library. Files discovers the adapter in the workspace
target directory, next to the application executable, or on PATH. For a separate
installation, set `LITTLE_CLIP_SIGROK_DECODER` to its executable. An operator can
choose a trusted decoder directory with `LITTLE_CLIP_SIGROK_DECODERS`.

Use **Add decoder → Sigrok catalog…** in a saved logic capture or scope viewer.
Scope pins receive the digital lanes produced by the channel thresholds.
Choose an installed decoder by name or ID, map its required pins, and decode the
selection or visible window. Captures with a shared integral hardware sample
rate use that clock. Other captures require a sampling grid in Hz. The viewer reports off-grid
transitions, gaps, and bounded coverage. Click an annotation to inspect its
source span and use **Back to decoded window** to restore the timeline.

The searchable catalog lists up to 512 installed standalone logic-input
decoders with annotation output and up to seven required plus optional
channels in total. Protocol stacks and decoders with wider inputs are outside
this transport. This version uses the installed decoder's default options.
The adapter receives bounded packed samples over stdin, with a reserved gap
value. An unknown or floating interval retires the current decoder session.
The adapter receives no capture pathname or descriptor. Installed decoder
modules execute as local code within the bounded subprocess.

The executable links to libsigrokdecode under GPL-2.0-or-later. Its source, build
instructions and GPL notice accompany workspace builds. To rebuild manually,
compile `main.c` with `cc -O2 -std=c11 main.c $(pkg-config --cflags --libs libsigrokdecode) -o clip-sigrok-decoder`.
Follow the corresponding-source and license requirements of the selected
runtime library and catalog if distributing them alongside the helper.
