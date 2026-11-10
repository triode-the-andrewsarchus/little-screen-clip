//! Mount associations are observations of this mount namespace, never device probes.
use super::super::model::Outcome;
use serde::{Deserialize, Serialize};
use std::{io, os::unix::ffi::OsStringExt, path::PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(in crate::file_manager) struct Mount {
    pub id: u64,
    pub parent: u64,
    pub device: (u32, u32),
    #[serde(with = "super::super::path_serde")]
    pub root: PathBuf,
    #[serde(with = "super::super::path_serde")]
    pub target: PathBuf,
    pub filesystem: String,
    #[serde(with = "super::super::path_serde")]
    pub source: PathBuf,
    pub read_only: bool,
    #[serde(default)]
    pub super_options: Vec<String>,
}
pub(in crate::file_manager::system) struct Inventory {
    pub records: Vec<Mount>,
    pub outcome: Outcome,
    pub malformed: usize,
}
pub(super) fn device_number(value: &str) -> Option<(u32, u32)> {
    let (major, minor) = value.split_once(':')?;
    Some((major.parse().ok()?, minor.parse().ok()?))
}
fn path(value: &[u8]) -> io::Result<PathBuf> {
    if value.contains(&0) {
        return Err(io::Error::other("NUL in mount path"));
    }
    let mut decoded = Vec::with_capacity(value.len());
    let mut index = 0;
    while index < value.len() {
        if value[index] != b'\\' {
            decoded.push(value[index]);
            index += 1;
            continue;
        }
        let digits = value
            .get(index + 1..index + 4)
            .ok_or_else(|| io::Error::other("Incomplete mount path escape"))?;
        let byte = match digits {
            b"040" => b' ',
            b"011" => b'\t',
            b"012" => b'\n',
            b"134" => b'\\',
            _ => return Err(io::Error::other("Invalid mount path escape")),
        };
        decoded.push(byte);
        index += 4;
    }
    Ok(std::ffi::OsString::from_vec(decoded).into())
}
fn record(line: &[u8]) -> Option<Mount> {
    let fields = line
        .split(|byte| byte.is_ascii_whitespace())
        .filter(|field| !field.is_empty())
        .collect::<Vec<_>>();
    let separator = fields.iter().position(|field| *field == b"-")?;
    if separator < 6 || fields.len() != separator + 4 {
        return None;
    }
    let id = mount_id(fields[0])?;
    let parent = mount_id(fields[1])?;
    let device = device_number(std::str::from_utf8(fields[2]).ok()?)?;
    let root = path(fields[3]).ok()?;
    let target = path(fields[4]).ok()?;
    if !root.is_absolute() || !target.is_absolute() {
        return None;
    }
    let filesystem = std::str::from_utf8(fields[separator + 1]).ok()?.to_string();
    Some(Mount {
        id,
        parent,
        device,
        root,
        target,
        filesystem,
        source: path(fields[separator + 2]).ok()?,
        super_options: std::str::from_utf8(fields[separator + 3])
            .ok()?
            .split(',')
            .map(str::to_owned)
            .collect(),
        read_only: fields[5]
            .split(|byte| *byte == b',')
            .any(|option| option == b"ro"),
    })
}
fn mount_id(value: &[u8]) -> Option<u64> {
    std::str::from_utf8(value).ok()?.parse().ok()
}
pub(in crate::file_manager::system) fn parse(bytes: &[u8]) -> Inventory {
    let mut records = Vec::new();
    let mut malformed = 0;
    let mut truncated = bytes.len() >= 4 * 1024 * 1024;
    for (index, line) in bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .enumerate()
    {
        if index >= 8192 {
            truncated = true;
            break;
        }
        match record(line) {
            Some(record) => records.push(record),
            None => malformed += 1,
        }
    }
    let outcome = if truncated {
        Outcome::Truncated
    } else if malformed != 0 {
        Outcome::Malformed
    } else {
        Outcome::Observed
    };
    Inventory {
        records,
        outcome,
        malformed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mountinfo_preserves_bind_roots_escapes_and_non_utf8_paths() {
        let data = b"36 25 8:1 /sub\\040dir /mnt/a\\040b rw,relatime shared:1 - ext4 /dev/sda1 rw\n37 25 8:1 / /mnt/\xff ro - ext4 /dev/sda1 rw\n";
        let inventory = parse(data);
        assert_eq!(inventory.outcome, Outcome::Observed);
        assert_eq!(inventory.records.len(), 2);
        assert_eq!(inventory.records[0].root, PathBuf::from("/sub dir"));
        assert_eq!(inventory.records[0].target, PathBuf::from("/mnt/a b"));
        assert!(inventory.records[1].read_only);
        let encoded = serde_json::to_vec(&inventory.records[1]).unwrap();
        let decoded: Mount = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded.target, inventory.records[1].target);
        assert_eq!(
            inventory.records[1].target,
            PathBuf::from(std::ffi::OsString::from_vec(b"/mnt/\xff".to_vec()))
        );
    }
    #[test]
    fn malformed_and_oversized_tables_do_not_claim_complete_empty_results() {
        assert_eq!(parse(b"invalid\n").outcome, Outcome::Malformed);
        let line = b"36 25 8:1 / /mnt rw - ext4 /dev/sda1 rw\n";
        let bytes = line.repeat(8193);
        let inventory = parse(&bytes);
        assert_eq!(inventory.records.len(), 8192);
        assert_eq!(inventory.outcome, Outcome::Truncated);
        assert!(device_number("8:1:2").is_none());
    }
}
