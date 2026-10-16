//! Checked Linux rtnetlink framing; no casts of potentially unaligned buffers.
use std::{
    io,
    net::{Ipv4Addr, Ipv6Addr},
};
pub(super) fn malformed() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "Malformed rtnetlink response")
}
fn word(bytes: &[u8], offset: usize) -> io::Result<u16> {
    Ok(u16::from_ne_bytes(
        bytes
            .get(offset..offset + 2)
            .ok_or_else(malformed)?
            .try_into()
            .map_err(|_| malformed())?,
    ))
}
pub(super) fn integer(bytes: &[u8], offset: usize) -> io::Result<u32> {
    Ok(u32::from_ne_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or_else(malformed)?
            .try_into()
            .map_err(|_| malformed())?,
    ))
}
fn aligned(length: usize) -> usize {
    (length + 3) & !3
}
pub(super) struct Message<'a> {
    pub kind: u16,
    pub flags: u16,
    pub sequence: u32,
    pub payload: &'a [u8],
}
fn frames(
    bytes: &[u8],
    minimum: usize,
    limit: usize,
    length: impl Fn(&[u8]) -> io::Result<usize>,
) -> io::Result<Vec<&[u8]>> {
    let mut bytes = bytes;
    let mut frames = Vec::new();
    while !bytes.is_empty() {
        if bytes.len() < minimum {
            if bytes.len() < 4 && bytes.iter().all(|byte| *byte == 0) {
                break;
            }
            return Err(malformed());
        }
        let size = length(bytes)?;
        if size < minimum || size > bytes.len() || frames.len() >= limit {
            return Err(malformed());
        }
        frames.push(&bytes[..size]);
        let next = aligned(size);
        if next > bytes.len() {
            if size == bytes.len() {
                break;
            }
            return Err(malformed());
        }
        bytes = &bytes[next..];
    }
    Ok(frames)
}
pub(super) fn messages(bytes: &[u8]) -> io::Result<Vec<Message<'_>>> {
    frames(bytes, 16, 1024, |bytes| {
        integer(bytes, 0).map(|size| size as usize)
    })?
    .into_iter()
    .map(|frame| {
        Ok(Message {
            kind: word(frame, 4)?,
            flags: word(frame, 6)?,
            sequence: integer(frame, 8)?,
            payload: &frame[16..],
        })
    })
    .collect()
}
fn attributes(bytes: &[u8]) -> io::Result<Vec<(u16, &[u8])>> {
    frames(bytes, 4, 256, |bytes| {
        word(bytes, 0).map(|size| size as usize)
    })?
    .into_iter()
    .map(|frame| Ok((word(frame, 2)? & 0x3fff, &frame[4..])))
    .collect()
}
fn bytes<'a>(attrs: &[(u16, &'a [u8])], kind: u16) -> Option<&'a [u8]> {
    attrs
        .iter()
        .find(|(id, _)| *id == kind)
        .map(|(_, value)| *value)
}
fn number(attrs: &[(u16, &[u8])], kind: u16) -> io::Result<Option<u32>> {
    bytes(attrs, kind)
        .map(|value| {
            if value.len() != 4 {
                return Err(malformed());
            }
            integer(value, 0)
        })
        .transpose()
}
fn string(value: &[u8], limit: usize) -> io::Result<Vec<u8>> {
    let end = value
        .iter()
        .position(|byte| *byte == 0)
        .ok_or_else(malformed)?;
    if end == 0 || end > limit {
        return Err(malformed());
    }
    Ok(value[..end].to_vec())
}
#[derive(Debug)]
pub(super) struct Link {
    pub index: u32,
    pub name: Vec<u8>,
    pub kind: Option<String>,
    pub vlan: Option<u16>,
    pub hardware: u16,
    pub flags: u32,
    pub address: Vec<u8>,
    pub master: Option<u32>,
    pub parent: Option<u32>,
    pub peer_namespace: Option<i32>,
    pub tx_queues: Option<u32>,
    pub rx_queues: Option<u32>,
}
struct LinkFields {
    master: Option<u32>,
    parent: Option<u32>,
    peer_namespace: Option<i32>,
    tx: Option<u32>,
    rx: Option<u32>,
}
fn link_fields(attrs: &[(u16, &[u8])]) -> io::Result<LinkFields> {
    Ok(LinkFields {
        master: number(attrs, 10)?,
        parent: number(attrs, 5)?,
        peer_namespace: number(attrs, 37)?.map(|value| value as i32),
        tx: number(attrs, 31)?,
        rx: number(attrs, 32)?,
    })
}
impl Link {
    pub(super) fn local_parent(&self) -> Option<u32> {
        self.parent
            .filter(|index| self.peer_namespace.is_none() && *index > 0 && *index != self.index)
    }
}
pub(super) fn link(payload: &[u8]) -> io::Result<Link> {
    if payload.len() < 16 {
        return Err(malformed());
    }
    let index = integer(payload, 4)?;
    if index == 0 || index > i32::MAX as u32 {
        return Err(malformed());
    }
    let attrs = attributes(&payload[16..])?;
    let name = string(bytes(&attrs, 3).ok_or_else(malformed)?, 15)?;
    if name.contains(&b'/') || name.iter().any(|byte| byte.is_ascii_whitespace()) {
        return Err(malformed());
    }
    let address = bytes(&attrs, 1).unwrap_or_default().to_vec();
    if address.len() > 32 {
        return Err(malformed());
    }
    let (kind, vlan) = link_info(bytes(&attrs, 18))?;
    let fields = link_fields(&attrs)?;
    Ok(Link {
        index,
        name,
        kind,
        vlan,
        hardware: word(payload, 2)?,
        flags: integer(payload, 8)?,
        address,
        master: fields.master,
        parent: fields.parent,
        peer_namespace: fields.peer_namespace,
        tx_queues: fields.tx,
        rx_queues: fields.rx,
    })
}
fn link_info(payload: Option<&[u8]>) -> io::Result<(Option<String>, Option<u16>)> {
    let Some(payload) = payload else {
        return Ok((None, None));
    };
    let attrs = attributes(payload)?;
    let kind = bytes(&attrs, 1)
        .map(|value| string(value, 32).map(|bytes| String::from_utf8_lossy(&bytes).into_owned()))
        .transpose()?;
    let vlan = if kind.as_deref() == Some("vlan") {
        let nested = bytes(&attrs, 2)
            .map(attributes)
            .transpose()?
            .unwrap_or_default();
        bytes(&nested, 1)
            .map(|value| {
                if value.len() != 2 {
                    return Err(malformed());
                }
                word(value, 0)
            })
            .transpose()?
    } else {
        None
    };
    Ok((kind, vlan))
}
#[derive(Debug)]
pub(super) struct Address {
    pub index: u32,
    pub local: String,
    pub peer: Option<String>,
    pub prefix: u8,
    pub scope: u8,
    pub flags: u32,
}
fn ip(family: u8, value: &[u8]) -> io::Result<String> {
    match family {
        2 if value.len() == 4 => {
            Ok(Ipv4Addr::new(value[0], value[1], value[2], value[3]).to_string())
        }
        10 if value.len() == 16 => {
            Ok(Ipv6Addr::from(<[u8; 16]>::try_from(value).map_err(|_| malformed())?).to_string())
        }
        _ => Err(malformed()),
    }
}
pub(super) fn address(payload: &[u8]) -> io::Result<Option<Address>> {
    if payload.len() < 8 {
        return Err(malformed());
    }
    let family = payload[0];
    if !matches!(family, 2 | 10) {
        return Ok(None);
    }
    let prefix = payload[1];
    if prefix > if family == 2 { 32 } else { 128 } {
        return Err(malformed());
    }
    let attrs = attributes(&payload[8..])?;
    let local = bytes(&attrs, 2)
        .or_else(|| bytes(&attrs, 1))
        .ok_or_else(malformed)?;
    let local = ip(family, local)?;
    let peer = bytes(&attrs, 1)
        .map(|value| ip(family, value))
        .transpose()?
        .filter(|peer| *peer != local);
    Ok(Some(Address {
        index: integer(payload, 4)?,
        local,
        peer,
        prefix,
        scope: payload[3],
        flags: number(&attrs, 8)?.unwrap_or(payload[2] as u32),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn attribute(kind: u16, data: &[u8]) -> Vec<u8> {
        let mut value = Vec::new();
        value.extend_from_slice(&((data.len() + 4) as u16).to_ne_bytes());
        value.extend_from_slice(&kind.to_ne_bytes());
        value.extend_from_slice(data);
        value.resize(aligned(value.len()), 0);
        value
    }
    fn link_payload() -> Vec<u8> {
        let mut value = vec![0; 16];
        value[2..4].copy_from_slice(&1u16.to_ne_bytes());
        value[4..8].copy_from_slice(&7u32.to_ne_bytes());
        value[8..12].copy_from_slice(&1u32.to_ne_bytes());
        value.extend(attribute(3, b"vlan42\0"));
        value.extend(attribute(1, &[0, 1, 2, 3, 4, 5]));
        value
    }
    #[test]
    fn nested_vlan_data_queue_counts_and_relationships_are_checked() {
        let mut value = link_payload();
        let mut info = attribute(1, b"vlan\0");
        info.extend(attribute(0x8002, &attribute(1, &42u16.to_ne_bytes())));
        value.extend(attribute(0x8012, &info));
        value.extend(attribute(5, &2u32.to_ne_bytes()));
        value.extend(attribute(10, &3u32.to_ne_bytes()));
        value.extend(attribute(31, &4u32.to_ne_bytes()));
        let link = link(&value).unwrap();
        assert_eq!(link.kind.as_deref(), Some("vlan"));
        assert_eq!(link.vlan, Some(42));
        assert_eq!(link.master, Some(3));
        assert_eq!(link.local_parent(), Some(2));
        assert_eq!(link.tx_queues, Some(4));
        assert!(link.rx_queues.is_none());
    }
    #[test]
    fn foreign_namespace_peer_indices_never_map_to_a_local_interface() {
        let mut value = link_payload();
        value.extend(attribute(5, &2u32.to_ne_bytes()));
        value.extend(attribute(37, &(-1i32).to_ne_bytes()));
        let link = link(&value).unwrap();
        assert_eq!(link.peer_namespace, Some(-1));
        assert!(link.local_parent().is_none());
    }
    #[test]
    fn local_and_point_to_point_addresses_are_not_confused() {
        let mut value = vec![2, 24, 0, 253, 0, 0, 0, 0];
        value[4..8].copy_from_slice(&7u32.to_ne_bytes());
        value.extend(attribute(1, &[10, 0, 0, 2]));
        value.extend(attribute(2, &[10, 0, 0, 1]));
        value.extend(attribute(8, &64u32.to_ne_bytes()));
        let address = address(&value).unwrap().unwrap();
        assert_eq!(address.local, "10.0.0.1");
        assert_eq!(address.peer.as_deref(), Some("10.0.0.2"));
        assert_eq!(address.scope, 253);
        assert_eq!(address.flags, 64);
    }
    #[test]
    fn ipv6_prefixes_and_lengths_are_validated() {
        let mut value = vec![10, 128, 0, 254, 1, 0, 0, 0];
        value.extend(attribute(1, &Ipv6Addr::LOCALHOST.octets()));
        assert_eq!(address(&value).unwrap().unwrap().local, "::1");
        value[1] = 129;
        assert!(address(&value).is_err());
        assert!(address(&[10; 7]).is_err());
    }
    #[test]
    fn malformed_framing_and_non_utf8_names_are_handled_without_unaligned_access() {
        let mut value = link_payload();
        value.extend_from_slice(&[0, 0, 1, 0]);
        assert!(link(&value).is_err());
        assert!(messages(&[0; 16]).is_err());
        let mut value = vec![0u8; 16];
        value[..4].copy_from_slice(&17u32.to_ne_bytes());
        assert!(messages(&value).is_err());
        let mut value = vec![0; 16];
        value[4..8].copy_from_slice(&7u32.to_ne_bytes());
        value.extend(attribute(3, &[b'v', 255, 0]));
        assert_eq!(link(&value).unwrap().name, vec![b'v', 255]);
    }
}
