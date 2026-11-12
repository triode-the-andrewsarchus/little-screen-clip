use super::*;

fn render_map_frame(
    ctx: &egui::Context,
    state: &mut ConstellationState,
    events: Vec<egui::Event>,
    play: &mut dyn FnMut(&std::path::Path),
) -> egui::FullOutput {
    ctx.run(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))),
            events,
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                state.render_starfield(ui, std::path::Path::new("/samples"), play, &mut |_| {});
            });
        },
    )
}

fn coarse_mesh(state: &mut ConstellationState) -> Vec<egui::epaint::Vertex> {
    let context = egui::Context::default();
    let output = render_map_frame(&context, state, vec![], &mut |_| {});
    output
        .shapes
        .into_iter()
        .filter_map(|shape| match shape.shape {
            // The shared vignette is an untextured overlay; count the dot batch only.
            egui::Shape::Mesh(mesh) if mesh.texture_id != egui::TextureId::default() => {
                Some(mesh.vertices.clone())
            }
            _ => None,
        })
        .flatten()
        .collect()
}

pub(super) fn sample_fixture(index: usize) -> ConstellationSample {
    ConstellationSample {
        file_path: std::path::PathBuf::from(format!("/samples/{index}.wav")),
        folder_path: "/samples".into(),
        filename: if index == 999 {
            "target.wav".into()
        } else {
            "background.wav".into()
        },
        category: if index == 999 {
            SampleCategory::Snare
        } else {
            SampleCategory::Kick
        },
        confidence: 1.0,
        duration_seconds: 0.5,
        pitch_hz: 0.0,
        pitch_clarity: 0.0,
        brightness: 0.0,
        x: 0.0,
        y: 0.0,
        file_size: 100,
        mtime: 0,
        sample_rate: 44100,
        channels: 1,
        bit_depth: 16,
        source_revision: None,
    }
}

fn note_sample(path: &str) -> ConstellationSample {
    let mut sample = sample_fixture(0);
    sample.file_path = path.into();
    sample.filename = sample
        .file_path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    // The key must use the actual parent, even when an index covers a recursive root.
    sample.folder_path = "/recursive-root".into();
    sample
}

fn piano_family_fixture() -> ConstellationState {
    let mut state = ConstellationState::default();
    state.samples = ["/a/piano_C3.wav", "/a/piano_C4.wav", "/a/piano_C5.wav"]
        .into_iter()
        .map(note_sample)
        .collect();
    state.sample_sets = super::super::multisample::SampleSets::build(&state.samples);
    state
}

#[test]
fn multisample_families_stay_in_actual_folders_and_preserve_variants() {
    let samples: Vec<_> = [
        "/a/old_piano_G4.wav",
        "/a/old_piano_C4.wav",
        "/a/old_piano_D4.wav",
        "/b/old_piano_C4.wav",
        "/b/old_piano_D4.wav",
        "/a/old_piano_C4_vel80.wav",
        "/a/old_piano_D4_vel90.wav",
        "/a/old_piano_01.wav",
        "/a/old_piano_02.wav",
        "/a/only_C4.wav",
        "/a/only_C4.flac",
    ]
    .into_iter()
    .map(note_sample)
    .collect();
    let sets = super::super::multisample::SampleSets::build(&samples);
    assert_eq!(sets.sets.len(), 2);
    let group = &sets.sets[sets.set_for(0).unwrap()];
    assert_eq!(group.members, vec![1, 2, 0]);
    assert_eq!(
        group.representative, 2,
        "audition the middle recorded pitch"
    );
    assert_ne!(sets.set_for(0), sets.set_for(3));
    for index in 5..samples.len() {
        assert_eq!(sets.set_for(index), None);
    }
    assert_eq!(sets.note(1), Some(60.0));
    assert_eq!(sets.note(7), None, "sequence numbers do not become pitches");
}

#[test]
fn close_pitch_views_keep_exact_projection_and_cover_all_stem_bases() {
    let mut state = piano_family_fixture();
    state.samples.extend((3..200).map(sample_fixture));
    for (index, sample) in state.samples.iter_mut().enumerate() {
        sample.x = 2.3 + index as f32 * 0.00001;
        sample.y = -1.5;
    }
    state.sample_sets = super::super::multisample::SampleSets::build(&state.samples);
    let viewport = Rect::from_min_size(Pos2::ZERO, Vec2::new(1400.0, 500.0));
    for expanded in [None, Some(0)] {
        state.expanded_set = expanded;
        for zoom in [40.0, 40.0001, 50.0, 160.0, 40.0] {
            state.zoom = zoom;
            let radius = map_point_radius(viewport, state.samples.len(), zoom);
            let (scale, _, points) = state.layout_map_points(viewport, viewport, radius, "");
            let raw_points = state.map_points(scale, radius, "");
            for point in &points {
                let raw = raw_points
                    .iter()
                    .find(|raw| raw.index == point.index)
                    .unwrap();
                assert_eq!(
                    point.position, raw.position,
                    "3D dots must never use collision cells"
                );
                assert_eq!(point.base, raw.base);
            }
            assert!(points.windows(2).all(|pair| pair[0].depth <= pair[1].depth));
        }
    }
    let ctx = egui::Context::default();
    state.zoom = 1.0;
    let (scale, center, points) = state.layout_map_points(viewport, viewport, 4.0, "");
    let output = ctx.run(Default::default(), |ctx| {
        let painter = ctx
            .layer_painter(egui::LayerId::new(
                egui::Order::Background,
                egui::Id::new("plane"),
            ))
            .with_clip_rect(viewport);
        state.paint_reference_plane(&painter, center, scale);
    });
    let endpoints: Vec<_> = output
        .shapes
        .iter()
        .filter_map(|shape| match shape.shape {
            egui::Shape::LineSegment { points, .. } => Some(points),
            _ => None,
        })
        .flatten()
        .collect();
    let plane_bounds = Rect::from_points(&endpoints);
    assert!(plane_bounds.is_finite() && plane_bounds.is_positive());
    assert!(
        points
            .iter()
            .all(|point| plane_bounds.contains(point.base + center.to_vec2())),
        "every outlying sample and expanded ring must have its stem base inside the finite plane"
    );
}

#[test]
fn front_dots_occlude_rear_selection_and_win_overlapping_hover() {
    let ctx = egui::Context::default();
    let mut state = ConstellationState::default();
    let mut far = note_sample("/a/far_C4.wav");
    let mut near = note_sample("/a/near_C5.wav");
    far.category = SampleCategory::Kick;
    near.category = SampleCategory::Snare;
    state.camera.elevation = 45.0;
    near.y = -super::super::multisample::pitch_height(Some(72.0));
    state.samples = vec![near, far]; // Opposite of the required painter's order.
    state.sample_sets = super::super::multisample::SampleSets::build(&state.samples);
    state.selected_path = Some(state.samples[1].file_path.clone());
    let output = render_map_frame(&ctx, &mut state, vec![], &mut |_| {});
    let texture = dot_texture(&ctx).id();
    let mesh = output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Mesh(mesh) if mesh.texture_id == texture => Some(mesh),
            _ => None,
        })
        .unwrap();
    assert!(
        mesh.indices[mesh.indices.len() - 6..]
            .iter()
            .all(|index| mesh.vertices[*index as usize].color == SampleCategory::Snare.color())
    );
    let center = (mesh.vertices[0].pos.to_vec2() + mesh.vertices[2].pos.to_vec2()) / 2.0;
    render_map_frame(
        &ctx,
        &mut state,
        vec![egui::Event::PointerMoved(center.to_pos2())],
        &mut |_| {},
    );
    assert_eq!(
        state.last_hovered_path.as_ref(),
        Some(&state.samples[0].file_path)
    );
    state.camera.azimuth = 180.0;
    // Equal-height samples swap depth order when looking from the opposite side.
    state.samples[0].y = -0.4;
    state.samples[1].y = 0.4;
    state.samples[0].file_path = "/a/near_C4.wav".into();
    state.sample_sets = super::super::multisample::SampleSets::build(&state.samples);
    let viewport = Rect::from_min_size(Pos2::ZERO, Vec2::splat(800.0));
    let (_, _, points) = state.layout_map_points(viewport, viewport, 4.0, "");
    assert_eq!(points[0].index, 0);
    state.camera.azimuth = 0.0;
    let (_, _, points) = state.layout_map_points(viewport, viewport, 4.0, "");
    assert_eq!(points[0].index, 1);
}

