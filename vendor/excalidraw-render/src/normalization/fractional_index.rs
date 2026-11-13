//! Fractional-index validation and repair: the `index` each element carries
//! so the reference renderer orders elements without renumbering them, as
//! `fractional-indexing` 3.2.0 defines it. Every key is an integer part whose
//! length its first letter encodes (`a`..`z` grow, `Z`..`A` shrink) followed by
//! an optional fraction over the base-62 digit alphabet, and the smallest
//! integer `A00000000000000000000000000` is reserved. Repair keeps every key
//! that already sits strictly between its neighbours and generates keys for the
//! runs that do not, reproducing the reference's `syncInvalidIndices` order
//! exactly. Nothing here knows about scenes or elements: a caller hands in the
//! source keys in element order and receives one valid key per element.

use std::collections::TryReserveError;

use super::try_owned;

const DIGITS: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

pub(super) fn are_strictly_ascending(indices: &[Option<&str>]) -> bool {
    indices.iter().all(Option::is_some)
        && indices
            .windows(2)
            .all(|pair| matches!(pair, [Some(left), Some(right)] if left < right))
}

pub(super) fn is_valid_key(index: &str) -> bool {
    let bytes = index.as_bytes();
    let Some(integer_length) = bytes.first().and_then(|head| match head {
        b'a'..=b'z' => Some(usize::from(*head - b'a') + 2),
        b'A'..=b'Z' => Some(usize::from(b'Z' - *head) + 2),
        _ => None,
    }) else {
        return false;
    };
    if bytes.len() < integer_length
        || bytes.iter().any(|byte| !DIGITS.contains(byte))
        || bytes[integer_length..].last() == Some(&b'0')
    {
        return false;
    }
    !(bytes[0] == b'A' && bytes.len() == 27 && bytes[1..].iter().all(|byte| *byte == b'0'))
}

pub(super) fn repair(source: &[Option<&str>]) -> Result<Vec<String>, TryReserveError> {
    struct InvalidGroup {
        lower: Option<usize>,
        members: Vec<usize>,
        upper: Option<usize>,
    }

    fn valid_between(current: Option<&str>, lower: Option<&str>, upper: Option<&str>) -> bool {
        let Some(current) = current else {
            return false;
        };
        lower.is_none_or(|lower| lower < current) && upper.is_none_or(|upper| current < upper)
    }

    fn lower_bound(
        source: &[Option<&str>],
        previous: Option<usize>,
        index: usize,
    ) -> Option<usize> {
        let previous_value = previous.and_then(|index| source[index]);
        let candidate = index.checked_sub(1).and_then(|index| source[index]);
        if candidate
            .is_some_and(|candidate| previous_value.is_none_or(|previous| candidate > previous))
        {
            index.checked_sub(1)
        } else {
            previous
        }
    }

    fn upper_bound(source: &[Option<&str>], previous: usize, index: usize) -> usize {
        let previous_value = source.get(previous).copied().flatten();
        if previous_value.is_some() && index < previous {
            return previous;
        }
        let mut candidate_index = previous.saturating_add(1);
        while candidate_index < source.len() {
            let candidate = source[candidate_index];
            if candidate
                .is_some_and(|candidate| previous_value.is_none_or(|previous| candidate > previous))
            {
                return candidate_index;
            }
            candidate_index += 1;
        }
        source.len()
    }

    let mut groups = Vec::new();
    let mut lower = None;
    let mut upper = 0;
    let mut index = 0;
    while index < source.len() {
        lower = lower_bound(source, lower, index);
        upper = upper_bound(source, upper, index);
        let lower_value = lower.and_then(|bound| source[bound]);
        let upper_value = source.get(upper).copied().flatten();
        if valid_between(source[index], lower_value, upper_value) {
            index += 1;
            continue;
        }

        let mut members = Vec::new();
        members.try_reserve(source.len().saturating_sub(index))?;
        members.push(index);
        index += 1;
        while index < source.len() {
            let next_lower = lower_bound(source, lower, index);
            let next_upper = upper_bound(source, upper, index);
            if valid_between(
                source[index],
                next_lower.and_then(|bound| source[bound]),
                source.get(next_upper).copied().flatten(),
            ) {
                break;
            }
            lower = next_lower;
            upper = next_upper;
            members.push(index);
            index += 1;
        }
        groups.try_reserve(1)?;
        groups.push(InvalidGroup {
            lower,
            members,
            upper: (upper < source.len()).then_some(upper),
        });
    }

    let mut repaired = Vec::new();
    repaired.try_reserve_exact(source.len())?;
    for index in source {
        repaired.push(match index {
            Some(index) => try_owned(index)?,
            None => String::new(),
        });
    }
    for group in groups {
        let lower = group.lower.map(|index| repaired[index].as_str());
        let upper = group.upper.map(|index| repaired[index].as_str());
        let generated = generate_n_keys(lower, upper, group.members.len())?;
        for (index, generated) in group.members.into_iter().zip(generated) {
            repaired[index] = generated;
        }
    }
    Ok(repaired)
}

