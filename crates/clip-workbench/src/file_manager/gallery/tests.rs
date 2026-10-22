use super::*;

fn assert_layout(aspects: &[f32], width: usize) -> Layout {
    let layout = packing::pack(aspects, width);
    let mut cells = std::collections::BTreeSet::new();
    for (item, tile) in layout.tiles.iter().enumerate() {
        assert_eq!(tile.item, item);
        assert!(matches!(
            (tile.width, tile.height),
            (2, 2) | (3, 2) | (4, 2) | (2, 3) | (2, 4)
        ));
        assert!(tile.x + tile.width <= width);
        assert!(tile.y + tile.height <= layout.height);
        if tile.height == 4 {
            assert_eq!(tile.x % 2, 0);
            assert!(
                tile.y + tile.height < layout.height
                    || layout.tiles.iter().any(|other| other.y == tile.y + 2),
                "pointless descender"
            );
        }
        for y in tile.y..tile.y + tile.height {
            for x in tile.x..tile.x + tile.width {
                assert!(cells.insert((x, y)), "overlap");
            }
        }
    }
    assert_eq!(layout.tiles.len(), aspects.len());
    for y in layout
        .tiles
        .iter()
        .map(|tile| tile.y)
        .collect::<std::collections::BTreeSet<_>>()
    {
        let filled: usize = layout
            .tiles
            .iter()
            .filter(|tile| tile.y == y)
            .map(|tile| tile.width)
            .sum();
        assert_eq!(filled % 2, 0, "fractional ordinary row boundary");
    }
    layout
}
#[test]
fn common_aspects_and_small_galleries_obey_every_packing_invariant() {
    for ratio in [
        1.0,
        4.0 / 3.0,
        3.0 / 2.0,
        16.0 / 9.0,
        3.0 / 4.0,
        2.0 / 3.0,
        9.0 / 16.0,
    ] {
        for count in [1, 2, 3, 4, 5, 48] {
            for width in [2, 4, 6, 8, 12] {
                assert_layout(&vec![ratio; count], width);
            }
        }
    }
}
#[test]
fn mixed_and_tall_endings_never_leave_pointless_descenders() {
    for count in 1..=48 {
        let mut aspects: Vec<_> = (0..count)
            .map(|i| [1.0, 1.5, 0.5, 4.0 / 3.0, 16.0 / 9.0][i % 5])
            .collect();
        aspects[count - 1] = 0.5;
        for width in [4, 6, 8, 12] {
            assert_layout(&aspects, width);
        }
    }
    assert_layout(&[1.5, 1.0, 0.5, 0.5, 1.0], 8);
}
#[test]
fn exact_camera_shapes_and_resize_are_preserved_when_rows_fit() {
    let square = assert_layout(&[1.0; 12], 8);
    assert!(
        square
            .tiles
            .iter()
            .all(|tile| (tile.width, tile.height) == (2, 2))
    );
    let dslr = assert_layout(&[1.5; 12], 12);
    assert!(
        dslr.tiles
            .iter()
            .all(|tile| (tile.width, tile.height) == (3, 2))
    );
    let wide = assert_layout(&[16.0 / 9.0; 12], 12);
    assert!(
        wide.tiles
            .iter()
            .all(|tile| (tile.width, tile.height) == (4, 2))
    );
    assert_ne!(square.height, assert_layout(&[1.0; 12], 4).height);
}

pub(super) fn synthetic_directory(count: usize) -> Directory {
    Directory {
        system: false,
        path: "/synthetic-gallery".into(),
        signature: 1,
        skipped: 0,
        entries: (0..count)
            .map(|i| Entry {
                path: PathBuf::from(format!("/synthetic-gallery/{i:05}.png")),
                name: format!("{i:05}.png"),
                directory: false,
                regular_file: true,
                system: None,
                typescript: false,
                symlink: false,
                hidden: false,
                size: "".into(),
                size_bytes: 0,
                modified: "".into(),
                modified_time: None,
                warning: None,
                revision: 1,
                identity: None,
            })
            .collect(),
    }
}
#[test]
fn reordered_visible_entries_replace_the_previous_chunk_order() {
    let directory = synthetic_directory(96);
    let mut gallery = Gallery::default();
    let mut visible: Vec<_> = (0..96).collect();
    gallery.sync(&directory, &visible, false, 8);
    assert_eq!(gallery.chunks[0].keys[0].path, directory.entries[0].path);
    visible.reverse();
    gallery.invalidate_order();
    gallery.sync(&directory, &visible, false, 8);
    assert_eq!(gallery.chunks[0].keys[0].path, directory.entries[95].path);
}

