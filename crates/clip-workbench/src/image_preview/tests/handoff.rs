use super::*;

#[cfg(unix)]
#[test]
fn krita_handoff_receives_current_source_image_file() {
    assert_configured_app_receives_source_image("fake-krita", |config, executable| {
        config.image.external_editor = executable.to_owned();
    });
}

#[cfg(unix)]
#[test]
fn inkscape_handoff_receives_current_source_image_file() {
    assert_configured_app_receives_source_image("fake-inkscape", |config, executable| {
        config.image.svg_editor = executable.to_owned();
    });
}

#[cfg(unix)]
#[test]
fn viewer_handoff_receives_current_source_image_file() {
    assert_configured_app_receives_source_image("fake-nomacs", |config, executable| {
        config.image.viewer = executable.to_owned();
    });
}

#[cfg(unix)]
#[test]
fn returning_from_krita_keeps_the_open_edited_image_session() {
    assert_external_handoff_keeps_edited_session("krita", |config| {
        config.image.external_editor.clone()
    });
}

#[cfg(unix)]
#[test]
fn returning_from_inkscape_keeps_the_open_edited_image_session() {
    assert_external_handoff_keeps_edited_session("inkscape", |config| {
        config.image.svg_editor.clone()
    });
}

#[cfg(unix)]
#[test]
fn returning_from_image_viewer_keeps_the_open_edited_image_session() {
    assert_external_handoff_keeps_edited_session("nomacs", |config| config.image.viewer.clone());
}

#[cfg(unix)]
#[test]
fn missing_krita_sets_image_studio_failure_toast_without_host_fallback() {
    assert_unavailable_configured_app_sets_failure_toast("krita", |config, executable| {
        config.image.external_editor = executable.to_owned();
    });
}

#[cfg(unix)]
#[test]
fn missing_inkscape_sets_image_studio_failure_toast_without_host_fallback() {
    assert_unavailable_configured_app_sets_failure_toast("inkscape", |config, executable| {
        config.image.svg_editor = executable.to_owned();
    });
}

#[cfg(unix)]
#[test]
fn missing_viewer_sets_image_studio_failure_toast_without_host_fallback() {
    assert_unavailable_configured_app_sets_failure_toast("nomacs", |config, executable| {
        config.image.viewer = executable.to_owned();
    });
}
