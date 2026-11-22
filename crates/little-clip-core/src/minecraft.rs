//! Guarded, read-only Minecraft Java save inventories and MinedMap raster previews.
use crate::{
    ImageThumbnail,
    file_inventory::{Identity, identity},
    kernel_interface::{classify_handle, open_content, pin, pin_content_directory},
};
use image::{DynamicImage, RgbaImage, imageops};
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    fs::File,
    io,
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

pub const REGION_SIZE: u32 = minedmap_render::TILE_SIZE;
pub type Position = [i32; 2];

const DIMENSIONS: [(&str, &str, &str); 3] = [
    ("Overworld", "overworld", "region"),
    ("Nether", "the_nether", "DIM-1/region"),
    ("End", "the_end", "DIM1/region"),
];
const MAX_DIRECTORY_ENTRIES: usize = 262_144;
const PREVIEW_REGIONS: usize = 5;

pub fn is_map_file(path: &Path) -> bool {
    is_level_file(path)
        || path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("mca"))
}

fn is_level_file(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(OsStr::to_str),
        Some("level.dat" | "level.dat_old")
    )
}

/// Observes a save-directory hint without decoding its files.
/// Include the level metadata revision so normal game saves invalidate previews.
pub(crate) fn directory_hint(path: &Path) -> Option<Identity> {
    let markers = ["level.dat", "level.dat_old"];
    if !markers.iter().any(|name| {
        path.join(name)
            .symlink_metadata()
            .is_ok_and(|metadata| metadata.is_file())
    }) {
        return None;
    }
    let root = pin_content_directory(path).ok()?;
    let level = markers.into_iter().find_map(|name| {
        let file = pin(&descriptor_path(&root).join(name), false).ok()?;
        let meta = file.metadata().ok()?;
        (meta.is_file() && !classify_handle(&file).ok()?.protected()).then(|| identity(&meta))
    })?;
    region_directories(&root).next()?;
    Some(level)
}

pub fn is_world_directory(path: &Path) -> bool {
    directory_hint(path).is_some()
}

/// Captures the file or save-marker identity used by the little-screen preview cache.
pub fn preview_revision(path: &Path) -> Option<Identity> {
    if is_map_file(path) {
        let file = pin(path, true).ok()?;
        let meta = file.metadata().ok()?;
        (meta.is_file() && !classify_handle(&file).ok()?.protected()).then(|| identity(&meta))
    } else {
        directory_hint(path)
    }
}

fn descriptor_path(file: &File) -> PathBuf {
    PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd()))
}

fn open_subdirectory(root: &File, relative: &Path) -> io::Result<File> {
    let mut directory = root.try_clone()?;
    for part in relative {
        directory = pin(&descriptor_path(&directory).join(part), false)?;
        if !directory.metadata()?.is_dir() || classify_handle(&directory)?.protected() {
            return Err(io::Error::other("Invalid Minecraft region directory"));
        }
    }
    Ok(directory)
}

fn region_directories(root: &File) -> impl Iterator<Item = (&'static str, File)> {
    DIMENSIONS
        .into_iter()
        .filter_map(move |(label, name, legacy)| {
            let modern = Path::new("dimensions")
                .join("minecraft")
                .join(name)
                .join("region");
            open_subdirectory(root, &modern)
                .or_else(|_| open_subdirectory(root, Path::new(legacy)))
                .ok()
                .map(|directory| (label, directory))
        })
}

#[derive(Debug)]
struct Region {
    name: OsString,
    revision: Identity,
}

#[derive(Debug)]
pub struct Dimension {
    pub name: &'static str,
    pub focus: Position,
    directory: File,
    regions: BTreeMap<Position, Region>,
    follow_region_alias: bool,
}

