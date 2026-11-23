//! Dimensioned arithmetic and predicates for captured signals.

mod dimension;
mod parser;

pub use dimension::Dimension;
pub(super) use parser::{rename_reference, rewrite_references};

use std::fmt;

/// A compiled expression bound to the channels of one scope capture.
///
/// An expression such as `> 1V` compares the active channel against one volt.
/// `abs(CH1 - CH2) <= 100mV` compares two time-aligned channels.
/// Parsing resolves names and units once. Evaluation uses the supplied values
/// for a single instant, with absent samples treated as gaps.
#[derive(Clone, Debug)]
pub struct Expression {
    root: Node,
    kind: Kind,
}

/// Type of a source or an intermediate signal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SignalType {
    /// Equally or unevenly sampled values in a physical unit.
    Analog(Dimension),
    /// Digital levels. Unknown and high-impedance observations are gaps.
    Digital,
}

/// Name and type supplied by the signal graph when compiling an expression.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Binding<'a> {
    /// Case-insensitive variable in the expression.
    pub name: &'a str,
    /// Kind of value supplied for this binding.
    pub signal_type: SignalType,
}

/// Value produced at a single sample time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Value {
    /// Numeric result in base units. See [`Expression::voltage_power`].
    Numeric(f64),
    /// Whether the point meets the expression.
    Predicate(bool),
}

/// Reason an expression could not be compiled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExpressionError {
    /// Byte offset of the unrecognized or incompatible input.
    pub position: usize,
    /// Description of the parse or type error.
    pub message: String,
}

impl fmt::Display for ExpressionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at byte {}", self.message, self.position)
    }
}

impl std::error::Error for ExpressionError {}

impl Expression {
    /// Compiles channel arithmetic and comparisons for an aligned set of channels.
    ///
    /// Channel names are case-insensitive identifiers and `x` refers to the
    /// active channel. Supply channel samples in volts. Numeric literals support
    /// voltage units such as `V`, `mV`, and `µV`. A bare number combines with
    /// the unit of its other operand for comparisons, addition, and subtraction.
    /// Multiplication and division combine the units.
    ///
    /// # Errors
    ///
    /// Returns the location of invalid syntax, incompatible units, an unknown
    /// channel, or an expression beyond the parser's length and nesting limits.
    pub fn parse(
        input: &str,
        channel_names: &[&str],
        active_channel: usize,
    ) -> Result<Self, ExpressionError> {
        let bindings: Vec<_> = channel_names
            .iter()
            .map(|&name| Binding {
                name,
                signal_type: SignalType::Analog(Dimension::VOLTAGE),
            })
            .collect();
        Self::parse_bound(input, &bindings, active_channel)
    }

