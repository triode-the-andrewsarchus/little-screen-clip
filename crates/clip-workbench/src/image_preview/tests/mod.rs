use super::*;

mod effects;
mod handoff;
mod jobs;
mod snapshots;
mod sources;
mod studio;
mod transforms;

pub(super) fn wait_for_dcs(state: &mut ImagePreviewState) {
    let ctx = egui::Context::default();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while state.dcs_state.is_computing && std::time::Instant::now() < deadline {
        state.poll_decorrelation_stretch(&ctx, BackgroundJobs::global());
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    state.poll_decorrelation_stretch(&ctx, BackgroundJobs::global());
    assert!(!state.dcs_state.is_computing);
    assert!(state.dcs_state.error.is_none());
}

pub(super) fn wait_for_pixel_resize(state: &mut ImagePreviewState) {
    let ctx = egui::Context::default();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while (state.pixel_resize_job.pending || state.pixel_resize_job.retry_at.is_some())
        && std::time::Instant::now() < deadline
    {
        state.poll_pixel_resize(&ctx, BackgroundJobs::global());
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    state.poll_pixel_resize(&ctx, BackgroundJobs::global());
    assert!(!state.pixel_resize_job.pending);
    assert!(state.pixel_resize_job.error.is_none());
}

pub(super) fn image_state_from_png(size: (u32, u32), title: &str) -> ImagePreviewState {
    let mut png = Vec::new();
    RgbaImage::new(size.0, size.1)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    ImagePreviewState::from_bytes_with_meta(
        png,
        "image/png".into(),
        None,
        title.into(),
        &AppConfig::default(),
    )
    .unwrap()
}

#[cfg(unix)]
pub(super) fn assert_configured_app_receives_source_image(
    executable_name: &str,
    configure: impl FnOnce(&mut AppConfig, &str),
) {
    use std::os::unix::fs::PermissionsExt;

    let temp_dir = tempfile::tempdir().unwrap();
    let source_path = temp_dir.path().join("source.png");
    RgbaImage::from_pixel(3, 2, image::Rgba([17, 83, 149, 255]))
        .save(&source_path)
        .unwrap();
    let expected_bytes = std::fs::read(&source_path).unwrap();
    let executable_path = temp_dir.path().join(executable_name);
    let argument_capture = temp_dir.path().join(format!("{executable_name}.arg"));
    let bytes_capture = temp_dir.path().join(format!("{executable_name}.bytes"));
    let script = format!(
        "#!/bin/sh\nprintf '%s' \"$1\" > '{}'\ncat \"$1\" > '{}'\n",
        argument_capture.display(),
        bytes_capture.display()
    );
    std::fs::write(&executable_path, script).unwrap();
    let mut permissions = std::fs::metadata(&executable_path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&executable_path, permissions).unwrap();

    let mut config = AppConfig::default();
    configure(&mut config, executable_path.to_str().unwrap());
    let mut state = ImagePreviewState::from_path(source_path.clone(), &config).unwrap();
    let ctx = egui::Context::default();
    let mut close = false;
    let app = match executable_name {
        "fake-krita" => state.external_editor.clone(),
        "fake-inkscape" => state.svg_editor.clone(),
        _ => state.viewer.clone(),
    };
    state.launch_external_app(&app, &ctx, &mut close);

    assert!(matches!(
        state.status_toast.as_ref(),
        Some((message, _)) if message.contains("Launched")
    ));
    assert!(!close, "external handoff leaves Image Studio open");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while (!argument_capture.exists() || !bytes_capture.exists())
        && std::time::Instant::now() < deadline
    {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(
        std::fs::read_to_string(argument_capture).unwrap(),
        source_path.to_string_lossy()
    );
    assert_eq!(std::fs::read(bytes_capture).unwrap(), expected_bytes);
}

#[cfg(unix)]
pub(super) fn assert_external_handoff_keeps_edited_session(
    app: &str,
    command: impl FnOnce(&AppConfig) -> String,
) {
    let temp_dir = tempfile::tempdir().unwrap();
    let source_path = temp_dir.path().join("edited-source.png");
    RgbaImage::from_pixel(3, 2, image::Rgba([17, 83, 149, 255]))
        .save(&source_path)
        .unwrap();
    let config = AppConfig::default();
    let mut state = ImagePreviewState::from_path(source_path.clone(), &config).unwrap();
    Arc::make_mut(&mut state.current_rgba).put_pixel(0, 0, image::Rgba([200, 30, 40, 255]));
    state.undo_stack.push(Arc::clone(&state.original_rgba));
    let edited_pixels = Arc::clone(&state.current_rgba);
    let undo_len = state.undo_stack.len();
    let command = command(&config);

    state.launch_external_app_with_path(
        &command,
        Ok(source_path.clone()),
        |app_name, target, path| {
            assert_eq!(app_name, app);
            assert_eq!(target, command);
            assert_eq!(path, source_path);
            Ok("Launched test editor".to_owned())
        },
    );

    assert!(Arc::ptr_eq(&state.current_rgba, &edited_pixels));
    assert_eq!(
        state.current_rgba.get_pixel(0, 0),
        &image::Rgba([200, 30, 40, 255])
    );
    assert_eq!(state.undo_stack.len(), undo_len);
    assert!(matches!(
        state.status_toast.as_ref(),
        Some((message, _)) if message == "Launched test editor"
    ));
}

#[cfg(unix)]
pub(super) fn assert_unavailable_configured_app_sets_failure_toast(
    executable_name: &str,
    configure: impl FnOnce(&mut AppConfig, &str),
) {
    let temp_dir = tempfile::tempdir().unwrap();
    let source_path = temp_dir.path().join("source.png");
    RgbaImage::from_pixel(3, 2, image::Rgba([17, 83, 149, 255]))
        .save(&source_path)
        .unwrap();
    let missing_app = temp_dir.path().join(format!("missing-{executable_name}"));
    let missing_fallback = temp_dir.path().join("missing-default-opener");
    let mut config = AppConfig::default();
    configure(&mut config, missing_app.to_str().unwrap());
    let mut state = ImagePreviewState::from_path(source_path, &config).unwrap();
    let app = match executable_name {
        "krita" => state.external_editor.clone(),
        "inkscape" => state.svg_editor.clone(),
        _ => state.viewer.clone(),
    };
    state.launch_external_app_with(&app, |app_name, command, path| {
        crate::shell::EscapeHatchLauncher::spawn_with_fallback_command(
            app_name,
            command,
            path,
            missing_fallback.to_str().unwrap(),
        )
    });

    let expected = format!(
        "⚠️ Failed to launch '{}' (tool not installed)",
        missing_app.display()
    );
    assert!(matches!(
        state.status_toast.as_ref(),
        Some((message, _)) if message == &expected
    ));
}
