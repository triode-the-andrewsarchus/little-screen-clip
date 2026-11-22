//! Checks the nesting depth before handing a document to the recursive deserializer.
use anyhow::{Result, ensure};
use serde::de::DeserializeOwned;

pub(crate) const MAX_NBT_BYTES: u64 = 16 * 1024 * 1024;
const MAX_DEPTH: u8 = 64;
const MAX_ELEMENTS: usize = 1 << 20;

pub(crate) fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
	let mut input = bytes;
	ensure!(
		read::<1>(&mut input) == Some([10])
			&& skip_string(&mut input).is_some()
			&& skip_payload(&mut input, 10, 0).is_some(),
		"Malformed NBT or nesting limit exceeded"
	);
	let options = fastnbt::DeOpts::new().max_seq_len(MAX_ELEMENTS);
	Ok(fastnbt::from_bytes_with_opts(bytes, options)?)
}

fn read<const N: usize>(input: &mut &[u8]) -> Option<[u8; N]> {
	let (value, rest) = input.split_first_chunk()?;
	*input = rest;
	Some(*value)
}

fn skip(input: &mut &[u8], count: usize) -> Option<()> {
	*input = input.get(count..)?;
	Some(())
}

fn skip_string(input: &mut &[u8]) -> Option<()> {
	let length = u16::from_be_bytes(read(input)?);
	skip(input, usize::from(length))
}

fn skip_payload(input: &mut &[u8], tag: u8, depth: u8) -> Option<()> {
	if depth > MAX_DEPTH {
		return None;
	}
	match tag {
		1..=6 => skip(input, [1, 2, 4, 8, 4, 8][usize::from(tag - 1)]),
		7 | 11 | 12 => {
			let length = usize::try_from(i32::from_be_bytes(read(input)?)).ok()?;
			let width = [1, 4, 8][(tag as usize).saturating_sub(10)];
			skip(input, length.checked_mul(width)?)
		}
		8 => skip_string(input),
		9 => {
			let [tag] = read(input)?;
			let length = i32::from_be_bytes(read(input)?).max(0) as usize;
			if length > MAX_ELEMENTS {
				return None;
			}
			for _ in 0..length {
				skip_payload(input, tag, depth + 1)?;
			}
			Some(())
		}
		10 => loop {
			let [tag] = read(input)?;
			if tag == 0 {
				break Some(());
			}
			skip_string(input)?;
			skip_payload(input, tag, depth + 1)?;
		},
		_ => None,
	}
}