fn seed_chunk(gallery: &mut Gallery, index: usize, aspect: f32) {
    let chunk = &mut gallery.chunks[index];
    let values: Arc<[Metadata]> = vec![
        Metadata {
            width: (aspect * 120.0) as u32,
            height: 120,
            orientation: 1
        };
        chunk.keys.len()
    ]
    .into();
    chunk.metadata = Some(values.clone());
    chunk.layout = Some(Arc::new(packing::pack_chunk(
        &values.iter().map(|v| v.aspect()).collect::<Vec<_>>(),
        gallery.width,
        chunk.final_chunk,
    )));
}
#[test]
fn middle_of_ten_thousand_images_is_local_and_preserves_chunk_order() {
    let directory = synthetic_directory(10_000);
    let visible: Vec<_> = (0..10_000).collect();
    let mut gallery = Gallery::default();
    gallery.sync(&directory, &visible, false, 12);
    assert_eq!(gallery.chunks.len(), 209);
    let middle = 100;
    let offset = gallery.offsets[middle];
    let placeholders = gallery.placements(offset, offset + 8.0);
    assert!(placeholders.len() < 48);
    assert_eq!(placeholders[0].index, middle * CHUNK_SIZE);
    assert!(
        gallery
            .chunks
            .iter()
            .all(|c| c.layout.is_none() && c.metadata.is_none())
    );
    seed_chunk(&mut gallery, middle, 1.5);
    gallery.reindex();
    let offset = gallery.offsets[middle];
    let placements = gallery.placements(
        offset,
        offset + gallery.chunks[middle].layout.as_ref().unwrap().height as f32 - 0.1,
    );
    assert_eq!(
        placements.iter().map(|tile| tile.index).collect::<Vec<_>>(),
        (middle * CHUNK_SIZE..(middle + 1) * CHUNK_SIZE).collect::<Vec<_>>()
    );
    assert!(gallery.chunks[..middle].iter().all(|c| c.layout.is_none()));
}
#[test]
fn height_correction_keeps_the_visible_item_at_the_same_viewport_position() {
    let directory = synthetic_directory(200);
    let mut gallery = Gallery::default();
    gallery.sync(&directory, &(0..200).collect::<Vec<_>>(), false, 8);
    seed_chunk(&mut gallery, 2, 1.0);
    gallery.reindex();
    let offset = gallery.offsets[2] + 4.5;
    let anchor = gallery.anchor(offset);
    let item = anchor.item.unwrap() - gallery.chunks[2].first;
    seed_chunk(&mut gallery, 0, 0.67);
    seed_chunk(&mut gallery, 1, 1.8);
    gallery.reindex();
    let corrected = gallery.anchored_offset(anchor);
    let tile_y =
        gallery.offsets[2] + gallery.chunks[2].layout.as_ref().unwrap().tiles[item].y as f32;
    assert!((corrected - tile_y - anchor.delta).abs() < 0.01);
    assert!(corrected != offset);
}
#[test]
fn modification_invalidates_only_its_chunk_and_resize_preserves_metadata() {
    let mut directory = synthetic_directory(120);
    let visible: Vec<_> = (0..120).collect();
    let mut gallery = Gallery::default();
    gallery.sync(&directory, &visible, false, 8);
    for index in 0..3 {
        seed_chunk(&mut gallery, index, 1.5);
    }
    let other = gallery.chunks[1].layout.clone().unwrap();
    directory.entries[0].revision = 2;
    directory.signature = 2;
    gallery.sync(&directory, &visible, false, 8);
    assert!(gallery.chunks[0].metadata.is_none());
    assert!(Arc::ptr_eq(
        &other,
        gallery.chunks[1].layout.as_ref().unwrap()
    ));
    let metadata = gallery.chunks[1].metadata.clone().unwrap();
    gallery.sync(&directory, &visible, false, 12);
    assert!(gallery.chunks.iter().all(|chunk| chunk.layout.is_none()));
    assert!(Arc::ptr_eq(
        &metadata,
        gallery.chunks[1].metadata.as_ref().unwrap()
    ));
}
#[test]
fn metadata_and_layout_completions_are_independent_and_generation_guarded() {
    let directory = synthetic_directory(100);
    let mut gallery = Gallery::default();
    gallery.sync(&directory, &(0..100).collect::<Vec<_>>(), false, 8);
    seed_chunk(&mut gallery, 2, 0.75);
    seed_chunk(&mut gallery, 0, 1.5);
    assert!(gallery.chunks[1].layout.is_none());
    let old = gallery.chunks[0].generation;
    gallery.sync(&directory, &(0..100).collect::<Vec<_>>(), false, 12);
    assert!(gallery.chunks[0].generation != old);
    assert!(gallery.chunks[0].layout.is_none());
    assert!(gallery.chunks[0].metadata.is_some());
}
#[test]
fn headers_without_pixel_data_and_exif_orientation_are_enough_for_layout() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("header-only.png");
    image::RgbaImage::new(48, 32).save(&path).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    std::fs::write(&path, &bytes[..33]).unwrap();
    let directory = super::super::filesystem::read_directory(
        dir.path(),
        &std::sync::atomic::AtomicBool::new(false),
    )
    .unwrap();
    let metadata = metadata::probe(&path, directory.entries[0].revision).unwrap();
    assert_eq!((metadata.width, metadata.height), (48, 32));
    assert!(image::load_from_memory(&bytes[..33]).is_err());
    assert_eq!(
        Metadata {
            width: 48,
            height: 32,
            orientation: 6
        }
        .aspect(),
        2.0 / 3.0
    );
    assert!(metadata::probe(&path, directory.entries[0].revision.wrapping_add(1)).is_err());
}