#[test]
fn folder_snapshots_reset_empty_ranges_but_preserve_useful_ranges_and_hide_missing_sets() {
    let mut state = ConstellationState::default();
    let mut samples = vec![
        note_sample("/a/short_C3.wav"),
        note_sample("/a/long_C5.wav"),
    ];
    samples[0].duration_seconds = 0.25;
    samples[1].duration_seconds = 2.0;
    let sets = super::super::multisample::SampleSets::build(&samples);
    for (pitch, duration) in [
        (Some((90.0, 100.0)), (0.1, 4.0)),
        (Some((48.0, 72.0)), (3.0, 4.0)),
        (Some((72.0, 72.0)), (0.1, 1.0)),
    ] {
        state.pitch_range = pitch;
        (state.min_duration, state.max_duration) = duration;
        state.install_samples(samples.clone(), sets.clone());
        assert_eq!(
            (state.pitch_range, state.min_duration, state.max_duration),
            (None, 0.0, 0.0)
        );
    }
    state.pitch_range = Some((48.0, 60.0));
    state.max_duration = 1.0;
    state.search_query = "does not match".into();
    state.category_filter = Some(SampleCategory::Vocal);
    state.multisamples_only = true;
    state.install_samples(samples, sets);
    assert_eq!(
        (state.pitch_range, state.max_duration),
        (Some((48.0, 60.0)), 1.0)
    );
    assert!(
        !state.multisamples_only,
        "a hidden filter must not exclude singleton folders"
    );
    let ctx = egui::Context::default();
    let palette = Default::default();
    let has_toggle = |output: egui::FullOutput| {
        output.shapes.iter().any(|shape| {
            matches!(
            &shape.shape, egui::Shape::Text(text) if text.galley.text() == "Multisamples only")
        })
    };
    assert!(!has_toggle(render_frame(
        &ctx,
        &mut state,
        &palette,
        Vec2::new(1600.0, 1000.0)
    )));
    state = piano_family_fixture();
    state.category_filter = Some(SampleCategory::Vocal);
    assert!(has_toggle(render_frame(
        &ctx,
        &mut state,
        &palette,
        Vec2::new(1600.0, 1000.0)
    )));
}

#[test]
fn named_pitch_families_filter_peers_and_keep_unrelated_singletons_separate() {
    let mut state = ConstellationState::default();
    state.samples = [
        "/a/bar_D2.wav",
        "/a/foo_C3.wav",
        "/a/C4_piano_vel80.wav",
        "/a/piano_D4_vel80.wav",
        "/a/piano_E4_vel90.wav",
        "/a/DoubleSawsC3.wav",
        "/a/DoubleSawsF#4.wav",
        "/bare/C3.wav",
        "/bare/D3.wav",
        "/a/solo_F2.wav",
        "/a/no_octave_C.wav",
    ]
    .into_iter()
    .map(note_sample)
    .collect();
    for sample in &mut state.samples {
        sample.pitch_hz = 440.0;
        sample.pitch_clarity = 1.0;
    }
    state.sample_sets = super::super::multisample::SampleSets::build(&state.samples);
    assert_eq!(state.sample_sets.sets.len(), 3);
    assert_eq!(state.sample_sets.note(0), Some(38.0));
    assert_eq!(state.sample_sets.note(9), Some(41.0));
    assert_eq!(
        state.sample_sets.note(10),
        Some(69.0),
        "no octave keeps measured pitch"
    );
    state.multisamples_only = true;
    let active: Vec<_> = state
        .samples
        .iter()
        .enumerate()
        .filter_map(|(index, _)| state.matches_filters(index, "").then_some(index))
        .collect();
    assert_eq!(active, [2, 3, 5, 6, 7, 8]);
    state.selected_path = Some(state.samples[9].file_path.clone());
    assert!(state.selected_set().is_none());
    assert_eq!(state.sample_count_label(), "6 / 11 samples");
    state.selected_path = Some(state.samples[2].file_path.clone());
    assert_eq!(
        state.sample_count_label(),
        "6 / 11 samples · piano_vel80 · 2 notes"
    );
}

#[test]
fn audition_trail_uses_the_same_folded_and_raised_positions_as_the_map() {
    let ctx = egui::Context::default();
    let mut state = piano_family_fixture();
    for index in [0, 2] {
        state.record_played_sample(&state.samples[index].clone());
    }
    let viewport = Rect::from_min_max(Pos2::new(8.0, 8.0), Pos2::new(792.0, 592.0));
    for expanded in [None, Some(0)] {
        state.expanded_set = expanded;
        let radius = map_point_radius(viewport, state.samples.len(), state.zoom);
        let (_, center, points) = state.layout_map_points(viewport, viewport, radius, "");
        let positions: Vec<_> = [0, 2]
            .into_iter()
            .map(|index| {
                points
                    .iter()
                    .find(|point| point.index == index || point.collapsed_set == Some(0))
                    .unwrap()
                    .position
                    + center.to_vec2()
            })
            .collect();
        let output = render_map_frame(&ctx, &mut state, vec![], &mut |_| {});
        assert!(
            output.shapes.iter().any(|shape| match &shape.shape {
                egui::Shape::LineSegment { points, stroke } =>
                    stroke.width >= 1.2
                        && points[0].distance(positions[0]) < 0.01
                        && points[1].distance(positions[1]) < 0.01,
                _ => false,
            }),
            "audition history must follow folded groups and actual pitch heights"
        );
    }
}

#[test]
fn pitch_projection_duration_and_ring_detail_share_stable_geometry() {
    let mut state = ConstellationState::default();
    state.samples = [
        "/a/piano_C3.wav",
        "/a/piano_C4.wav",
        "/a/piano_C5.wav",
        "/a/noise.wav",
    ]
    .into_iter()
    .map(note_sample)
    .collect();
    state.samples[0].duration_seconds = 0.001;
    state.samples[2].duration_seconds = 200.0;
    state.samples[3].pitch_hz = 440.0;
    state.samples[3].pitch_clarity = 0.2;
    state.sample_sets = super::super::multisample::SampleSets::build(&state.samples);
    let overview = state.map_points(100.0, 4.0, "");
    assert_eq!(overview.len(), 2);
    let group = overview
        .iter()
        .find(|point| point.collapsed_set.is_some())
        .unwrap();
    assert_eq!(group.index, 1);
    assert_eq!(group.base, group.position, "C4 is the plane");
    let noise = overview.iter().find(|point| point.index == 3).unwrap();
    assert_eq!(noise.base, noise.position, "uncertain pitch stays on plane");
    state.selected_path = Some(state.samples[1].file_path.clone());
    state.expanded_set = state.selected_set();
    state.zoom = 12.0;
    let detail = state.map_points(100.0, 4.0, "");
    assert_eq!(detail.len(), 4);
    let low = detail.iter().find(|point| point.index == 0).unwrap();
    let high = detail.iter().find(|point| point.index == 2).unwrap();
    assert!(
        low.position.y > low.base.y,
        "low notes descend from their flat ring"
    );
    assert!(
        high.position.y < high.base.y,
        "high notes rise from their flat ring"
    );
    for point in detail.iter().filter(|point| point.index < 3) {
        assert!(
            (point.position.y - point.base.y + state.note_height(point.index) * 100.0).abs()
                < 0.0001
        );
    }
    assert!(low.radius < high.radius);
    assert!(detail.iter().all(|point| point.collapsed_set.is_none()));
    let visible = Rect::from_min_size(Pos2::ZERO, Vec2::new(500.0, 240.0));
    state.pending_set_focus = Some(state.selected_set().unwrap());
    let (_, center, focused) = state.layout_map_points(visible, visible, 8.0, "");
    assert!(
        focused
            .iter()
            .filter(|point| state.sample_sets.set_for(point.index).is_some())
            .all(|point| visible
                .shrink(20.0)
                .contains(point.position + center.to_vec2())),
        "Explore must fit every recorded pitch into a short canvas"
    );
    state.pitch_height_enabled = false;
    assert!(
        state
            .map_points(100.0, 4.0, "")
            .iter()
            .all(|point| point.base == point.position)
    );
    state.zoom = 1.0;
    state.expanded_set = None;
    let filtered = state.map_points(100.0, 4.0, "c5");
    assert_eq!(
        filtered
            .iter()
            .find(|point| point.collapsed_set.is_some())
            .unwrap()
            .index,
        2,
        "an explicitly filtered note remains available"
    );
}

