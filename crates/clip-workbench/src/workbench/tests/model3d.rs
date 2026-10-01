use super::*;

#[test]
fn model3d_toolbar_clicks_change_only_their_view_controls() {
    fn frame(
        ctx: &egui::Context,
        app: &mut WorkbenchApp,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 700.0),
                )),
                time: Some(1.0),
                events,
                ..Default::default()
            },
            |ctx| assert!(app.render_modal_viewers(ctx)),
        )
    }

    fn label_rect(output: &egui::FullOutput, label: &str) -> egui::Rect {
        output
            .shapes
            .iter()
            .find_map(|shape| {
                let egui::epaint::Shape::Text(text) = &shape.shape else {
                    return None;
                };
                (text.galley.job.text == label)
                    .then(|| text.galley.rect.translate(text.pos.to_vec2()))
            })
            .unwrap_or_else(|| panic!("toolbar label {label:?} was not rendered"))
    }

    fn click_label(ctx: &egui::Context, app: &mut WorkbenchApp, label: &str) {
        let rect = label_rect(&frame(ctx, app, Vec::new()), label);
        let pos = rect.center();
        let pointer = egui::Event::PointerMoved(pos);
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let _ = frame(ctx, app, vec![pointer.clone(), button(true)]);
        let _ = frame(ctx, app, vec![pointer, button(false)]);
    }

    #[derive(Debug, PartialEq)]
    struct ViewerSnapshot {
        route: WorkbenchView,
        path: Option<PathBuf>,
        title: String,
        status: model3d_view::ModelLoadingStatus,
        model: Option<(usize, String, String, usize)>,
        pitch: f32,
        yaw: f32,
        zoom: f32,
        pan_offset: egui::Vec2,
        auto_rotate: bool,
        wireframe: bool,
        up_axis: model3d_view::UpAxis,
    }

    #[derive(Clone, Copy)]
    enum ChangedControl {
        AutoRotate,
        Wireframe,
        UpAxis,
        Camera,
    }

    fn snapshot(app: &WorkbenchApp) -> ViewerSnapshot {
        let Some(ViewerSession::Model3d(state)) = &app.viewer else {
            panic!("3D viewer should remain active")
        };
        ViewerSnapshot {
            route: app.view,
            path: state.path.clone(),
            title: state.title.clone(),
            status: state.status.clone(),
            model: state.mesh.as_ref().map(|mesh| {
                (
                    mesh as *const _ as usize,
                    mesh.name.clone(),
                    mesh.format.badge_label().to_owned(),
                    mesh.triangles.len(),
                )
            }),
            pitch: state.pitch,
            yaw: state.yaw,
            zoom: state.zoom,
            pan_offset: state.pan_offset,
            auto_rotate: state.auto_rotate,
            wireframe: state.wireframe,
            up_axis: state.up_axis,
        }
    }

    fn assert_unchanged_except(
        before: &ViewerSnapshot,
        after: &ViewerSnapshot,
        changed: ChangedControl,
    ) {
        assert_eq!(
            (
                &before.route,
                &before.path,
                &before.title,
                &before.status,
                &before.model
            ),
            (
                &after.route,
                &after.path,
                &after.title,
                &after.status,
                &after.model
            ),
            "route, load status, and model session remain unchanged"
        );
        match changed {
            ChangedControl::AutoRotate => assert_eq!(
                (
                    before.pitch,
                    before.yaw,
                    before.zoom,
                    before.pan_offset,
                    before.wireframe,
                    before.up_axis
                ),
                (
                    after.pitch,
                    after.yaw,
                    after.zoom,
                    after.pan_offset,
                    after.wireframe,
                    after.up_axis
                )
            ),
            ChangedControl::Wireframe => assert_eq!(
                (
                    before.pitch,
                    before.yaw,
                    before.zoom,
                    before.pan_offset,
                    before.auto_rotate,
                    before.up_axis
                ),
                (
                    after.pitch,
                    after.yaw,
                    after.zoom,
                    after.pan_offset,
                    after.auto_rotate,
                    after.up_axis
                )
            ),
            ChangedControl::UpAxis => assert_eq!(
                (
                    before.pitch,
                    before.yaw,
                    before.zoom,
                    before.pan_offset,
                    before.auto_rotate,
                    before.wireframe
                ),
                (
                    after.pitch,
                    after.yaw,
                    after.zoom,
                    after.pan_offset,
                    after.auto_rotate,
                    after.wireframe
                )
            ),
            ChangedControl::Camera => assert_eq!(
                (before.auto_rotate, before.wireframe, before.up_axis),
                (after.auto_rotate, after.wireframe, after.up_axis)
            ),
        }
    }

    fn click_and_snapshot(
        ctx: &egui::Context,
        app: &mut WorkbenchApp,
        label: &str,
        changed: ChangedControl,
    ) -> (ViewerSnapshot, ViewerSnapshot) {
        let before = snapshot(app);
        click_label(ctx, app, label);
        let after = snapshot(app);
        assert_unchanged_except(&before, &after, changed);
        (before, after)
    }

    let model = little_clip_core::presentation::model3d::Model3d {
        name: "synthetic.obj".to_owned(),
        format: little_clip_core::presentation::model3d::ModelFormat::Obj,
        triangles: Vec::new(),
        thumbnail_triangles: Vec::new(),
        min_bounds: little_clip_core::presentation::model3d::Vec3::ZERO,
        max_bounds: little_clip_core::presentation::model3d::Vec3::ZERO,
        center: little_clip_core::presentation::model3d::Vec3::ZERO,
        size: little_clip_core::presentation::model3d::Vec3::ZERO,
        max_dimension: 1.0,
    };
    let mut app = test_workbench_app(None, VecDeque::new());
    app.view = WorkbenchView::NodeMachine;
    app.viewer = Some(ViewerSession::Model3d(Model3dViewerState::from_model(
        model,
        Some(PathBuf::from("synthetic.obj")),
    )));
    let ctx = egui::Context::default();
    crate::ui_ext::configure_workbench_style(&ctx);

    // The initial frame both measures the real button bounds and starts
    // the egui clock. Reusing its fixed timestamp avoids animation drift
    // while exercising successive clicks.
    let _ = frame(&ctx, &mut app, Vec::new());

    let (before, after) =
        click_and_snapshot(&ctx, &mut app, "Pause Rotation", ChangedControl::AutoRotate);
    assert!(
        before.auto_rotate && !after.auto_rotate,
        "Pause Rotation toggles auto-rotate off"
    );

    let (before, after) =
        click_and_snapshot(&ctx, &mut app, "Auto-Rotate", ChangedControl::AutoRotate);
    assert!(
        !before.auto_rotate && after.auto_rotate,
        "Auto-Rotate resumes"
    );

    let (before, after) =
        click_and_snapshot(&ctx, &mut app, "Wireframe", ChangedControl::Wireframe);
    assert!(!before.wireframe && after.wireframe);

    let (before, after) = click_and_snapshot(&ctx, &mut app, "Shaded", ChangedControl::Wireframe);
    assert!(before.wireframe && !after.wireframe, "Shaded restores fill");

    let (before, after) = click_and_snapshot(&ctx, &mut app, "Axis: Y-Up", ChangedControl::UpAxis);
    assert_eq!(before.up_axis, model3d_view::UpAxis::YUp);
    assert_eq!(after.up_axis, model3d_view::UpAxis::ZUp);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    let Some(ViewerSession::Model3d(state)) = &app.viewer else {
        panic!("3D viewer should remain active after axis toggle");
    };
    assert_eq!(
        state.status_toast.message(now),
        Some("Orientation set to Z-Up (CAD / 3D printing)")
    );

    let (before, after) = click_and_snapshot(&ctx, &mut app, "Axis: Z-Up", ChangedControl::UpAxis);
    assert_eq!(before.up_axis, model3d_view::UpAxis::ZUp);
    assert_eq!(after.up_axis, model3d_view::UpAxis::YUp);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    let Some(ViewerSession::Model3d(state)) = &app.viewer else {
        panic!("3D viewer should remain active after axis toggle");
    };
    assert_eq!(
        state.status_toast.message(now),
        Some("Orientation set to Y-Up (3D Graphics)")
    );

    if let Some(ViewerSession::Model3d(state)) = &mut app.viewer {
        state.auto_rotate = false;
        state.pitch = 1.1;
        state.yaw = 2.2;
        state.zoom = 3.5;
        state.pan_offset = egui::vec2(17.0, -9.0);
    }
    let (before, after) = click_and_snapshot(&ctx, &mut app, "Reset View", ChangedControl::Camera);
    assert_eq!(
        (after.pitch, after.yaw, after.zoom, after.pan_offset),
        (0.4, 0.6, 1.0, egui::Vec2::ZERO)
    );
    assert_ne!(before.pitch, after.pitch);
}