fn generate_n_keys(
    lower: Option<&str>,
    upper: Option<&str>,
    count: usize,
) -> Result<Vec<String>, TryReserveError> {
    fn append_keys(
        output: &mut Vec<String>,
        lower: Option<&str>,
        upper: Option<&str>,
        count: usize,
    ) -> Result<(), TryReserveError> {
        if count == 0 {
            return Ok(());
        }
        if count == 1 {
            output.try_reserve(1)?;
            output.push(generate_key(lower, upper)?);
            return Ok(());
        }
        if upper.is_none() {
            let mut current = generate_key(lower, None)?;
            output.try_reserve(count)?;
            output.push(try_owned(&current)?);
            for _ in 1..count {
                current = generate_key(Some(&current), None)?;
                output.push(try_owned(&current)?);
            }
            return Ok(());
        }
        if lower.is_none() {
            let mut reverse = Vec::new();
            reverse.try_reserve_exact(count)?;
            let mut current = generate_key(None, upper)?;
            reverse.push(try_owned(&current)?);
            for _ in 1..count {
                current = generate_key(None, Some(&current))?;
                reverse.push(try_owned(&current)?);
            }
            output.try_reserve(count)?;
            output.extend(reverse.into_iter().rev());
            return Ok(());
        }

        let middle = generate_key(lower, upper)?;
        let left_count = count / 2;
        append_keys(output, lower, Some(&middle), left_count)?;
        output.try_reserve(1)?;
        output.push(try_owned(&middle)?);
        append_keys(output, Some(&middle), upper, count - left_count - 1)
    }

    let mut keys = Vec::new();
    keys.try_reserve_exact(count)?;
    append_keys(&mut keys, lower, upper, count)?;
    Ok(keys)
}

fn generate_key(lower: Option<&str>, upper: Option<&str>) -> Result<String, TryReserveError> {
    match (lower, upper) {
        (None, None) => try_owned("a0"),
        (None, Some(upper)) => {
            let integer = integer_part(upper);
            let fraction = &upper[integer.len()..];
            if integer == "A00000000000000000000000000" {
                let mut key = try_owned(integer)?;
                let midpoint = midpoint("", Some(fraction))?;
                try_push_str(&mut key, &midpoint)?;
                return Ok(key);
            }
            if integer < upper {
                return try_owned(integer);
            }
            decrement_integer(integer)
        }
        (Some(lower), None) => {
            let integer = integer_part(lower);
            let fraction = &lower[integer.len()..];
            if let Some(incremented) = increment_integer(integer)? {
                Ok(incremented)
            } else {
                let mut key = try_owned(integer)?;
                let midpoint = midpoint(fraction, None)?;
                try_push_str(&mut key, &midpoint)?;
                Ok(key)
            }
        }
        (Some(lower), Some(upper)) => {
            let lower_integer = integer_part(lower);
            let upper_integer = integer_part(upper);
            if lower_integer == upper_integer {
                let mut key = try_owned(lower_integer)?;
                let midpoint = midpoint(
                    &lower[lower_integer.len()..],
                    Some(&upper[upper_integer.len()..]),
                )?;
                try_push_str(&mut key, &midpoint)?;
                return Ok(key);
            }
            if let Some(incremented) = increment_integer(lower_integer)? {
                if incremented.as_str() < upper {
                    return Ok(incremented);
                }
            }
            let mut key = try_owned(lower_integer)?;
            let midpoint = midpoint(&lower[lower_integer.len()..], None)?;
            try_push_str(&mut key, &midpoint)?;
            Ok(key)
        }
    }
}