#[test]
fn collapsed_hover_auditions_middle_note_and_expanded_hover_picks_individuals() {
    let ctx = egui::Context::default();
    let mut state = piano_family_fixture();
    let mut auditions = Vec::new();
    let mut frame = |state: &mut ConstellationState, pointer: Option<Pos2>| {
        render_map_frame(
            &ctx,
            state,
            pointer.map(egui::Event::PointerMoved).into_iter().collect(),
            &mut |path| auditions.push(path.to_owned()),
        )
    };
    let first = frame(&mut state, None);
    let star = first
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Mesh(mesh) if mesh.texture_id != egui::TextureId::default() => {
                Some(mesh.calc_bounds().center())
            }
            _ => None,
        })
        .unwrap();
    let _ = frame(&mut state, Some(star));
    assert_eq!(auditions, vec![state.samples[1].file_path.clone()]);
    state.selected_path = Some(state.samples[1].file_path.clone());
    state.expanded_set = state.selected_set();
    state.zoom = 12.0;
    state.pending_map_reveal = Some(state.samples[2].file_path.clone());
    state.last_hovered_path = None;
    let _ = render_map_frame(
        &ctx,
        &mut state,
        vec![egui::Event::PointerMoved(Pos2::new(400.0, 300.0))],
        &mut |path| auditions.push(path.to_owned()),
    );
    assert_eq!(auditions.last(), Some(&state.samples[2].file_path));
}

#[test]
fn explore_centers_lowest_sample_and_crossing_each_ring_auditions_its_recording() {
    let ctx = egui::Context::default();
    let mut state = ConstellationState::default();
    state.samples = [
        "/a/piano_C5.wav",
        "/a/piano_C3.wav",
        "/a/piano_C4.wav",
        "/a/piano_C4.flac",
    ]
    .into_iter()
    .map(note_sample)
    .collect();
    state.sample_sets = super::super::multisample::SampleSets::build(&state.samples);
    let set = state.sample_sets.set_for(0).unwrap();
    let members = state.sample_sets.sets[set].members.clone();
    state.selected_path = Some(state.samples[members[1]].file_path.clone());
    state.camera.orbit(Vec2::new(75.0, 20.0));
    state.pan = Vec2::new(130.0, -42.0);
    state.zoom = 3.7;
    let before = (
        state.camera,
        state.pan,
        state.zoom,
        state.selected_path.clone(),
    );
    state.explore_set(set);
    assert_eq!(
        state.selected_path.as_ref(),
        Some(&state.samples[members[0]].file_path)
    );
    let initial = render_map_frame(&ctx, &mut state, vec![], &mut |_| {});
    let rings = initial
        .shapes
        .iter()
        .filter(|shape| matches!(&shape.shape, egui::Shape::Path(path) if path.points.len() == 97))
        .count();
    assert_eq!(
        rings,
        members.len(),
        "duplicate-pitch recordings need separate rings too"
    );
    let viewport = Rect::from_min_max(Pos2::new(8.0, 8.0), Pos2::new(792.0, 592.0));
    let (scale, center, points) = state.layout_map_points(viewport, viewport, 4.0, "");
    let origin = points
        .iter()
        .find(|point| point.index == members[0])
        .unwrap()
        .position
        + center.to_vec2();
    assert!(
        (origin - viewport.center()).length() < 0.01,
        "Explore must center the lowest note"
    );
    let origin = center + state.map_base(state.sample_sets.sets[set].center) * scale;
    let mut played = Vec::new();
    for (ordinal, index) in members.iter().enumerate() {
        let radius = (ordinal + 1) as f32 * 0.025;
        // Pick a circumference far from the dot, to test ring rather than dot hit testing.
        let pointer = origin + state.map_base(Pos2::new(0.0, 1.0)) * radius * scale;
        assert_eq!(state.ring_sample_at(pointer, center, scale), Some(*index));
        let _ = render_map_frame(
            &ctx,
            &mut state,
            vec![egui::Event::PointerMoved(pointer)],
            &mut |path| played.push(path.to_owned()),
        );
    }
    assert_eq!(
        played,
        members
            .iter()
            .map(|index| state.samples[*index].file_path.clone())
            .collect::<Vec<_>>()
    );
    state.selected_path = Some(state.samples[members[2]].file_path.clone());
    state.camera.orbit(Vec2::new(-40.0, -20.0));
    state.leave_set();
    assert!(state.expanded_set.is_none());
    assert!(state.pending_set_focus.is_none());
    assert_eq!(
        (
            state.camera,
            state.pan,
            state.zoom,
            state.selected_path.clone()
        ),
        before
    );
}

