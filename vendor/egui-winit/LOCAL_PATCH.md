This is egui-winit 0.37.0, licensed under MIT or Apache-2.0. The local behavior
lives in `src/local_input.rs`, called from upstream's keyboard translation.

- The Files list and the audio/MIDI strips opt into raw clipboard shortcuts only
  while focused. This skips the native text read for file paste. A focus shortcut
  such as Ctrl+F restores text clipboard handling for the rest of the input batch.
  Upstream's clipboard switch controls the entire viewport.
- Constellation subscribes to a bounded queue of unmodified logical characters
  for its configurable pad keys. It preserves Shift assignment on the active
  keyboard layout, including punctuation that lacks an egui Key variant.

Wayland startup sizing and logical-to-physical key fallback now use upstream
behavior.
