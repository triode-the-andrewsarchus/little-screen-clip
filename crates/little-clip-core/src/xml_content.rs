//! Shared preflight budgets before materializing untrusted XML trees.
/// Borrows the first root tag without interpreting entities or following references.
pub(crate) fn root(bytes: &[u8]) -> Option<quick_xml::events::BytesStart<'_>> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_reader(bytes);
    for _ in 0..1024 {
        match reader.read_event().ok()? {
            Event::Start(element) | Event::Empty(element) => return Some(element),
            Event::Eof => return None,
            _ => {}
        }
    }
    None
}

pub(crate) fn validate(bytes: &[u8], max_events: usize, max_depth: usize) -> Result<(), String> {
    let mut reader = quick_xml::Reader::from_reader(bytes);
    let mut depth = 0usize;
    let mut events = 0usize;
    loop {
        events += 1;
        if events > max_events {
            return Err("XML exceeds the event budget".into());
        }
        use quick_xml::events::Event;
        match reader.read_event().map_err(|e| e.to_string())? {
            Event::Start(_) => {
                depth += 1;
                if depth > max_depth {
                    return Err("XML is too deeply nested".into());
                }
            }
            Event::End(_) => depth = depth.checked_sub(1).ok_or("Unbalanced XML")?,
            Event::DocType(dtd) if dtd.contains(&b'[') => {
                return Err("XML custom entities are not supported".into());
            }
            Event::Eof => break,
            _ => (),
        }
    }
    if depth != 0 {
        return Err("Truncated XML".into());
    }
    Ok(())
}