#[test]
fn explore_overrides_ranges_and_escape_distinguishes_clicked_notes_from_hover() {
    let ctx = egui::Context::default();
    let mut state = ConstellationState::default();
    state.samples = [
        "/a/piano_C3.wav",
        "/a/piano_C4.wav",
        "/a/piano_C5.wav",
        "/a/noise.wav",
    ]
    .into_iter()
    .map(note_sample)
    .collect();
    state.samples[0].duration_seconds = 0.1;
    state.samples[1].duration_seconds = 0.7;
    state.samples[2].duration_seconds = 2.0;
    state.sample_sets = super::super::multisample::SampleSets::build(&state.samples);
    state.min_duration = 0.5;
    state.max_duration = 1.0;
    state.pitch_range = Some((60.0, 60.0));
    state.selected_path = Some(state.samples[1].file_path.clone());
    state.zoom = 2.4;
    state.pan = Vec2::new(30.0, -40.0);
    assert_eq!(
        state.sample_count_label(),
        "1 / 4 samples · piano · 3 notes"
    );
    let escape = |state: &mut ConstellationState| {
        let _ = egui::Context::default().run(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: egui::Key::Escape,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ctx| {
                state.handle_explore_escape(ctx);
                assert!(!ctx.input(|input| input.key_pressed(egui::Key::Escape)));
            },
        );
    };
    state.explore_set(0);
    assert_eq!(
        state.sample_count_label(),
        "3 / 4 samples · piano · 3 notes"
    );
    assert!(state.matches_filters(0, "unmatched search"));
    assert!(!state.matches_filters(3, ""));
    state.last_hovered_path = Some(state.samples[2].file_path.clone());
    escape(&mut state);
    assert_eq!(
        state.selected_path.as_ref(),
        Some(&state.samples[1].file_path)
    );
    assert_eq!(state.zoom, 2.4);
    assert_eq!(state.pan, Vec2::new(30.0, -40.0));
    assert_eq!(
        state.sample_count_label(),
        "1 / 4 samples · piano · 3 notes"
    );
    assert!(state.map_hover_suspended);
    state.explore_set(0);
    let _ = render_map_frame(&ctx, &mut state, vec![], &mut |_| {});
    let viewport = Rect::from_min_max(Pos2::new(8.0, 8.0), Pos2::new(792.0, 592.0));
    let (_, center, points) = state.layout_map_points(viewport, viewport, 4.0, "");
    let point = points
        .iter()
        .find(|point| point.index == 2)
        .unwrap()
        .position
        + center.to_vec2();
    let mut auditions = Vec::new();
    let _ = render_map_frame(
        &ctx,
        &mut state,
        vec![egui::Event::PointerMoved(point + Vec2::new(30.0, 0.0))],
        &mut |_| {},
    );
    for pressed in [true, false] {
        let _ = render_map_frame(
            &ctx,
            &mut state,
            vec![
                egui::Event::PointerMoved(point),
                egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            &mut |path| auditions.push(path.to_owned()),
        );
    }
    assert_eq!(
        state.explore_selection.as_ref(),
        Some(&state.samples[2].file_path),
        "click the elevated dot even when a different flat ring lies behind it"
    );
    assert_eq!(auditions.last(), Some(&state.samples[2].file_path));
    state.last_hovered_path = Some(state.samples[0].file_path.clone());
    escape(&mut state);
    assert_eq!(
        state.selected_path.as_ref(),
        Some(&state.samples[2].file_path)
    );
    assert_eq!(
        state.pending_map_reveal.as_ref(),
        Some(&state.samples[2].file_path)
    );
    assert_eq!(
        (state.min_duration, state.max_duration, state.pitch_range),
        (0.5, 1.0, Some((60.0, 60.0)))
    );
    assert_eq!(state.zoom, 2.4);
    assert!(state.expanded_set.is_none());
}

#[test]
fn pitch_range_filters_measured_notes_and_keeps_unknowns_out_of_active_ranges() {
    let mut state = ConstellationState::default();
    state.samples = [
        "/a/piano_C3.wav",
        "/a/piano_C4.wav",
        "/a/piano_C5.wav",
        "/a/noise.wav",
        "/a/boundary.wav",
        "/a/new-tone.wav",
    ]
    .into_iter()
    .map(note_sample)
    .collect();
    for index in [4, 5] {
        state.samples[index].pitch_hz = 1000.0;
        state.samples[index].pitch_clarity = 0.99;
    }
    state.samples[5].source_revision =
        Some(super::super::classifier::PITCH_ANALYSIS_REVISION.to_vec());
    state.sample_sets = super::super::multisample::SampleSets::build(&state.samples);
    assert_eq!(
        state.sample_sets.note(4),
        None,
        "ignore old search-boundary artifacts immediately"
    );
    assert!(
        state.sample_sets.note(5).is_some(),
        "a newly measured real 1 kHz tone is valid"
    );
    state.pitch_range = Some((60.0, 72.0));
    let matches: Vec<_> = (0..state.samples.len())
        .map(|index| state.matches_filters(index, ""))
        .collect();
    assert_eq!(matches, vec![false, true, true, false, false, false]);
    state.pitch_range = None;
    assert!((0..state.samples.len()).all(|index| state.matches_filters(index, "")));
}

#[test]
fn assigned_rack_samples_gain_a_map_halo_and_lose_it_on_removal() {
    use super::super::sample_pack::SlotSample;
    let ctx = egui::Context::default();
    let mut state = ConstellationState::default();
    state.pack_shelf_open = false;
    state.samples.push(sample_fixture(0));
    let circles = |output: egui::FullOutput| {
        output
            .shapes
            .into_iter()
            .filter(|shape| matches!(shape.shape, egui::Shape::Circle(_)))
            .count()
    };
    let before = circles(render_frame(
        &ctx,
        &mut state,
        &Default::default(),
        Vec2::new(800.0, 600.0),
    ));
    state.pack_bank.slots[15].samples.push(SlotSample::new(
        state.samples[0].file_path.clone(),
        "hit.wav".into(),
        None,
        0.5,
    ));
    let assigned = circles(render_frame(
        &ctx,
        &mut state,
        &Default::default(),
        Vec2::new(800.0, 600.0),
    ));
    assert_eq!(assigned, before + 3);
    state.pack_bank.slots[15].samples.clear();
    assert_eq!(
        circles(render_frame(
            &ctx,
            &mut state,
            &Default::default(),
            Vec2::new(800.0, 600.0)
        )),
        before
    );
}

#[test]
fn pad_clicks_cycle_layers_and_reveal_them_without_opening_studio() {
    let mut state = ConstellationState::default();
    state.samples = (0..3).map(sample_fixture).collect();
    state.pack_bank.slots[0].samples = state.samples[..2]
        .iter()
        .map(|sample| {
            super::super::sample_pack::SlotSample::new(
                sample.file_path.clone(),
                sample.filename.clone(),
                None,
                1.0,
            )
        })
        .collect();
    state.pack_bank.slots[1].samples = vec![state.pack_bank.slots[0].samples[0].clone()];
    state.zoom = 4.0;
    for (pad, layer) in [(0, 0), (0, 1), (1, 0), (0, 0)] {
        state.reveal_pad_layer(pad);
        assert_eq!(state.selected_layers[pad], layer);
        assert_eq!(
            state.selected_path,
            Some(state.samples[layer].file_path.clone())
        );
        coarse_mesh(&mut state);
        assert!(state.pending_map_reveal.is_none());
        assert_eq!(state.zoom, 4.0);
        assert!(state.studio_request.is_none());
    }
    state.pack_bank.slots[0].samples.clear();
    state.reveal_pad_layer(0); // Empty pads cannot divide by zero.
}

#[test]
fn middle_drag_on_a_sample_pans_without_creating_a_file_drag_or_audition() {
    let ctx = egui::Context::default();
    let mut state = ConstellationState::new();
    state.samples.push(sample_fixture(0));
    let mut auditions = 0;
    let mut frame = |events| {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_starfield(
                        ui,
                        std::path::Path::new("/samples"),
                        &mut |_| auditions += 1,
                        &mut |_| {},
                    )
                });
            },
        )
    };
    frame(vec![]);
    let start = Pos2::new(400.0, 300.0);
    let end = start + Vec2::new(50.0, 40.0);
    frame(vec![
        egui::Event::PointerMoved(start),
        egui::Event::PointerButton {
            pos: start,
            button: egui::PointerButton::Middle,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        },
    ]);
    frame(vec![egui::Event::PointerMoved(end)]);
    frame(vec![egui::Event::PointerButton {
        pos: end,
        button: egui::PointerButton::Middle,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    }]);
    assert_eq!(state.pan, Vec2::new(50.0, 40.0));
    assert_eq!(auditions, 0);
    assert!(!egui::DragAndDrop::has_payload_of_type::<
        crate::file_manager::transfers::FileDrag,
    >(&ctx));
}

#[test]
fn exponential_duration_range_gives_short_samples_room_and_preserves_endpoints() {
    let (low, high) = (0.001, 600.0);
    let fraction = duration_fraction(0.1, low, high);
    assert!(
        fraction > 0.3,
        "100 ms should have substantial track space in a ten-minute library"
    );
    assert!(duration_at(0.5, low, high) < 2.0);
    assert_eq!(duration_at(0.0, low, high), low);
    assert_eq!(duration_at(1.0, low, high), high);
    for time in [0.001, 0.01, 0.1, 1.0, 60.0, 600.0] {
        assert!((duration_at(duration_fraction(time, low, high), low, high) - time).abs() < 0.001);
    }
    assert_eq!(duration_at(0.5, 0.0, 0.0), 0.0);
    assert_eq!(duration_fraction(0.0, 0.0, 0.0), 0.0);
}

