//! Shared UI compatibility exports and desktop runtime services.
//!
//! Audio-device and native-dialog discovery stay outside the pure widget crate.
pub use clip_ui::ui_ext::*;

static ZENITY_AVAILABLE: std::sync::LazyLock<bool> =
    std::sync::LazyLock::new(|| little_clip_core::command_available("zenity").is_some());

/// Returns whether the `zenity` native dialog utility is installed and available.
/// The result is cached after the first check to prevent frame drops on the UI thread.
pub fn is_zenity_available() -> bool {
    *ZENITY_AVAILABLE
}

static SHARED_AUDIO_SINK: std::sync::LazyLock<Option<rodio::stream::MixerDeviceSink>> =
    std::sync::LazyLock::new(|| {
        rodio::stream::DeviceSinkBuilder::open_default_sink()
            .ok()
            .map(|mut s| {
                s.log_on_drop(false);
                s
            })
    });

/// Attaches an initialized source to the shared application audio sink.
///
/// Populate queued sources before connecting: the mixer reads their channel count
/// and sample rate immediately. The caller retains any playback controls.
/// Returns `None` when the shared audio device is unavailable.
pub fn connect_shared_audio_source(source: impl rodio::Source + Send + 'static) -> Option<()> {
    SHARED_AUDIO_SINK.as_ref()?.mixer().add(source);
    Some(())
}

/// Creates a new `rodio::Player` connected to the shared application audio sink.
/// This avoids opening competing audio hardware sinks across preview widgets.
pub fn new_shared_audio_player() -> Option<rodio::Player> {
    SHARED_AUDIO_SINK
        .as_ref()
        .map(|s| rodio::Player::connect_new(s.mixer()))
}
