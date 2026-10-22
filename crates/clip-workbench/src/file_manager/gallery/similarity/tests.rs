//! Unit tests for perceptual similarity clustering, grid layout, and canvas transforms.
use super::{
    dhash::{compute_dhash, hamming_distance},
    embedding::project_hashes_to_2d,
    grid::{grid_dimensions, snap_to_grid},
    *,
};
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

    // The tile should be positioned cleanly in the upper portion of the viewport (y == 110.0)
    assert!((top_tile_rect.min.y - 110.0).abs() < 1.0);
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