#[test]
fn selecting_a_folded_set_keeps_its_dot_and_canvas_fixed_at_high_zoom() {
    let ctx = egui::Context::default();
    let mut state = ConstellationState::default();
    state.pack_shelf_open = false;
    state.samples = ["/a/piano_C3.wav", "/a/piano_C4.wav", "/a/piano_C5.wav"]
        .into_iter()
        .map(note_sample)
        .collect();
    for sample in &mut state.samples {
        sample.x = 0.1;
        sample.y = 0.1;
    }
    state.sample_sets = super::super::multisample::SampleSets::build(&state.samples);
    state.zoom = 3.0;
    let mut selections = 0;
    let mut frame = |state: &mut ConstellationState, events| {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 800.0))),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_ui(
                        ui,
                        std::path::Path::new("/a"),
                        &Default::default(),
                        &mut |_| {},
                        &mut |_| selections += 1,
                    );
                });
            },
        )
    };
    let center_of = |output: egui::FullOutput| {
        output
            .shapes
            .into_iter()
            .find_map(|shape| match shape.shape {
                egui::Shape::Mesh(mesh) if mesh.texture_id != egui::TextureId::default() => {
                    Some(mesh.calc_bounds().center())
                }
                _ => None,
            })
            .unwrap()
    };
    let _ = frame(&mut state, vec![]);
    let before = center_of(frame(&mut state, vec![]));
    for _ in 0..2 {
        for pressed in [true, false] {
            let _ = frame(
                &mut state,
                vec![
                    egui::Event::PointerMoved(before),
                    egui::Event::PointerButton {
                        pos: before,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
        let after = center_of(frame(&mut state, vec![]));
        assert!(
            (after - before).length() < 0.01,
            "selection must not move the set or resize the map"
        );
        assert!(
            state.expanded_set.is_none(),
            "only Explore set should expand rings"
        );
    }
    assert_eq!(selections, 2);
}

#[test]
fn right_drag_orbits_without_panning_auditioning_or_starting_a_file_drag() {
    let ctx = egui::Context::default();
    let mut state = ConstellationState::default();
    state.samples.push(sample_fixture(0));
    let original = state.camera;
    let start = Pos2::new(400.0, 300.0);
    let end = start + Vec2::new(80.0, 40.0);
    let mut auditions = 0;
    let _ = render_map_frame(&ctx, &mut state, vec![], &mut |_| auditions += 1);
    for (pos, pressed) in [(start, Some(true)), (end, None), (end, Some(false))] {
        let mut events = vec![egui::Event::PointerMoved(pos)];
        if let Some(pressed) = pressed {
            events.push(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Secondary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
        }
        let _ = render_map_frame(&ctx, &mut state, events, &mut |_| auditions += 1);
    }
    assert_ne!(state.camera, original);
    assert_eq!(state.pan, Vec2::ZERO);
    assert_eq!(auditions, 0);
    assert!(!egui::DragAndDrop::has_payload_of_type::<
        crate::file_manager::transfers::FileDrag,
    >(&ctx));
}

#[test]
fn ordinary_selection_and_collection_have_no_set_marker_rings() {
    let ctx = egui::Context::default();
    let mut state = ConstellationState::default();
    state.pack_shelf_open = false;
    state.samples.push(sample_fixture(0));
    state.selected_path = Some(state.samples[0].file_path.clone());
    let output = render_map_frame(
        &ctx,
        &mut state,
        vec![egui::Event::PointerMoved(Pos2::new(400.0, 300.0))],
        &mut |_| {},
    );
    assert!(!output.shapes.iter().any(|shape| matches!(&shape.shape,
            egui::Shape::Circle(circle) if circle.stroke.width > 0.0)));
}

#[test]
fn duration_handles_keep_the_grabbed_endpoint_and_reset_on_double_click() {
    let ctx = egui::Context::default();
    let mut state = ConstellationState::new();
    state.samples = (0..2).map(sample_fixture).collect();
    state.samples[1].duration_seconds = 2.0;
    let render = |state: &mut ConstellationState, events, time| {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(500.0, 300.0))),
                events,
                time: Some(time),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| state.render_duration_range(ui));
            },
        )
    };
    let output = render(&mut state, vec![], 0.0);
    let handles: Vec<_> = output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Circle(circle) if circle.radius == 6.0 => Some(circle.center),
            _ => None,
        })
        .collect();
    assert_eq!(handles.len(), 2);
    for (round, (start, fraction, expected_low, expected_high)) in [
        (handles[0], 0.75, 1.414, 0.0),
        (handles[1], 0.9, 1.414, 1.741),
    ]
    .into_iter()
    .enumerate()
    {
        let target = handles[0].lerp(handles[1], fraction);
        for phase in 0..3 {
            let pos = if phase == 0 { start } else { target };
            let mut events = vec![egui::Event::PointerMoved(pos)];
            if phase != 1 {
                events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: phase == 0,
                    modifiers: egui::Modifiers::NONE,
                });
            }
            render(&mut state, events, 0.1 + round as f64 + phase as f64 * 0.1);
        }
        assert!((state.min_duration - expected_low).abs() < 0.001);
        assert!((state.max_duration - expected_high).abs() < 0.001);
    }
    let pos = handles[0].lerp(handles[1], 0.5);
    for (index, pressed) in [true, false, true, false].into_iter().enumerate() {
        render(
            &mut state,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            3.0 + index as f64 * 0.05,
        );
    }
    assert_eq!((state.min_duration, state.max_duration), (0.0, 0.0));
}

#[test]
fn n_advances_only_when_unbound_and_outside_text_entry() {
    for (assigned, editing) in [(false, false), (true, false), (false, true)] {
        let ctx = egui::Context::default();
        let mut state = ConstellationState::new();
        state.samples = (0..2).map(sample_fixture).collect();
        state.samples[1].x = 0.1;
        state.selected_path = Some(state.samples[0].file_path.clone());
        if assigned {
            state.pad_keys[0] = 'n';
        }
        state.focus_search = editing;
        state.playback.lock().unwrap().show_midi_notes = true;
        let mut auditions = Vec::new();
        let _ = ctx.run(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: egui::Key::N,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    state.render_ui(
                        ui,
                        std::path::Path::new("/samples"),
                        &Default::default(),
                        &mut |path| auditions.push(path.to_owned()),
                        &mut |_| {},
                    )
                });
            },
        );
        assert_eq!(auditions.len(), usize::from(!assigned && !editing));
        assert_eq!(state.playback.lock().unwrap().show_midi_notes, editing);
    }
    assert_eq!(
        freq_to_note_str(440.0 * 2.0_f32.powf((36.0 - 69.0) / 12.0)),
        "C2"
    );
    assert_eq!(
        freq_to_note_str(440.0 * 2.0_f32.powf((51.0 - 69.0) / 12.0)),
        "D#3"
    );
}

#[test]
fn space_plays_only_the_active_editor_layer_through_the_bus() {
    use super::super::sample_pack::{CachedAudio, SlotSample};
    use std::sync::Arc;
    let mut state = ConstellationState::new();
    let map = sample_fixture(0);
    state.selected_path = Some(map.file_path.clone());
    state.samples.push(map);
    state.pack_bank.compressor_bypass = true;
    for value in [0.2, 0.6] {
        let mut layer = SlotSample::new("layer.wav".into(), "layer.wav".into(), None, 1.0);
        let pcm = Arc::new(vec![value; 48000]);
        layer.cached_audio = Some(Arc::new(CachedAudio {
            channels: 1,
            sample_rate: 48000,
            samples: pcm.clone(),
            mono_samples: pcm,
            waveform_peaks: vec![],
            waveform_colors: vec![],
        }));
        state.pack_bank.slots[0].samples.push(layer);
    }
    state.selected_layers[0] = 1;
    let (player, mut rendered) = rodio::Player::new();
    state.playback.lock().unwrap().output =
        Some(super::super::output::PadOutput::with_player(player, true));
    assert_eq!(state.space_audition(false), None);
    for _ in 0..4000 {
        rendered.next();
    }
    assert!((rendered.next().unwrap() - 0.6 * 0.7943282).abs() < 0.00001);
    assert!(state.stop_map_audition);
    state.pack_shelf_open = false;
    assert_eq!(
        state.space_audition(false),
        Some(state.samples[0].file_path.clone())
    );
}

#[test]
fn rendered_edge_dots_keep_one_alpha_at_all_four_vertices() {
    let ctx = egui::Context::default();
    let palette = little_clip_core::palette::AppPalette::default();
    let mut state = ConstellationState::new();
    state.pack_shelf_open = false;
    state.samples.push(sample_fixture(0));
    let texture = dot_texture(&ctx).id();
    let inspect = |output: egui::FullOutput| {
        output
            .shapes
            .into_iter()
            .find_map(|shape| match shape.shape {
                egui::Shape::Mesh(mesh) if mesh.texture_id == texture => Some((
                    shape.clip_rect,
                    mesh.calc_bounds(),
                    mesh.vertices.iter().map(|v| v.color).collect::<Vec<_>>(),
                )),
                _ => None,
            })
            .unwrap()
    };
    let size = Vec2::new(800.0, 600.0);
    let output = render_frame(&ctx, &mut state, &palette, size);
    let dividers: Vec<_> = output
        .shapes
        .iter()
        .filter_map(|shape| match shape.shape {
            egui::Shape::LineSegment { points, .. }
                if points[0].y == points[1].y && points[1].x - points[0].x > 500.0 =>
            {
                Some(points[0].y)
            }
            _ => None,
        })
        .collect();
    let (clip, dot, _) = inspect(output);
    assert!(
        dividers.iter().any(|y| (y - clip.top()).abs() < 0.5),
        "The header divider must meet the dot canvas without an extra top gap"
    );
    let target = Pos2::new(clip.left() + dot.width() * 0.5 + 3.0, clip.center().y);
    state.pan += target - dot.center();
    let (_, _, colors) = inspect(render_frame(&ctx, &mut state, &palette, size));
    assert_eq!(colors.len(), 4);
    assert!(colors.iter().all(|color| *color == colors[0]));
    assert!(colors[0].a() > 0 && colors[0].a() < 240);
    state.pan.x -= 3.0;
    state.selected_path = Some(state.samples[0].file_path.clone());
    let (_, _, colors) = inspect(render_frame(&ctx, &mut state, &palette, size));
    assert_eq!(
        colors.len(),
        16,
        "Selection retains its ordinary dot plus three depth-ordered highlight quads"
    );
    assert!(colors.iter().all(|color| color.a() == 0));
}

