//! Unit tests for perceptual similarity clustering, grid layout, and canvas transforms.
use std::path::PathBuf;

use super::{
    dhash::{compute_dhash, hamming_distance},
    embedding::{FeatureVector, project_hashes_to_2d},
    grid::{grid_dimensions, snap_to_grid},
    *,
};
use crate::file_manager::filesystem::Entry;
use little_clip_core::ImageThumbnail;

fn create_test_thumbnail(
    width: u32,
    height: u32,
    fill: impl Fn(usize, usize) -> [u8; 4],
) -> ImageThumbnail {
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height as usize {
        for x in 0..width as usize {
            let pixel = fill(x, y);
            rgba.extend_from_slice(&pixel);
        }
    }
    ImageThumbnail {
        width,
        height,
        premultiplied_rgba: rgba,
    }
}

#[test]
fn test_dhash_constant_and_gradient() {
    // Constant image: every pixel is identical => x < x+1 is always false => hash is 0
    let constant = create_test_thumbnail(18, 16, |_, _| [128, 128, 128, 255]);
    let h_const = compute_dhash(&constant);
    assert_eq!(h_const, 0);

    // Left-to-right ascending gradient: x < x+1 is always true => all 64 bits set
    let gradient = create_test_thumbnail(18, 16, |x, _| {
        let val = (x * 14).min(255) as u8;
        [val, val, val, 255]
    });
    let h_grad = compute_dhash(&gradient);
    assert_eq!(h_grad, !0u64);

    // Inverted gradient (descending): x < x+1 is always false => all bits 0
    let inv_gradient = create_test_thumbnail(18, 16, |x, _| {
        let val = 255 - (x * 14).min(255) as u8;
        [val, val, val, 255]
    });
    let h_inv = compute_dhash(&inv_gradient);
    assert_eq!(h_inv, 0);

    // Hamming distance between ascending and descending gradients is 64
    assert_eq!(hamming_distance(h_grad, h_inv), 64);
}

#[test]
fn test_dhash_similarity_metric() {
    // Two nearly identical images with slight noise
    let base = create_test_thumbnail(18, 16, |x, y| {
        let val = ((x * 20 + y * 5) % 256) as u8;
        [val, val, val, 255]
    });
    let noisy = create_test_thumbnail(18, 16, |x, y| {
        let delta = if (x + y) % 3 == 0 { 2 } else { 0 };
        let val = ((x * 20 + y * 5 + delta) % 256) as u8;
        [val, val, val, 255]
    });
    let h1 = compute_dhash(&base);
    let h2 = compute_dhash(&noisy);

    let distance = hamming_distance(h1, h2);
    assert!(
        distance <= 8,
        "Noisy variation should have small Hamming distance, got {distance}"
    );
}

#[test]
fn test_pca_embedding_bounds_and_clusters() {
    assert!(project_hashes_to_2d(&[]).is_empty());
    assert_eq!(project_hashes_to_2d(&[0x1234]), vec![(0.0, 0.0)]);

    // Two distinct clusters:
    // Cluster A: hashes around 0x0000_0000_FFFF_FFFF
    // Cluster B: hashes around 0xFFFF_FFFF_0000_0000
    let mut hashes = Vec::new();
    for i in 0..10 {
        hashes.push(0x0000_0000_FFFF_FFFF ^ (1u64 << i));
    }
    for i in 0..10 {
        hashes.push(0xFFFF_FFFF_0000_0000 ^ (1u64 << (32 + i)));
    }

    let coords = project_hashes_to_2d(&hashes);
    assert_eq!(coords.len(), 20);

    // Check cluster separation: average position of Cluster A vs Cluster B
    let avg_a_x: f32 = coords[0..10].iter().map(|c| c.0).sum::<f32>() / 10.0;
    let avg_b_x: f32 = coords[10..20].iter().map(|c| c.0).sum::<f32>() / 10.0;

    let dist = (avg_a_x - avg_b_x).abs();
    assert!(
        dist > 0.5,
        "Clusters should be distinctly separated along principal axis, got distance {dist}"
    );
}

#[test]
fn test_high_dimensional_embedding_gemma_support() {
    // Test with 728-dimensional feature vectors (Gemma Embedding 2 shape)
    const D: usize = 728;
    let mut features = Vec::new();

    // Group 1: strong positive signal in first 100 dimensions
    for i in 0..5 {
        let mut v = vec![0.0f32; D];
        for j in 0..100 {
            v[j] = 1.0 + (i as f32) * 0.05;
        }
        features.push(v);
    }

    // Group 2: strong positive signal in dimensions 100..200
    for i in 0..5 {
        let mut v = vec![0.0f32; D];
        for j in 100..200 {
            v[j] = 1.0 + (i as f32) * 0.05;
        }
        features.push(v);
    }

    let coords = super::embedding::project_features_to_2d(&features);
    assert_eq!(coords.len(), 10);

    let avg1: f32 = coords[0..5].iter().map(|c| c.0).sum::<f32>() / 5.0;
    let avg2: f32 = coords[5..10].iter().map(|c| c.0).sum::<f32>() / 5.0;
    assert!(
        (avg1 - avg2).abs() > 0.5,
        "728-dim clusters must be separated along 1st component"
    );
}

