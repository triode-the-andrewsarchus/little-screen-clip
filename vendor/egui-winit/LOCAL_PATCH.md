This is `egui-winit` 0.27.2 from the egui project, licensed under MIT or
Apache-2.0. The adjacent license files are from the upstream 0.27.2 tag.

Local change: `create_winit_window_builder` passes initial size, size limits,
and position as Winit logical values scaled only by egui zoom. The unmodified
crate converts them to physical pixels using a monitor chosen before the
Wayland window exists. On the project's Niri setup this produced a 4710×2648
first map for a requested 2355×1324 popup, followed by a visible correction.
The local change follows the approach in newer egui-winit releases while
keeping the workbench on its current egui API. Remove this patch when a
coherent egui upgrade has been validated against the full viewer inventory.