fn integer_part(key: &str) -> &str {
    let head = key.as_bytes()[0];
    let length = if head.is_ascii_lowercase() {
        usize::from(head - b'a') + 2
    } else {
        usize::from(b'Z' - head) + 2
    };
    &key[..length]
}

fn increment_integer(integer: &str) -> Result<Option<String>, TryReserveError> {
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(integer.len().saturating_add(1))?;
    bytes.extend_from_slice(integer.as_bytes());
    let mut carry = true;
    for digit in bytes[1..].iter_mut().rev() {
        if !carry {
            break;
        }
        let position = DIGITS
            .iter()
            .position(|candidate| candidate == digit)
            .unwrap_or(0)
            + 1;
        if position == DIGITS.len() {
            *digit = DIGITS[0];
        } else {
            *digit = DIGITS[position];
            carry = false;
        }
    }
    if carry {
        match bytes[0] {
            b'Z' => return Ok(Some(try_owned("a0")?)),
            b'z' => return Ok(None),
            head => {
                bytes[0] = head + 1;
                if bytes[0] > b'a' {
                    bytes.try_reserve(1)?;
                    bytes.push(b'0');
                } else {
                    bytes.pop();
                }
            }
        }
    }
    let value = std::str::from_utf8(&bytes).expect("fractional keys are ASCII");
    Ok(Some(try_owned(value)?))
}

fn decrement_integer(integer: &str) -> Result<String, TryReserveError> {
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(integer.len().saturating_add(1))?;
    bytes.extend_from_slice(integer.as_bytes());
    let mut borrow = true;
    for digit in bytes[1..].iter_mut().rev() {
        if !borrow {
            break;
        }
        let position = DIGITS
            .iter()
            .position(|candidate| candidate == digit)
            .unwrap_or(0);
        if position == 0 {
            *digit = *DIGITS.last().expect("digit alphabet is nonempty");
        } else {
            *digit = DIGITS[position - 1];
            borrow = false;
        }
    }
    if borrow {
        match bytes[0] {
            b'a' => return try_owned("Zz"),
            head => {
                bytes[0] = head - 1;
                if bytes[0] < b'Z' {
                    bytes.try_reserve(1)?;
                    bytes.push(b'z');
                } else {
                    bytes.pop();
                }
            }
        }
    }
    try_owned(std::str::from_utf8(&bytes).expect("fractional keys are ASCII"))
}

fn midpoint(lower: &str, upper: Option<&str>) -> Result<String, TryReserveError> {
    let mut output = String::new();
    output.try_reserve(lower.len().max(upper.map_or(0, str::len)).saturating_add(1))?;
    let mut lower = lower.as_bytes();
    let mut upper = upper.map(str::as_bytes);

    if let Some(upper_bytes) = upper {
        let mut common = 0;
        while common < upper_bytes.len()
            && lower.get(common).copied().unwrap_or(b'0') == upper_bytes[common]
        {
            common += 1;
        }
        if common > 0 {
            output.push_str(
                std::str::from_utf8(&upper_bytes[..common]).expect("fractional keys are ASCII"),
            );
            lower = lower.get(common..).unwrap_or_default();
            upper = Some(&upper_bytes[common..]);
        }
    }

    loop {
        let lower_digit = lower
            .first()
            .and_then(|byte| DIGITS.iter().position(|digit| digit == byte))
            .unwrap_or(0);
        let upper_digit = upper
            .and_then(|bytes| bytes.first())
            .and_then(|byte| DIGITS.iter().position(|digit| digit == byte))
            .unwrap_or(DIGITS.len());
        if upper_digit.saturating_sub(lower_digit) > 1 {
            output.push(char::from(DIGITS[(lower_digit + upper_digit).div_ceil(2)]));
            return Ok(output);
        }
        if let Some(upper) = upper.filter(|upper| upper.len() > 1) {
            output.push(char::from(upper[0]));
            return Ok(output);
        }
        output.push(char::from(DIGITS[lower_digit]));
        lower = lower.get(1..).unwrap_or_default();
        upper = None;
    }
}

