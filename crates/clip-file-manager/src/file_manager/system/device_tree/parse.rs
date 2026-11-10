//! Device-tree wire types, independent of filesystem access.
use std::io;

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

pub(super) fn strings(bytes: &[u8]) -> io::Result<Vec<String>> {
    let Some(body) = bytes.strip_suffix(&[0]) else {
        return Err(invalid("Device-tree string is not NUL terminated"));
    };
    let parts = body.split(|byte| *byte == 0).collect::<Vec<_>>();
    if parts.len() > 32 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "Device-tree string list exceeds 32 entries",
        ));
    }
    parts
        .into_iter()
        .map(|part| {
            let text = std::str::from_utf8(part)
                .map_err(|_| invalid("Invalid UTF-8 device-tree string"))?;
            if text.len() > 1024 || text.chars().any(char::is_control) {
                return Err(invalid(
                    "Device-tree string exceeds display limits or contains controls",
                ));
            }
            Ok(text.to_string())
        })
        .collect()
}

pub(super) fn cells(bytes: &[u8]) -> io::Result<Vec<u32>> {
    if bytes.len() % 4 != 0 {
        return Err(invalid("Device-tree cells are not aligned to four bytes"));
    }
    if bytes.len() > 4096 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "Device-tree cell list exceeds 1024 cells",
        ));
    }
    Ok(bytes
        .chunks_exact(4)
        .map(|cell| u32::from_be_bytes(cell.try_into().unwrap()))
        .collect())
}

pub(super) fn scalar(bytes: &[u8]) -> io::Result<u32> {
    let cells = cells(bytes)?;
    if cells.len() != 1 {
        return Err(invalid("Expected exactly one device-tree cell"));
    }
    Ok(cells[0])
}

pub(super) fn registers(
    bytes: &[u8],
    address_cells: u32,
    size_cells: u32,
) -> io::Result<Vec<String>> {
    if address_cells > 4 || size_cells > 4 || address_cells + size_cells == 0 {
        return Err(invalid("Unsupported device-tree register cell widths"));
    }
    let cells = cells(bytes)?;
    let width = (address_cells + size_cells) as usize;
    if cells.len() % width != 0 {
        return Err(invalid("Incomplete device-tree register tuple"));
    }
    if cells.len() / width > 32 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "Device-tree register list exceeds 32 tuples",
        ));
    }
    let integer = |cells: &[u32]| {
        cells
            .iter()
            .fold(0u128, |value, cell| (value << 32) | u128::from(*cell))
    };
    Ok(cells
        .chunks_exact(width)
        .map(|tuple| {
            let address = integer(&tuple[..address_cells as usize]);
            let address = if address_cells == 0 {
                "no address cells".into()
            } else {
                format!("parent-bus address 0x{address:x}")
            };
            if size_cells == 0 {
                format!("{address} (no size cells)")
            } else {
                format!(
                    "{address} · extent 0x{:x}",
                    integer(&tuple[address_cells as usize..])
                )
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strings_and_cells_follow_binary_wire_encoding() {
        assert_eq!(
            strings(b"vendor,uart\0ns16550\0").unwrap(),
            ["vendor,uart", "ns16550"]
        );
        assert_eq!(scalar(&[0, 0, 0, 2]).unwrap(), 2);
        assert_eq!(cells(&[0x12, 0x34, 0x56, 0x78]).unwrap(), [0x12345678]);
        for bytes in [b"unterminated".as_slice(), b"bad\n\0", &[0xff, 0]] {
            assert!(strings(bytes).is_err());
        }
        assert!(cells(&[0, 0, 1]).is_err());
        assert!(scalar(&[0; 8]).is_err());
    }
    #[test]
    fn registers_use_parent_widths_without_translating_bus_addresses() {
        let raw = [0, 1, 0x23456789, 0, 0x100]
            .into_iter()
            .flat_map(u32::to_be_bytes)
            .collect::<Vec<_>>();
        assert!(registers(&raw, 3, 2).unwrap()[0].contains("0x123456789"));
        assert!(registers(&1u32.to_be_bytes(), 1, 0).unwrap()[0].contains("no size cells"));
        assert!(registers(&raw, 2, 2).is_err());
        assert!(registers(&[], 0, 0).is_err());
        assert!(registers(&raw, 5, 0).is_err());
    }
}
