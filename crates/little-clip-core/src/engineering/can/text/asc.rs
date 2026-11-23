use super::{
    super::{Channel, Direction, Frame, FrameKind, Identifier},
    data_bytes, decimal_ns, radix_value,
};

#[derive(Debug)]
pub(super) struct Asc {
    radix: u32,
    relative: bool,
}

impl Asc {
    pub(super) fn recognize(text: &str) -> Option<Self> {
        let mut context = false;
        let mut settings = None;
        for line in text.lines().take(48) {
            let line = line.trim().to_ascii_lowercase();
            if line.starts_with("date ")
                || line.starts_with("begin triggerblock ")
                || line == "internal events logged"
                || line == "no internal events logged"
            {
                context = true;
            }
            let mut parts = line.split_ascii_whitespace();
            if parts.next() != Some("base") {
                continue;
            }
            let radix = match parts.next() {
                Some("hex") => 16,
                Some("dec") => 10,
                _ => continue,
            };
            let relative = match (parts.next(), parts.next()) {
                (Some("timestamps"), Some("absolute")) => false,
                (Some("timestamps"), Some("relative")) => true,
                _ => continue,
            };
            settings = Some(Self { radix, relative });
        }
        context.then_some(settings).flatten()
    }

    pub(super) fn parse(&self, line: &str, clock: &mut i128, bytes: bool) -> Option<Frame> {
        let mut words = line.split_ascii_whitespace();
        let time = decimal_ns(words.next()?, 9)?;
        let timestamp = if self.relative {
            *clock = clock.checked_add(time)?;
            *clock
        } else {
            time
        };
        let bus = words.next()?;
        if bus.eq_ignore_ascii_case("CANFD") {
            return self.fd(timestamp, words, bytes);
        }
        let channel = if bus.eq_ignore_ascii_case("CAN") {
            words.next()?.parse::<u16>().ok()?
        } else {
            bus.parse::<u16>().ok()?
        };
        self.classic(timestamp, Channel::Number(channel), words, bytes)
    }

    fn classic<'a>(
        &self,
        timestamp: i128,
        channel: Channel,
        mut words: impl Iterator<Item = &'a str>,
        bytes: bool,
    ) -> Option<Frame> {
        let token = words.next()?;
        if token.eq_ignore_ascii_case("ErrorFrame") {
            return Some(Frame::new(timestamp, channel, FrameKind::Error));
        }
        let id = self.identifier(token)?;
        let direction = Direction::parse(words.next()?);
        if direction == Direction::Unknown {
            return None;
        }
        let kind = match words.next()? {
            "r" | "R" => FrameKind::Remote,
            "d" | "D" => FrameKind::Classic,
            _ => return None,
        };
        let length = words.next().and_then(|n| n.parse::<u16>().ok());
        let mut frame = Frame::new(timestamp, channel, kind);
        frame.id = Some(id);
        frame.direction = direction;
        frame.dlc = length;
        if kind == FrameKind::Classic {
            frame.data_len = length?.min(8);
            frame.data = data_bytes(words, self.radix, frame.data_len, bytes)?;
        }
        Some(frame)
    }

    fn fd<'a>(
        &self,
        timestamp: i128,
        mut words: impl Iterator<Item = &'a str>,
        bytes: bool,
    ) -> Option<Frame> {
        let channel = Channel::Number(words.next()?.parse::<u16>().ok()?);
        let token = words.next()?;
        if token.eq_ignore_ascii_case("ErrorFrame") {
            return Some(Frame::new(timestamp, channel, FrameKind::Error));
        }
        let direction = Direction::parse(token);
        if direction == Direction::Unknown {
            return None;
        }
        let token = words.next()?;
        if token.eq_ignore_ascii_case("ErrorFrame") {
            let mut frame = Frame::new(timestamp, channel, FrameKind::Error);
            frame.direction = direction;
            return Some(frame);
        }
        let id = self.identifier(token)?;
        let flag = words.next()?;
        let brs = if matches!(flag, "0" | "1") {
            flag
        } else {
            words.next()?
        };
        let esi = words.next()?;
        if !matches!(brs, "0" | "1") || !matches!(esi, "0" | "1") {
            return None;
        }
        let dlc = u16::try_from(radix_value(words.next()?, self.radix)?).ok()?;
        let length = words.next()?.parse::<u16>().ok()?;
        if length > 64 || dlc > 15 {
            return None;
        }
        let kind = if length == 0 && dlc != 0 {
            FrameKind::Remote
        } else {
            FrameKind::Flexible {
                bitrate_switch: brs == "1",
                error_state: esi == "1",
            }
        };
        let mut frame = Frame::new(timestamp, channel, kind);
        frame.id = Some(id);
        frame.direction = direction;
        frame.dlc = Some(dlc);
        frame.data_len = length;
        frame.data = data_bytes(words, self.radix, length, bytes)?;
        Some(frame)
    }

    fn identifier(&self, word: &str) -> Option<Identifier> {
        let extended = word.ends_with(['x', 'X']);
        let number = word.strip_suffix(['x', 'X']).unwrap_or(word);
        Identifier::new(radix_value(number, self.radix)?, extended)
    }
}