#[test]
fn circle_edges_fade_uniformly_and_disappear_before_being_cut_off() {
    let bounds = Rect::from_min_max(Pos2::ZERO, Pos2::new(200.0, 200.0));
    for radius in [0.35, 4.0, 8.0, 25.5] {
        for (clearance, expected) in [(-1.0, 0.0), (0.0, 0.0), (3.0, 0.5), (6.0, 1.0), (30.0, 1.0)]
        {
            let distance = radius + clearance;
            for point in [
                Pos2::new(distance, 100.0),
                Pos2::new(200.0 - distance, 100.0),
                Pos2::new(100.0, distance),
                Pos2::new(100.0, 200.0 - distance),
            ] {
                assert!((circle_edge_opacity(bounds, point, radius) - expected).abs() < 0.0001);
            }
        }
    }
    assert_eq!(circle_edge_opacity(bounds, Pos2::new(12.0, 4.0), 4.0), 0.0);
}

#[test]
fn status_count_includes_search_category_duration_and_channel_filters() {
    let mut state = ConstellationState::new();
    state.samples = (0..4).map(sample_fixture).collect();
    state.samples[1].duration_seconds = 2.0;
    state.samples[2].channels = 2;
    state.samples[3].category = SampleCategory::Snare;
    assert_eq!(state.sample_count_label(), "4 / 4 samples");
    state.channel_filter = 1;
    state.max_duration = 1.0;
    state.category_filter = Some(SampleCategory::Kick);
    assert_eq!(state.sample_count_label(), "1 / 4 samples");
    state.search_query = "absent".into();
    assert_eq!(state.sample_count_label(), "0 / 4 samples");
}

#[test]
fn overlap_cache_invalidates_on_coordinate_or_spacing_changes() {
    let mut layout = OverlapLayout::default();
    let points = vec![Pos2::ZERO; 2];
    let first = layout.positions(points.clone(), 10.0).to_vec();
    assert_eq!(layout.positions(points.clone(), 10.0), first);
    assert_ne!(layout.positions(points, 20.0), first);
    let changed = vec![Pos2::new(100.0, 100.0), Pos2::new(200.0, 200.0)];
    assert_eq!(layout.positions(changed.clone(), 20.0), changed);
}

#[test]
fn high_zoom_overlap_placement_moves_only_by_the_pan_delta() {
    let mut state = ConstellationState::new();
    state.zoom = 50.0;
    state.samples = (0..30)
        .map(|index| {
            let mut sample = sample_fixture(index);
            sample.x = (index % 5) as f32 * 0.0008;
            sample.y = (index / 5) as f32 * 0.0008;
            sample
        })
        .collect();
    let original = coarse_mesh(&mut state);
    assert_eq!(original.len(), 120);
    for pan in [Vec2::new(3.25, 5.75), Vec2::new(-16.0, 10.0), Vec2::ZERO] {
        state.pan = pan;
        let translated = coarse_mesh(&mut state);
        assert_eq!(translated.len(), original.len());
        for (before, after) in original.iter().zip(translated) {
            assert!(
                (after.pos - before.pos - pan).length() < 0.01,
                "Panning must not recompute a different overlap offset"
            );
        }
    }
}

#[test]
fn spacing_only_runs_at_extreme_zoom_and_is_bounded_for_coincident_points() {
    for zoom in [0.5, 1.0, 2.0, 5.0, 10.0, 20.0, 28.0] {
        assert_eq!(spacing_strength(zoom), 0.0);
    }
    assert_eq!(spacing_strength(50.0), 1.0);
    let point = Pos2::new(100.0, 100.0);
    let mut points = vec![point; 68_055];
    relax_star_positions(&mut points, 18.0);
    assert_eq!(points[0], point);
    for first in &points {
        assert!((*first - point).length() <= 64.001);
    }
    for other in &points[1..4] {
        assert!((*other - point).length() >= 18.0);
    }
    let mut repeat = vec![point; points.len()];
    relax_star_positions(&mut repeat, 18.0);
    assert_eq!(points, repeat);
}

#[test]
fn space_retriggers_and_neighbors_never_revisit_until_click_reset() {
    let mut state = ConstellationState::new();
    state.samples = (0..4)
        .map(|index| {
            let mut sample = sample_fixture(index);
            sample.x = index as f32 * 0.1;
            sample
        })
        .collect();
    state.selected_path = Some(state.samples[0].file_path.clone());
    let first = state.space_audition(false).unwrap();
    assert_eq!(state.space_audition(false), Some(first.clone()));
    for index in 1..4 {
        assert_eq!(
            state.space_audition(true),
            Some(state.samples[index].file_path.clone())
        );
    }
    assert_eq!(state.space_audition(true), None);
    state.visited_samples.clear(); // Explicit click starts a new exploration.
    state.selected_path = Some(first.clone());
    let sample = state.samples[0].clone();
    state.record_played_sample(&sample);
    state.channel_filter = 2;
    assert_eq!(state.space_audition(true), None, "respect active filters");
    state.channel_filter = 0;
    assert_eq!(
        state.space_audition(true),
        Some(state.samples[1].file_path.clone())
    );
    state.browser_open = true;
    assert_eq!(state.space_audition(false), None);
}

#[test]
fn all_points_share_one_radius_at_every_zoom_in_sparse_and_dense_libraries() {
    let viewport = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
    assert_eq!(map_point_radius(viewport, 100, 1.0), 4.0);
    assert!(map_point_radius(viewport, 68_055, 1.0) < 1.0);
    for count in [100, 68_055] {
        let mut state = ConstellationState::new();
        state.samples = (0..count).map(sample_fixture).collect();
        // A sparse outlier must have precisely the same size as the dense cluster.
        state.samples[0].x = 0.001;
        for zoom in [0.5, 1.0, 2.0, 5.0, 20.0, 50.0] {
            state.zoom = zoom;
            let mesh = coarse_mesh(&mut state);
            assert_eq!(mesh.len(), count * 4);
            let radii: Vec<_> = mesh
                .as_chunks::<4>()
                .0
                .iter()
                .map(|vertices| vertices[1].pos.x - vertices[0].pos.x)
                .collect();
            assert!(radii[0] > 0.0);
            assert!(
                radii
                    .iter()
                    .all(|radius| (*radius - radii[0]).abs() < 0.0001)
            );
        }
    }
}

#[test]
fn category_and_duration_exclusions_render_only_faint_soft_density() {
    use super::super::sample_pack::SlotSample;
    let ctx = egui::Context::default();
    let mut state = ConstellationState::default();
    state.samples = (0..3).map(sample_fixture).collect();
    for (index, sample) in state.samples.iter_mut().enumerate() {
        sample.x = (index as f32 - 1.0) * 0.4;
    }
    state.samples[1].category = SampleCategory::Snare;
    state.samples[1].duration_seconds = 2.0;
    state.samples[2].category = SampleCategory::Snare;
    state.category_filter = Some(SampleCategory::Snare);
    state.max_duration = 0.5;
    state.selected_path = Some(state.samples[0].file_path.clone());
    state.pack_bank.slots[0].samples.push(SlotSample::new(
        state.samples[0].file_path.clone(),
        "excluded.wav".into(),
        None,
        0.5,
    ));
    for sample in state.samples[..2].to_vec() {
        state.record_played_sample(&sample);
    }
    let output = render_map_frame(&ctx, &mut state, vec![], &mut |_| {});
    let meshes: Vec<_> = output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Mesh(mesh) if mesh.texture_id != egui::TextureId::default() => Some(mesh),
            _ => None,
        })
        .collect();
    let soft = meshes
        .iter()
        .find(|mesh| mesh.vertices.iter().all(|vertex| vertex.uv.x >= 0.5))
        .unwrap();
    assert_eq!(
        soft.vertices.len(),
        8,
        "both category and duration exclusions use the soft texture"
    );
    assert!(soft.vertices.iter().all(|vertex| vertex.color.a() <= 3));
    assert!(
        soft.vertices.iter().all(|vertex| {
            let color = vertex.color;
            color.r().max(color.g()).max(color.b()) <= color.a()
        }),
        "context tint must remain premultiplied for gamma-space blending"
    );
    let sharp = meshes
        .iter()
        .find(|mesh| mesh.vertices.iter().any(|vertex| vertex.uv.x < 0.5))
        .unwrap();
    assert_eq!(sharp.vertices.len(), 4);
    assert!(sharp.vertices.iter().all(|vertex| vertex.color.a() == 255));
    assert!(
        !output
            .shapes
            .iter()
            .any(|shape| matches!(shape.shape, egui::Shape::Circle(_))),
        "excluded selection, collection and history must not produce bright markers"
    );
    let atlas = output
        .textures_delta
        .set
        .iter()
        .find_map(|(_, delta)| match &delta.image {
            egui::ImageData::Color(image) if image.size == [64, 32] => Some(image),
            _ => None,
        })
        .unwrap();
    assert!(atlas[(21, 15)].a() > 240);
    assert!(
        atlas[(53, 15)].a() < 180,
        "the context profile must fall off well inside the disk"
    );
    assert!(atlas[(60, 15)].a() < 20);
    assert_eq!(atlas[(63, 15)].a(), 0);
    assert!(
        (32..64).all(|x| (0..32).all(|y| atlas[(x, y)].r() == atlas[(x, y)].a())),
        "soft texture must not brighten overlapping samples on a light canvas"
    );
}

