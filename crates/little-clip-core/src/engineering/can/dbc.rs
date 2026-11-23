use super::{Frame, FrameKind, Identifier};
use can_dbc::{
    ByteOrder, Dbc, ExtendedMultiplex, MessageId, MultiplexIndicator, Signal,
    SignalExtendedValueType, ValueType,
};
use std::{
    collections::BTreeMap,
    ops::RangeInclusive,
    sync::atomic::{AtomicBool, Ordering},
};

/// Largest database parsed in one request.
pub const MAX_BYTES: usize = 8 * 1024 * 1024;

// Bound multiplex evaluation even on a cyclic or highly connected definition.
const MAX_MUX_DEPTH: usize = 32;
const MAX_MUX_CHECKS: u16 = 256;

/// CAN message definitions for interpreting a recorded capture.
pub struct Database {
    messages: BTreeMap<Identifier, Message>,
}

impl Database {
    /// Loads a UTF-8 or Windows-1252 DBC database.
    ///
    /// # Errors
    /// Returns an error for an oversized or invalid database, ambiguous frame
    /// identifiers, or cancellation.
    pub fn parse(bytes: &[u8], cancel: &AtomicBool) -> Result<Self, String> {
        if bytes.len() > MAX_BYTES {
            return Err("DBC database exceeds 8 MiB".into());
        }
        cancelled(cancel)?;
        let text = std::str::from_utf8(bytes).map_or_else(
            |_| can_dbc::encodings::WINDOWS_1252.decode(bytes).0,
            std::borrow::Cow::Borrowed,
        );
        let parsed = Dbc::try_from(text.trim_start_matches('\u{feff}')).map_err(|error| {
            let message = error.to_string();
            format!(
                "Invalid DBC: {}",
                &message[..message.floor_char_boundary(message.len().min(800))]
            )
        })?;
        Self::compile(parsed, cancel)
    }

    fn compile(parsed: Dbc, cancel: &AtomicBool) -> Result<Self, String> {
        let mut messages = BTreeMap::new();
        let mut mux = group_fields(parsed.extended_multiplex, |rule| rule.message_id);
        let mut types = group_fields(parsed.signal_extended_value_type_list, |field| {
            field.message_id
        });
        for message in parsed.messages {
            cancelled(cancel)?;
            let Some(id) = identifier(message.id) else {
                continue;
            };
            let extended = mux.remove(&message.id.raw()).unwrap_or_default();
            let types = types.remove(&message.id.raw()).unwrap_or_default();
            let mut result = Message::new(id, message, &extended)?;
            for entry in types {
                if let Some(index) = result.signal_index(&entry.signal_name) {
                    result.signals[index].encoding = Encoding::from(
                        entry.signal_extended_value_type,
                        result.signals[index].encoding,
                    );
                }
            }
            if messages.insert(id, result).is_some() {
                return Err(format!(
                    "DBC contains more than one definition for CAN ID {id}"
                ));
            }
        }
        if messages.is_empty() {
            return Err("DBC contains no supported CAN identifiers".into());
        }
        cancelled(cancel)?;
        Ok(Self { messages })
    }

    /// Looks up the exact standard or extended arbitration identifier.
    pub fn message(&self, id: Identifier) -> Option<&Message> {
        self.messages.get(&id)
    }

    /// Returns the number of exact arbitration identifiers in the database.
    pub fn message_count(&self) -> usize {
        self.messages.len()
    }
}

fn group_fields<T>(fields: Vec<T>, id: impl Fn(&T) -> MessageId) -> BTreeMap<u32, Vec<T>> {
    let mut groups = BTreeMap::<_, Vec<_>>::new();
    for field in fields {
        groups.entry(id(&field).raw()).or_default().push(field);
    }
    groups
}

fn identifier(id: MessageId) -> Option<Identifier> {
    match id {
        MessageId::Standard(id) => Identifier::new(u32::from(id), false),
        MessageId::Extended(id) => Identifier::new(id, true),
    }
}

/// Signal layout attached to one exact arbitration identifier.
pub struct Message {
    pub id: Identifier,
    pub name: String,
    pub signals: Vec<Field>,
}

