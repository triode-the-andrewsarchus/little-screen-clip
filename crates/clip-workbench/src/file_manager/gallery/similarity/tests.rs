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
