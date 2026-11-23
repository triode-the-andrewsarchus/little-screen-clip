use super::{
    Binary, Binding, Dimension, Expression, ExpressionError, Function, Kind, Node, SignalType, Unit,
};

const MAX_BYTES: usize = 512;
const MAX_TOKENS: usize = 128;
const MAX_DEPTH: usize = 24;
const MAX_DELAY: u16 = 8192;

type Result<T> = std::result::Result<T, ExpressionError>;

#[derive(Clone, Copy)]
enum Mode {
    Signal { active: usize, history: bool },
    Scalar,
}

pub(super) fn parse(
    source: &str,
    channels: &[Binding<'_>],
    active: usize,
    history: bool,
) -> Result<Expression> {
    parse_with(source, channels, Mode::Signal { active, history }, None)
}

pub(super) fn parse_scalar(source: &str, bindings: &[Binding<'_>]) -> Result<Expression> {
    parse_with(source, bindings, Mode::Scalar, None)
}

pub(super) fn bind_input_names(
    source: &str,
    channels: &[Binding<'_>],
    active: usize,
    names: &[String],
) -> Result<String> {
    if channels.len() != names.len() {
        return Err(error(0, "Missing persistent waveform names"));
    }
    let mut comparisons = Vec::new();
    parse_with(
        source,
        channels,
        Mode::Signal {
            active,
            history: false,
        },
        Some(&mut comparisons),
    )?;
    let mut edits = Vec::with_capacity(comparisons.len() * 2);
    for span in comparisons {
        edits.push((span.start, "(x "));
        edits.push((source[..span.end].trim_end().len(), ")"));
    }
    edits.sort_by_key(|&(at, _)| at);
    let mut explicit = String::with_capacity(source.len());
    let mut previous = 0;
    for (at, insert) in edits {
        explicit.push_str(&source[previous..at]);
        explicit.push_str(insert);
        previous = at;
    }
    explicit.push_str(&source[previous..]);
    rewrite_references(&explicit, |name| {
        if name.eq_ignore_ascii_case("x") {
            Some(names[active].as_str())
        } else {
            channels
                .iter()
                .position(|channel| channel.name.eq_ignore_ascii_case(name))
                .map(|index| names[index].as_str())
        }
    })
}

fn parse_with(
    source: &str,
    channels: &[Binding<'_>],
    mode: Mode,
    comparisons: Option<&mut Vec<std::ops::Range<usize>>>,
) -> Result<Expression> {
    if source.len() > MAX_BYTES {
        return Err(error(0, "Expression is too long"));
    }
    if let Mode::Signal { active, .. } = mode
        && active >= channels.len()
    {
        return Err(error(0, "No active waveform"));
    }
    let mut lexer = Lexer {
        rest: source,
        source_len: source.len(),
    };
    let mut tokens = Vec::new();
    loop {
        let lexeme = lexer.next()?;
        if matches!(mode, Mode::Scalar) && !lexeme.token.scalar() {
            return Err(error(lexeme.at, "Expected decimal arithmetic"));
        }
        if lexeme.token == Token::End {
            tokens.push(lexeme);
            break;
        }
        if tokens.len() == MAX_TOKENS {
            return Err(error(lexeme.at, "Expression has too many terms"));
        }
        tokens.push(lexeme);
    }
    let mut parser = Parser {
        tokens: &tokens,
        channels,
        mode,
        comparisons,
    };
    let expression = parser.expression(1, 0)?;
    parser.expect(Token::End, "Unexpected text after the expression")?;
    Ok(expression)
}

fn error(position: usize, message: impl Into<String>) -> ExpressionError {
    ExpressionError {
        position,
        message: message.into(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Token<'a> {
    Number(f64, Unit),
    Name(&'a str),
    Operator(Binary),
    Open,
    Close,
    IndexOpen,
    IndexClose,
    Comma,
    Not,
    End,
}

impl Token<'_> {
    fn scalar(self) -> bool {
        matches!(
            self,
            Self::Number(_, Unit::Implicit)
                | Self::Name(_)
                | Self::Operator(
                    Binary::Add | Binary::Subtract | Binary::Multiply | Binary::Divide
                )
                | Self::Open
                | Self::Close
                | Self::End
        )
    }
}

#[derive(Clone, Copy)]
struct Lexeme<'a> {
    token: Token<'a>,
    at: usize,
}

struct Lexer<'a> {
    rest: &'a str,
    source_len: usize,
}

pub(in crate::engineering::analysis) fn rename_reference(
    source: &str,
    previous: &str,
    renamed: &str,
) -> Result<String> {
    rewrite_references(source, |name| {
        name.eq_ignore_ascii_case(previous).then_some(renamed)
    })
}

pub(in crate::engineering::analysis) fn rewrite_references<'a>(
    source: &str,
    replacement: impl Fn(&str) -> Option<&'a str>,
) -> Result<String> {
    let mut lexer = Lexer {
        rest: source,
        source_len: source.len(),
    };
    let mut output = String::with_capacity(source.len());
    let mut copied = 0;
    let mut subscript = false;
    loop {
        let Lexeme { token, at } = lexer.next()?;
        match token {
            Token::End => break,
            Token::IndexOpen => subscript = true,
            Token::IndexClose => subscript = false,
            Token::Name(name) if !subscript && !lexer.rest.trim_start().starts_with('(') => {
                if let Some(name_replacement) = replacement(name) {
                    output.push_str(&source[copied..at]);
                    output.push_str(name_replacement);
                    copied = at + name.len();
                }
            }
            _ => {}
        }
    }
    output.push_str(&source[copied..]);
    Ok(output)
}

impl<'a> Lexer<'a> {
    fn next(&mut self) -> Result<Lexeme<'a>> {
        self.rest = self.rest.trim_start();
        let at = self.source_len - self.rest.len();
        let token = match self.rest.chars().next() {
            None => Token::End,
            Some(c) if c.is_ascii_digit() || c == '.' => self.number(at)?,
            Some(c) if c.is_alphabetic() || c == '_' => self.name(),
            Some(c) => self
                .symbol()
                .ok_or_else(|| error(at, format!("Unexpected character {c}")))?,
        };
        Ok(Lexeme { token, at })
    }

    fn number(&mut self, position: usize) -> Result<Token<'a>> {
        let (value, rest) =
            number_value(self.rest).ok_or_else(|| error(position, "Invalid number"))?;
        let without_space = rest.trim_start();
        let suffix_len = without_space
            .find(|c: char| !c.is_alphabetic())
            .unwrap_or(without_space.len());
        let suffix = &without_space[..suffix_len];
        let (scaled, unit, rest) = if let Some((dimension, factor)) = Dimension::literal(suffix) {
            (
                value * factor,
                Unit::Explicit(dimension),
                &without_space[suffix_len..],
            )
        } else if !suffix.is_empty() && rest.as_ptr() == without_space.as_ptr() {
            return Err(error(position, format!("Unknown sample unit {suffix}")));
        } else {
            (value, Unit::Implicit, rest)
        };
        if !scaled.is_finite() {
            return Err(error(position, "Number is out of range"));
        }
        self.rest = rest;
        Ok(Token::Number(scaled, unit))
    }

    fn name(&mut self) -> Token<'a> {
        let len = self
            .rest
            .find(|c: char| !(c.is_alphanumeric() || c == '_'))
            .unwrap_or(self.rest.len());
        let (name, tail) = self.rest.split_at(len);
        self.rest = tail;
        match name {
            "and" | "AND" => Token::Operator(Binary::And),
            "or" | "OR" => Token::Operator(Binary::Or),
            "xor" | "XOR" => Token::Operator(Binary::Xor),
            "not" | "NOT" => Token::Not,
            _ => Token::Name(name),
        }
    }

    fn symbol(&mut self) -> Option<Token<'a>> {
        use Binary::*;
        use Token::{Close, Comma, IndexClose, IndexOpen, Not, Open, Operator};
        for (text, token) in [
            ("&&", Operator(And)),
            ("||", Operator(Or)),
            ("&", Operator(And)),
            ("|", Operator(Or)),
            ("^", Operator(Xor)),
            ("~", Not),
            ("==", Operator(Equal)),
            ("!=", Operator(NotEqual)),
            ("<=", Operator(AtMost)),
            (">=", Operator(AtLeast)),
            ("<", Operator(Less)),
            (">", Operator(Greater)),
            ("+", Operator(Add)),
            ("-", Operator(Subtract)),
            ("*", Operator(Multiply)),
            ("/", Operator(Divide)),
            ("(", Open),
            (")", Close),
            ("[", IndexOpen),
            ("]", IndexClose),
            (",", Comma),
            ("!", Not),
        ] {
            if let Some(rest) = self.rest.strip_prefix(text) {
                self.rest = rest;
                return Some(token);
            }
        }
        None
    }
}