fn try_push_str(output: &mut String, value: &str) -> Result<(), TryReserveError> {
    output.try_reserve(value.len())?;
    output.push_str(value);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{are_strictly_ascending, generate_n_keys, is_valid_key, repair};

    #[test]
    fn validity_follows_the_fractional_indexing_key_grammar() {
        for (key, valid) in [
            ("a0", true),
            ("a1V", true),
            ("Zz", true),
            ("b00", true),
            ("A00000000000000000000000001", true),
            ("", false),
            ("a", false),
            ("b0", false),
            ("a00", false),
            ("a1!", false),
            ("0a", false),
            ("A00000000000000000000000000", false),
        ] {
            assert_eq!(is_valid_key(key), valid, "{key:?}");
        }
    }

    #[test]
    fn strict_ascent_requires_every_key_present_and_increasing() {
        assert!(are_strictly_ascending(&[]));
        assert!(are_strictly_ascending(&[Some("a0")]));
        assert!(are_strictly_ascending(&[
            Some("a0"),
            Some("a1"),
            Some("a1V")
        ]));
        assert!(!are_strictly_ascending(&[Some("a1"), Some("a0")]));
        assert!(!are_strictly_ascending(&[Some("a0"), Some("a0")]));
        assert!(!are_strictly_ascending(&[Some("a0"), None]));
        assert!(!are_strictly_ascending(&[None]));
    }

    #[test]
    fn repair_matches_the_pinned_source_order_vectors() {
        for (source, expected) in [
            (vec![None], vec!["a0"]),
            (vec![None, None, None], vec!["a0", "a1", "a2"]),
            (vec![Some("a0"), None, Some("a2")], vec!["a0", "a1", "a2"]),
            (
                vec![Some("a0"), Some("a0"), Some("a2")],
                vec!["a0", "a1", "a2"],
            ),
            (
                vec![Some("a1"), Some("a1"), Some("a2")],
                vec!["a1", "a1V", "a2"],
            ),
            (vec![Some("a2"), Some("a1")], vec!["a2", "a3"]),
            (
                vec![Some("a0"), Some("a2"), Some("a1"), Some("a4")],
                vec!["a0", "a2", "a3", "a4"],
            ),
        ] {
            let repaired = repair(&source).expect("fractional-index repair allocates");
            assert_eq!(repaired, expected);
            let repaired_source = repaired
                .iter()
                .map(|index| Some(index.as_str()))
                .collect::<Vec<_>>();
            assert_eq!(
                repair(&repaired_source).expect("idempotent fractional-index repair allocates"),
                repaired
            );
        }
    }

    #[test]
    fn repair_leaves_an_already_ascending_sequence_untouched() {
        let source = [Some("a0"), Some("a0V"), Some("a1"), Some("b00")];
        assert_eq!(
            repair(&source).expect("repair allocates"),
            ["a0", "a0V", "a1", "b00"]
        );
    }

    #[test]
    fn key_generation_matches_version_3_2_0_vectors() {
        for (lower, upper, count, expected) in [
            (None, None, 5, vec!["a0", "a1", "a2", "a3", "a4"]),
            (Some("a0"), None, 3, vec!["a1", "a2", "a3"]),
            (None, Some("a2"), 3, vec!["Zz", "a0", "a1"]),
            (Some("a0"), Some("a1"), 3, vec!["a0G", "a0V", "a0l"]),
        ] {
            assert_eq!(
                generate_n_keys(lower, upper, count).expect("fractional key generation allocates"),
                expected
            );
        }
    }

    #[test]
    fn generated_keys_are_valid_and_sit_strictly_between_their_bounds() {
        for (lower, upper) in [
            (None, None),
            (Some("a0"), None),
            (None, Some("a0")),
            (Some("a0"), Some("a1")),
            (Some("a0"), Some("a0V")),
            (Some("Zz"), Some("a0")),
            (Some("zzzzzzzzzzzzzzzzzzzzzzzzzzz"), None),
        ] {
            let keys = generate_n_keys(lower, upper, 4).expect("generation allocates");
            assert_eq!(keys.len(), 4, "{lower:?}..{upper:?}");
            let mut previous = lower.map(str::to_owned);
            for key in &keys {
                assert!(is_valid_key(key), "{lower:?}..{upper:?} produced {key:?}");
                if let Some(previous) = &previous {
                    assert!(previous < key, "{previous:?} < {key:?}");
                }
                if let Some(upper) = upper {
                    assert!(key.as_str() < upper, "{key:?} < {upper:?}");
                }
                previous = Some(key.clone());
            }
        }
    }
}
