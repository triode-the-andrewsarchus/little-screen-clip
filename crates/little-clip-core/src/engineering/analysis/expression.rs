//! Sample-by-sample arithmetic and predicate evaluation for voltage captures.

mod parser;

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
        parser::parse(input, channel_names, active_channel)
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
            Kind::Numeric(unit) => Some(unit.exponent()),
            Kind::Predicate => None,
        }
    }

    /// Evaluates the compiled expression using the channel values at one instant.
    ///
    /// Supply a closure that returns a sample in volts for its zero-based
    /// channel index. Missing samples and non-finite results return `None`.
    pub fn evaluate(&self, channel_sample: impl Fn(usize) -> Option<f64>) -> Option<Value> {
        self.root.evaluate(&channel_sample)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Unit {
    Implicit,
    Volts(i8),
}

impl Unit {
    fn exponent(self) -> i8 {
        match self {
            Self::Implicit => 0,
            Self::Volts(power) => power,
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
        let power = if divide {
            self.exponent().checked_sub(other.exponent())?
        } else {
            self.exponent().checked_add(other.exponent())?
        };
        Some(Self::Volts(power))
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
    Channel(usize),
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
}

impl Binary {
    // Follows the local difference-equation calculator's distinction between
    // arithmetic, comparison, and logical precedence.
    fn precedence(self) -> u8 {
        match self {
            Self::Or => 1,
            Self::And => 2,
            Self::Equal
            | Self::NotEqual
            | Self::Less
            | Self::AtMost
            | Self::Greater
            | Self::AtLeast => 3,
            Self::Add | Self::Subtract => 4,
            Self::Multiply | Self::Divide => 5,
        }
    }

    fn kind(self, left: Kind, right: Kind) -> Option<Kind> {
        let (Kind::Numeric(a), Kind::Numeric(b)) = (left, right) else {
            return (matches!(self, Self::And | Self::Or) && left == right)
                .then_some(Kind::Predicate);
        };
        match self {
            Self::Add | Self::Subtract => a.common(b).map(Kind::Numeric),
            Self::Multiply | Self::Divide => a.combine(b, self == Self::Divide).map(Kind::Numeric),
            Self::And | Self::Or => None,
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

    fn evaluate(self, left: Value, right: impl FnOnce() -> Option<Value>) -> Option<Value> {
        match left {
            Value::Predicate(left) => match self {
                Self::And if !left => Some(Value::Predicate(false)),
                Self::Or if left => Some(Value::Predicate(true)),
                Self::And | Self::Or => Some(Value::Predicate(right()?.predicate()?)),
                _ => None,
            },
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
            (Self::SquareRoot, None) if left.exponent() % 2 == 0 => {
                Some(Kind::Numeric(Unit::Volts(left.exponent() / 2)))
            }
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
    fn evaluate(&self, sample: &impl Fn(usize) -> Option<f64>) -> Option<Value> {
        match self {
            Self::Constant(value) => Some(Value::Numeric(*value)),
            Self::Channel(index) => numeric(sample(*index)?),
            Self::Negate(node) => numeric(-node.evaluate(sample)?.numeric()?),
            Self::Not(node) => Some(Value::Predicate(!node.evaluate(sample)?.predicate()?)),
            Self::Binary(operator, left, right) => {
                operator.evaluate(left.evaluate(sample)?, || right.evaluate(sample))
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
    fn invalid_or_oversized_queries_report_a_location() {
        let error = Expression::parse("CH1 @ 1V", &["CH1"], 0).unwrap_err();
        assert_eq!(error.position, 4);
        let nested = format!("{}CH1{}", "(".repeat(50), ")".repeat(50));
        assert!(Expression::parse(&nested, &["CH1"], 0).is_err());
        assert!(Expression::parse(&"1+".repeat(1000), &["CH1"], 0).is_err());
    }
}
