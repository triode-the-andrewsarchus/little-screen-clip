use super::{
    Encoding, Error, MAX_HEADER_BYTES, MAX_PORTS, Matrix, Metadata, Parameter, PortMode, Version,
    extension_ports, is_candidate_path,
    stream::{Lines, number},
};
use std::{
    io::{BufRead, Seek},
    path::Path,
};

pub(super) struct Header {
    pub metadata: Metadata,
    pub frequency_multiplier: f64,
    pub encoding: Encoding,
    pub matrix: Matrix,
    pub transpose_two_port: bool,
    pub network_start: u64,
}

impl Header {
    pub fn read(input: &mut Lines<impl BufRead + Seek>, path: &Path) -> Result<Self, Error> {
        if !is_candidate_path(path) {
            return Err(Error::Unsupported("filename extension"));
        }
        let first = input.content(MAX_HEADER_BYTES)?.ok_or(Error::Truncated)?;
        if let Some(options) = first.text.strip_prefix('#') {
            let options = Options::parse(options)?;
            return legacy(input, path, options);
        }
        let (name, argument) = keyword(first.text)?;
        let version = match (name.as_str(), argument.as_str()) {
            ("version", "2.0") => Version::V2,
            ("version", "2.1") => Version::V2_1,
            ("version", _) => return Err(Error::Unsupported("version")),
            _ => return Err(Error::Invalid("expected option line or [Version]")),
        };
        modern(input, path, version)
    }

    pub fn matrix_slots(&self) -> Vec<(usize, usize)> {
        let ports = self.metadata.ports;
        if self.transpose_two_port && ports == 2 && self.matrix == Matrix::Full {
            // Legacy two-port data varies the response axis fastest.
            return vec![(0, 0), (2, 2), (1, 1), (3, 3)];
        }
        let mut slots = Vec::with_capacity(ports * ports);
        for row in 0..ports {
            let columns = match self.matrix {
                Matrix::Full => 0..ports,
                Matrix::Lower => 0..row + 1,
                Matrix::Upper => row..ports,
            };
            for col in columns {
                let index = row * ports + col;
                let mirror = if self.matrix == Matrix::Full {
                    index
                } else {
                    col * ports + row
                };
                slots.push((index, mirror));
            }
        }
        slots
    }
}

struct Options {
    frequency: Option<f64>,
    parameter: Option<Parameter>,
    encoding: Option<Encoding>,
    resistance: Vec<f64>,
}

impl Options {
    fn parse(text: &str) -> Result<Self, Error> {
        let mut options = Self {
            frequency: None,
            parameter: None,
            encoding: None,
            resistance: Vec::new(),
        };
        let lowercase = text.to_ascii_lowercase();
        let mut tokens = lowercase.split_ascii_whitespace().peekable();
        while let Some(token) = tokens.next() {
            match token {
                "hz" | "khz" | "mhz" | "ghz" => options.set_frequency(token)?,
                "s" | "z" | "y" | "h" | "g" => options.set_parameter(token)?,
                "ri" | "ma" | "db" => options.set_encoding(token)?,
                "r" if options.resistance.is_empty() => {
                    options
                        .resistance
                        .push(resistance(tokens.next().ok_or(Error::Truncated)?)?);
                    while let Some(value) = tokens.next_if(|next| next.parse::<f64>().is_ok()) {
                        options.push_resistance(value)?;
                    }
                    if options.resistance.len() > 1 && tokens.peek().is_some() {
                        return Err(Error::Invalid(
                            "per-port resistance must end the option line",
                        ));
                    }
                }
                _ => return Err(Error::Invalid("unrecognized or repeated option")),
            }
        }
        Ok(options)
    }

    fn set_frequency(&mut self, token: &str) -> Result<(), Error> {
        let scale = match token {
            "hz" => 1.0,
            "khz" => 1.0e3,
            "mhz" => 1.0e6,
            _ => 1.0e9,
        };
        set_once(&mut self.frequency, scale)
    }

    fn set_parameter(&mut self, token: &str) -> Result<(), Error> {
        let parameter = match token {
            "s" => Parameter::S,
            "z" => Parameter::Z,
            "y" => Parameter::Y,
            "h" => Parameter::H,
            _ => Parameter::G,
        };
        set_once(&mut self.parameter, parameter)
    }

    fn set_encoding(&mut self, token: &str) -> Result<(), Error> {
        let encoding = match token {
            "ri" => Encoding::RealImaginary,
            "ma" => Encoding::MagnitudeAngle,
            _ => Encoding::DecibelAngle,
        };
        set_once(&mut self.encoding, encoding)
    }

