//! Only the documented process identity/session fields; never argv or memory.
use serde::{Deserialize, Serialize};
use std::{io, io::Read, path::Path};
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Stat {
    pub pid: u32,
    pub start: u64,
    pub name: String,
    pub parent: i32,
    pub group: i32,
    pub session: i32,
    pub tty: u32,
    pub foreground: i32,
}
pub(super) fn read(path: &Path) -> io::Result<Stat> {
    let mut bytes = Vec::new();
    super::super::collector::kernel_file(
        &path.join("stat"),
        little_clip_core::kernel_interface::Filesystem::Proc,
    )?
    .take(4096)
    .read_to_end(&mut bytes)?;
    if bytes.len() == 4096 {
        return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
    }
    parse(&bytes)
}
pub(super) fn parse(bytes: &[u8]) -> io::Result<Stat> {
    let malformed = || io::Error::new(io::ErrorKind::InvalidData, "Malformed process stat");
    let open = bytes
        .iter()
        .position(|b| *b == b'(')
        .ok_or_else(malformed)?;
    let close = bytes
        .iter()
        .rposition(|b| *b == b')')
        .filter(|i| *i > open)
        .ok_or_else(malformed)?;
    let pid = std::str::from_utf8(&bytes[..open])
        .map_err(|_| malformed())?
        .trim()
        .parse::<u32>()
        .map_err(|_| malformed())?;
    let tail = std::str::from_utf8(&bytes[close + 1..]).map_err(|_| malformed())?;
    let fields = tail.split_whitespace().take(20).collect::<Vec<_>>();
    if fields.len() != 20 || fields[0].len() != 1 {
        return Err(malformed());
    }
    let number = |i: usize| fields[i].parse::<i32>().map_err(|_| malformed());
    Ok(Stat {
        pid,
        start: fields[19].parse().map_err(|_| malformed())?,
        name: String::from_utf8_lossy(&bytes[open + 1..close])
            .chars()
            .take(64)
            .collect(),
        parent: number(1)?,
        group: number(2)?,
        session: number(3)?,
        tty: number(4)? as u32,
        foreground: number(5)?,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    pub(super) fn fixture(pid: u32, name: &str, start: u64) -> Vec<u8> {
        let mut fields = vec![
            "S".to_string(),
            "1".into(),
            "10".into(),
            "10".into(),
            "34817".into(),
            "10".into(),
        ];
        fields.extend(std::iter::repeat_n("0".to_string(), 13));
        fields.push(start.to_string());
        format!("{pid} ({name}) {}\n", fields.join(" ")).into_bytes()
    }
    #[test]
    fn parentheses_and_newlines_in_comm_do_not_shift_identity_fields() {
        let value = parse(&fixture(42, "a ) (b\nname", 12345)).unwrap();
        assert_eq!(value.pid, 42);
        assert_eq!(value.start, 12345);
        assert_eq!(value.group, 10);
        assert_eq!(value.tty, 34817);
        assert_eq!(value.name, "a ) (b\nname");
        assert!(parse(b"42 (short) S 1").is_err());
        assert!(parse(b"42 no parentheses").is_err());
    }
}
