use super::*;

#[test]
fn splat_extensions_use_the_scene_viewer_and_regular_meshes_keep_their_viewer() {
    let mut app = test_workbench_app(None, VecDeque::new());
    for extension in ["ply", "SPLAT", "sog", "spz", "GLB"] {
        app.open_slot_file_preview(&PathBuf::from(format!("missing.{extension}")), false);
        assert!(matches!(app.viewer, Some(ViewerSession::Splat(_))));
    }
    for extension in ["stl", "obj", "gltf"] {
        app.open_slot_file_preview(&PathBuf::from(format!("missing.{extension}")), false);
        assert!(matches!(app.viewer, Some(ViewerSession::Model3d(_))));
    }
}

#[test]
fn queued_scene_completion_waits_until_the_active_session_closes() {
    let scene = scene_file_viewer(PathBuf::from("missing.sog"), &AppConfig::default());
    let mut app = test_workbench_app(Some(gallery_viewer()), VecDeque::from([scene]));
    let ctx = egui::Context::default();
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        assert!(app.render_modal_viewers(ctx));
    });
    assert!(matches!(
        app.viewer,
        Some(ViewerSession::ComponentGallery(_))
    ));
    assert_eq!(app.startup_viewers.len(), 1);
    app.replace_viewer(None);
    app.advance_startup();
    assert!(matches!(app.viewer, Some(ViewerSession::Splat(_))));
    assert!(app.startup_viewers.is_empty());
}
