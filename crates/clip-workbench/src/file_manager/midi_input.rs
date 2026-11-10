//! Shared hotplug reconciliation; callers own routing and bounded event delivery.
use std::collections::{HashMap, HashSet};

pub(super) type Connections = HashMap<String, midir::MidiInputConnection<()>>;

/// Returns whether an input disappeared, so sustained voices can be released.
pub(super) fn refresh(
    connections: &mut Connections,
    name: &str,
    callback: impl Fn(&[u8]) + Clone + Send + 'static,
) -> Result<bool, String> {
    let inventory = midir::MidiInput::new(name).map_err(|e| e.to_string())?;
    // Optional device-name prefix also confines private acceptance to its owned source.
    let prefix = std::env::var("LITTLE_CLIP_MIDI_INPUT_PREFIX").ok();
    let ports: Vec<_> = inventory
        .ports()
        .into_iter()
        .filter(|port| {
            prefix.as_ref().is_none_or(|prefix| {
                inventory
                    .port_name(port)
                    .is_ok_and(|name| name.starts_with(prefix))
            })
        })
        .collect();
    let ids: HashSet<_> = ports.iter().map(|port| port.id()).collect();
    let old = connections.len();
    connections.retain(|id, _| ids.contains(id));
    let removed = old != connections.len();
    let mut errors = Vec::new();
    for port in ports {
        let id = port.id();
        if connections.contains_key(&id) {
            continue;
        }
        let mut input = midir::MidiInput::new(name).map_err(|e| e.to_string())?;
        input.ignore(midir::Ignore::All);
        let callback = callback.clone();
        match input.connect(&port, name, move |_, bytes, _| callback(bytes), ()) {
            Ok(connection) => {
                connections.insert(id, connection);
            }
            Err(error) => errors.push(error.to_string()),
        }
    }
    if errors.is_empty() {
        Ok(removed)
    } else {
        Err(errors.join("; "))
    }
}