#[test]
fn test_grid_dimensions() {
    assert_eq!(grid_dimensions(0), (0, 0));
    assert_eq!(grid_dimensions(1), (1, 1));
    assert_eq!(grid_dimensions(2), (2, 1));
    assert_eq!(grid_dimensions(4), (2, 2));
    assert_eq!(grid_dimensions(9), (3, 3));
    assert_eq!(grid_dimensions(10), (4, 3));
    assert_eq!(grid_dimensions(16), (4, 4));
    assert_eq!(grid_dimensions(25), (5, 5));
    assert_eq!(grid_dimensions(50), (8, 7));
}

#[test]
fn test_grid_snapping_invariants() {
    for n in [1, 2, 5, 8, 16, 23, 49, 100] {
        let (cols, rows) = grid_dimensions(n);
        assert!(cols * rows >= n);

        // Generate synthetic points
        let points: Vec<(f32, f32)> = (0..n)
            .map(|i| {
                let theta = i as f32 * 0.3;
                (theta.cos(), theta.sin())
            })
            .collect();

        let placements = snap_to_grid(&points);
        assert_eq!(placements.len(), n);

        let mut seen_cells = std::collections::HashSet::new();
        for p in &placements {
            assert!(p.col < cols, "col {} must be < {}", p.col, cols);
            assert!(p.row < rows, "row {} must be < {}", p.row, rows);
            assert!(
                seen_cells.insert((p.col, p.row)),
                "Duplicate cell assignment at ({}, {})",
                p.col,
                p.row
            );
        }
    }
}

#[test]
fn test_similarity_canvas_initial_fit() {
    let mut canvas = SimilarityCanvas::default();
    canvas.tile_size = 100.0;
    canvas.spacing = 4.0;

    // Simulate 16 items in a 4x4 grid
    canvas.grid_cols = 4;
    canvas.grid_rows = 4;

    let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
    canvas.fit_to_view(viewport);

    assert!(canvas.zoom > 0.1 && canvas.zoom < 4.0);
    assert!(canvas.pan.x >= 0.0);
    assert!(canvas.pan.y >= 0.0);
}

#[test]
fn test_siglip_giant_1536d_embedding_and_normalization() {
    use super::embedding::FeatureVector;
    use super::siglip::SIGLIP_BATCH_SIZE;

    assert_eq!(SIGLIP_BATCH_SIZE, 8);

    // SigLIP 2 Giant embedding dimension: 1536
    const D: usize = 1536;
    let mut features = Vec::new();

    // Group 1: strong signal in dimensions 0..100
    for i in 0..5 {
        let mut v = vec![0.0f32; D];
        for j in 0..100 {
            v[j] = 1.0 + (i as f32) * 0.02;
        }
        let feat = FeatureVector::Dense(v);
        assert_eq!(feat.dimension(), D);
        features.push(feat.to_vec());
    }

    // Group 2: strong signal in dimensions 200..300
    for i in 0..5 {
        let mut v = vec![0.0f32; D];
        for j in 200..300 {
            v[j] = 1.0 + (i as f32) * 0.02;
        }
        let feat = FeatureVector::Dense(v);
        features.push(feat.to_vec());
    }

    let coords = super::embedding::project_features_to_2d(&features);
    assert_eq!(coords.len(), 10);

    let avg1: f32 = coords[0..5].iter().map(|c| c.0).sum::<f32>() / 5.0;
    let avg2: f32 = coords[5..10].iter().map(|c| c.0).sum::<f32>() / 5.0;
    assert!(
        (avg1 - avg2).abs() > 0.5,
        "1536-dim SigLIP 2 clusters must be distinctly separated"
    );
}

#[test]
fn test_heterogeneous_feature_vector_pca() {
    use super::embedding::FeatureVector;

    // Mix of 64-bit dhash and 1536-bit dense features
    let mut features = Vec::new();
    features.push(FeatureVector::DHash(0x1234_5678_9ABC_DEF0).to_vec());
    features.push(FeatureVector::DHash(0xFEDC_BA98_7654_3210).to_vec());

    let mut dense1 = vec![0.0f32; 1536];
    dense1[0] = 1.0;
    features.push(FeatureVector::Dense(dense1).to_vec());

    let mut dense2 = vec![0.0f32; 1536];
    dense2[1] = 1.0;
    features.push(FeatureVector::Dense(dense2).to_vec());

    let coords = super::embedding::project_features_to_2d(&features);
    assert_eq!(coords.len(), 4);
    for (x, y) in coords {
        assert!(x.is_finite());
        assert!(y.is_finite());
        assert!(x >= -1.0 && x <= 1.0);
        assert!(y >= -1.0 && y <= 1.0);
    }
}