#[test]
fn dense_coarse_mesh_keeps_every_point_and_all_filter_matches() {
    let count = 68_055;
    let mut state = ConstellationState::new();
    state.samples = (0..count).map(sample_fixture).collect();
    assert_eq!(
        coarse_mesh(&mut state).len(),
        count * 4,
        "Every point is rendered, including coincident samples"
    );
    state.category_filter = Some(SampleCategory::Snare);
    let solo = coarse_mesh(&mut state);
    assert_eq!(
        solo.len(),
        count * 4,
        "filtering dims points without omitting them"
    );
    assert!(
        solo.iter()
            .any(|vertex| vertex.color == SampleCategory::Snare.color())
    );
    state.category_filter = None;
    state.search_query = "target".into();
    let search = coarse_mesh(&mut state);
    assert_eq!(search.len(), count * 4);
    assert!(
        search
            .iter()
            .any(|vertex| vertex.color == SampleCategory::Snare.color())
    );
    state.min_duration = 0.6;
    assert!(!state.matches_filters(999, "target"));
    state.min_duration = 0.0;
    state.max_duration = 0.25;
    assert!(!state.matches_filters(999, "target"));
    state.max_duration = 0.0;
    state.channel_filter = 2;
    assert!(!state.matches_filters(999, "target"));
    state.channel_filter = 1;
    assert!(state.matches_filters(999, "target"));
    state.channel_filter = 0;
    state.samples.truncate(1);
    state.samples[0].x = -2.3;
    state.samples[0].y = 1.5;
    state.search_query.clear();
    let fitted = coarse_mesh(&mut state);
    assert_eq!(
        fitted.len(),
        4,
        "a sample beyond the unit square stays visible"
    );
    assert!(
        fitted
            .iter()
            .all(|vertex| (8.0..792.0).contains(&vertex.pos.x)
                && (8.0..592.0).contains(&vertex.pos.y))
    );
}

pub(super) fn render_frame(
    ctx: &egui::Context,
    state: &mut ConstellationState,
    palette: &little_clip_core::palette::AppPalette,
    size: Vec2,
) -> egui::FullOutput {
    ctx.run(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                state.render_ui(
                    ui,
                    std::path::Path::new("/samples"),
                    palette,
                    &mut |_| {},
                    &mut |_| {},
                );
            });
        },
    )
}

#[test]
fn mixer_controls_and_filename_tabs_remain_visible_at_every_editor_width() {
    use super::super::sample_pack::SlotSample;
    for appearance in [
        little_clip_core::config::WorkbenchAppearance::Dark,
        little_clip_core::config::WorkbenchAppearance::Light,
    ] {
        let ctx = egui::Context::default();
        crate::ui_ext::configure_workbench_style(&ctx);
        ctx.style_mut(|style| style.animation_time = 0.0);
        let palette = little_clip_core::palette::AppPalette::for_appearance(appearance);
        crate::ui_appearance::apply_visuals(&ctx, &palette);
        let mut state = ConstellationState::new();
        state.samples.push(sample_fixture(0));
        state.pad_editor_open = true;
        state.pack_bank.slots[0].choke_group = 1;
        state.pack_bank.slots[1].choke_group = 1;
        state.pack_bank.slots[0].choke_priority = 2;
        state.pack_bank.slots[0].samples.push(SlotSample::new(
            "/samples/kick.wav".into(),
            "kick.wav".into(),
            None,
            1.0,
        ));
        for size in [
            Vec2::new(840.0, 650.0),
            Vec2::new(1400.0, 1000.0),
            Vec2::new(2200.0, 1300.0),
            Vec2::new(3200.0, 1800.0),
        ] {
            render_frame(&ctx, &mut state, &palette, size);
            let output = render_frame(&ctx, &mut state, &palette, size);

            let editor = egui::containers::panel::PanelState::load(
                &ctx,
                egui::Id::new("constellation_selected_pad"),
            )
            .unwrap()
            .rect;
            assert!(
                !output.shapes.iter().any(|shape| matches!(&shape.shape,
                    egui::Shape::Rect(rect) if rect.rect.width() <= 12.0
                        && rect.rect.height() > 40.0
                        && rect.rect.right() > editor.right() - 24.0
                        && editor.contains_rect(rect.rect))),
                "The default editor should not need a vertical scrollbar at {size:?}"
            );
            for wanted in [
                "kick.wav",
                "Gain",
                "Pan",
                "Pitch",
                "Speed",
                "Retrigger",
                "+1.00×",
                "+0 oct",
                "A",
                "R",
                "Edit in Audio Studio",
            ] {
                assert!(
                    output.shapes.iter().any(|shape| match &shape.shape {
                        egui::Shape::Text(text) if text.galley.text() == wanted => {
                            shape.clip_rect.contains_rect(
                                Rect::from_min_size(text.pos, text.galley.size()).shrink(0.5),
                            )
                        }
                        _ => false,
                    }),
                    "{wanted} is clipped at {size:?} in {appearance:?}"
                );
            }
            let center = |label: &str| {
                output
                    .shapes
                    .iter()
                    .find_map(|shape| match &shape.shape {
                        egui::Shape::Text(text) if text.galley.text() == label => {
                            Some(text.pos + text.galley.size() * 0.5)
                        }
                        _ => None,
                    })
                    .unwrap()
            };
            for label in ["+1.00×", "+0 oct"] {
                let point = center(label);
                let readout = output
                    .shapes
                    .iter()
                    .find_map(|shape| match &shape.shape {
                        egui::Shape::Rect(rect)
                            if rect.rect.contains(point) && rect.rect.width() < 100.0 =>
                        {
                            Some((shape.clip_rect, rect))
                        }
                        _ => None,
                    })
                    .unwrap();
                assert!(
                    readout
                        .0
                        .contains_rect(readout.1.rect.expand(readout.1.stroke.width * 0.5)),
                    "{label} border clipped at {size:?}: {readout:?}"
                );
            }
            let pitch_readout = center("+0 oct");
            let studio = center("Edit in Audio Studio");
            assert!(
                studio.y > pitch_readout.y + 10.0 || studio.x > pitch_readout.x + 95.0,
                "Studio must follow the complete pitch readout without overlap: {studio:?}, {pitch_readout:?}"
            );
            assert!(center("Pan").y + 2.0 >= center("Retrigger").y);
            assert!(center("Pan").y > center("Gain").y);
            assert!(!output.shapes.iter().any(|shape| matches!(&shape.shape,
                    egui::Shape::Circle(circle) if circle.radius == 13.0)));
            assert!(!output.shapes.iter().any(|shape| matches!(&shape.shape,
                    egui::Shape::Text(text) if text.galley.text() == "P" || text.galley.text() == "T")));
            assert!(!output.shapes.iter().any(|shape| matches!(&shape.shape,
                    egui::Shape::Text(text) if text.galley.text().contains("drag IN / OUT")
                        || text.galley.text() == "Start" || text.galley.text() == "End")));
        }
    }
}