#[test]
fn file_backed_3d_viewer_escape_returns_to_workbench_and_drops_pending_session() {
    let temp_dir = tempfile::tempdir().unwrap();
    let model_path = temp_dir.path().join("fixture.obj");
    std::fs::write(
        &model_path,
        "v 0 0 0\nv 1 0 0\nv 0 1 0\nv 0 0 1\nf 1 2 3\nf 1 2 4\nf 1 3 4\nf 2 3 4\n",
    )
    .unwrap();
    let mut app = test_workbench_app(None, VecDeque::new());
    app.view = WorkbenchView::Palette;

    app.open_slot_file_preview(&model_path, false);

    let Some(ViewerSession::Model3d(state)) = app.viewer.as_ref() else {
        panic!("OBJ route should open the file-backed 3D viewer");
    };
    assert_eq!(state.path.as_deref(), Some(model_path.as_path()));
    assert!(state.rx.is_some(), "file parsing should remain async");
    assert_eq!(app.view, WorkbenchView::Palette);

    let ctx = egui::Context::default();
    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 700.0),
            )),
            events: vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |ctx| assert!(app.render_modal_viewers(ctx)),
    );
    assert!(
        app.viewer.is_none(),
        "Escape should drop the 3D viewer session"
    );
    assert_eq!(app.view, WorkbenchView::Palette);

    // A subsequent modal poll must not let a completed parse publish into
    // or replace the workbench after the viewer has been closed.
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        assert!(!app.render_modal_viewers(ctx));
    });
    assert!(app.viewer.is_none());
    assert_eq!(app.view, WorkbenchView::Palette);
}