#[test]
fn test_multi_grid_search_retention_and_chaining() {
    use super::canvas::SimilarityCanvas;
    use super::embedding::FeatureVector;
    use crate::file_manager::filesystem::Entry;
    use std::path::PathBuf;

    let mut canvas = SimilarityCanvas::default();
    canvas.similarity_threshold = 0.5;

    let count = 12;
    let mut entries = Vec::new();
    for i in 0..count {
        let path = PathBuf::from(format!("/test_img_{i}.png"));
        entries.push(Entry {
            path: path.clone(),
            name: format!("img_{i}.png"),
            directory: false,
            regular_file: true,
            system: None,
            typescript: false,
            symlink: false,
            hidden: false,
            size: "1 KB".to_string(),
            size_bytes: 1024,
            modified: "today".to_string(),
            modified_time: None,
            warning: None,
            revision: 1,
            identity: None,
        });

        // 1536-dimensional feature vectors:
        // Items 0..4 have high signal in dimension 0 (concept "animal")
        // Items 0..2 also have high signal in dimension 2 (concept "indoors")
        // Items 4..12 have high signal in dimension 1 (other)
        let mut vec = vec![0.0f32; 1536];
        if i < 4 {
            vec[0] = 1.0;
        } else {
            vec[1] = 1.0;
        }
        if i < 2 {
            vec[2] = 1.0;
        }
        canvas.insert_hash(path, FeatureVector::Dense(vec), 1);
    }

    canvas.set_entries(entries);

    // Initial state: Base grid retains all 12 photos
    assert_eq!(canvas.grids.len(), 1);
    assert_eq!(canvas.grids[0].id, 0);
    assert_eq!(canvas.grids[0].indices.len(), 12);
    assert!(canvas.grids[0].selected_for_search);

    // 1. Search for concept "animal" (pointing along dimension 0)
    let mut animal_concept = vec![0.0f32; 1536];
    animal_concept[0] = 1.0;

    let candidate_indices = canvas.grids[0].indices.clone();
    let grid1_id = canvas.add_search_grid("animal", false, candidate_indices, animal_concept);

    assert_eq!(canvas.grids.len(), 2);
    // Base grid STILL retains all 12 photos!
    assert_eq!(canvas.grids[0].indices.len(), 12);
    // Grid 1 contains exactly the 4 matching photos
    assert_eq!(canvas.grids[1].id, grid1_id);
    assert_eq!(canvas.grids[1].indices.len(), 4);
    assert_eq!(canvas.grids[1].indices, vec![0, 1, 2, 3]);

    // Grid 1 was automatically selected for next search, Base grid deselected
    assert!(!canvas.grids[0].selected_for_search);
    assert!(canvas.grids[1].selected_for_search);

    // Verify layout placements: Grid 0 cols < Grid 1 col_offset (appears to the right)
    let base_max_col = canvas.grids[0].col_offset + canvas.grids[0].grid_cols;
    assert!(canvas.grids[1].col_offset >= base_max_col);

    // 2. Chained subset search: search for "indoors" within selected Grid 1 ("animal")
    let mut indoors_concept = vec![0.0f32; 1536];
    indoors_concept[2] = 1.0;

    // Candidates are collected from selected_for_search grids (which is Grid 1)
    let chained_candidates: Vec<usize> = canvas
        .grids
        .iter()
        .filter(|g| g.selected_for_search)
        .flat_map(|g| g.indices.iter().copied())
        .collect();
    assert_eq!(chained_candidates, vec![0, 1, 2, 3]);

    let grid2_id = canvas.add_search_grid("indoors", false, chained_candidates, indoors_concept);

    assert_eq!(canvas.grids.len(), 3);
    // Base grid still retains all 12 photos!
    assert_eq!(canvas.grids[0].indices.len(), 12);
    // Grid 1 still has 4 photos
    assert_eq!(canvas.grids[1].indices.len(), 4);
    // Grid 2 has the subset (items 0 and 1, which have indoors signal)
    assert_eq!(canvas.grids[2].id, grid2_id);
    assert_eq!(canvas.grids[2].indices.len(), 2);
    assert_eq!(canvas.grids[2].indices, vec![0, 1]);

    // 3. Inverted search: search for "!animal" from base grid (all photos)
    let mut inv_animal_concept = vec![0.0f32; 1536];
    inv_animal_concept[0] = 1.0;

    let base_candidates = canvas.grids[0].indices.clone();
    let grid3_id = canvas.add_search_grid("!animal", true, base_candidates, inv_animal_concept);

    assert_eq!(canvas.grids.len(), 4);
    // Inverted search should match items 4..12 (items without animal signal in dim 0)
    assert_eq!(canvas.grids[3].id, grid3_id);
    assert_eq!(canvas.grids[3].indices.len(), 8);
    for idx in &canvas.grids[3].indices {
        assert!(
            *idx >= 4,
            "Inverted search must only match non-animal items, got {idx}"
        );
    }

    // 4. Test solo selection (Ctrl+click logic): solo select Grid 0
    let solo_id = 0;
    for g in &mut canvas.grids {
        g.selected_for_search = g.id == solo_id;
    }
    assert!(canvas.grids[0].selected_for_search);
    assert!(!canvas.grids[1].selected_for_search);
    assert!(!canvas.grids[2].selected_for_search);
    assert!(!canvas.grids[3].selected_for_search);

    // 5. Test removing a search grid: remove Grid 1
    canvas.remove_grid(grid1_id);
    assert_eq!(canvas.grids.len(), 3);
    assert!(!canvas.grids.iter().any(|g| g.id == grid1_id));
    assert_eq!(canvas.grids[0].indices.len(), 12);

    // 6. Test clear all searches
    canvas.clear_all_searches();
    assert_eq!(canvas.grids.len(), 1);
    assert_eq!(canvas.grids[0].id, 0);
    assert_eq!(canvas.grids[0].indices.len(), 12);
    assert!(canvas.grids[0].selected_for_search);
}

