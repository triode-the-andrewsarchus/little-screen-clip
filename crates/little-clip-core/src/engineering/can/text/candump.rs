use super::{
    super::{Channel, Direction, Frame, FrameKind, Identifier},
    decimal_ns, radix_value,
};

pub(super) fn parse(line: &str, bytes: bool) -> Option<Frame> {
    let mut fields = line.trim().split_ascii_whitespace();
    let time = fields.next()?.strip_prefix('(')?.strip_suffix(')')?;
    let time = decimal_ns(time, 9)?;
    let channel = interface(fields.next()?)?;
    let (token, payload) = fields.next()?.split_once('#')?;
    if !matches!(token.len(), 3 | 8) {
        return None;
    }
    let raw_id = radix_value(token, 16)?;
    let direction = match (fields.next(), fields.next()) {
        (None, None) => Direction::Unknown,
        (Some("R" | "Rx"), None) => Direction::Rx,
        (Some("T" | "Tx"), None) => Direction::Tx,
        _ => return None,
    };
    let (kind, payload, dlc) = payload_kind(raw_id, payload)?;
    let mut frame = Frame::new(time, channel, kind);
    if kind != FrameKind::Error {
        frame.id = Some(Identifier::new(raw_id, token.len() == 8)?);
    }
    frame.direction = direction;
    frame.dlc = dlc;
    let maximum = if matches!(kind, FrameKind::Flexible { .. }) {
        64
    } else {
        8
    };
    let (length, data) = payload_bytes(payload, maximum, bytes)?;
    frame.data_len = length;
    frame.data = data;
    Some(frame)
}

fn interface(name: &str) -> Option<Channel> {
    let characters = |byte: u8| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte);
    (name.len() <= 15 && name.bytes().all(characters)).then(|| Channel::Interface(name.to_owned()))
}

fn payload_kind(raw_id: u32, payload: &str) -> Option<(FrameKind, &str, Option<u16>)> {
    if let Some(payload) = payload.strip_prefix('#') {
        let flags = radix_value(payload.get(..1)?, 16)?;
        return Some((
            FrameKind::Flexible {
                bitrate_switch: flags & 1 != 0,
                error_state: flags & 2 != 0,
            },
            &payload[1..],
            None,
        ));
    }
    if let Some(length) = payload.strip_prefix(['r', 'R']) {
        let dlc = if length.is_empty() {
            None
        } else {
            Some(length.parse::<u16>().ok().filter(|length| *length <= 8)?)
        };
        return Some((FrameKind::Remote, "", dlc));
    }
    let kind = if raw_id & 0x2000_0000 == 0 {
        FrameKind::Classic
    } else {
        FrameKind::Error
    };
    Some((kind, payload, None))
}

fn payload_bytes(payload: &str, maximum: usize, retain: bool) -> Option<(u16, Vec<u8>)> {
    let length = payload.len() / 2;
    if length > maximum || !payload.len().is_multiple_of(2) || !payload.is_ascii() {
        return None;
    }
    let mut data = if retain {
        Vec::with_capacity(length)
    } else {
        Vec::new()
    };
    for at in (0..payload.len()).step_by(2) {
        let byte = radix_value(&payload[at..at + 2], 16)? as u8;
        if retain {
            data.push(byte);
        }
    }
    Some((length as u16, data))
}