    /// Compiles an expression against named analog and digital inputs.
    ///
    /// The active binding handles the `x` alias and comparison shorthand.
    /// Samples and numeric literals use base units internally.
    ///
    /// # Errors
    /// Reports invalid syntax, incompatible units, or missing bindings.
    pub fn parse_bound(
        input: &str,
        bindings: &[Binding<'_>],
        active: usize,
    ) -> Result<Self, ExpressionError> {
        parser::parse(input, bindings, active, false)
    }

    /// Compiles the numeric subset shared by scalar configuration expressions.
    pub(crate) fn parse_scalar(
        input: &str,
        bindings: &[Binding<'_>],
    ) -> Result<Self, ExpressionError> {
        parser::parse_scalar(input, bindings)
    }

    pub(super) fn parse_history(
        input: &str,
        bindings: &[Binding<'_>],
        active: usize,
    ) -> Result<Self, ExpressionError> {
        parser::parse(input, bindings, active, true)
    }

    /// Returns the type of the generated signal.
    pub fn signal_type(&self) -> SignalType {
        match self.kind {
            Kind::Numeric(unit) => SignalType::Analog(unit.dimension()),
            Kind::Predicate => SignalType::Digital,
        }
    }

    /// Returns whether results are suitable for highlighting matching regions.
    pub fn is_predicate(&self) -> bool {
        self.kind == Kind::Predicate
    }

    /// Returns the voltage exponent of a numeric result.
    ///
    /// `Some(1)` means volts and `Some(0)` means dimensionless. Predicate
    /// results return `None`.
    pub fn voltage_power(&self) -> Option<i8> {
        match self.kind {
            Kind::Numeric(unit) => unit.dimension().voltage_power(),
            Kind::Predicate => None,
        }
    }

    /// Evaluates the compiled expression using the channel values at one instant.
    ///
    /// Supply a closure that returns a sample in volts for its zero-based
    /// channel index. Missing samples and non-finite results return `None`.
    pub fn evaluate(&self, channel_sample: impl Fn(usize) -> Option<f64>) -> Option<Value> {
        self.evaluate_bound(|index| numeric(channel_sample(index)?))
    }

    pub(crate) fn visit_inputs(&self, mut visit: impl FnMut(usize, u16)) {
        self.root.visit_inputs(&mut visit);
    }

    /// Evaluates a row of analog and digital inputs.
    ///
    /// Missing inputs and non-finite results remain gaps.
    pub fn evaluate_bound(&self, sample: impl Fn(usize) -> Option<Value>) -> Option<Value> {
        self.evaluate_history(|index, delay| if delay == 0 { sample(index) } else { None })
    }

    pub(super) fn evaluate_history(
        &self,
        sample: impl Fn(usize, u16) -> Option<Value>,
    ) -> Option<Value> {
        self.root.evaluate(&sample)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Unit {
    Implicit,
    Explicit(Dimension),
}

impl Unit {
    fn dimension(self) -> Dimension {
        match self {
            Self::Implicit => Dimension::SCALAR,
            Self::Explicit(dimension) => dimension,
        }
    }

    fn common(self, other: Self) -> Option<Self> {
        match (self, other) {
            (Self::Implicit, unit) | (unit, Self::Implicit) => Some(unit),
            (left, right) if left == right => Some(left),
            _ => None,
        }
    }

    fn combine(self, other: Self, divide: bool) -> Option<Self> {
        if self == Self::Implicit && other == Self::Implicit {
            return Some(Self::Implicit);
        }
        self.dimension()
            .combine(other.dimension(), divide)
            .map(Self::Explicit)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Numeric(Unit),
    Predicate,
}

#[derive(Clone, Debug)]
enum Node {
    Constant(f64),
    Channel(usize, SignalType, u16),
    Negate(Box<Node>),
    Not(Box<Node>),
    Binary(Binary, Box<Node>, Box<Node>),
    Function(Function, Box<Node>, Option<Box<Node>>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Binary {
    Add,
    Subtract,
    Multiply,
    Divide,
    Equal,
    NotEqual,
    Less,
    AtMost,
    Greater,
    AtLeast,
    And,
    Or,
    Xor,
}

impl Binary {
    // Follows the local difference-equation calculator's distinction between
    // arithmetic, comparison, and logical precedence.
    fn precedence(self) -> u8 {
        match self {
            Self::Or => 1,
            Self::Xor => 2,
            Self::And => 3,
            Self::Equal
            | Self::NotEqual
            | Self::Less
            | Self::AtMost
            | Self::Greater
            | Self::AtLeast => 4,
            Self::Add | Self::Subtract => 5,
            Self::Multiply | Self::Divide => 6,
        }
    }

    fn comparison(self) -> bool {
        matches!(
            self,
            Self::Equal
                | Self::NotEqual
                | Self::Less
                | Self::AtMost
                | Self::Greater
                | Self::AtLeast
        )
    }

    fn kind(self, left: Kind, right: Kind) -> Option<Kind> {
        let (Kind::Numeric(a), Kind::Numeric(b)) = (left, right) else {
            return (matches!(
                self,
                Self::And | Self::Or | Self::Xor | Self::Equal | Self::NotEqual
            ) && left == right)
                .then_some(Kind::Predicate);
        };
        match self {
            Self::Add | Self::Subtract => a.common(b).map(Kind::Numeric),
            Self::Multiply | Self::Divide => a.combine(b, self == Self::Divide).map(Kind::Numeric),
            Self::And | Self::Or | Self::Xor => None,
            _ => a.common(b).map(|_| Kind::Predicate),
        }
    }

    fn compare(self, left: f64, right: f64) -> Option<bool> {
        Some(match self {
            Self::Equal => left == right,
            Self::NotEqual => left != right,
            Self::Less => left < right,
            Self::AtMost => left <= right,
            Self::Greater => left > right,
            Self::AtLeast => left >= right,
            _ => return None,
        })
    }

    fn evaluate(self, left: Option<Value>, right: impl FnOnce() -> Option<Value>) -> Option<Value> {
        match self {
            Self::And | Self::Or => {
                let decisive = self == Self::Or;
                let left = left.and_then(Value::predicate);
                if left == Some(decisive) {
                    return Some(Value::Predicate(decisive));
                }
                let right = right().and_then(Value::predicate);
                if right == Some(decisive) {
                    return Some(Value::Predicate(decisive));
                }
                left.zip(right).map(|_| Value::Predicate(!decisive))
            }
            _ => match left? {
                Value::Predicate(left) => {
                    let right = right()?.predicate()?;
                    match self {
                        Self::Equal => Some(Value::Predicate(left == right)),
                        Self::NotEqual | Self::Xor => Some(Value::Predicate(left != right)),
                        _ => None,
                    }
                }
                Value::Numeric(left) => {
                    let right = right()?.numeric()?;
                    match self {
                        Self::Add => numeric(left + right),
                        Self::Subtract => numeric(left - right),
                        Self::Multiply => numeric(left * right),
                        Self::Divide => numeric(left / right),
                        _ => self.compare(left, right).map(Value::Predicate),
                    }
                }
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Function {
    Abs,
    SquareRoot,
    Min,
    Max,
}

impl Function {
    fn parse(name: &str) -> Option<Self> {
        [
            ("abs", Self::Abs),
            ("sqrt", Self::SquareRoot),
            ("min", Self::Min),
            ("max", Self::Max),
        ]
        .into_iter()
        .find(|(label, _)| label.eq_ignore_ascii_case(name))
        .map(|(_, function)| function)
    }

    fn kind(self, left: Kind, right: Option<Kind>) -> Option<Kind> {
        let Kind::Numeric(left) = left else {
            return None;
        };
        match (self, right) {
            (Self::Abs, None) => Some(Kind::Numeric(left)),
            (Self::SquareRoot, None) => left
                .dimension()
                .square_root()
                .map(|dimension| Kind::Numeric(Unit::Explicit(dimension))),
            (Self::Min | Self::Max, Some(Kind::Numeric(right))) => {
                left.common(right).map(Kind::Numeric)
            }
            _ => None,
        }
    }

    fn evaluate(self, left: f64, right: impl FnOnce() -> Option<f64>) -> Option<Value> {
        numeric(match self {
            Self::Abs => left.abs(),
            Self::SquareRoot => left.sqrt(),
            Self::Min => left.min(right()?),
            Self::Max => left.max(right()?),
        })
    }
}

impl Node {
    fn visit_inputs(&self, visit: &mut impl FnMut(usize, u16)) {
        match self {
            Self::Constant(_) => {}
            Self::Channel(index, _, delay) => visit(*index, *delay),
            Self::Negate(node) | Self::Not(node) => node.visit_inputs(visit),
            Self::Binary(_, left, right) => {
                left.visit_inputs(visit);
                right.visit_inputs(visit);
            }
            Self::Function(_, left, right) => {
                left.visit_inputs(visit);
                if let Some(right) = right {
                    right.visit_inputs(visit);
                }
            }
        }
    }

    fn evaluate(&self, sample: &impl Fn(usize, u16) -> Option<Value>) -> Option<Value> {
        match self {
            Self::Constant(value) => Some(Value::Numeric(*value)),
            Self::Channel(index, signal_type, delay) => {
                match (sample(*index, *delay)?, signal_type) {
                    (Value::Numeric(value), SignalType::Analog(_)) => numeric(value),
                    (value @ Value::Predicate(_), SignalType::Digital) => Some(value),
                    _ => None,
                }
            }
            Self::Negate(node) => numeric(-node.evaluate(sample)?.numeric()?),
            Self::Not(node) => Some(Value::Predicate(!node.evaluate(sample)?.predicate()?)),
            Self::Binary(operator, left, right) => {
                operator.evaluate(left.evaluate(sample), || right.evaluate(sample))
            }
            Self::Function(function, left, right) => function
                .evaluate(left.evaluate(sample)?.numeric()?, || {
                    right.as_ref()?.evaluate(sample)?.numeric()
                }),
        }
    }
}

impl Value {
    fn numeric(self) -> Option<f64> {
        match self {
            Self::Numeric(value) => Some(value),
            Self::Predicate(_) => None,
        }
    }

    fn predicate(self) -> Option<bool> {
        match self {
            Self::Numeric(_) => None,
            Self::Predicate(value) => Some(value),
        }
    }
}

fn numeric(value: f64) -> Option<Value> {
    value.is_finite().then_some(Value::Numeric(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn measure(input: &str, samples: &[f64], active: usize) -> Option<Value> {
        Expression::parse(input, &["CH1", "CH2"], active)
            .unwrap()
            .evaluate(|index| samples.get(index).copied())
    }

    #[test]
    fn binary_channel_math_distinguishes_unknown_from_a_decisive_input() {
        let bindings = ["A", "B"].map(|name| Binding {
            name,
            signal_type: SignalType::Digital,
        });
        let evaluate = |text, samples: [Option<bool>; 2]| {
            Expression::parse_history(text, &bindings, 0)
                .unwrap()
                .evaluate_bound(|index| samples[index].map(Value::Predicate))
        };
        assert_eq!(
            evaluate("A ^ ~B", [Some(true), Some(false)]),
            Some(Value::Predicate(false))
        );
        assert_eq!(
            evaluate("A | B & A", [Some(true), Some(false)]),
            Some(Value::Predicate(true))
        );
        assert_eq!(
            evaluate("A & B", [None, Some(false)]),
            Some(Value::Predicate(false))
        );
        assert_eq!(
            evaluate("A | B", [None, Some(true)]),
            Some(Value::Predicate(true))
        );
        assert_eq!(evaluate("A xor B", [None, Some(true)]), None);
    }

    #[test]
    fn shorthand_targets_the_active_channel_and_uses_scaled_volts() {
        assert_eq!(
            measure("> 1V", &[2.0, 0.4], 1),
            Some(Value::Predicate(false))
        );
        assert_eq!(
            measure("> 400 mV", &[2.0, 0.5], 1),
            Some(Value::Predicate(true))
        );
        assert_eq!(
            measure(">= 400µV && < 1e-3 V", &[0.0007, 0.5], 0),
            Some(Value::Predicate(true))
        );
    }

    #[test]
    fn arithmetic_and_logical_precedence_apply_to_multiple_channels() {
        assert_eq!(
            measure("abs(CH1 - 2 * ch2) <= 500mV or CH1 == 0", &[1.8, 0.8], 0),
            Some(Value::Predicate(true))
        );
        assert_eq!(
            measure("CH1 > 1V && CH2 < 0V || CH2 > 2V", &[1.8, 0.8], 0),
            Some(Value::Predicate(false))
        );
        assert_eq!(
            measure("sqrt(CH1 * CH1)", &[-1.25, 0.0], 0),
            Some(Value::Numeric(1.25))
        );
    }

    #[test]
    fn rejects_unrecognized_channels_and_incompatible_units() {
        assert!(Expression::parse("CH1 / CH2 > 1V", &["CH1", "CH2"], 0).is_err());
        assert!(Expression::parse("CH1 * CH2 + 1V", &["CH1", "CH2"], 0).is_err());
        assert!(Expression::parse("CH1 > 1A", &["CH1"], 0).is_err());
        assert!(Expression::parse("CH3", &["CH1"], 0).is_err());
        assert!(Expression::parse("CH1 and CH1", &["CH1"], 0).is_err());
    }

    #[test]
    fn missing_samples_and_invalid_math_are_gaps() {
        assert_eq!(measure("CH1 / CH2", &[1.0, 0.0], 0), None);
        assert_eq!(measure("CH1 > 1V", &[f64::NAN], 0), None);
        assert_eq!(measure("CH1 + CH2", &[1.0], 0), None);
        assert_eq!(
            measure("CH1 > 1V && CH2 > 1V", &[0.0], 0),
            Some(Value::Predicate(false))
        );
    }

    #[test]
    fn mixed_volts_current_and_digital_levels_keep_their_types() {
        let inputs = [
            Binding {
                name: "supply",
                signal_type: SignalType::Analog(Dimension::VOLTAGE),
            },
            Binding {
                name: "load",
                signal_type: SignalType::Analog(Dimension::CURRENT),
            },
            Binding {
                name: "enable",
                signal_type: SignalType::Digital,
            },
        ];
        let values = [
            Value::Numeric(4.0),
            Value::Numeric(0.03),
            Value::Predicate(true),
        ];
        let wave = Expression::parse_bound("supply * load", &inputs, 0).unwrap();
        assert_eq!(wave.signal_type(), SignalType::Analog(Dimension::POWER));
        assert_eq!(
            wave.evaluate_bound(|index| values.get(index).copied()),
            Some(Value::Numeric(0.12))
        );
        let active =
            Expression::parse_bound("enable && supply * load > 100mW", &inputs, 0).unwrap();
        assert_eq!(active.signal_type(), SignalType::Digital);
        assert_eq!(
            active.evaluate_bound(|index| values.get(index).copied()),
            Some(Value::Predicate(true))
        );
        assert!(Expression::parse_bound("supply + load", &inputs, 0).is_err());
    }

    #[test]
    fn invalid_or_oversized_queries_report_a_location() {
        let error = Expression::parse("CH1 @ 1V", &["CH1"], 0).unwrap_err();
        assert_eq!(error.position, 4);
        let nested = format!("{}CH1{}", "(".repeat(50), ")".repeat(50));
        assert!(Expression::parse(&nested, &["CH1"], 0).is_err());
        assert!(Expression::parse(&"1+".repeat(1000), &["CH1"], 0).is_err());
    }
}