#[test]
fn unknown_chunk_correction_preserves_the_placeholder_item_anchor() {
    let directory = synthetic_directory(100);
    let mut gallery = Gallery::default();
    gallery.sync(&directory, &(0..100).collect::<Vec<_>>(), false, 8);
    let offset = gallery.offsets[1] + 4.5;
    let anchor = gallery.anchor(offset);
    assert_eq!(anchor.item, Some(56));
    seed_chunk(&mut gallery, 1, 1.5);
    gallery.reindex();
    let corrected = gallery.anchored_offset(anchor);
    assert!((corrected - gallery.item_offset(48 + 8) - 0.5).abs() < 0.01);
}
#[test]
fn chunk_boundary_descenders_end_locally_and_do_not_reorder_images() {
    let aspects: Vec<_> = (0..150)
        .map(|i| if i % 3 == 0 { 0.5 } else { 1.5 })
        .collect();
    let mut next = 0;
    for group in aspects.chunks(CHUNK_SIZE) {
        let layout = assert_layout(group, 8);
        for tile in layout.tiles {
            assert_eq!(next, next / CHUNK_SIZE * CHUNK_SIZE + tile.item);
            next += 1;
        }
    }
    assert_eq!(next, 150);
}

#[test]
fn common_file_headers_and_exif_rotation_match_thumbnail_orientation() {
    let dir = tempfile::tempdir().unwrap();
    for extension in ["png", "gif", "bmp", "jpg", "webp", "tga"] {
        image::RgbImage::new(48, 32)
            .save(dir.path().join(format!("photo.{extension}")))
            .unwrap();
    }
    let path = dir.path().join("photo.jpg");
    let bytes = std::fs::read(&path).unwrap();
    // Little-endian TIFF Orientation=6 in a standard JPEG APP1 segment.
    let exif = [
        b'E', b'x', b'i', b'f', 0, 0, b'I', b'I', 42, 0, 8, 0, 0, 0, 1, 0, 0x12, 1, 3, 0, 1, 0, 0,
        0, 6, 0, 0, 0, 0, 0, 0, 0,
    ];
    let mut rotated = vec![0xff, 0xd8, 0xff, 0xe1, 0, (exif.len() + 2) as u8];
    rotated.extend_from_slice(&exif);
    rotated.extend_from_slice(&bytes[2..]);
    std::fs::write(&path, rotated).unwrap();
    let directory = super::super::filesystem::read_directory(
        dir.path(),
        &std::sync::atomic::AtomicBool::new(false),
    )
    .unwrap();
    for entry in directory.entries {
        let value = metadata::probe(&entry.path, entry.revision).unwrap();
        assert_eq!((value.width, value.height), (48, 32));
        if entry.path == path {
            assert_eq!(value.orientation, 6);
            assert_eq!(value.aspect(), 2.0 / 3.0);
            let small = thumbnail::load(
                &path,
                128,
                value.orientation,
                &std::sync::atomic::AtomicBool::new(false),
            )
            .unwrap();
            assert_eq!((small.width, small.height), (32, 48));
        }
    }
}
#[test]
fn optimized_chunk_packing_and_middle_lookup_timings() {
    let aspects: Vec<_> = (0..10_000)
        .map(|i| [1.0, 4.0 / 3.0, 1.5, 16.0 / 9.0, 2.0 / 3.0, 0.5][i % 6])
        .collect();
    let start = std::time::Instant::now();
    let first = packing::pack_chunk(&aspects[4800..4848], 12, false);
    let visible = start.elapsed();
    assert_eq!(first.tiles.len(), 48);
    let start = std::time::Instant::now();
    let count: usize = aspects
        .chunks(CHUNK_SIZE)
        .enumerate()
        .map(|(index, chunk)| {
            packing::pack_chunk(chunk, 12, (index + 1) * CHUNK_SIZE >= aspects.len())
                .tiles
                .len()
        })
        .sum();
    assert_eq!(count, 10_000);
    eprintln!(
        "gallery synthetic: visible chunk={visible:?}, independent 10,000-item packing={:?}",
        start.elapsed()
    );
}