    fn push_resistance(&mut self, value: &str) -> Result<(), Error> {
        if self.resistance.len() == MAX_PORTS {
            return Err(Error::Limit("port count"));
        }
        self.resistance.push(resistance(value)?);
        Ok(())
    }

    fn finish(
        self,
        version: Version,
        ports: usize,
        reference: Option<Vec<f64>>,
        network_start: u64,
    ) -> Result<Header, Error> {
        let parameter = self.parameter.unwrap_or(Parameter::S);
        if matches!(parameter, Parameter::G | Parameter::H) && ports != 2 {
            return Err(Error::Invalid(
                "hybrid parameters require a two-port matrix",
            ));
        }
        let reference_ohms = match reference {
            Some(values) if values.len() == ports => values,
            Some(_) => {
                return Err(Error::Invalid(
                    "reference resistance count does not match ports",
                ));
            }
            None => match self.resistance.as_slice() {
                [] => vec![50.0; ports],
                [global] => vec![*global; ports],
                per_port if version.legacy() && per_port.len() == ports => self.resistance,
                _ => {
                    return Err(Error::Invalid(
                        "reference resistance count does not match ports",
                    ));
                }
            },
        };
        Ok(Header {
            metadata: Metadata {
                version,
                parameter,
                ports,
                reference_ohms,
                declared_points: None,
                mixed_mode: Vec::new(),
                declared_noise_points: None,
            },
            frequency_multiplier: self.frequency.unwrap_or(1.0e9),
            encoding: self.encoding.unwrap_or(Encoding::MagnitudeAngle),
            matrix: Matrix::Full,
            transpose_two_port: version.legacy(),
            network_start,
        })
    }
}

fn resistance(token: &str) -> Result<f64, Error> {
    let value = number(token)?;
    if value <= 0.0 {
        return Err(Error::Invalid("reference resistance must be positive"));
    }
    Ok(value)
}

fn set_once<T>(slot: &mut Option<T>, value: T) -> Result<(), Error> {
    if slot.replace(value).is_some() {
        Err(Error::Invalid("repeated header field"))
    } else {
        Ok(())
    }
}

fn legacy(
    input: &mut Lines<impl BufRead + Seek>,
    path: &Path,
    options: Options,
) -> Result<Header, Error> {
    let start = input.position();
    let ports = match extension_ports(path) {
        Some(ports) => ports,
        None => {
            // Ambiguous .ts files need an identifiable option line and a
            // complete first record with a square number of parameter pairs.
            if options.frequency.is_none()
                || options.parameter.is_none()
                || options.encoding.is_none()
            {
                return Err(Error::Invalid("ambiguous legacy option line"));
            }
            let ports = legacy_ts_ports(input)?;
            input.seek(start)?;
            ports
        }
    };
    let version = if options.resistance.len() > 1 {
        Version::V1_1
    } else {
        Version::V1
    };
    options.finish(version, ports, None, start)
}

fn legacy_ts_ports(input: &mut Lines<impl BufRead + Seek>) -> Result<usize, Error> {
    let line = input.content(MAX_HEADER_BYTES)?.ok_or(Error::Truncated)?;
    let count = line.text.split_ascii_whitespace().count();
    if count < 3 || count % 2 != 1 {
        return Err(Error::Invalid("expected a frequency and parameter pairs"));
    }
    let mut values = count - 1;
    while let Some(line) = input.content(MAX_HEADER_BYTES)? {
        let count = line.text.split_ascii_whitespace().count();
        if count % 2 == 1 {
            break;
        }
        values += count;
        if values > 2 * MAX_PORTS * MAX_PORTS {
            return Err(Error::Limit("port count"));
        }
    }
    let pairs = values / 2;
    let ports = pairs.isqrt();
    if ports * ports != pairs || !(1..=MAX_PORTS).contains(&ports) {
        return Err(Error::Invalid("cannot infer a square network matrix"));
    }
    Ok(ports)
}

fn modern(
    input: &mut Lines<impl BufRead + Seek>,
    path: &Path,
    version: Version,
) -> Result<Header, Error> {
    let (options, ports) = modern_preamble(input, path)?;
    let fields = Modern::read(input, ports)?;
    let mut header = options.finish(version, ports, fields.reference, input.position())?;
    header.metadata.declared_points = fields.frequencies;
    header.metadata.declared_noise_points = fields.noise_frequencies;
    header.metadata.mixed_mode = fields.mixed_mode.unwrap_or_default();
    header.transpose_two_port = fields.transpose.unwrap_or(false);
    header.matrix = fields.matrix.unwrap_or_default();
    Ok(header)
}

