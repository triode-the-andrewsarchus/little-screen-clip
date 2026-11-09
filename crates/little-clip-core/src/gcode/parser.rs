use super::{MAX_MOVES, Move, arc};
#[derive(Clone)]
pub(super) struct State {
    position: [f64; 3],
    offset: [f64; 3],
    extrusion: f64,
    extrusion_offset: f64,
    absolute: bool,
    extrusion_absolute: bool,
    unit: f64,
    mode: u8,
    feed: Option<f64>,
    anchor: bool,
    pub homed: bool,
}
impl Default for State {
    fn default() -> Self {
        Self {
            position: [0.0; 3],
            offset: [0.0; 3],
            extrusion: 0.0,
            extrusion_offset: 0.0,
            absolute: true,
            extrusion_absolute: true,
            unit: 1.0,
            mode: 0,
            feed: None,
            anchor: false,
            homed: false,
        }
    }
}
impl State {
    pub fn line(
        &mut self,
        source: &str,
        line: usize,
        moves: &mut Vec<Move>,
    ) -> Result<bool, String> {
        let words = words(source)?;
        if words.is_empty() {
            return Ok(false);
        }
        let mut next = self.clone();
        let action = next.codes(&words)?;

        if action == Action::Home {
            next.position = [0.0; 3];
            next.offset = [0.0; 3];
            next.anchor = true;
            next.homed = true;
        } else if action == Action::Reset {
            next.reset(&words)?;
        } else if action != Action::Dwell {
            next.motion(&words, line, moves)?;
        }
        *self = next;
        Ok(action == Action::End)
    }
    fn codes(&mut self, words: &[(u8, f64)]) -> Result<Action, String> {
        let mut action = Action::Move;
        for &(_, number) in words.iter().filter(|w| w.0 == b'G') {
            let command = self.g_command(number)?;
            if command != Action::Move {
                action = command;
            }
        }
        for &(_, number) in words.iter().filter(|w| w.0 == b'M') {
            let command = self.m_command(number)?;
            if command != Action::Move {
                action = command;
            }
        }
        for &(letter, _) in words {
            if !b"GMFXYZEIJRNSTPL".contains(&letter) {
                return Err(format!(
                    "unsupported word {}; additional axes and subprograms are not inferred",
                    letter as char
                ));
            }
        }
        if let Some(feed) = value(words, b'F') {
            if feed <= 0.0 {
                return Err("feed must be positive".into());
            }
            self.feed = Some(feed * self.unit);
        }
        Ok(action)
    }
    fn g_command(&mut self, number: f64) -> Result<Action, String> {
        match number {
            0.0..=3.0 if number.fract() == 0.0 => self.mode = number as u8,
            4.0 => return Ok(Action::Dwell),
            17.0 | 40.0 | 49.0 | 54.0 | 91.1 => {}
            20.0 => self.unit = 25.4,
            21.0 => self.unit = 1.0,
            80.0 => self.mode = 4,
            90.0 | 91.0 => {
                self.absolute = number == 90.0;
                self.extrusion_absolute = self.absolute;
            }
            28.0 => return Ok(Action::Home),
            92.0 => return Ok(Action::Reset),
            _ => {
                return Err(format!(
                    "unsupported G{number}; offsets, cycles and other planes are not inferred"
                ));
            }
        }
        Ok(Action::Move)
    }
    fn m_command(&mut self, number: f64) -> Result<Action, String> {
        match number {
            82.0 | 83.0 => self.extrusion_absolute = number == 82.0,
            2.0 | 30.0 => return Ok(Action::End),
            0.0 | 1.0 | 3.0 | 4.0 | 5.0 | 7.0 | 8.0 | 9.0 | 17.0 | 18.0 | 84.0 | 104.0 | 105.0
            | 106.0 | 107.0 | 109.0 | 115.0 | 140.0 | 190.0 | 204.0 | 205.0 | 400.0 => {}
            _ => {
                return Err(format!(
                    "unsupported M{number}; its motion effects are not inferred"
                ));
            }
        }
        Ok(Action::Move)
    }
    fn reset(&mut self, words: &[(u8, f64)]) -> Result<(), String> {
        for (axis, letter) in b"XYZ".iter().enumerate() {
            if let Some(value) = value(words, *letter) {
                self.offset[axis] = self.position[axis] - value * self.unit;
            }
        }
        if let Some(value) = value(words, b'E') {
            self.extrusion_offset = self.extrusion - value * self.unit;
        }
        Ok(())
    }
    fn motion(
        &mut self,
        words: &[(u8, f64)],
        line: usize,
        moves: &mut Vec<Move>,
    ) -> Result<(), String> {
        let mut end = self.position;
        for (axis, letter) in b"XYZ".iter().enumerate() {
            if let Some(value) = value(words, *letter) {
                end[axis] = if self.absolute {
                    value * self.unit + self.offset[axis]
                } else {
                    end[axis] + value * self.unit
                };
            }
        }
        if end.iter().any(|v| v.abs() > 1e9) {
            return Err("coordinate exceeds the 1,000,000,000 mm preview limit".into());
        }
        let extrusion = value(words, b'E').map_or(self.extrusion, |v| {
            if self.extrusion_absolute {
                v * self.unit + self.extrusion_offset
            } else {
                self.extrusion + v * self.unit
            }
        });
        let has_motion = end != self.position
            || value(words, b'I').is_some()
            || value(words, b'J').is_some()
            || value(words, b'R').is_some();
        if has_motion && self.mode == 4 {
            return Err("G80 cancelled motion; an explicit G0/G1/G2/G3 is required".into());
        }
        if has_motion && !self.anchor {
            let points = if self.mode >= 2 {
                arc::points(self.position, end, words, self.unit, self.mode == 2)?
            } else {
                vec![end]
            };
            if moves.len() + points.len() > MAX_MOVES {
                return Err("200,000 drawing-interval limit reached".into());
            }
            let mut from = self.position;
            for to in points {
                moves.push(Move {
                    from,
                    to,
                    line,
                    rapid: self.mode == 0,
                    extruding: extrusion > self.extrusion,
                    feed_mm_min: self.feed,
                });
                from = to;
            }
        }
        if has_motion {
            self.anchor = false;
        }
        self.position = end;
        self.extrusion = extrusion;
        Ok(())
    }
}
#[derive(PartialEq)]
enum Action {
    Move,
    Reset,
    Home,
    End,
    Dwell,
}
pub(super) fn value(words: &[(u8, f64)], letter: u8) -> Option<f64> {
    words.iter().find(|w| w.0 == letter).map(|w| w.1)
}
fn words(source: &str) -> Result<Vec<(u8, f64)>, String> {
    if source.len() > 16384 {
        return Err("source line exceeds 16 KiB".into());
    }
    let mut clean = Vec::new();
    let mut depth = 0_u8;
    for byte in source.bytes() {
        match byte {
            b';' | b'*' if depth == 0 => break,
            b'(' => {
                depth = depth
                    .checked_add(1)
                    .filter(|n| *n <= 16)
                    .ok_or("comment nesting limit")?;
            }
            b')' => depth = depth.checked_sub(1).ok_or("unmatched comment")?,
            _ if depth == 0 && !byte.is_ascii_whitespace() => clean.push(byte.to_ascii_uppercase()),
            _ => {}
        }
    }
    if depth != 0 {
        return Err("unterminated comment".into());
    }
    if clean == b"%" || clean.is_empty() {
        return Ok(Vec::new());
    }
    tokenize(&clean)
}
fn tokenize(source: &[u8]) -> Result<Vec<(u8, f64)>, String> {
    let mut result = Vec::new();
    let mut index = 0;
    while index < source.len() {
        let letter = source[index];
        index += 1;
        if !letter.is_ascii_alphabetic() {
            return Err(
                "expressions, parameters and nonliteral instructions are unsupported".into(),
            );
        }
        let start = index;
        if source.get(index).is_some_and(|b| matches!(b, b'+' | b'-')) {
            index += 1;
        }
        while source
            .get(index)
            .is_some_and(|b| b.is_ascii_digit() || *b == b'.')
        {
            index += 1;
        }
        let number = std::str::from_utf8(&source[start..index])
            .ok()
            .and_then(|s| s.parse::<f64>().ok())
            .filter(|n| n.is_finite() && n.abs() <= 1e9)
            .ok_or("invalid numeric word")?;
        if letter != b'G' && letter != b'M' && result.iter().any(|&(l, _)| l == letter) {
            return Err("duplicate coordinate or parameter word".into());
        }
        result.push((letter, number));
        if result.len() > 64 {
            return Err("too many words on a source line".into());
        }
    }
    Ok(result)
}