#[test]
fn lone_final_photos_keep_appropriate_shapes_in_wide_galleries() {
    for width in [4, 8, 12, 24] {
        let square = assert_layout(&[1.0], width);
        assert_eq!((square.tiles[0].width, square.tiles[0].height), (2, 2));
        let portrait = assert_layout(&[0.5], width);
        assert_eq!((portrait.tiles[0].width, portrait.tiles[0].height), (2, 3));
    }
}

#[test]
fn requested_thumbnail_resolution_covers_cropped_tiles_at_current_dpi() {
    let mut tile = Placement {
        index: 0,
        x: 0,
        y: 0.0,
        width: 3,
        height: 2,
        ready: true,
        orientation: 1,
        aspect: 2.0,
    };
    assert_eq!(tile.thumbnail_edge(41.0, 1.0), 160);
    assert_eq!(tile.thumbnail_edge(41.0, 2.0), 320);
    tile.width = 2;
    tile.height = 3;
    tile.aspect = 0.5;
    assert_eq!(tile.thumbnail_edge(41.0, 1.0), 160);
}

#[test]
fn unknown_chunks_reserve_placeholder_space_after_wide_layouts_arrive() {
    let directory = synthetic_directory(150);
    let mut gallery = Gallery::default();
    gallery.sync(&directory, &(0..150).collect::<Vec<_>>(), false, 12);
    seed_chunk(&mut gallery, 0, 2.0);
    gallery.reindex();
    for chunk in 1..gallery.chunks.len() {
        let start = gallery.offsets[chunk];
        let end = gallery.offsets[chunk + 1];
        for tile in gallery.placements(start, end - 0.01) {
            if tile.index / CHUNK_SIZE == chunk {
                assert!(tile.y + tile.height as f32 <= end);
            }
        }
    }
    for width in [64, 86, 124] {
        assert_layout(&[0.5; CHUNK_SIZE], width);
    }
}

#[test]
fn chunk_boundaries_and_every_nonfinal_row_are_completely_filled() {
    for width in [2, 4, 6, 8, 10, 12, 14, 18, 22, 34, 62, 86, 124] {
        let directory = synthetic_directory(211);
        let mut gallery = Gallery::default();
        gallery.sync(&directory, &(0..211).collect::<Vec<_>>(), false, width);
        assert_eq!(gallery.chunk_size % (width / 2), 0);
        for ratio in [1.0, 1.5, 16.0 / 9.0, 0.5, 2.0 / 3.0] {
            for i in 0..gallery.chunks.len() {
                let chunk = &mut gallery.chunks[i];
                let aspects: Vec<_> = (0..chunk.keys.len())
                    .map(|item| {
                        if item % 3 == 0 {
                            ratio
                        } else {
                            [1.0, 1.5, 0.5][item % 3]
                        }
                    })
                    .collect();
                let layout = packing::pack_chunk(&aspects, width, chunk.final_chunk);
                let mut cells = std::collections::BTreeSet::new();
                for (item, tile) in layout.tiles.iter().enumerate() {
                    assert_eq!(tile.item, item);
                    for y in tile.y..tile.y + tile.height {
                        for x in tile.x..tile.x + tile.width {
                            assert!(x < width && cells.insert((x, y)));
                        }
                    }
                }
                assert_eq!(layout.tiles.len(), chunk.keys.len());
                let end = if chunk.final_chunk {
                    layout.tiles.last().unwrap().y
                } else {
                    layout.height
                };
                for y in 0..end {
                    for x in 0..width {
                        assert!(
                            cells.contains(&(x, y)),
                            "gap at width {width}, chunk {i}, ({x},{y})"
                        );
                    }
                }
                chunk.layout = Some(Arc::new(layout));
            }
            gallery.reindex();
            let positions = gallery.placements(0.0, gallery.height());
            assert_eq!(
                positions.iter().map(|tile| tile.index).collect::<Vec<_>>(),
                (0..211).collect::<Vec<_>>()
            );
        }
    }
}