#[test]
fn ready_idle_3d_viewer_escape_returns_to_originating_workbench_view() {
    let model = little_clip_core::presentation::model3d::Model3d {
        name: "ready synthetic.obj".to_owned(),
        format: little_clip_core::presentation::model3d::ModelFormat::Obj,
        triangles: Vec::new(),
        thumbnail_triangles: Vec::new(),
        min_bounds: little_clip_core::presentation::model3d::Vec3::ZERO,
        max_bounds: little_clip_core::presentation::model3d::Vec3::ZERO,
        center: little_clip_core::presentation::model3d::Vec3::ZERO,
        size: little_clip_core::presentation::model3d::Vec3::ZERO,
        max_dimension: 1.0,
    };
    let mut app = test_workbench_app(None, VecDeque::new());
    app.view = WorkbenchView::NodeMachine;
    let originating_view = app.view;
    app.selected_slots = vec![SlotId::RightHand, SlotId::LeftPrior1];
    let originating_slots = app.selected_slots.clone();
    app.viewer = Some(ViewerSession::Model3d(Model3dViewerState::from_model(
        model, None,
    )));
    let Some(ViewerSession::Model3d(state)) = app.viewer.as_ref() else {
        panic!("synthetic model should open the 3D viewer");
    };
    assert_eq!(state.status, model3d_view::ModelLoadingStatus::Ready);
    assert!(state.rx.is_none());

    let ctx = egui::Context::default();
    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 700.0),
            )),
            events: vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |ctx| assert!(app.render_modal_viewers(ctx)),
    );

    assert!(app.viewer.is_none(), "Escape should close the ready viewer");
    assert_eq!(app.view, originating_view);
    assert_eq!(app.selected_slots, originating_slots);
}