#[test]
fn test_search_pan_to_grid_results() {
    use super::canvas::SimilarityCanvas;
    use super::embedding::FeatureVector;
    use crate::file_manager::filesystem::Entry;
    use egui::{Rect, pos2, vec2};
    use std::path::PathBuf;

    let mut canvas = SimilarityCanvas::default();
    canvas.similarity_threshold = 0.5;

    let count = 12;
    let mut entries = Vec::new();
    for i in 0..count {
        let path = PathBuf::from(format!("/test_pan_img_{i}.png"));
        entries.push(Entry {
            path: path.clone(),
            name: format!("img_{i}.png"),
            directory: false,
            regular_file: true,
            system: None,
            typescript: false,
            symlink: false,
            hidden: false,
            size: "1 KB".to_string(),
            size_bytes: 1024,
            modified: "today".to_string(),
            modified_time: None,
            warning: None,
            revision: 1,
            identity: None,
        });

        let mut vec = vec![0.0f32; 1536];
        if i < 4 {
            vec[0] = 1.0;
        } else {
            vec[1] = 1.0;
        }
        canvas.insert_hash(path, FeatureVector::Dense(vec), 1);
    }

    canvas.set_entries(entries);

    let viewport = Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 800.0));
    canvas.fit_to_view(viewport);

    // Initial state: no pending pan
    assert_eq!(canvas.pending_pan_to_grid, None);

    // Add search grid
    let mut concept = vec![0.0f32; 1536];
    concept[0] = 1.0;
    let candidate_indices = canvas.grids[0].indices.clone();
    let grid_id = canvas.add_search_grid("query", false, candidate_indices, concept);

    // add_search_grid sets pending_pan_to_grid
    assert_eq!(canvas.pending_pan_to_grid, Some(grid_id));

    // Pan to grid
    canvas.pan_to_grid(grid_id, viewport);

    // Check that the top-left tile of the result grid is now in frame
    let result_grid = canvas.grids.iter().find(|g| g.id == grid_id).unwrap();
    let top_tile_rect =
        canvas.tile_screen_rect(viewport.min, result_grid.col_offset, result_grid.row_offset);

    // The tile should be positioned cleanly below the header bar in the viewport (y == 122.0)
    assert!((top_tile_rect.min.y - 122.0).abs() < 1.0);
    assert!(viewport.contains(top_tile_rect.center()));
}

#[test]
fn test_vertical_packing_and_order_retention() {
    use super::canvas::SimilarityCanvas;
    use super::embedding::FeatureVector;
    use crate::file_manager::filesystem::Entry;
    use std::path::PathBuf;

    let mut canvas = SimilarityCanvas::default();
    canvas.similarity_threshold = 0.5;

    // 100 entries => 10x10 base grid
    let count = 100;
    let mut entries = Vec::new();
    for i in 0..count {
        let path = PathBuf::from(format!("/test_pack_{i}.png"));
        entries.push(Entry {
            path: path.clone(),
            name: format!("img_{i}.png"),
            directory: false,
            regular_file: true,
            system: None,
            typescript: false,
            symlink: false,
            hidden: false,
            size: "1 KB".to_string(),
            size_bytes: 1024,
            modified: "today".to_string(),
            modified_time: None,
            warning: None,
            revision: 1,
            identity: None,
        });

        let mut vec = vec![0.0f32; 1536];
        vec[0] = 1.0;
        canvas.insert_hash(path, FeatureVector::Dense(vec), 1);
    }

    canvas.set_entries(entries);
    assert_eq!(canvas.grids[0].grid_cols, 10);
    assert_eq!(canvas.grids[0].grid_rows, 10);
    assert_eq!(canvas.grids[0].col_offset, 0);
    assert_eq!(canvas.grids[0].row_offset, 0);

    // Search 1: 9 items => 3x3 grid
    let mut concept1 = vec![0.0f32; 1536];
    concept1[0] = 1.0;
    let g1 = canvas.add_search_grid("s1", false, (0..9).collect(), concept1);
    assert_eq!(canvas.grids[1].id, g1);
    assert_eq!(canvas.grids[1].grid_cols, 3);
    assert_eq!(canvas.grids[1].grid_rows, 3);
    assert_eq!(canvas.grids[1].col_offset, 12); // 10 + COL_GAP(2)
    assert_eq!(canvas.grids[1].row_offset, 0);

    // Search 2: 9 items => 3x3 grid
    // Fits vertically in Column 1 below Search 1 (3 + 1 + 3 = 7 <= 10)
    let mut concept2 = vec![0.0f32; 1536];
    concept2[0] = 1.0;
    let g2 = canvas.add_search_grid("s2", false, (0..9).collect(), concept2);
    assert_eq!(canvas.grids[2].id, g2);
    assert_eq!(canvas.grids[2].grid_cols, 3);
    assert_eq!(canvas.grids[2].grid_rows, 3);
    assert_eq!(canvas.grids[2].col_offset, 12); // Same column!
    assert_eq!(canvas.grids[2].row_offset, 4); // Stacked vertically: 3 + ROW_GAP(1)

    // Search 3: 81 items => 9x9 grid
    // Does NOT fit in Column 1 (4 + 3 + 1 + 9 = 17 > 10) => Starts Column 2 to the right!
    let mut concept3 = vec![0.0f32; 1536];
    concept3[0] = 1.0;
    let g3 = canvas.add_search_grid("s3", false, (0..81).collect(), concept3);
    assert_eq!(canvas.grids[3].id, g3);
    assert_eq!(canvas.grids[3].grid_cols, 9);
    assert_eq!(canvas.grids[3].grid_rows, 9);
    assert_eq!(canvas.grids[3].col_offset, 17); // 12 + 3 + COL_GAP(2)
    assert_eq!(canvas.grids[3].row_offset, 0);

    // Search 4: 9 items => 3x3 grid
    // Current Column 2 has 9 rows used; 9 + 1 + 3 = 13 > 10 => Does NOT fit in Column 2!
    // Must NOT backfill into Column 1. Strictly retains search order and starts Column 3!
    let mut concept4 = vec![0.0f32; 1536];
    concept4[0] = 1.0;
    let g4 = canvas.add_search_grid("s4", false, (0..9).collect(), concept4);
    assert_eq!(canvas.grids[4].id, g4);
    assert_eq!(canvas.grids[4].grid_cols, 3);
    assert_eq!(canvas.grids[4].grid_rows, 3);
    assert_eq!(canvas.grids[4].col_offset, 28); // 17 + 9 + COL_GAP(2)
    assert_eq!(canvas.grids[4].row_offset, 0);
}

