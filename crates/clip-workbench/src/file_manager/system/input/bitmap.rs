use std::io;
pub(super) fn events(raw: &str) -> io::Result<String> {
    let raw = raw.trim();
    if raw.is_empty() || !raw.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid input event-type bitmap",
        ));
    }
    // EV_MAX is 0x1f: one 32-bit word on both 32- and 64-bit kernels.
    let mask = u32::from_str_radix(raw, 16).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "Input event bitmap exceeds EV_MAX",
        )
    })?;
    let names = (0..32)
        .filter(|bit| mask & (1 << bit) != 0)
        .map(|bit| match bit {
            0 => "EV_SYN".into(),
            1 => "EV_KEY".into(),
            2 => "EV_REL".into(),
            3 => "EV_ABS".into(),
            4 => "EV_MSC".into(),
            5 => "EV_SW".into(),
            17 => "EV_LED".into(),
            18 => "EV_SND".into(),
            20 => "EV_REP".into(),
            21 => "EV_FF".into(),
            22 => "EV_PWR".into(),
            23 => "EV_FF_STATUS".into(),
            _ => format!("Event type {bit}"),
        })
        .collect::<Vec<String>>();
    Ok(if names.is_empty() {
        "No event types advertised".into()
    } else {
        names.join(" · ")
    })
}
pub(super) fn bus(raw: &str) -> io::Result<String> {
    let id = u16::from_str_radix(raw.trim(), 16)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Invalid input bus ID"))?;
    let name = match id {
        1 => "PCI",
        3 => "USB",
        5 => "Bluetooth",
        6 => "Virtual",
        0x11 => "i8042",
        0x18 => "I²C",
        0x19 => "Host",
        0x1c => "SPI",
        _ => "Other input bus",
    };
    Ok(format!("{name} (0x{id:04x})"))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn categories_are_capabilities_not_live_input_or_device_class_guesses() {
        assert_eq!(events("1b").unwrap(), "EV_SYN · EV_KEY · EV_ABS · EV_MSC");
        assert!(events("220026").unwrap().contains("EV_FF"));
        assert_eq!(events("80000000").unwrap(), "Event type 31");
        for raw in ["", "1 0", "-1", "0x1", "100000000"] {
            assert!(events(raw).is_err());
        }
        assert_eq!(bus("0005").unwrap(), "Bluetooth (0x0005)");
    }
}
