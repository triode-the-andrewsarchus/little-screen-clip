//! In-memory tile rendering adapted from MinedMap.
mod util;
mod world;

use std::{
	io::{Read, Seek},
	sync::LazyLock,
};

use anyhow::{Context, Result, bail};
use indexmap::IndexSet;
use minedmap_resource as resource;
use minedmap_types as types;
use resource::{Biome, Colorf, block_color, needs_biome};
use types::*;
use util::coord_offset;
use world::layer;

pub use minedmap_nbt::data;
pub use world::de::{LevelDat, LevelDatData};

pub const TILE_SIZE: u32 = (BLOCKS_PER_CHUNK * CHUNKS_PER_REGION) as u32;

static BLOCKS: LazyLock<resource::BlockTypes> = LazyLock::new(resource::BlockTypes::default);
static BIOMES: LazyLock<resource::BiomeTypes> = LazyLock::new(resource::BiomeTypes::default);

/// Renders the Java Anvil region terrain from an already opened seekable input.
pub fn render_region(
	reader: impl Read + Seek,
	mut cancelled: impl FnMut() -> bool,
) -> Result<image::RgbaImage> {
	let mut check = || {
		if cancelled() {
			bail!("Minecraft map rendering cancelled");
		}
		Ok(())
	};
	let mut region = ProcessedRegion::default();
	let mut biomes = IndexSet::new();
	let mut first_error = None;
	minedmap_nbt::region::from_reader(reader).foreach_chunk_result(
		&mut check,
		|coords, result| {
			let result = result.and_then(|data: world::de::Chunk| {
				let (chunk, _) = world::chunk::Chunk::new(&data, &BLOCKS, &BIOMES)?;
				layer::top_layer(&mut biomes, &chunk)
			});
			match result {
				Ok(Some(layer::LayerData {
					blocks,
					biomes,
					depths,
					..
				})) => {
					region.chunks[coords] = Some(Box::new(ProcessedChunk {
						blocks,
						biomes,
						depths,
					}));
				}
				Ok(None) => {}
				Err(error) => {
					first_error.get_or_insert(error);
				}
			}
			Ok(())
		},
	)?;
	region.biome_list = biomes.into_iter().collect();

	let mut image = image::RgbaImage::new(TILE_SIZE, TILE_SIZE);
	for (coords, chunk) in region.chunks.iter() {
		if let Some(chunk) = chunk {
			check()?;
			render_chunk(&mut image, &region, chunk, coords);
		}
	}
	if !image.as_raw().chunks_exact(4).any(|rgba| rgba[3] != 0) {
		if let Some(error) = first_error {
			return Err(error).context("No renderable Minecraft chunks");
		}
		bail!("No renderable Minecraft chunks");
	}
	Ok(image)
}

/// Data structure for storing chunk data between processing and rendering steps
#[derive(Debug)]
struct ProcessedChunk {
	/// Block type data
	pub blocks: Box<layer::BlockArray>,
	/// Biome data
	pub biomes: Box<layer::BiomeArray>,
	/// Block height/depth data
	pub depths: Box<layer::DepthArray>,
}

/// Data structure for storing region data between processing and rendering steps
#[derive(Debug, Default)]
struct ProcessedRegion {
	/// List of biomes used in the region
	///
	/// Indexed by [ProcessedChunk] biome data
	pub biome_list: Vec<Biome>,
	/// Processed chunk data
	pub chunks: ChunkArray<Option<Box<ProcessedChunk>>>,
}

/// Returns the index of the biome at a block coordinate
///
/// The passed chunk and block coordinates relative to the center of the
/// region group is offset by *dx* and *dz*.
///
/// Returns the biome index within this tile, if the offset stays in the region.
fn biome_at(
	region: &ProcessedRegion,
	chunk: ChunkCoords,
	block: LayerBlockCoords,
	dx: i32,
	dz: i32,
) -> Option<u16> {
	let (region_x, chunk_x, block_x) = coord_offset(chunk.x, block.x, dx);
	let (region_z, chunk_z, block_z) = coord_offset(chunk.z, block.z, dz);
	let chunk = ChunkCoords {
		x: chunk_x,
		z: chunk_z,
	};
	let block = LayerBlockCoords {
		x: block_x,
		z: block_z,
	};
	if (region_x, region_z) != (0, 0) {
		return None;
	}
	Some(region.chunks[chunk].as_ref()?.biomes[block]?.get() - 1)
}

/// Computes the color of a tile pixel
fn block_color_at(
	region: &ProcessedRegion,
	chunk: &ProcessedChunk,
	chunk_coords: ChunkCoords,
	block_coords: LayerBlockCoords,
) -> Option<Colorf> {
	/// One quadrant of the kernel used to smooth biome edges
	///
	/// The kernel is mirrored in X und Z direction to build the full 5x5
	/// smoothing kernel.
	const SMOOTH: [[f32; 3]; 3] = [[41.0, 26.0, 7.0], [26.0, 16.0, 4.0], [7.0, 4.0, 1.0]];
	/// Maximum X coordinate offset to take into account for biome smoothing
	const X: isize = SMOOTH[0].len() as isize - 1;
	/// Maximum Z coordinate offset to take into account for biome smoothing
	const Z: isize = SMOOTH.len() as isize - 1;

	let block = chunk.blocks[block_coords]?;
	let depth = chunk.depths[block_coords]?;

	if !needs_biome(block) {
		return Some(block_color(block, None, depth.0 as f32));
	}

	let mut weights = rustc_hash::FxHashMap::<u16, f32>::default();
	for dz in -Z..=Z {
		for dx in -X..=X {
			let w = SMOOTH[dz.unsigned_abs()][dx.unsigned_abs()];
			if w == 0.0 {
				continue;
			}

			let Some(biome) = biome_at(region, chunk_coords, block_coords, dx as i32, dz as i32)
			else {
				continue;
			};

			*weights.entry(biome).or_default() += w;
		}
	}

	if weights.is_empty() {
		return None;
	}

	let mut color = Colorf::ZERO;
	let mut total = 0.0;

	for (index, w) in weights {
		let biome = region.biome_list.get(usize::from(index))?;

		total += w;
		color += w * block_color(block, Some(biome), depth.0 as f32);
	}

	Some(color / total)
}

/// Renders a chunk subtile into a region tile image
fn render_chunk(
	image: &mut image::RgbaImage,
	region: &ProcessedRegion,
	chunk: &ProcessedChunk,
	chunk_coords: ChunkCoords,
) {
	/// Width/height of a chunk subtile
	const N: u32 = BLOCKS_PER_CHUNK as u32;

	let chunk_image = image::RgbaImage::from_fn(N, N, |x, z| {
		let block_coords = LayerBlockCoords {
			x: BlockX::new(x),
			z: BlockZ::new(z),
		};
		let color = block_color_at(region, chunk, chunk_coords, block_coords);
		image::Rgba(
			color
				.map(|c| [c[0] as u8, c[1] as u8, c[2] as u8, 255])
				.unwrap_or_default(),
		)
	});
	image::imageops::overlay(
		image,
		&chunk_image,
		i64::from(chunk_coords.x.0) * BLOCKS_PER_CHUNK as i64,
		i64::from(chunk_coords.z.0) * BLOCKS_PER_CHUNK as i64,
	);
}