#[test]
fn test_similarity_canvas_hud_does_not_block_popup_or_hover() {
    let ctx = egui::Context::default();
    let raw = egui::RawInput::default();
    let _ = ctx.run(raw, |ctx| {
        egui::Area::new(egui::Id::new("similarity_canvas_search"))
            .order(egui::Order::Middle)
            .show(ctx, |ui| {
                ui.label("Search");
            });
        egui::Area::new(egui::Id::new("similarity_canvas_hud"))
            .order(egui::Order::Middle)
            .show(ctx, |ui| {
                ui.label("HUD");
            });
    });

    // popup_open should be false so hover previews function
    assert!(!crate::file_manager::ui::popup_open(&ctx));
}

#[test]
fn test_gallery_hover_preview_sizing_respects_original_and_window_bounds() {
    // Helper function matching the sizing algorithm in paint_tile_hover_preview
    let compute_size = |screen_size: egui::Vec2, orig: egui::Vec2| -> egui::Vec2 {
        let max_image_w = (screen_size.x - 32.0).max(100.0);
        let max_image_h = (screen_size.y - 120.0).max(100.0);
        let scale = (max_image_w / orig.x.max(1.0))
            .min(max_image_h / orig.y.max(1.0))
            .min(1.0);
        orig * scale
    };

    // Case 1: Original image smaller than window bounds => stays exact original size (no downscaling or artificial 340x240 clamping)
    let screen_1080p = egui::vec2(1920.0, 1080.0);
    let small_orig = egui::vec2(500.0, 400.0);
    let size1 = compute_size(screen_1080p, small_orig);
    assert_eq!(size1.x, 500.0);
    assert_eq!(size1.y, 400.0);

    // Case 2: Original image larger than window (e.g. 4000x3000 camera photo) => scaled down proportionally to fit window
    let large_orig = egui::vec2(4000.0, 3000.0);
    let size2 = compute_size(screen_1080p, large_orig);
    assert!(size2.x <= 1920.0 - 32.0);
    assert!(size2.y <= 1080.0 - 120.0);
    // Preserves aspect ratio 4:3
    assert!(((size2.x / size2.y) - (4.0 / 3.0)).abs() < 0.001);

    // Case 3: Smaller window (e.g. 800x600) => scales down to fit available bounds
    let small_screen = egui::vec2(800.0, 600.0);
    let size3 = compute_size(small_screen, large_orig);
    assert!(size3.x <= 800.0 - 32.0);
    assert!(size3.y <= 600.0 - 120.0);
    assert!(((size3.x / size3.y) - (4.0 / 3.0)).abs() < 0.001);
}

#[test]
fn test_search_from_photo_creates_ranked_grid() {
    let mut canvas = SimilarityCanvas::default();
    let p1 = PathBuf::from("/test/photo1.jpg");
    let p2 = PathBuf::from("/test/photo2.jpg");
    let p3 = PathBuf::from("/test/photo3.jpg");

    let make_entry = |p: &PathBuf, name: &str| Entry {
        path: p.clone(),
        name: name.to_string(),
        directory: false,
        regular_file: true,
        system: None,
        typescript: false,
        symlink: false,
        hidden: false,
        size: "1 KB".to_string(),
        size_bytes: 1024,
        modified: "today".to_string(),
        modified_time: None,
        warning: None,
        revision: 1,
        identity: None,
    };

    canvas.entries = vec![
        make_entry(&p1, "photo1.jpg"),
        make_entry(&p2, "photo2.jpg"),
        make_entry(&p3, "photo3.jpg"),
    ];

    // p1 and p2 are very similar; p3 is orthogonal
    let mut v1 = vec![0.0f32; 1536];
    v1[0] = 1.0;
    let mut v2 = vec![0.0f32; 1536];
    v2[0] = 0.9;
    v2[1] = 0.43588989; // unit length
    let mut v3 = vec![0.0f32; 1536];
    v3[10] = 1.0;

    canvas
        .hashes
        .insert(p1.clone(), (FeatureVector::Dense(v1.clone()), 1));
    canvas
        .hashes
        .insert(p2.clone(), (FeatureVector::Dense(v2), 1));
    canvas
        .hashes
        .insert(p3.clone(), (FeatureVector::Dense(v3), 1));

    canvas.recompute_layout();
    assert_eq!(canvas.grids.len(), 1);

    // Search visual similarity to p1
    let grid_id = canvas.search_from_photo(&p1);
    assert_eq!(grid_id, Some(1));
    assert_eq!(canvas.grids.len(), 2);

    let search_grid = &canvas.grids[1];
    assert_eq!(search_grid.id, 1);
    assert_eq!(search_grid.title, "Similar to photo1.jpg");
    assert_eq!(canvas.pending_pan_to_grid, Some(1));

    // P1 (index 0) has score 1.0, P2 (index 1) has score 0.9 => both match threshold 0.08
    // P3 (index 2) has score 0.0 => does not match
    assert_eq!(search_grid.indices, vec![0, 1]);
}