fn number_value(mut text: &str) -> Option<(f64, &str)> {
    let original = text;
    text = text.trim_start_matches(|c: char| c.is_ascii_digit());
    if let Some(rest) = text.strip_prefix('.') {
        text = rest.trim_start_matches(|c: char| c.is_ascii_digit());
    }
    if let Some(exponent) = text.strip_prefix(['e', 'E']) {
        text = exponent.strip_prefix(['+', '-']).unwrap_or(exponent);
        text = text.trim_start_matches(|c: char| c.is_ascii_digit());
    }
    let len = original.len() - text.len();
    let number = original[..len].parse::<f64>().ok()?;
    Some((number, text))
}

struct Parser<'a, 'input> {
    tokens: &'a [Lexeme<'input>],
    channels: &'a [Binding<'a>],
    mode: Mode,
    comparisons: Option<&'a mut Vec<std::ops::Range<usize>>>,
}

impl<'input> Parser<'_, 'input> {
    fn peek(&self) -> Lexeme<'input> {
        // The terminal token stays in place for diagnostics after consumption.
        self.tokens[0]
    }

    fn take(&mut self) -> Lexeme<'input> {
        let token = self.peek();
        if token.token != Token::End {
            self.tokens = &self.tokens[1..];
        }
        token
    }

    fn expect(&mut self, expected: Token<'_>, message: &'static str) -> Result<()> {
        let next = self.take();
        if next.token == expected {
            Ok(())
        } else {
            Err(error(next.at, message))
        }
    }

    fn expression(&mut self, minimum: u8, depth: usize) -> Result<Expression> {
        if depth >= MAX_DEPTH {
            return Err(error(self.peek().at, "Expression is nested too deeply"));
        }
        let mut left = self.prefix(depth)?;
        while let Token::Operator(operator) = self.peek().token {
            if operator.precedence() < minimum {
                break;
            }
            let at = self.take().at;
            left = self.binary(operator, left, at, depth)?;
        }
        Ok(left)
    }

    fn prefix(&mut self, depth: usize) -> Result<Expression> {
        let Lexeme { token, at } = self.take();
        match token {
            Token::Number(value, unit) => Ok(Expression {
                root: Node::Constant(value),
                kind: Kind::Numeric(unit),
            }),
            Token::Name(name) if self.peek().token == Token::Open => self.function(name, at, depth),
            Token::Name(name) => self.named_value(name, at),
            Token::Open => {
                let value = self.expression(1, depth + 1)?;
                self.expect(Token::Close, "Expected a closing parenthesis")?;
                Ok(value)
            }
            Token::Not => self.unary(true, at, depth),
            Token::Operator(Binary::Subtract) => self.unary(false, at, depth),
            Token::Operator(operator) if operator.comparison() => {
                let Mode::Signal { active, .. } = self.mode else {
                    return Err(error(at, "Scalar expressions use numeric arithmetic"));
                };
                let expression = self.binary(operator, self.channel(active, 0), at, depth)?;
                let end = self.peek().at;
                if let Some(comparisons) = &mut self.comparisons {
                    comparisons.push(at..end);
                }
                Ok(expression)
            }
            _ => Err(error(at, "Expected a waveform, number, or expression")),
        }
    }

    fn named_value(&mut self, name: &str, at: usize) -> Result<Expression> {
        let index = match self.mode {
            Mode::Signal { active, .. } if name.eq_ignore_ascii_case("x") => Some(active),
            Mode::Signal { .. } => self
                .channels
                .iter()
                .position(|candidate| candidate.name.eq_ignore_ascii_case(name)),
            Mode::Scalar => self
                .channels
                .iter()
                .position(|candidate| candidate.name == name),
        }
        .ok_or_else(|| match self.mode {
            Mode::Signal { .. } => error(at, format!("Unknown waveform {name}")),
            Mode::Scalar => error(at, format!("Unknown name {name}")),
        })?;
        let delay = if self.peek().token == Token::IndexOpen {
            if !matches!(self.mode, Mode::Signal { history: true, .. }) {
                return Err(error(
                    at,
                    "Sample history is available in the Derived lanes editor",
                ));
            }
            self.delay()?
        } else {
            0
        };
        Ok(self.channel(index, delay))
    }

    fn delay(&mut self) -> Result<u16> {
        self.take(); // Opening bracket
        self.expect(Token::Name("n"), "Use n for the current sample")?;
        let delay = if self.peek().token == Token::Operator(Binary::Subtract) {
            self.take();
            let offset = self.take();
            match offset.token {
                Token::Number(value, Unit::Implicit)
                    if value.fract() == 0.0 && (1.0..=f64::from(MAX_DELAY)).contains(&value) =>
                {
                    value as u16
                }
                _ => {
                    return Err(error(
                        offset.at,
                        format!("Use a delay of 1 to {MAX_DELAY} samples"),
                    ));
                }
            }
        } else {
            0
        };
        self.expect(Token::IndexClose, "Expected ] after a causal sample index")?;
        Ok(delay)
    }

    fn channel(&self, index: usize, delay: u16) -> Expression {
        let signal_type = self.channels[index].signal_type;
        let kind = match signal_type {
            SignalType::Analog(unit) => Kind::Numeric(Unit::Explicit(unit)),
            SignalType::Digital => Kind::Predicate,
        };
        Expression {
            root: Node::Channel(index, signal_type, delay),
            kind,
        }
    }

    fn unary(&mut self, predicate: bool, at: usize, depth: usize) -> Result<Expression> {
        let operand = self.expression(7, depth + 1)?;
        match (predicate, operand.kind) {
            (true, Kind::Predicate) => Ok(Expression {
                root: Node::Not(Box::new(operand.root)),
                kind: Kind::Predicate,
            }),
            (false, Kind::Numeric(_)) => Ok(Expression {
                root: Node::Negate(Box::new(operand.root)),
                kind: operand.kind,
            }),
            _ => Err(error(at, "Unary operator does not accept this operand")),
        }
    }

    fn binary(
        &mut self,
        operator: Binary,
        left: Expression,
        at: usize,
        depth: usize,
    ) -> Result<Expression> {
        let right = self.expression(operator.precedence() + 1, depth + 1)?;
        let kind = operator
            .kind(left.kind, right.kind)
            .ok_or_else(|| error(at, "Incompatible operands"))?;
        Ok(Expression {
            root: Node::Binary(operator, Box::new(left.root), Box::new(right.root)),
            kind,
        })
    }

    fn function(&mut self, name: &str, at: usize, depth: usize) -> Result<Expression> {
        if matches!(self.mode, Mode::Scalar) {
            if name != "float" || self.channels.iter().any(|binding| binding.name == name) {
                return Err(error(at, format!("Unsupported numeric call {name}")));
            }
            self.take(); // Opening parenthesis
            let result = self.expression(1, depth + 1)?;
            self.expect(Token::Close, "Expected a closing parenthesis")?;
            return Ok(result);
        }
        let function =
            Function::parse(name).ok_or_else(|| error(at, format!("Unknown function {name}")))?;
        self.take(); // Opening parenthesis
        let left = self.expression(1, depth + 1)?;
        let right = if self.peek().token == Token::Comma {
            self.take();
            Some(self.expression(1, depth + 1)?)
        } else {
            None
        };
        self.expect(Token::Close, "Expected a closing parenthesis")?;
        let kind = function
            .kind(left.kind, right.as_ref().map(|expr| expr.kind))
            .ok_or_else(|| error(at, "Function does not accept these arguments or units"))?;
        Ok(Expression {
            root: Node::Function(
                function,
                Box::new(left.root),
                right.map(|expr| Box::new(expr.root)),
            ),
            kind,
        })
    }
}
