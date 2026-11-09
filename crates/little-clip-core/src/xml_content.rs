//! Shared preflight budgets before materializing untrusted XML trees.
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