#[test]
fn test_similarity_canvas_keyboard_shortcuts() {
    let mut canvas = SimilarityCanvas::default();
    canvas.zoom = 1.0;
    canvas.pan = egui::vec2(50.0, 50.0);

    let canvas_rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));

    // Test '+' key zooms in
    let ctx = egui::Context::default();
    let mut raw = egui::RawInput::default();
    raw.events.push(egui::Event::Key {
        key: egui::Key::Plus,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    });
    let _ = ctx.run(raw, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let resp = ui.allocate_response(egui::vec2(800.0, 600.0), egui::Sense::hover());
            canvas.handle_input(ui, canvas_rect, &resp);
        });
    });

    assert!(
        canvas.zoom > 1.0,
        "Zoom should increase with Plus key, got {}",
        canvas.zoom
    );
}

#[test]
fn test_similarity_canvas_middle_click_pans_canvas() {
    let mut canvas = SimilarityCanvas::default();
    canvas.initial_fit_done = true;
    canvas.zoom = 1.0;
    canvas.pan = egui::vec2(50.0, 50.0);

    let canvas_rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));

    let ctx = egui::Context::default();
    // Frame 1: Press middle button down at (100, 100)
    let mut raw1 = egui::RawInput::default();
    raw1.events.push(egui::Event::PointerButton {
        pos: egui::pos2(100.0, 100.0),
        button: egui::PointerButton::Middle,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    let _ = ctx.run(raw1, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let resp =
                ui.allocate_response(egui::vec2(800.0, 600.0), egui::Sense::click_and_drag());
            canvas.handle_input(ui, canvas_rect, &resp);
        });
    });

    // Frame 2: Move pointer to (140, 120) -> pointer delta is (+40, +20)
    let mut raw2 = egui::RawInput::default();
    raw2.events
        .push(egui::Event::PointerMoved(egui::pos2(140.0, 120.0)));
    let _ = ctx.run(raw2, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let resp =
                ui.allocate_response(egui::vec2(800.0, 600.0), egui::Sense::click_and_drag());
            canvas.handle_input(ui, canvas_rect, &resp);
        });
    });

    assert_eq!(
        canvas.pan,
        egui::vec2(90.0, 70.0),
        "Middle click drag must pan canvas by delta (50+40, 50+20)"
    );
    assert!(
        canvas.is_middle_panning,
        "Middle drag must set is_middle_panning to true"
    );
}

