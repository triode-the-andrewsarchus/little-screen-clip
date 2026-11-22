//! Functions for reading and deserializing compressed NBT data

use std::{fs::File, io::prelude::*, path::Path};

use crate::bounds::{MAX_NBT_BYTES, decode};
use anyhow::{Context, Result, bail};
use flate2::read::GzDecoder;
use serde::de::DeserializeOwned;

/// Reads compressed NBT data from a reader and deserializes to a given data structure
pub fn from_reader<R, T>(reader: R) -> Result<T>
where
	R: Read,
	T: DeserializeOwned,
{
	let mut decoder = GzDecoder::new(reader).take(MAX_NBT_BYTES + 1);
	let mut buf = vec![];
	decoder
		.read_to_end(&mut buf)
		.context("Failed to read file")?;

	if buf.len() as u64 > MAX_NBT_BYTES {
		bail!("NBT data exceeds the decompression limit");
	}
	decode(&buf).context("Failed to decode NBT data")
}

/// Reads compressed NBT data from a file and deserializes to a given data structure
pub fn from_file<P, T>(path: P) -> Result<T>
where
	P: AsRef<Path>,
	T: DeserializeOwned,
{
	let file = File::open(path).context("Failed to open file")?;
	from_reader(file)
}