// The version 2 preamble fixes the encoding and rank before any optional
// fields, whose validation depends on the rank.
fn modern_preamble(
    input: &mut Lines<impl BufRead + Seek>,
    path: &Path,
) -> Result<(Options, usize), Error> {
    let option = input.content(MAX_HEADER_BYTES)?.ok_or(Error::Truncated)?;
    let options = option
        .text
        .strip_prefix('#')
        .ok_or(Error::Invalid("expected an option line"))?;
    let options = Options::parse(options)?;
    if options.resistance.len() > 1 {
        return Err(Error::Invalid(
            "version 2 overrides references with [Reference]",
        ));
    }
    let line = input.content(MAX_HEADER_BYTES)?.ok_or(Error::Truncated)?;
    let (name, argument) = keyword(line.text)?;
    if name != "number of ports" {
        return Err(Error::Invalid("expected [Number of Ports]"));
    }
    let ports = positive_count(&argument)?
        .try_into()
        .map_err(|_| Error::Limit("port count"))?;
    if ports > MAX_PORTS {
        return Err(Error::Limit("port count"));
    }
    if extension_ports(path).is_some_and(|extension| extension != ports) {
        return Err(Error::Invalid("header port count disagrees with extension"));
    }
    Ok((options, ports))
}

#[derive(Default)]
struct Modern {
    frequencies: Option<u64>,
    noise_frequencies: Option<u64>,
    reference: Option<Vec<f64>>,
    transpose: Option<bool>,
    matrix: Option<Matrix>,
    mixed_mode: Option<Vec<PortMode>>,
}

impl Modern {
    fn read(input: &mut Lines<impl BufRead + Seek>, ports: usize) -> Result<Self, Error> {
        let mut fields = Self::default();
        loop {
            let line = input.content(MAX_HEADER_BYTES)?.ok_or(Error::Truncated)?;
            let (name, value) = keyword(line.text)?;
            if name == "network data" && value.is_empty() {
                fields.validate(ports)?;
                return Ok(fields);
            }
            fields.read_field(name, value, input, ports)?;
        }
    }

    fn read_field(
        &mut self,
        name: String,
        value: String,
        input: &mut Lines<impl BufRead + Seek>,
        ports: usize,
    ) -> Result<(), Error> {
        match name.as_str() {
            "number of frequencies" => set_once(&mut self.frequencies, positive_count(&value)?),
            "number of noise frequencies" => {
                set_once(&mut self.noise_frequencies, positive_count(&value)?)
            }
            "two port data order" => {
                let transpose = match value.as_str() {
                    "21_12" => true,
                    "12_21" => false,
                    _ => return Err(Error::Invalid("unknown two-port order")),
                };
                set_once(&mut self.transpose, transpose)
            }
            "matrix format" => {
                let matrix = match value.as_str() {
                    "full" => Matrix::Full,
                    "lower" => Matrix::Lower,
                    "upper" => Matrix::Upper,
                    _ => return Err(Error::Invalid("unknown matrix format")),
                };
                set_once(&mut self.matrix, matrix)
            }
            "reference" => set_once(&mut self.reference, references(value, input, ports)?),
            "mixed mode order" => set_once(&mut self.mixed_mode, modes(value, input, ports)?),
            "begin information" if value.is_empty() => skip_information(input),
            _ => Err(Error::Unsupported("header keyword")),
        }
    }

    fn validate(&self, ports: usize) -> Result<(), Error> {
        if self.frequencies.is_none() {
            return Err(Error::Invalid("missing [Number of Frequencies]"));
        }
        if self.transpose.is_some() != (ports == 2) {
            return Err(Error::Invalid(
                "two-port order must be declared for a two-port capture",
            ));
        }
        if ports != 2 && self.noise_frequencies.is_some() {
            return Err(Error::Invalid("noise records require a two-port capture"));
        }
        Ok(())
    }
}

fn positive_count(value: &str) -> Result<u64, Error> {
    value
        .parse::<u64>()
        .ok()
        .filter(|count| *count > 0)
        .ok_or(Error::Invalid("expected a positive integer"))
}

