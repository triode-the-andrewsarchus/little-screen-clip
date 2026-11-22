//! Functions for reading and deserializing region data

use std::{
	fs::File,
	io::{SeekFrom, prelude::*},
	path::Path,
};

use crate::bounds::{MAX_NBT_BYTES, decode};
use anyhow::{Context, Result, bail};
use flate2::read::{GzDecoder, ZlibDecoder};
use serde::de::DeserializeOwned;

use minedmap_types::*;

/// Data block size of region data files
///
/// After one header block, the region file consists of one or more consecutive blocks
/// of data for each populated chunk.
const BLOCKSIZE: usize = 4096;

/// Chunk descriptor extracted from region file header
#[derive(Debug)]
struct ChunkDesc {
	/// Offset of data block where the chunk starts
	offset: u32,
	/// Number of data block used by the chunk
	len: u8,
	/// Coodinates of chunk described by this descriptor
	coords: ChunkCoords,
}

/// Parses the header of a region data file
fn parse_header(header: &ChunkArray<u32>) -> Vec<ChunkDesc> {
	let mut chunks: Vec<_> = header
		.iter()
		.filter_map(|(coords, &chunk)| {
			let offset_len = u32::from_be(chunk);

			let offset = offset_len >> 8;
			let len = offset_len as u8;

			if offset == 0 || len == 0 {
				return None;
			}

			Some(ChunkDesc {
				offset,
				len,
				coords,
			})
		})
		.collect();

	chunks.sort_by_key(|chunk| chunk.offset);

	chunks
}

/// Decompresses chunk data and deserializes to a given data structure
fn decode_chunk<T>(buf: &[u8]) -> Result<T>
where
	T: DeserializeOwned,
{
	let (&format, buf) = buf
		.split_first()
		.context("Missing chunk compression type")?;
	let mut decode_buffer = Vec::new();
	let decode_bytes = match format {
		1 => GzDecoder::new(buf)
			.take(MAX_NBT_BYTES + 1)
			.read_to_end(&mut decode_buffer),
		2 => ZlibDecoder::new(buf)
			.take(MAX_NBT_BYTES + 1)
			.read_to_end(&mut decode_buffer),
		3 => return decode(buf).context("Failed to decode NBT data"),
		_ => bail!("Unsupported chunk compression type {format}"),
	}
	.context("Failed to decompress chunk data")?;
	if decode_bytes as u64 > MAX_NBT_BYTES {
		bail!("Chunk exceeds the decompression limit");
	}
	decode(&decode_buffer).context("Failed to decode NBT data")
}

/// Wraps a reader used to read a region data file
#[derive(Debug)]
pub struct Region<R: Read + Seek> {
	/// The wrapper reader
	reader: R,
}

impl<R: Read + Seek> Region<R> {
	/// Iterates over chunks in file order, leaving error handling to the caller.
	pub fn foreach_chunk_result<T: DeserializeOwned>(
		self,
		mut check: impl FnMut() -> Result<()>,
		mut visit: impl FnMut(ChunkCoords, Result<T>) -> Result<()>,
	) -> Result<()> {
		let Region { mut reader } = self;
		let mut header = ChunkArray::<u32>::default();
		reader
			.read_exact(bytemuck::cast_mut::<_, [u8; BLOCKSIZE]>(&mut header.0))
			.context("Failed to read region header")?;
		let mut previous_end = 2;
		for chunk in parse_header(&header) {
			check()?;
			let result = if chunk.offset < previous_end {
				Err(anyhow::anyhow!("Overlapping chunk sectors"))
			} else {
				previous_end = chunk.offset + u32::from(chunk.len);
				read_chunk(&mut reader, &chunk)
			};
			visit(
				chunk.coords,
				result.with_context(|| format!("Chunk {:?}", chunk.coords)),
			)?;
		}
		Ok(())
	}

	/// Iterates over readable chunks, returning the first decoding error.
	pub fn foreach_chunk<T: DeserializeOwned>(
		self,
		mut visit: impl FnMut(ChunkCoords, T) -> Result<()>,
	) -> Result<()> {
		self.foreach_chunk_result(|| Ok(()), |coords, chunk| visit(coords, chunk?))
	}
}

/// Reads the bounded payload identified by one location table entry.
fn read_chunk<T: DeserializeOwned>(
	reader: &mut (impl Read + Seek),
	chunk: &ChunkDesc,
) -> Result<T> {
	reader
		.seek(SeekFrom::Start(u64::from(chunk.offset) * BLOCKSIZE as u64))
		.context("Failed to seek chunk data")?;
	let mut len_buf = [0u8; 4];
	reader
		.read_exact(&mut len_buf)
		.context("Failed to read chunk length")?;
	let byte_len = u32::from_be_bytes(len_buf) as usize;
	if byte_len < 1 || byte_len > usize::from(chunk.len) * BLOCKSIZE - 4 {
		bail!("Invalid chunk length");
	}
	let mut buffer = vec![0; byte_len];
	reader
		.read_exact(&mut buffer)
		.context("Failed to read chunk data")?;
	decode_chunk(&buffer)
}

/// Creates a new [Region] from a reader
pub fn from_reader<R>(reader: R) -> Region<R>
where
	R: Read + Seek,
{
	Region { reader }
}

/// Creates a new [Region] for a file
pub fn from_file<P>(path: P) -> Result<Region<File>>
where
	P: AsRef<Path>,
{
	let file = File::open(path).context("Failed to open file")?;
	Ok(from_reader(file))
}
