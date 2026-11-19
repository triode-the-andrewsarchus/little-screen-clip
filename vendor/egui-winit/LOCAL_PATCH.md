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

The upstream 0.37 audit found two application hooks in `src/lib.rs`. The
remaining source files match upstream. Upstream's `set_clipboard_shortcuts`
applies to the entire viewport, and the translated `egui::Event::Key` carries
the translated logical and physical `Key` values. Constellation needs winit's
unmodified logical character for its configurable bindings, including when
Shift changes the translated key.

Private Weston/Niri checks exercised native file and text copy, cut and paste,
including Ctrl+F followed by Ctrl+V in the same input sequence. They also
exercised assignment with Shift+Apostrophe, Shift+Semicolon and Shift+1, then
undid each assignment with Ctrl+Z. Both hooks are still needed.
