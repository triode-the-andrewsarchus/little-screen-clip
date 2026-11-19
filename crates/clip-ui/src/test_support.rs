//! Helpers for desktop UI tests that inspect egui platform output.

/// Returns the last text copy command emitted during the pass.
pub fn copied_text(output: &egui::PlatformOutput) -> &str {
    output
        .commands
        .iter()
        .rev()
        .find_map(|command| match command {
            egui::OutputCommand::CopyText(text) => Some(text.as_str()),
            _ => None,
        })
        .unwrap_or_default()
}

/// Returns the last URL open command emitted during the pass.
pub fn opened_url(output: &egui::PlatformOutput) -> Option<&egui::OpenUrl> {
    output
        .commands
        .iter()
        .rev()
        .find_map(|command| match command {
            egui::OutputCommand::OpenUrl(url) => Some(url),
            _ => None,
        })
}

/// Simulates a wheel gesture expressed in logical points.
pub fn scroll(delta: egui::Vec2) -> egui::Event {
    egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta,
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Unknown,
        modifiers: egui::Modifiers::NONE,
    }
}

/// Runs a headless pass and preserves its shapes and platform events.
pub fn run_frame(
    ctx: &egui::Context,
    input: egui::RawInput,
    draw: impl FnMut(&egui::Context),
) -> egui::FullOutput {
    let mut output = ctx.run_pass(input, draw);
    output.textures_delta.clear();
    output
}

/// Runs a headless pass and captures texture uploads for image assertions.
pub fn run_frame_with_uploads(
    ctx: &egui::Context,
    input: egui::RawInput,
    draw: impl FnMut(&egui::Context),
) -> (
    egui::FullOutput,
    Vec<(egui::TextureId, egui::epaint::ImageDelta)>,
) {
    let mut output = ctx.run_pass(input, draw);
    let uploads = output
        .textures_delta
        .set
        .drain()
        .flat_map(|(id, deltas)| deltas.into_iter().map(move |delta| (id, delta)))
        .collect();
    output.textures_delta.clear();
    (output, uploads)
}