impl Message {
    fn new(
        id: Identifier,
        message: can_dbc::Message,
        extended: &[ExtendedMultiplex],
    ) -> Result<Self, String> {
        let names: BTreeMap<_, _> = message
            .signals
            .iter()
            .enumerate()
            .map(|(index, signal)| (signal.name.as_str(), index))
            .collect();
        if names.len() != message.signals.len() {
            return Err(format!(
                "DBC message {} has duplicate signal names",
                message.name
            ));
        }
        let root = single_multiplexer(&message.signals);
        let mut explicit = BTreeMap::<&str, Vec<&ExtendedMultiplex>>::new();
        for rule in extended {
            explicit.entry(&rule.signal_name).or_default().push(rule);
        }
        let signals = message
            .signals
            .iter()
            .map(|signal| {
                let gate = gate(
                    signal,
                    root,
                    explicit.get(signal.name.as_str()).map(Vec::as_slice),
                    &names,
                );
                Field::new(signal, message.size, gate)
            })
            .collect();
        Ok(Self {
            id,
            name: message.name,
            signals,
        })
    }

    /// Finds the index of a signal by its exact DBC name.
    pub fn signal_index(&self, name: &str) -> Option<usize> {
        self.signals.iter().position(|field| field.name == name)
    }

    /// Decodes a field only if its source bytes and multiplexers are present.
    pub fn decode(&self, field: usize, frame: &Frame) -> Option<DecodedValue> {
        let mut budget = MAX_MUX_CHECKS;
        if frame.id != Some(self.id)
            || !matches!(frame.kind, FrameKind::Classic | FrameKind::Flexible { .. })
            || !self.active(field, &frame.data, 0, &mut budget)
        {
            return None;
        }
        self.signals.get(field)?.decode(&frame.data)
    }

    fn active(&self, field: usize, bytes: &[u8], depth: usize, budget: &mut u16) -> bool {
        if depth >= MAX_MUX_DEPTH || *budget == 0 {
            return false;
        }
        *budget -= 1;
        let Some(field) = self.signals.get(field) else {
            return false;
        };
        match &field.gate {
            Gate::Always => true,
            Gate::Unresolved => false,
            Gate::Conditional(selectors) => selectors.iter().all(|(index, values)| {
                self.active(*index, bytes, depth + 1, budget)
                    && self.signals[*index]
                        .decode(bytes)
                        .and_then(|decoded| decoded.raw.selector_value())
                        .is_some_and(|value| values.iter().any(|range| range.contains(&value)))
            }),
        }
    }
}

fn single_multiplexer(signals: &[Signal]) -> Option<usize> {
    let mut roots = signals
        .iter()
        .enumerate()
        .filter(|(_, signal)| signal.multiplexer_indicator == MultiplexIndicator::Multiplexor);
    let (index, _) = roots.next()?;
    roots.next().is_none().then_some(index)
}

type Selector = (usize, Vec<RangeInclusive<u64>>);

enum Gate {
    Always,
    Conditional(Vec<Selector>),
    Unresolved,
}

fn gate(
    signal: &Signal,
    root: Option<usize>,
    explicit: Option<&[&ExtendedMultiplex]>,
    names: &BTreeMap<&str, usize>,
) -> Gate {
    if let Some(explicit) = explicit {
        return explicit_gate(explicit, names);
    }
    let selector = match signal.multiplexer_indicator {
        MultiplexIndicator::Plain | MultiplexIndicator::Multiplexor => return Gate::Always,
        MultiplexIndicator::MultiplexedSignal(value)
        | MultiplexIndicator::MultiplexorAndMultiplexedSignal(value) => value,
    };
    root.map_or(Gate::Unresolved, |index| {
        Gate::Conditional(vec![(index, vec![selector..=selector])])
    })
}