#[test]
fn test_per_grid_match_allowance_adjustment() {
    let mut canvas = SimilarityCanvas::default();
    canvas.allowance = 0.50;

    let p1 = PathBuf::from("/test/photo1.jpg");
    let p2 = PathBuf::from("/test/photo2.jpg");
    let p3 = PathBuf::from("/test/photo3.jpg");
    let p4 = PathBuf::from("/test/photo4.jpg");

    let make_test_entry = |p: &PathBuf, name: &str| Entry {
        path: p.clone(),
        name: name.to_string(),
        directory: false,
        regular_file: true,
        system: None,
        typescript: false,
        symlink: false,
        hidden: false,
        size: "1 KB".to_string(),
        size_bytes: 1024,
        modified: "today".to_string(),
        modified_time: None,
        warning: None,
        revision: 1,
        identity: None,
    };

    let entries = vec![
        make_test_entry(&p1, "photo1.jpg"),
        make_test_entry(&p2, "photo2.jpg"),
        make_test_entry(&p3, "photo3.jpg"),
        make_test_entry(&p4, "photo4.jpg"),
    ];
    canvas.set_entries(entries);

    // Normalized vectors:
    // v1 = [1.0, 0.0]
    // v2: dot product with v1 = 0.20
    // v3: dot product with v1 = 0.06
    // v4: dot product with v1 = 0.00
    let v1 = vec![1.0, 0.0];
    let v2 = vec![0.20, (1.0f32 - 0.20 * 0.20).sqrt()];
    let v3 = vec![0.06, (1.0f32 - 0.06 * 0.06).sqrt()];
    let v4 = vec![0.0, 1.0];

    canvas
        .hashes
        .insert(p1.clone(), (FeatureVector::Dense(v1.clone()), 1));
    canvas
        .hashes
        .insert(p2.clone(), (FeatureVector::Dense(v2), 1));
    canvas
        .hashes
        .insert(p3.clone(), (FeatureVector::Dense(v3), 1));
    canvas
        .hashes
        .insert(p4.clone(), (FeatureVector::Dense(v4), 1));

    canvas.recompute_layout();

    // 1. Search visual similarity to p1 with initial allowance 50% (~0.08 threshold)
    let grid_id = canvas.search_from_photo(&p1).expect("search grid created");
    assert_eq!(grid_id, 1);
    assert_eq!(canvas.grids.len(), 2);

    let search_grid = &canvas.grids[1];
    assert!((search_grid.allowance - 0.50).abs() < 1e-4);
    // At default 50% allowance, P1 (1.0) and P2 (0.20) match, while P3 (0.06) does not
    assert_eq!(search_grid.indices, vec![0, 1]);

    // 2. Click '+' to INCREASE match allowance (include more photos): +15% => 65% allowance
    canvas.grids[1].adjust_allowance(0.15);
    assert!((canvas.grids[1].allowance - 0.65).abs() < 1e-4);

    canvas.recompute_layout();
    // At 65% allowance, threshold drops to ~0.04, so P1 (1.0), P2 (0.20), and P3 (0.06) all match
    assert_eq!(canvas.grids[1].indices, vec![0, 1, 2]);

    // 3. Click '-' to DECREASE match allowance (remove photos): -50% => 15% allowance
    canvas.grids[1].adjust_allowance(-0.50);
    assert!((canvas.grids[1].allowance - 0.15).abs() < 1e-4);

    canvas.recompute_layout();
    // At 15% allowance, threshold is ~0.27, so only exact/high match P1 (1.0) remains
    assert_eq!(canvas.grids[1].indices, vec![0]);

    // 4. Test inverted search grid allowance behavior
    // Concept vector = [1.0, 0.0] (same as p1)
    let inv_id = canvas.add_search_grid("!p1", true, vec![0, 1, 2, 3], v1);
    assert_eq!(inv_id, 2);
    // At default 50% allowance, threshold is ~0.08.
    // Inverted matches where score < 0.08: P3 (0.06) and P4 (0.00) match
    assert_eq!(canvas.grids[2].indices, vec![3, 2]);

    // Click '+' to INCREASE match allowance (include more photos): allowance up to 90%
    canvas.grids[2].adjust_allowance(0.40);
    canvas.recompute_layout();
    // At 90% allowance, threshold is ~0.30, so P2 (0.20), P3 (0.06), and P4 (0.00) now all match!
    assert_eq!(canvas.grids[2].indices, vec![3, 2, 1]);

    // Click '-' to DECREASE match allowance (remove photos): allowance down to 10%
    canvas.grids[2].adjust_allowance(-0.80);
    canvas.recompute_layout();
    // At 10% allowance, threshold is ~0.011, so only P4 (0.00) matches!
    assert_eq!(canvas.grids[2].indices, vec![3]);
}