impl Dimension {
    pub fn positions(&self) -> impl Iterator<Item = Position> + '_ {
        self.regions.keys().copied()
    }

    pub fn positions_in(
        &self,
        min: Position,
        max: Position,
    ) -> impl Iterator<Item = Position> + '_ {
        self.regions
            .range([min[0], i32::MIN]..=[max[0], i32::MAX])
            .map(|(&position, _)| position)
            .filter(move |position| (min[1]..=max[1]).contains(&position[1]))
    }

    pub fn bounds(&self) -> [Position; 2] {
        let mut min = self.focus;
        let mut max = self.focus;
        for position in self.positions() {
            for i in 0..2 {
                min[i] = min[i].min(position[i]);
                max[i] = max[i].max(position[i]);
            }
        }
        [min, max]
    }

    /// Reads and renders one tile, verifying its identity against the world inventory.
    pub fn render(&self, position: Position, cancel: &AtomicBool) -> Result<RgbaImage, String> {
        check_cancelled(cancel)?;
        let region = self
            .regions
            .get(&position)
            .ok_or("Minecraft region is missing")?;
        let path = descriptor_path(&self.directory).join(&region.name);
        let mut file = open_content(&path, self.follow_region_alias).map_err(|e| e.to_string())?;
        let validate = |file: &File| -> Result<(), String> {
            if identity(&file.metadata().map_err(|e| e.to_string())?) != region.revision {
                return Err("Minecraft region changed. Reopen the map to refresh it.".into());
            }
            Ok(())
        };
        validate(&file)?;
        let raster = minedmap_render::render_region(&mut file, || cancel.load(Ordering::Relaxed))
            .map_err(|error| format!("{error:#}"))?;
        validate(&file)?;
        validate(&pin(&path, self.follow_region_alias).map_err(|e| e.to_string())?)?;
        check_cancelled(cancel)?;
        Ok(raster)
    }

    fn scan(
        directory: File,
        name: &'static str,
        preferred: Position,
        cancel: &AtomicBool,
    ) -> Result<Self, String> {
        let mut regions = BTreeMap::new();
        let entries = std::fs::read_dir(descriptor_path(&directory)).map_err(|e| e.to_string())?;
        for (index, entry) in entries.enumerate() {
            check_cancelled(cancel)?;
            if index >= MAX_DIRECTORY_ENTRIES {
                return Err("Minecraft region directory exceeds the map inventory limit".into());
            }
            let entry = entry.map_err(|e| e.to_string())?;
            let name = entry.file_name();
            let Some(position) = parse_region_name(&name) else {
                continue;
            };
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_file() && meta.len() >= 8192 {
                regions.insert(
                    position,
                    Region {
                        name,
                        revision: identity(&meta),
                    },
                );
            }
        }
        let focus = regions
            .keys()
            .copied()
            .min_by_key(|p| distance(*p, preferred))
            .ok_or("No Minecraft Anvil regions found")?;
        Ok(Self {
            name,
            focus,
            directory,
            regions,
            follow_region_alias: false,
        })
    }
}

#[derive(Debug)]
pub struct World {
    pub title: String,
    pub dimensions: Vec<Dimension>,
}

impl World {
    /// Opens a Java save folder, its level.dat, or an individual Anvil region.
    pub fn open(path: &Path, cancel: &AtomicBool) -> Result<Self, String> {
        check_cancelled(cancel)?;
        let source = pin(path, true).map_err(|e| e.to_string())?;
        if classify_handle(&source)
            .map_err(|e| e.to_string())?
            .protected()
        {
            return Err("Minecraft sources must be ordinary files or folders".into());
        }
        let metadata = source.metadata().map_err(|e| e.to_string())?;
        if metadata.is_dir() {
            return Self::open_save(path, source, None, cancel);
        }
        if !metadata.is_file() || !is_map_file(path) {
            return Err("Expected a Minecraft Java save folder, level.dat, or .mca region".into());
        }
        let source_revision = identity(&metadata);
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let directory = pin_content_directory(parent).map_err(|e| e.to_string())?;
        let name = path.file_name().ok_or("Missing Minecraft filename")?;
        let held_path = descriptor_path(&directory).join(name);
        let pinned_child = pin(&held_path, true).map_err(|e| e.to_string())?;
        if identity(&pinned_child.metadata().map_err(|e| e.to_string())?) != source_revision {
            return Err("Minecraft source changed during opening".into());
        }
        if is_level_file(path) {
            return Self::open_save(parent, directory, Some(name), cancel);
        }

        let position = parse_region_name(name).unwrap_or([0, 0]);
        let region = Region {
            name: name.to_owned(),
            revision: source_revision,
        };
        Ok(Self {
            title: crate::path_address::display_path(Path::new(name)),
            dimensions: vec![Dimension {
                name: "Region",
                focus: position,
                directory,
                regions: BTreeMap::from([(position, region)]),
                follow_region_alias: true,
            }],
        })
    }