fn explicit_gate(explicit: &[&ExtendedMultiplex], names: &BTreeMap<&str, usize>) -> Gate {
    let mut conditions = BTreeMap::<_, Vec<_>>::new();
    for rule in explicit {
        let Some(&index) = names.get(rule.multiplexor_signal_name.as_str()) else {
            return Gate::Unresolved;
        };
        let limits = conditions.entry(index).or_default();
        limits.extend(rule.mappings.iter().filter_map(|mapping| {
            (mapping.min_value <= mapping.max_value)
                .then_some(mapping.min_value..=mapping.max_value)
        }));
    }
    Gate::Conditional(conditions.into_iter().collect())
}

/// A named physical value and its least-significant-first source bit layout.
pub struct Field {
    pub name: String,
    pub unit: String,
    pub factor: f64,
    pub offset: f64,
    bits: Vec<u16>,
    encoding: Encoding,
    gate: Gate,
}

impl Field {
    fn new(signal: &Signal, message_size: u64, gate: Gate) -> Self {
        Self {
            name: signal.name.clone(),
            unit: signal.unit.clone(),
            factor: signal.factor,
            offset: signal.offset,
            bits: bit_layout(signal, message_size),
            encoding: match signal.value_type {
                ValueType::Signed => Encoding::Signed,
                ValueType::Unsigned => Encoding::Unsigned,
            },
            gate,
        }
    }

    /// Lists captured bit locations in ascending significance order.
    pub fn bits(&self) -> &[u16] {
        &self.bits
    }

    fn decode(&self, bytes: &[u8]) -> Option<DecodedValue> {
        let width = self.bits.len();
        if width == 0 || !self.factor.is_finite() || !self.offset.is_finite() {
            return None;
        }
        let mut word = 0u64;
        for (index, &location) in self.bits.iter().enumerate() {
            let byte = bytes.get(usize::from(location / 8))?;
            let bit = (byte >> (location % 8)) & 1;
            word |= u64::from(bit) << index;
        }
        let raw = self.encoding.decode(word, width)?;
        let value = raw.as_f64().mul_add(self.factor, self.offset);
        value.is_finite().then_some(DecodedValue { raw, value })
    }
}

fn bit_layout(signal: &Signal, message_size: u64) -> Vec<u16> {
    let payload_bits = message_size.min(64) * 8;
    if signal.size == 0 || signal.size > 64 || signal.start_bit >= payload_bits {
        return Vec::new();
    }
    let mut bits = Vec::with_capacity(signal.size as usize);
    let mut bit = signal.start_bit;
    for _ in 0..signal.size {
        if bit >= payload_bits {
            return Vec::new();
        }
        bits.push(bit as u16);
        bit = match signal.byte_order {
            ByteOrder::LittleEndian => bit + 1,
            ByteOrder::BigEndian if bit % 8 == 0 => bit + 15,
            ByteOrder::BigEndian => bit - 1,
        };
    }
    if signal.byte_order == ByteOrder::BigEndian {
        bits.reverse();
    }
    bits
}

#[derive(Clone, Copy)]
enum Encoding {
    Signed,
    Unsigned,
    Float32,
    Float64,
}

impl Encoding {
    fn from(extended: SignalExtendedValueType, integer: Self) -> Self {
        match extended {
            SignalExtendedValueType::SignedOrUnsignedInteger => integer,
            SignalExtendedValueType::IEEEfloat32Bit => Self::Float32,
            SignalExtendedValueType::IEEEdouble64bit => Self::Float64,
        }
    }

    fn decode(self, word: u64, width: usize) -> Option<RawValue> {
        match self {
            Self::Signed => {
                let shift = 64 - width;
                Some(RawValue::Signed(((word << shift) as i64) >> shift))
            }
            Self::Unsigned => Some(RawValue::Unsigned(word)),
            Self::Float32 if width == 32 => {
                Some(RawValue::Float(f64::from(f32::from_bits(word as u32))))
            }
            Self::Float64 if width == 64 => Some(RawValue::Float(f64::from_bits(word))),
            _ => None,
        }
    }
}

/// A captured value before physical scale and offset are applied.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RawValue {
    Signed(i64),
    Unsigned(u64),
    Float(f64),
}