fn references(
    mut value: String,
    input: &mut Lines<impl BufRead + Seek>,
    ports: usize,
) -> Result<Vec<f64>, Error> {
    let mut values = Vec::with_capacity(ports);
    loop {
        for token in value.split_ascii_whitespace() {
            if values.len() == ports {
                return Err(Error::Invalid("too many reference resistances"));
            }
            values.push(resistance(token)?);
        }
        if values.len() == ports {
            return Ok(values);
        }
        value = input
            .content(MAX_HEADER_BYTES)?
            .ok_or(Error::Truncated)?
            .text
            .to_owned();
    }
}

fn modes(
    mut value: String,
    input: &mut Lines<impl BufRead + Seek>,
    ports: usize,
) -> Result<Vec<PortMode>, Error> {
    loop {
        if let Some(modes) = parse_modes(&value, ports)? {
            validate_modes(&modes, ports)?;
            return Ok(modes);
        }
        let line = input.content(MAX_HEADER_BYTES)?.ok_or(Error::Truncated)?;
        if line.text.starts_with('[') {
            return Err(Error::Invalid("incomplete mixed-mode order"));
        }
        value.push(' ');
        value.push_str(line.text);
    }
}

fn parse_modes(value: &str, ports: usize) -> Result<Option<Vec<PortMode>>, Error> {
    let mut modes = Vec::with_capacity(ports);
    // Spaces and line breaks around the comma are allowed.
    let value = value.to_ascii_lowercase().replace(',', " , ");
    let mut tokens = value.split_ascii_whitespace();
    while let Some(token) = tokens.next() {
        let Some((mode, first)) = token.split_at_checked(1) else {
            return Err(Error::Invalid("invalid mixed-mode port descriptor"));
        };
        let first = port_number(first, ports)?;
        let mode = match mode {
            "s" => PortMode::Single(first),
            "d" | "c" => {
                let Some(comma) = tokens.next() else {
                    return Ok(None);
                };
                if comma != "," {
                    return Err(Error::Invalid("expected comma in mixed-mode pair"));
                }
                let Some(second) = tokens.next() else {
                    return Ok(None);
                };
                let second = port_number(second, ports)?;
                if first == second {
                    return Err(Error::Invalid("mixed-mode pair repeats a port"));
                }
                if mode == "d" {
                    PortMode::Differential(first, second)
                } else {
                    PortMode::Common(first, second)
                }
            }
            _ => return Err(Error::Invalid("unknown mixed-mode descriptor")),
        };
        if modes.len() == ports {
            return Err(Error::Invalid(
                "mixed-mode descriptor count exceeds port count",
            ));
        }
        modes.push(mode);
    }
    Ok((modes.len() == ports).then_some(modes))
}

fn port_number(token: &str, ports: usize) -> Result<usize, Error> {
    token
        .parse()
        .ok()
        .filter(|port| (1..=ports).contains(port))
        .ok_or(Error::Invalid("mixed-mode port is outside matrix"))
}

fn validate_modes(modes: &[PortMode], ports: usize) -> Result<(), Error> {
    let mut used = vec![false; ports];
    for (index, &mode) in modes.iter().enumerate() {
        match mode {
            PortMode::Single(port) if !used[port - 1] => used[port - 1] = true,
            PortMode::Differential(a, b) if !used[a - 1] && !used[b - 1] => {
                if !modes.contains(&PortMode::Common(a, b)) {
                    return Err(Error::Invalid(
                        "missing common-mode axis for differential pair",
                    ));
                }
                used[a - 1] = true;
                used[b - 1] = true;
            }
            PortMode::Common(a, b)
                if modes.contains(&PortMode::Differential(a, b))
                    && !modes[..index].contains(&mode) => {}
            _ => return Err(Error::Invalid("conflicting mixed-mode port assignments")),
        }
    }
    Ok(())
}

fn skip_information(input: &mut Lines<impl BufRead + Seek>) -> Result<(), Error> {
    while let Some(line) = input.content(MAX_HEADER_BYTES)? {
        if line.text.eq_ignore_ascii_case("[End Information]") {
            return Ok(());
        }
    }
    Err(Error::Truncated)
}

pub(super) fn keyword(text: &str) -> Result<(String, String), Error> {
    let (name, argument) = text
        .strip_prefix('[')
        .and_then(|text| text.split_once(']'))
        .ok_or(Error::Invalid("expected a header keyword"))?;
    Ok((
        name.to_ascii_lowercase().replace('-', " "),
        argument.trim().to_ascii_lowercase(),
    ))
}