    fn open_save(
        path: &Path,
        root: File,
        level_name: Option<&OsStr>,
        cancel: &AtomicBool,
    ) -> Result<Self, String> {
        let preferred = level_name
            .into_iter()
            .chain(["level.dat", "level.dat_old"].map(OsStr::new))
            .find_map(|name| read_spawn(&root, name))
            .unwrap_or_default();
        let mut dimensions = Vec::new();
        let mut first_error = None;
        for (name, directory) in region_directories(&root) {
            let focus = if name == "Overworld" {
                preferred
            } else {
                [0, 0]
            };
            match Dimension::scan(directory, name, focus, cancel) {
                Ok(dimension) => dimensions.push(dimension),
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
            check_cancelled(cancel)?;
        }
        if dimensions.is_empty() {
            return Err(
                first_error.unwrap_or_else(|| "No Minecraft Java region folder found".into())
            );
        }
        let title = path.file_name().map_or_else(
            || crate::path_address::display_path(path),
            |name| crate::path_address::display_path(Path::new(name)),
        );
        Ok(Self { title, dimensions })
    }

    /// Renders a small spawn-area map without decoding an entire save.
    pub fn thumbnail(&self, cancel: &AtomicBool) -> Result<ImageThumbnail, String> {
        let terrain = self.preview_terrain(cancel)?;
        // Fill the preview area even for a few isolated generated chunks.
        let thumbnail = DynamicImage::ImageRgba8(terrain).thumbnail(440, 260);
        Ok(ImageThumbnail::from_rgba(thumbnail.to_rgba8()))
    }

    /// Renders the bounded spawn-area preview at one pixel per block.
    pub fn preview_terrain(&self, cancel: &AtomicBool) -> Result<RgbaImage, String> {
        let dimension = self
            .dimensions
            .first()
            .ok_or("Minecraft world has no regions")?;
        let [x, z] = dimension.focus;
        let positions = dimension.positions_in(
            [x.saturating_sub(1), z.saturating_sub(1)],
            [x.saturating_add(1), z.saturating_add(1)],
        );
        let mut positions: Vec<_> = positions.collect();
        positions.sort_by_key(|p| distance(*p, dimension.focus));
        let mut canvas = RgbaImage::new(REGION_SIZE * 3, REGION_SIZE * 3);
        let mut first_error = None;
        let mut bounds = [canvas.width(), canvas.height(), 0, 0];
        for position in positions.into_iter().take(PREVIEW_REGIONS) {
            check_cancelled(cancel)?;
            match dimension.render(position, cancel) {
                Ok(tile) => add_preview_tile(
                    &mut canvas,
                    &mut bounds,
                    &tile,
                    [position[0] - x + 1, position[1] - z + 1],
                ),
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        check_cancelled(cancel)?;
        let [left, top, right, bottom] = bounds;
        if left >= right || top >= bottom {
            return Err(first_error.unwrap_or_else(|| "No renderable Minecraft regions".into()));
        }
        Ok(imageops::crop_imm(&canvas, left, top, right - left, bottom - top).to_image())
    }
}

fn add_preview_tile(
    canvas: &mut RgbaImage,
    bounds: &mut [u32; 4],
    tile: &RgbaImage,
    position: Position,
) {
    let [offset_x, offset_z] = position.map(|n| n as u32 * REGION_SIZE);
    for (x, z, pixel) in tile
        .enumerate_pixels()
        .filter(|(_, _, pixel)| pixel[3] != 0)
    {
        let x = x + offset_x;
        let z = z + offset_z;
        canvas.put_pixel(x, z, *pixel);
        bounds[0] = bounds[0].min(x);
        bounds[1] = bounds[1].min(z);
        bounds[2] = bounds[2].max(x + 1);
        bounds[3] = bounds[3].max(z + 1);
    }
}

fn read_spawn(root: &File, name: &OsStr) -> Option<Position> {
    use minedmap_render::{LevelDat, LevelDatData};
    let file = open_content(&descriptor_path(root).join(name), false).ok()?;
    let level: LevelDat = minedmap_render::data::from_reader(file).ok()?;
    let position = match level.data {
        LevelDatData::V0 { spawn_x, spawn_z } => [spawn_x, spawn_z],
        LevelDatData::V1_21_9 { spawn } if spawn.dimension == "minecraft:overworld" => {
            [*spawn.pos.first()?, *spawn.pos.get(2)?]
        }
        _ => return None,
    };
    Some(position.map(|n| n.div_euclid(REGION_SIZE as i32)))
}

fn parse_region_name(name: &OsStr) -> Option<Position> {
    let mut parts = name.to_str()?.split('.');
    let ("r", x, z, extension, None) = (
        parts.next()?,
        parts.next()?,
        parts.next()?,
        parts.next()?,
        parts.next(),
    ) else {
        return None;
    };
    extension.eq_ignore_ascii_case("mca").then_some(())?;
    Some([x.parse().ok()?, z.parse().ok()?])
}

fn distance(a: Position, b: Position) -> u64 {
    u64::from(a[0].abs_diff(b[0])) + u64::from(a[1].abs_diff(b[1]))
}

fn check_cancelled(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err("Minecraft map rendering cancelled".into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REGION: &[u8] = include_bytes!("../tests/fixtures/minecraft/region/r.-1.2.mca");
    const LEVEL: &[u8] = include_bytes!("../tests/fixtures/minecraft/level.dat");

    #[test]
    fn terrain_preview_keeps_the_full_map_pixels() {
        let file = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/minecraft/region/r.-1.2.mca");
        let cancel = AtomicBool::new(false);
        let world = World::open(&file, &cancel).unwrap();
        let full_tile = world.dimensions[0].render([-1, 2], &cancel).unwrap();
        let expected = imageops::crop_imm(&full_tile, 0, 0, 32, 32).to_image();
        assert_eq!(world.preview_terrain(&cancel).unwrap(), expected);
    }

    #[test]
    fn save_directory_level_marker_and_region_share_the_minedmap_preview() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/minecraft");
        let region = fixture.join("region/r.-1.2.mca");
        let cancel = AtomicBool::new(false);
        let world = World::open(&fixture, &cancel).unwrap();
        let preview = world.thumbnail(&cancel).unwrap();
        assert_eq!([preview.width, preview.height], [260, 260]);
        assert!(is_world_directory(&fixture));
        assert_eq!(world.dimensions[0].focus, [-1, 2]);
        for source in [fixture.join("level.dat"), region] {
            let opened = World::open(&source, &cancel).unwrap();
            assert_eq!(opened.thumbnail(&cancel).unwrap(), preview);
        }
        // A damaged chunk leaves one quadrant transparent while nearby terrain
        // retains the colors of its three block types.
        let pixel = |x, z| {
            let offset = (z * preview.width as usize + x) * 4;
            &preview.premultiplied_rgba[offset..offset + 4]
        };
        assert_eq!(pixel(245, 245)[3], 0);
        let terrain: std::collections::BTreeSet<_> =
            [pixel(15, 15), pixel(245, 15), pixel(15, 245)]
                .into_iter()
                .collect();
        assert!(terrain.iter().all(|rgba| rgba[3] == 255));
        assert_eq!(terrain.len(), 3);
    }

    #[test]
    fn modern_dimension_inventory_rejects_replaced_regions_and_protected_aliases() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let overworld = root.join("dimensions/minecraft/overworld/region");
        std::fs::create_dir_all(&overworld).unwrap();
        let path = overworld.join("r.-1.2.mca");
        std::fs::write(&path, REGION).unwrap();
        std::fs::write(root.join("level.dat"), LEVEL).unwrap();
        std::os::unix::fs::symlink("/proc", root.join("DIM-1")).unwrap();
        let cancel = AtomicBool::new(false);
        let world = World::open(root, &cancel).unwrap();
        assert_eq!(world.dimensions.len(), 1);
        let replacement = root.join("replacement");
        std::fs::write(&replacement, REGION).unwrap();
        std::fs::rename(&replacement, &path).unwrap();
        assert!(
            world.dimensions[0]
                .render([-1, 2], &cancel)
                .unwrap_err()
                .contains("changed")
        );
        assert!(World::open(root, &AtomicBool::new(true)).is_err());
    }
}