impl RawValue {
    fn as_f64(self) -> f64 {
        match self {
            Self::Signed(value) => value as f64,
            Self::Unsigned(value) => value as f64,
            Self::Float(value) => value,
        }
    }

    fn selector_value(self) -> Option<u64> {
        match self {
            Self::Signed(value) => u64::try_from(value).ok(),
            Self::Unsigned(value) => Some(value),
            Self::Float(_) => None,
        }
    }
}

/// Raw integer or float and the corresponding physical value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DecodedValue {
    pub raw: RawValue,
    pub value: f64,
}

fn cancelled(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        return Err("DBC parsing cancelled".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engineering::can::Channel;

    const DATABASE: &str = r#"VERSION ""

NS_ :

BS_:

BU_: Node

BO_ 291 Console: 8 Node
 SG_ Mode M : 0|4@1+ (1,0) [0|15] "" Node
 SG_ Voltage m1 : 8|16@1- (0.01,-2) [-100|100] "V" Node
 SG_ Level m2 : 31|12@0+ (0.5,1) [0|2048] "A" Node
 SG_ Measurement : 32|32@1+ (2,1) [0|100] "W" Node

BO_ 2147483939 Extended: 8 Node
 SG_ Code : 0|4@1+ (1,0) [0|15] "" Node

SIG_VALTYPE_ 291 Measurement : 1;
SG_MUL_VAL_ 291 Level Mode 2-3;
"#;

    #[test]
    fn decodes_signed_motorola_float_and_mux_from_recorded_bytes() {
        let database = Database::parse(DATABASE.as_bytes(), &AtomicBool::new(false)).unwrap();
        assert_eq!(database.message_count(), 2);
        let id = Identifier::new(0x123, false).unwrap();
        let message = database.message(id).unwrap();
        assert_eq!(message.name, "Console");
        let mut frame = Frame::new(0, Channel::Unknown, FrameKind::Classic);
        frame.id = Some(id);
        frame.data = vec![1, 0x9c, 0xff, 0, 0, 0, 0, 0];
        frame.data[4..8].copy_from_slice(&2.5f32.to_le_bytes());

        let voltage = message
            .decode(message.signal_index("Voltage").unwrap(), &frame)
            .unwrap();
        assert_eq!(voltage.raw, RawValue::Signed(-100));
        assert!((voltage.value + 3.0).abs() < 1e-12);
        let power = message
            .decode(message.signal_index("Measurement").unwrap(), &frame)
            .unwrap();
        assert_eq!(
            power,
            DecodedValue {
                raw: RawValue::Float(2.5),
                value: 6.0
            }
        );
        let level = message.signal_index("Level").unwrap();
        assert!(message.decode(level, &frame).is_none());

        frame.data[0] = 3;
        frame.data[3] = 0xab;
        frame.data[4] = 0xc0;
        assert_eq!(
            message.decode(level, &frame),
            Some(DecodedValue {
                raw: RawValue::Unsigned(0xabc),
                value: 1375.0
            })
        );
        assert_eq!(
            message.signals[level].bits(),
            &[36, 37, 38, 39, 24, 25, 26, 27, 28, 29, 30, 31]
        );
        frame.data.truncate(4);
        assert!(message.decode(level, &frame).is_none());

        let extended = Identifier::new(0x123, true).unwrap();
        assert_eq!(database.message(extended).unwrap().name, "Extended");
        frame.id = Some(extended);
        assert!(message.decode(0, &frame).is_none());
    }

    #[test]
    fn cyclic_multiplexer_and_missing_parent_leave_the_signal_unavailable() {
        let text = format!("{DATABASE}\nSG_MUL_VAL_ 291 Mode Mode 1-1;\n");
        let database = Database::parse(text.as_bytes(), &AtomicBool::new(false)).unwrap();
        let id = Identifier::new(0x123, false).unwrap();
        let message = database.message(id).unwrap();
        let mut frame = Frame::new(0, Channel::Unknown, FrameKind::Classic);
        frame.id = Some(id);
        frame.data = vec![1, 0, 0];
        assert!(
            message
                .decode(message.signal_index("Voltage").unwrap(), &frame)
                .is_none()
        );
    }
}
