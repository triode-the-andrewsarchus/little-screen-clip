//! Bounded, lossless observations of this process's own mount namespace.
use crate::Result;
use std::{
    fs::File,
    io::Read,
    os::unix::ffi::OsStringExt,
    path::{Path, PathBuf},
};
#[derive(Debug)]
pub(crate) struct Mount {
    pub id: u64,
    pub target: PathBuf,
    pub source: PathBuf,
    pub filesystem: String,
    pub root: PathBuf,
    pub read_only: bool,
}
pub(crate) fn inventory() -> Result<Vec<Mount>> {
    let mut bytes = Vec::new();
    File::open("/proc/self/mountinfo")?
        .take(2 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 2 * 1024 * 1024 {
        return Err("Mount inventory exceeds the bounded metadata limit".into());
    }
    parse(&bytes)
}
fn path(bytes: &[u8]) -> PathBuf {
    let mut output = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\'
            && i + 3 < bytes.len()
            && bytes[i + 1..i + 4]
                .iter()
                .all(|b| (b'0'..=b'7').contains(b))
        {
            let value = (bytes[i + 1] - b'0') as u16 * 64
                + (bytes[i + 2] - b'0') as u16 * 8
                + (bytes[i + 3] - b'0') as u16;
            if value <= 255 {
                output.push(value as u8);
                i += 4;
                continue;
            }
        }
        output.push(bytes[i]);
        i += 1;
    }
    std::ffi::OsString::from_vec(output).into()
}
fn parse(bytes: &[u8]) -> Result<Vec<Mount>> {
    let mut result = Vec::new();
    for line in bytes.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
        let fields = line
            .split(|b| *b == b' ')
            .filter(|f| !f.is_empty())
            .collect::<Vec<_>>();
        let separator = fields
            .iter()
            .position(|f| *f == b"-")
            .ok_or("Malformed mount metadata")?;
        if separator < 6 || fields.len() < separator + 4 {
            return Err("Incomplete mount metadata".into());
        }
        let id = std::str::from_utf8(fields[0])
            .ok()
            .and_then(|s| s.parse().ok())
            .ok_or("Unknown mount identity")?;
        result.push(Mount {
            id,
            target: path(fields[4]),
            source: path(fields[separator + 2]),
            filesystem: String::from_utf8_lossy(fields[separator + 1]).into(),
            root: path(fields[3]),
            read_only: fields[5].split(|b| *b == b',').any(|v| v == b"ro"),
        });
    }
    Ok(result)
}
pub(crate) fn catalog_mount(target: &Path, id: &str) -> Result<Option<Mount>> {
    let matches = inventory()?
        .into_iter()
        .filter(|m| m.target == target)
        .collect::<Vec<_>>();
    if matches.len() > 1 {
        return Err("Overlapping mounts make catalog ownership ambiguous".into());
    }
    Ok(matches.into_iter().next().filter(|m| {
        m.source == Path::new(&format!("little-clip-catalog:{id}"))
            && m.filesystem.starts_with("fuse")
    }))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn octal_and_non_unicode_mount_names_remain_lossless() {
        let data=b"45 1 8:1 / /media/a\\040b_\xff rw - vfat /dev/sdb1 rw\n46 1 0:3 / /tmp/catalog ro - fuse little-clip-catalog:fixture rw\n";
        let mounts = parse(data).unwrap();
        assert_eq!(
            mounts[0].target.as_os_str().as_encoded_bytes(),
            b"/media/a b_\xff"
        );
        assert!(mounts[1].read_only);
        assert_eq!(mounts[1].id, 46);
        assert!(parse(b"broken").is_err());
    }
}
