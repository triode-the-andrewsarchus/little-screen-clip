// SPDX-License-Identifier: MPL-2.0
//! d2renderers/d2svgimport/path.go ParsePath: the complete SVG path grammar
//! to absolute typed d2scene commands, used for the built-in paper and
//! sketch-streak patterns. (The rest of d2svgimport is not ported here.)

use d2_rs_scene::{PathCommand, PathCommandKind, Point};

pub(crate) struct PathLimits {
    pub max_bytes: usize,
    pub max_commands: usize,
}

pub(crate) fn parse_path(
    source: &str,
    data: &str,
    limits: PathLimits,
) -> Result<Vec<PathCommand>, String> {
    if limits.max_bytes == 0 || limits.max_commands == 0 {
        return Err("d2svgimport: path limits must be positive".into());
    }
    let source = if source.is_empty() {
        "SVG asset"
    } else {
        source
    };
    if data.len() > limits.max_bytes {
        return Err(format!(
            "d2svgimport: {source} path data is {} bytes, exceeding limit {}",
            data.len(),
            limits.max_bytes
        ));
    }
    let mut parser = Parser {
        source,
        data: data.as_bytes(),
        offset: 0,
        max_commands: limits.max_commands,
        commands: Vec::new(),
        parsed_commands: 0,
        after_explicit_command: false,
        current: Point::default(),
        subpath_start: Point::default(),
        have_current: false,
        previous: 0,
        last_cubic: Point::default(),
        last_quad: Point::default(),
    };
    parser.parse()
}

struct Parser<'a> {
    source: &'a str,
    data: &'a [u8],
    offset: usize,
    max_commands: usize,
    commands: Vec<PathCommand>,
    parsed_commands: usize,
    after_explicit_command: bool,
    current: Point,
    subpath_start: Point,
    have_current: bool,
    previous: u8,
    last_cubic: Point,
    last_quad: Point,
}

fn is_command(value: u8) -> bool {
    matches!(
        value,
        b'M' | b'm'
            | b'Z'
            | b'z'
            | b'L'
            | b'l'
            | b'H'
            | b'h'
            | b'V'
            | b'v'
            | b'C'
            | b'c'
            | b'S'
            | b's'
            | b'Q'
            | b'q'
            | b'T'
            | b't'
            | b'A'
            | b'a'
    )
}

fn is_whitespace(value: u8) -> bool {
    matches!(value, b' ' | b'\t' | b'\n' | b'\r' | b'\x0c')
}

fn previous_upper(command: u8) -> u8 {
    command.to_ascii_uppercase()
}

fn reflect_point(control: Point, around: Point) -> Point {
    Point::new(2.0 * around.x - control.x, 2.0 * around.y - control.y)
}

/// Go math.Remainder.
fn go_remainder(mut x: f64, mut y: f64) -> f64 {
    const TINY: f64 = 4.450_147_717_014_403e-308;
    const HALF_MAX: f64 = f64::MAX / 2.0;
    if x.is_nan() || y.is_nan() || x.is_infinite() || y == 0.0 {
        return f64::NAN;
    }
    if y.is_infinite() {
        return x;
    }
    let mut sign = false;
    if x < 0.0 {
        x = -x;
        sign = true;
    }
    if y < 0.0 {
        y = -y;
    }
    if x == y {
        return if sign { -0.0 } else { 0.0 };
    }
    if y <= HALF_MAX {
        x %= y + y;
    }
    if y < TINY {
        if x + x > y {
            x -= y;
            if x + x >= y {
                x -= y;
            }
        }
    } else {
        let y_half = 0.5 * y;
        if x > y_half {
            x -= y;
            if x >= y_half {
                x -= y;
            }
        }
    }
    if sign { -x } else { x }
}