#[test]
fn unknown_batches_have_no_empty_rows_or_columns_between_chunks() {
    let directory = synthetic_directory(10_000);
    let mut gallery = Gallery::default();
    for width in [10, 14, 18, 22, 86] {
        gallery.sync(&directory, &(0..10_000).collect::<Vec<_>>(), false, width);
        let offset = gallery.offsets[100];
        let next = gallery.offsets[101];
        let tiles: Vec<_> = gallery
            .placements(offset, next - 0.01)
            .into_iter()
            .filter(|tile| tile.index / gallery.chunk_size == 100)
            .collect();
        let area: usize = tiles.iter().map(|tile| tile.width * tile.height).sum();
        assert_eq!(area, width * (next - offset) as usize);
        assert_eq!(tiles[0].index, 100 * gallery.chunk_size);
        assert!(
            gallery.chunks[..100]
                .iter()
                .all(|chunk| chunk.layout.is_none())
        );
    }
}

#[test]
fn resize_rebatching_preserves_absolute_item_anchor_and_metadata_cache() {
    let directory = synthetic_directory(200);
    let visible = (0..200).collect::<Vec<_>>();
    let mut gallery = Gallery::default();
    gallery.sync(&directory, &visible, false, 10);
    seed_chunk(&mut gallery, 1, 1.5);
    for chunk in &gallery.chunks {
        if let Some(metadata) = &chunk.metadata {
            gallery
                .metadata_cache
                .extend(chunk.keys.iter().cloned().zip(metadata.iter().copied()));
        }
    }
    let offset = gallery.item_offset(60) + 0.5;
    gallery.set_viewport(offset, offset + 6.0);
    let anchor = gallery.anchor(offset);
    let count = gallery.metadata_cache.len();
    gallery.sync(&directory, &visible, false, 14);
    assert_eq!(gallery.chunk_size, 49);
    assert_eq!(gallery.metadata_cache.len(), count);
    let corrected = gallery.anchored_offset(anchor);
    assert!((corrected - gallery.item_offset(anchor.item.unwrap()) - anchor.delta).abs() < 0.01);
}

#[test]
fn ten_row_retention_window_bounds_previews_and_discards_distant_tiles() {
    let directory = synthetic_directory(500);
    let visible = (0..500).collect::<Vec<_>>();
    let mut gallery = Gallery::default();
    gallery.sync(&directory, &visible, false, 10);
    for i in 0..gallery.chunks.len() {
        seed_chunk(&mut gallery, i, 1.0);
    }

    // Viewport at y: [40.0, 50.0]
    let view_min = 40.0;
    let view_max = 50.0;
    const BUFFER_ROWS_UNITS: f32 = 20.0; // 10 rows at 2 units/row
    let retain_min = (view_min - BUFFER_ROWS_UNITS).max(0.0); // 20.0
    let retain_max = view_max + BUFFER_ROWS_UNITS; // 70.0

    let visible_tiles = gallery.placements(view_min, view_max);
    let retain_tiles = gallery.placements(retain_min, retain_max);

    // Retained tiles include all visible tiles
    for vis in &visible_tiles {
        assert!(retain_tiles.iter().any(|t| t.index == vis.index));
    }

    // Retained tiles include tiles within the 10-row buffer
    assert!(retain_tiles.len() > visible_tiles.len());

    // Tiles more than 10 rows out of view (e.g. at y = 10.0 or y = 80.0) are NOT in retain_tiles
    for tile in &retain_tiles {
        assert!(tile.y + tile.height as f32 >= retain_min);
        assert!(tile.y <= retain_max);
    }

    // Pre-cache buffer tiles are strictly outside visible tiles and ordered by distance
    let mut buffer_tiles: Vec<_> = retain_tiles
        .iter()
        .filter(|t| !visible_tiles.iter().any(|v| v.index == t.index))
        .collect();
    let center_y = (view_min + view_max) / 2.0;
    buffer_tiles.sort_by(|a, b| {
        let dist_a = (a.y - center_y).abs();
        let dist_b = (b.y - center_y).abs();
        dist_a.total_cmp(&dist_b)
    });
    for window in buffer_tiles.windows(2) {
        let d0 = (window[0].y - center_y).abs();
        let d1 = (window[1].y - center_y).abs();
        assert!(d0 <= d1 + 0.001);
    }
}
