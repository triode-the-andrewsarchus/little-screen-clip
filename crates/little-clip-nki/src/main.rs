// SPDX-License-Identifier: LGPL-3.0-only
//! Standalone mapping reader. No application crate links this executable.
mod binary;
mod container;
mod exs;
mod legacy;
mod reader;
#[cfg(test)]
mod tests;
use std::io::{Read, Write};
const INPUT: usize = 64 * 1024 * 1024;
const PAYLOAD: usize = 16 * 1024 * 1024;
type Result<T> = std::result::Result<T, String>;

fn read(bytes: &[u8]) -> Result<Vec<serde_json::Value>> {
    if bytes.len() > INPUT {
        return Err("NKI exceeds 64 MiB".into());
    }
    if exs::recognizes(bytes) {
        exs::read(bytes)
    } else if bytes.get(12..16) == Some(b"hsin") {
        let preset = container::preset(bytes)?;
        binary::read(&preset)
    } else {
        legacy::read(bytes)
    }
}

fn run() -> Result<()> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .lock()
        .take(INPUT as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    let instruments = read(&bytes)?;
    let output = serde_json::to_vec(&instruments).map_err(|e| e.to_string())?;
    if output.len() > 4 * 1024 * 1024 {
        return Err("Mapping response exceeds 4 MiB".into());
    }
    std::io::stdout()
        .lock()
        .write_all(&output)
        .map_err(|e| e.to_string())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