#[test]
fn test_allowance_bounds_disabling_and_hover_keyboard_controls() {
    let mut canvas = SimilarityCanvas::default();
    canvas.allowance = 0.50;
    canvas.initial_fit_done = true;
    canvas.zoom = 1.0;
    canvas.pan = egui::Vec2::ZERO;

    let p1 = PathBuf::from("/test/photo1.jpg");
    let p2 = PathBuf::from("/test/photo2.jpg");
    let p3 = PathBuf::from("/test/photo3.jpg");

    let make_test_entry = |p: &PathBuf, name: &str| Entry {
        path: p.clone(),
        name: name.to_string(),
        directory: false,
        regular_file: true,
        system: None,
        typescript: false,
        symlink: false,
        hidden: false,
        size: "1 KB".to_string(),
        size_bytes: 1024,
        modified: "today".to_string(),
        modified_time: None,
        warning: None,
        revision: 1,
        identity: None,
    };

    let entries = vec![
        make_test_entry(&p1, "photo1.jpg"),
        make_test_entry(&p2, "photo2.jpg"),
        make_test_entry(&p3, "photo3.jpg"),
    ];
    canvas.set_entries(entries);

    // Concept vector = [1.0, 0.0]
    // v1 dot = 0.20
    // v2 dot = 0.06
    // v3 dot = -0.50 (negative, will never match normal search)
    let concept = vec![1.0, 0.0];
    let v1 = vec![0.20, (1.0f32 - 0.20 * 0.20).sqrt()];
    let v2 = vec![0.06, (1.0f32 - 0.06 * 0.06).sqrt()];
    let v3 = vec![-0.50, (1.0f32 - 0.50 * 0.50).sqrt()];

    canvas
        .hashes
        .insert(p1.clone(), (FeatureVector::Dense(v1), 1));
    canvas
        .hashes
        .insert(p2.clone(), (FeatureVector::Dense(v2), 1));
    canvas
        .hashes
        .insert(p3.clone(), (FeatureVector::Dense(v3), 1));

    // Search grid with candidate indices [0, 1, 2]
    let grid_id = canvas.add_search_grid("concept", false, vec![0, 1, 2], concept);
    assert_eq!(grid_id, 1);
    canvas.recompute_layout();

    // At default 50% allowance (threshold ~0.08), only P1 (0.20) matches -> 1 photo
    assert_eq!(canvas.grids[1].indices.len(), 1);

    // Decreasing allowance to 45% (threshold ~0.10) keeps P1 (0.20) -> 1 photo
    assert!(canvas.can_decrease_grid_allowance(1));
    assert!(canvas.adjust_grid_allowance(1, -0.05));
    canvas.recompute_layout();
    assert_eq!(canvas.grids[1].indices.len(), 1);

    // At 25% allowance (threshold ~0.19), P1 (0.20) still matches (1 photo)
    canvas.grids[1].set_allowance(0.25);
    canvas.recompute_layout();
    assert_eq!(canvas.grids[1].indices.len(), 1);

    // If we step down to 20% (threshold ~0.23), P1 (0.20) will no longer match (0 photos)!
    // Therefore, right now before pressing (-), can_decrease_grid_allowance MUST be false!
    assert!(
        !canvas.can_decrease_grid_allowance(1),
        "(-) button must be grayed out/prevented right before pressing it yields 0 results"
    );
    assert!(
        !canvas.adjust_grid_allowance(1, -0.05),
        "adjust_grid_allowance must refuse to decrease when it would yield 0 results"
    );

    // Now test (+) upper bound:
    // Broaden search to 70% allowance: P1 (0.20) and P2 (0.06) match -> 2 photos
    canvas.grids[1].set_allowance(0.70);
    canvas.recompute_layout();
    assert_eq!(canvas.grids[1].indices.len(), 2);

    // Candidate 3 has score -0.50, so even at 100% allowance (threshold 0.01), it can never match.
    // The maximum possible photos that could ever match is 2.
    // Since current count is 2 (equals maximum possible matches):
    assert!(
        !canvas.can_increase_grid_allowance(1),
        "(+) button must be grayed out/prevented when no matter how high value goes it wouldn't include more photos"
    );
    assert!(
        !canvas.adjust_grid_allowance(1, 0.05),
        "adjust_grid_allowance must refuse to increase when no higher value could include more photos"
    );

    // Test condition: "when it's now so broad that it includes all of the original search"
    // Create grid with candidates [0] only
    let small_grid_id = canvas.add_search_grid("p1_only", false, vec![0], vec![1.0, 0.0]);
    canvas.recompute_layout();
    assert_eq!(canvas.grids[small_grid_id].indices.len(), 1);
    assert_eq!(canvas.grids[small_grid_id].candidate_indices.len(), 1);
    assert!(
        !canvas.can_increase_grid_allowance(small_grid_id),
        "(+) must be grayed out when it includes all photos from the original search"
    );

    // Test +/- keyboard shortcuts acting on whichever grid the mouse is currently over
    let canvas_rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1600.0, 1000.0));
    canvas.pending_pan_to_grid = None;
    canvas.pan = egui::Vec2::ZERO;
    canvas.zoom = 1.0;
    canvas.grids[1].set_allowance(0.50);
    canvas.recompute_layout();
    assert!((canvas.grids[1].allowance - 0.50).abs() < 1e-4);

    // Locate grid 1 on screen
    let g1_screen = canvas.tile_screen_rect(
        canvas_rect.min,
        canvas.grids[1].col_offset,
        canvas.grids[1].row_offset,
    );
    let g1_hover_pos = g1_screen.center();

    let ctx = egui::Context::default();

    // 1. Hover over Grid 1 and press '+' key: should increase Grid 1 allowance from 50% to 55%
    let mut raw_plus = egui::RawInput::default();
    raw_plus
        .events
        .push(egui::Event::PointerMoved(g1_hover_pos));
    raw_plus.events.push(egui::Event::Key {
        key: egui::Key::Plus,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    });
    let _ = ctx.run(raw_plus, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let resp = ui.allocate_response(canvas_rect.size(), egui::Sense::click_and_drag());
            canvas.handle_input(ui, canvas_rect, &resp);
        });
    });

    assert!(
        (canvas.grids[1].allowance - 0.55).abs() < 1e-4,
        "Pressing '+' over Grid 1 must increase its allowance to 0.55, got {}",
        canvas.grids[1].allowance
    );

    // 2. Hover over Grid 1 and press '-' key: should decrease Grid 1 allowance from 55% back to 50%
    let mut raw_minus = egui::RawInput::default();
    raw_minus
        .events
        .push(egui::Event::PointerMoved(g1_hover_pos));
    raw_minus.events.push(egui::Event::Key {
        key: egui::Key::Minus,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    });
    let _ = ctx.run(raw_minus, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let resp = ui.allocate_response(canvas_rect.size(), egui::Sense::click_and_drag());
            canvas.handle_input(ui, canvas_rect, &resp);
        });
    });

    assert!(
        (canvas.grids[1].allowance - 0.50).abs() < 1e-4,
        "Pressing '-' over Grid 1 must decrease its allowance back to 0.50, got {}",
        canvas.grids[1].allowance
    );

    // 3. Move pointer away from Grid 1 (to empty canvas at 10.0, 10.0) and press '+'
    let initial_zoom = canvas.zoom;
    let mut raw_zoom = egui::RawInput::default();
    raw_zoom
        .events
        .push(egui::Event::PointerMoved(egui::pos2(10.0, 10.0)));
    raw_zoom.events.push(egui::Event::Key {
        key: egui::Key::Plus,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    });
    let _ = ctx.run(raw_zoom, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let resp = ui.allocate_response(canvas_rect.size(), egui::Sense::click_and_drag());
            canvas.handle_input(ui, canvas_rect, &resp);
        });
    });

    assert!(
        canvas.zoom > initial_zoom,
        "Pressing '+' over empty canvas must zoom canvas in"
    );
    assert!(
        (canvas.grids[1].allowance - 0.50).abs() < 1e-4,
        "Grid 1 allowance must NOT change when pointer is not over Grid 1"
    );
}
