//! Bounded raw-name xattrs, including exposed POSIX ACL attributes.
use super::*;

const ATTRIBUTE_BYTES: usize = 64 * 1024;
const ENTRY_BYTES: usize = 1024 * 1024;
const ENTRY_ATTRIBUTES: usize = 1024;

pub(super) fn capture(object: &Object, facts: &mut Facts, cancel: &AtomicBool) -> Result<()> {
    let mut names = vec![0; ATTRIBUTE_BYTES];
    let count = match object.list(&mut names) {
        Ok(count) => count,
        Err(error) => {
            facts.attributes_complete = false;
            facts.attribute_names_complete = false;
            facts.attribute_list_errno = Some(error.raw_os_error());
            return Ok(());
        }
    };
    let mut total = 0;
    for name in names[..count]
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
    {
        cancelled(cancel)?;
        if facts.attributes.len() >= ENTRY_ATTRIBUTES || total + ATTRIBUTE_BYTES > ENTRY_BYTES {
            facts.attributes_complete = false;
            facts.attribute_names_complete = false;
            facts.note(
                Field::ExtendedAttributeList,
                None,
                None,
                "Per-entry attribute count/byte budget reached (1024 attributes, 1 MiB)",
            );
            break;
        }
        let mut value = vec![0; ATTRIBUTE_BYTES];
        let value = match object.get(name, &mut value) {
            Ok(count) => {
                value.truncate(count);
                total += name.len() + value.len();
                Some(value)
            }
            Err(error) => {
                facts.attributes_complete = false;
                facts.note(
                    Field::ExtendedAttribute,
                    Some(name),
                    Some(&error.into()),
                    "Source attribute could not be captured (64 KiB value limit)",
                );
                None
            }
        };
        facts.attributes.push(Attribute {
            name: name.to_vec(),
            value,
        });
    }
    facts
        .attributes
        .sort_unstable_by(|a, b| a.name.cmp(&b.name));
    Ok(())
}

pub(super) fn remove_obsolete(
    output: &Object,
    facts: &mut Facts,
    cancel: &AtomicBool,
) -> Result<()> {
    if !facts.attribute_names_complete {
        return Ok(());
    }
    let mut names = vec![0; ATTRIBUTE_BYTES];
    let count = match output.list(&mut names) {
        Ok(count) => count,
        Err(error) => {
            facts.note(
                Field::ExtendedAttributeList,
                None,
                Some(&error.into()),
                "Destination attributes could not be checked for stale inherited values",
            );
            return Ok(());
        }
    };
    for name in names[..count]
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
    {
        cancelled(cancel)?;
        if !facts
            .attributes
            .iter()
            .any(|attribute| attribute.name == name)
        {
            let result = output.remove(name).map_err(Into::into);
            let absent = output.get(name, &mut []).err() == Some(rustix::io::Errno::NODATA);
            facts.verify(Field::ExtendedAttribute, Some(name), result, absent);
        }
    }
    Ok(())
}

pub(super) fn restore(output: &Object, facts: &mut Facts, cancel: &AtomicBool) -> Result<()> {
    let mut checked = vec![0; ATTRIBUTE_BYTES];
    // Keep observations in place; only the omission list changes during restoration.
    let mut failures = Vec::new();
    for attribute in &facts.attributes {
        cancelled(cancel)?;
        let Some(value) = &attribute.value else {
            continue;
        };
        let result = output.set(&attribute.name, value);
        let matches = output
            .get(&attribute.name, &mut checked)
            .is_ok_and(|count| checked[..count] == *value);
        if result.is_err() || !matches {
            failures.push((attribute.name.clone(), result.map_err(Into::into), matches));
        }
    }
    for (name, result, matches) in failures {
        facts.verify(Field::ExtendedAttribute, Some(&name), result, matches);
    }
    Ok(())
}