#[test]
fn docks_do_not_move_visible_stars_at_coarse_or_relaxed_zoom() {
    for zoom in [1.0, 2.5] {
        let ctx = egui::Context::default();
        let palette = little_clip_core::palette::AppPalette::default();
        let mut state = ConstellationState::new();
        state.zoom = zoom;
        let mut sample = sample_fixture(0);
        // Stay in the top-left exposed portion even when both docks are open.
        sample.x = -0.4 / zoom;
        sample.y = 0.4 / zoom;
        state.samples.push(sample);
        state.pack_bank.slots[0]
            .samples
            .push(super::super::sample_pack::SlotSample::new(
                "/samples/0.wav".into(),
                "0.wav".into(),
                None,
                0.5,
            ));
        let mut expected = None;
        for rack in [true, false, true] {
            state.pack_shelf_open = rack;
            for editor in [false, true, false] {
                state.pad_editor_open = editor;
                let output = render_frame(&ctx, &mut state, &palette, Vec2::new(1000.0, 800.0));
                let points: Vec<_> = output
                    .shapes
                    .into_iter()
                    .filter_map(|shape| match shape.shape {
                        egui::Shape::Mesh(mesh) => Some(
                            mesh.vertices
                                .iter()
                                .filter(|vertex| vertex.color == SampleCategory::Kick.color())
                                .map(|vertex| vertex.pos)
                                .collect::<Vec<_>>(),
                        ),
                        _ => None,
                    })
                    .flatten()
                    .collect();
                assert_eq!(points.len(), 4, "fixture star must remain visible");
                if let Some(ref expected) = expected {
                    assert_eq!(&points, expected);
                }
                expected = Some(points);
            }
        }
    }
}

#[test]
fn rack_layout_does_not_grow_across_frames_or_open_a_pack_window() {
    for appearance in [
        little_clip_core::config::WorkbenchAppearance::Dark,
        little_clip_core::config::WorkbenchAppearance::Light,
    ] {
        let ctx = egui::Context::default();
        crate::ui_ext::configure_workbench_style(&ctx);
        let palette = little_clip_core::palette::AppPalette::for_appearance(appearance);
        crate::ui_appearance::apply_visuals(&ctx, &palette);
        let mut state = ConstellationState::new();
        for size in [Vec2::new(840.0, 550.0), Vec2::new(1200.0, 1000.0)] {
            for frame in 0..120 {
                render_frame(&ctx, &mut state, &palette, size);
                assert_eq!(state.pad_drop_targets.len(), 16);
                assert!(
                    state
                        .pad_drop_targets
                        .iter()
                        .all(|(rect, _)| rect.height() >= 27.9),
                    "every pad must remain visible at the compact window size"
                );
                if size.y >= 1000.0 {
                    for (rect, _) in &state.pad_drop_targets {
                        assert!(
                            (rect.width() - rect.height()).abs() < 1.0 && rect.height() > 44.0,
                            "pad bounds {rect:?}, frame {frame}, size {size:?}"
                        );
                    }
                }
                let panel = egui::containers::panel::PanelState::load(
                    &ctx,
                    egui::Id::new("constellation_pack_shelf_panel"),
                )
                .unwrap();
                assert!(
                    panel.rect.width() <= 361.0,
                    "rack expanded on frame {frame}: {:?}",
                    panel.rect
                );
            }
        }
        let id = egui::Id::new("constellation_pack_shelf_panel");
        let mut user_panel = egui::containers::panel::PanelState::load(&ctx, id).unwrap();
        user_panel.rect.min.x = user_panel.rect.max.x - 312.0;
        ctx.data_mut(|data| data.insert_persisted(id, user_panel));
        for _ in 0..30 {
            render_frame(&ctx, &mut state, &palette, Vec2::new(1200.0, 1000.0));
            assert!(
                (egui::containers::panel::PanelState::load(&ctx, id)
                    .unwrap()
                    .rect
                    .width()
                    - 312.0)
                    .abs()
                    < 1.0
            );
        }
        state.pack_shelf_open = false;
        render_frame(&ctx, &mut state, &palette, Vec2::new(1200.0, 1000.0));
        state.pack_shelf_open = true;
        render_frame(&ctx, &mut state, &palette, Vec2::new(1200.0, 1000.0));
        assert!(
            (egui::containers::panel::PanelState::load(&ctx, id)
                .unwrap()
                .rect
                .width()
                - 312.0)
                .abs()
                < 1.0
        );
        state.browser_open = true;
        render_frame(&ctx, &mut state, &palette, Vec2::new(840.0, 550.0));
        state.browser_open = false;
        state.output_editor_open = true;
        render_frame(&ctx, &mut state, &palette, Vec2::new(840.0, 550.0));
        let panel = egui::containers::panel::PanelState::load(
            &ctx,
            egui::Id::new("constellation_pack_shelf_panel"),
        )
        .unwrap();
        assert!(panel.rect.width() <= 361.0 && panel.rect.bottom() <= 550.0);
        assert!(!ctx.memory(|memory| {
            memory
                .areas()
                .visible_layer_ids()
                .iter()
                .any(|layer| layer.order == egui::Order::Middle)
        }));
    }
}

#[test]
fn pad_assignment_uses_configured_keys_and_only_text_focus_blocks_it() {
    let ctx = egui::Context::default();
    let mut state = ConstellationState::new();
    let sample = ConstellationSample {
        file_path: "/samples/kick.wav".into(),
        folder_path: "/samples".into(),
        filename: "kick.wav".into(),
        category: SampleCategory::Kick,
        confidence: 1.0,
        duration_seconds: 0.5,
        pitch_hz: 60.0,
        pitch_clarity: 0.0,
        brightness: 0.0,
        x: 0.0,
        y: 0.0,
        file_size: 100,
        mtime: 0,
        sample_rate: 44100,
        channels: 1,
        bit_depth: 16,
        source_revision: None,
    };
    state.selected_path = Some(sample.file_path.clone());
    state.samples.push(sample);
    let _ = ctx.run(Default::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.button("Canvas control").request_focus();
        });
    });
    assert!(ctx.wants_keyboard_input());
    let input = || egui::RawInput {
        events: vec![
            egui::Event::Key {
                // Some layouts map Shift+digit to a different supported logical key.
                key: egui::Key::Plus,
                physical_key: Some(egui::Key::Num1),
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::SHIFT,
            },
            egui::Event::Key {
                // Virtual keyboards can assign digits to arbitrary physical codes.
                key: egui::Key::Num2,
                physical_key: Some(egui::Key::A),
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::SHIFT,
            },
        ],
        ..Default::default()
    };
    let _ = ctx.run(input(), |ctx| {
        state.handle_shelf_keyboard_shortcuts(ctx);
        assert!(ctx.input(|input| input.events.is_empty()));
    });
    assert_eq!(state.pack_bank.slots[12].samples.len(), 1);
    assert_eq!(state.pack_bank.slots[13].samples.len(), 1);
    let mut text = String::new();
    let _ = ctx.run(Default::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.text_edit_singleline(&mut text).request_focus();
        });
    });
    let _ = ctx.run(input(), |ctx| {
        state.handle_shelf_keyboard_shortcuts(ctx);
        assert!(!ctx.input(|input| input.events.is_empty()));
    });
    assert_eq!(state.pack_bank.slots[12].samples.len(), 1);
}

#[test]
fn escape_clears_search_before_the_text_edit_surrenders_focus() {
    let ctx = egui::Context::default();
    let mut state = ConstellationState::new();
    state.search_query = "kick".into();
    state.focus_search = true;
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            state.render_toolbar(
                ui,
                std::path::Path::new("/samples"),
                &mut |_| {},
                &little_clip_core::palette::AppPalette::default(),
            );
        });
    });
    assert!(ctx.wants_keyboard_input());
    let input = egui::RawInput {
        events: vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
        ..Default::default()
    };
    let _ = ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            state.render_toolbar(
                ui,
                std::path::Path::new("/samples"),
                &mut |_| {},
                &little_clip_core::palette::AppPalette::default(),
            );
        });
    });
    assert!(state.search_query.is_empty());
}

#[test]
fn unicode_sample_names_are_shortened_without_splitting_utf8() {
    let name = "打楽器のサンプル🎵_café_very_long_sample.wav";
    let short = short_label(name, 22);
    assert_eq!(short.chars().count(), 22);
    assert!(short.ends_with("..."));
    assert_eq!(short_label("短い.wav", 22), "短い.wav");
}