impl Parser<'_> {
    fn error_at(&self, offset: usize, message: &str) -> String {
        format!("d2svgimport: {} path byte {offset}: {message}", self.source)
    }

    fn errorf(&self, message: &str) -> String {
        self.error_at(self.offset, message)
    }

    fn parse(&mut self) -> Result<Vec<PathCommand>, String> {
        let mut command = 0u8;
        loop {
            self.skip_whitespace();
            if self.offset == self.data.len() {
                return Ok(std::mem::take(&mut self.commands));
            }
            let mut explicit = false;
            if is_command(self.data[self.offset]) {
                command = self.data[self.offset];
                self.offset += 1;
                explicit = true;
                if command == b'Z' || command == b'z' {
                    if !self.have_current {
                        return Err(self.errorf("close command appears before a move"));
                    }
                    self.charge_command()?;
                    self.append(PathCommand::close())?;
                    self.current = self.subpath_start;
                    self.previous = command;
                    command = 0;
                    continue;
                }
            } else if command == 0 {
                return Err(self.errorf("expected a path command"));
            }
            let first_move = command == b'M' || command == b'm';
            self.charge_command()?;
            self.after_explicit_command = explicit;
            self.parse_group(command)?;
            if first_move {
                command = if command == b'M' { b'L' } else { b'l' };
            }
        }
    }

    fn parse_group(&mut self, command: u8) -> Result<(), String> {
        let relative = command.is_ascii_lowercase();
        let upper = command.to_ascii_uppercase();
        if upper != b'M' && !self.have_current {
            return Err(self.errorf(&format!(
                "{} command appears before a move",
                command as char
            )));
        }
        let point = |current: Point, x: f64, y: f64| {
            if relative {
                Point::new(x + current.x, y + current.y)
            } else {
                Point::new(x, y)
            }
        };
        match upper {
            b'M' => {
                let (x, y) = self.number_pair()?;
                let end = point(self.current, x, y);
                self.append(PathCommand::move_to(end.x, end.y))?;
                (self.current, self.subpath_start, self.have_current) = (end, end, true);
                self.reset_controls();
            }
            b'L' => {
                let (x, y) = self.number_pair()?;
                let end = point(self.current, x, y);
                self.append(PathCommand::line_to(end.x, end.y))?;
                self.current = end;
                self.reset_controls();
            }
            b'H' => {
                let mut x = self.number()?;
                if relative {
                    x += self.current.x;
                }
                self.append(PathCommand::line_to(x, self.current.y))?;
                self.current.x = x;
                self.reset_controls();
            }
            b'V' => {
                let mut y = self.number()?;
                if relative {
                    y += self.current.y;
                }
                self.append(PathCommand::line_to(self.current.x, y))?;
                self.current.y = y;
                self.reset_controls();
            }
            b'C' => {
                let v = self.numbers(6)?;
                let c1 = point(self.current, v[0], v[1]);
                let c2 = point(self.current, v[2], v[3]);
                let end = point(self.current, v[4], v[5]);
                self.append(PathCommand::cubic_to(c1.x, c1.y, c2.x, c2.y, end.x, end.y))?;
                (self.current, self.last_cubic, self.last_quad) = (end, c2, Point::default());
            }
            b'S' => {
                let v = self.numbers(4)?;
                let mut c1 = self.current;
                if matches!(previous_upper(self.previous), b'C' | b'S') {
                    c1 = reflect_point(self.last_cubic, self.current);
                }
                let c2 = point(self.current, v[0], v[1]);
                let end = point(self.current, v[2], v[3]);
                self.append(PathCommand::cubic_to(c1.x, c1.y, c2.x, c2.y, end.x, end.y))?;
                (self.current, self.last_cubic, self.last_quad) = (end, c2, Point::default());
            }
            b'Q' => {
                let v = self.numbers(4)?;
                let control = point(self.current, v[0], v[1]);
                let end = point(self.current, v[2], v[3]);
                self.append(PathCommand::quadratic_to(
                    control.x, control.y, end.x, end.y,
                ))?;
                (self.current, self.last_quad, self.last_cubic) = (end, control, Point::default());
            }
            b'T' => {
                let (x, y) = self.number_pair()?;
                let mut control = self.current;
                if matches!(previous_upper(self.previous), b'Q' | b'T') {
                    control = reflect_point(self.last_quad, self.current);
                }
                let end = point(self.current, x, y);
                self.append(PathCommand::quadratic_to(
                    control.x, control.y, end.x, end.y,
                ))?;
                (self.current, self.last_quad, self.last_cubic) = (end, control, Point::default());
            }
            b'A' => {
                let rx = self.number()?;
                let ry = self.number()?;
                let rotation = self.number()?;
                let large_arc = self.flag()?;
                let sweep = self.flag()?;
                let (x, y) = self.number_pair()?;
                if rx < 0.0 || ry < 0.0 {
                    return Err(self.errorf("arc radii must be non-negative"));
                }
                let end = point(self.current, x, y);
                if end != self.current {
                    if rx == 0.0 || ry == 0.0 {
                        self.append(PathCommand::line_to(end.x, end.y))?;
                    } else {
                        let radians = go_remainder(rotation, 360.0) / 180.0 * std::f64::consts::PI;
                        self.append(PathCommand::arc_to(
                            rx, ry, radians, large_arc, sweep, end.x, end.y,
                        ))?;
                    }
                }
                self.current = end;
                self.reset_controls();
            }
            _ => {
                return Err(self.errorf(&format!("unsupported path command '{}'", command as char)));
            }
        }
        self.previous = command;
        Ok(())
    }

    fn reset_controls(&mut self) {
        self.last_cubic = Point::default();
        self.last_quad = Point::default();
    }

    fn number_pair(&mut self) -> Result<(f64, f64), String> {
        Ok((self.number()?, self.number()?))
    }

    fn numbers(&mut self, count: usize) -> Result<Vec<f64>, String> {
        (0..count).map(|_| self.number()).collect()
    }

    fn number(&mut self) -> Result<f64, String> {
        if self.after_explicit_command {
            self.after_explicit_command = false;
            self.skip_whitespace();
            if self.data.get(self.offset) == Some(&b',') {
                return Err(self.errorf("comma is not allowed before a command's first argument"));
            }
        } else {
            self.separator()?;
        }
        let start = self.offset;
        if matches!(self.data.get(self.offset), Some(b'+' | b'-')) {
            self.offset += 1;
        }
        let mut digits = 0;
        while self.data.get(self.offset).is_some_and(u8::is_ascii_digit) {
            self.offset += 1;
            digits += 1;
        }
        if self.data.get(self.offset) == Some(&b'.') {
            self.offset += 1;
            while self.data.get(self.offset).is_some_and(u8::is_ascii_digit) {
                self.offset += 1;
                digits += 1;
            }
        }
        if digits == 0 {
            return Err(self.error_at(start, "expected a finite number"));
        }
        if matches!(self.data.get(self.offset), Some(b'e' | b'E')) {
            let exponent = self.offset;
            self.offset += 1;
            if matches!(self.data.get(self.offset), Some(b'+' | b'-')) {
                self.offset += 1;
            }
            let mut exponent_digits = 0;
            while self.data.get(self.offset).is_some_and(u8::is_ascii_digit) {
                self.offset += 1;
                exponent_digits += 1;
            }
            if exponent_digits == 0 {
                return Err(self.error_at(exponent, "number exponent has no digits"));
            }
        }
        let text = std::str::from_utf8(&self.data[start..self.offset]).expect("ASCII number");
        match d2_rs_core::parse_d2_float(text) {
            Some(value) if value.is_finite() => Ok(value),
            _ => Err(self.error_at(start, "expected a finite number")),
        }
    }

    fn flag(&mut self) -> Result<bool, String> {
        self.separator()?;
        match self.data.get(self.offset) {
            Some(b'0') => {
                self.offset += 1;
                Ok(false)
            }
            Some(b'1') => {
                self.offset += 1;
                Ok(true)
            }
            _ => Err(self.errorf("arc flag must be 0 or 1")),
        }
    }

    fn separator(&mut self) -> Result<(), String> {
        self.skip_whitespace();
        if self.data.get(self.offset) == Some(&b',') {
            self.offset += 1;
            self.skip_whitespace();
            if self.data.get(self.offset) == Some(&b',') {
                return Err(self.errorf("repeated comma in path data"));
            }
        }
        Ok(())
    }

    fn skip_whitespace(&mut self) {
        while self
            .data
            .get(self.offset)
            .is_some_and(|b| is_whitespace(*b))
        {
            self.offset += 1;
        }
    }

    fn append(&mut self, command: PathCommand) -> Result<(), String> {
        let finite_point = |p: Point| p.x.is_finite() && p.y.is_finite();
        let valid = match command.kind {
            PathCommandKind::MOVE | PathCommandKind::LINE => finite_point(command.p1),
            PathCommandKind::QUADRATIC => finite_point(command.p1) && finite_point(command.p2),
            PathCommandKind::CUBIC => {
                finite_point(command.p1) && finite_point(command.p2) && finite_point(command.p3)
            }
            PathCommandKind::ARC => {
                finite_point(command.p1)
                    && command.radius_x.is_finite()
                    && command.radius_y.is_finite()
                    && command.rotation.is_finite()
            }
            PathCommandKind::CLOSE => true,
            _ => false,
        };
        if !valid {
            return Err(self.errorf("path command has a non-finite derived coordinate"));
        }
        self.commands.push(command);
        Ok(())
    }

    fn charge_command(&mut self) -> Result<(), String> {
        if self.parsed_commands >= self.max_commands {
            return Err(self.errorf(&format!(
                "path command count exceeds limit {}",
                self.max_commands
            )));
        }
        self.parsed_commands += 1;
        Ok(())
    }
}
