//! Kernel registration names, not discovered electrical addresses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Object {
    I2cAdapter(u32),
    I2cAccess(u32),
    I2cClient(u32, u16),
    SpiController(u32),
    SpiDevice(u32, u32),
    SpiAccess(u32, u32),
}
pub(super) fn number(raw: &str) -> Option<u32> {
    if raw.is_empty() || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    raw.parse().ok()
}
impl Object {
    pub fn parse(name: &str, subsystem: &str) -> Option<Self> {
        match subsystem {
            "i2c" => i2c(name),
            "i2c-dev" => Some(Self::I2cAccess(number(name.strip_prefix("i2c-")?)?)),
            "spi_master" | "spi_slave" => {
                Some(Self::SpiController(number(name.strip_prefix("spi")?)?))
            }
            "spi" => {
                let (bus, select) = spi(name, "spi")?;
                Some(Self::SpiDevice(bus, select))
            }
            "spidev" => {
                let (bus, select) = spi(name, "spidev")?;
                Some(Self::SpiAccess(bus, select))
            }
            _ => None,
        }
    }
    pub fn bus(self) -> u32 {
        match self {
            Self::I2cAdapter(bus)
            | Self::I2cAccess(bus)
            | Self::I2cClient(bus, _)
            | Self::SpiController(bus)
            | Self::SpiDevice(bus, _)
            | Self::SpiAccess(bus, _) => bus,
        }
    }
    pub fn title(self) -> &'static str {
        match self {
            Self::I2cAdapter(_) => "I²C adapter",
            Self::I2cAccess(_) => "I²C adapter access",
            Self::I2cClient(..) => "Registered I²C device",
            Self::SpiController(_) => "SPI controller",
            Self::SpiDevice(..) => "Registered SPI device",
            Self::SpiAccess(..) => "SPI access interface",
        }
    }
    pub fn controller(self, candidate: Self) -> bool {
        match (self, candidate) {
            (
                Self::I2cAdapter(_) | Self::I2cAccess(_) | Self::I2cClient(..),
                Self::I2cAdapter(bus),
            ) => self.bus() == bus,
            (
                Self::SpiController(_) | Self::SpiDevice(..) | Self::SpiAccess(..),
                Self::SpiController(bus),
            ) => self.bus() == bus,
            _ => false,
        }
    }
}
fn i2c(name: &str) -> Option<Object> {
    if let Some(bus) = name.strip_prefix("i2c-") {
        return Some(Object::I2cAdapter(number(bus)?));
    }
    let (bus, address) = name.split_once('-')?;
    if address.len() != 4 || !address.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    Some(Object::I2cClient(
        number(bus)?,
        u16::from_str_radix(address, 16).ok()?,
    ))
}
fn spi(name: &str, prefix: &str) -> Option<(u32, u32)> {
    let (bus, select) = name.strip_prefix(prefix)?.split_once('.')?;
    Some((number(bus)?, number(select)?))
}
/// Linux encodes ten-bit and target registrations in otherwise unused address bits.
pub(super) fn address(encoded: u16) -> Option<(u16, u8, bool)> {
    let target = encoded & 0x1000 != 0;
    let raw = encoded & !0x1000;
    if raw <= 0x7f {
        Some((raw, 7, target))
    } else if (0xa000..=0xa3ff).contains(&raw) {
        Some((raw - 0xa000, 10, target))
    } else {
        None
    }
}
pub(super) fn address_label(encoded: u16) -> String {
    match address(encoded) {
        Some((address, bits, target)) => format!(
            "0x{address:02x} · {bits}-bit{}",
            if target { " target registration" } else { "" }
        ),
        None => format!("Encoded address 0x{encoded:04x} · unsupported flags"),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn registration_names_and_bus_roles_are_strict() {
        assert_eq!(
            Object::parse("6-0050", "i2c"),
            Some(Object::I2cClient(6, 0x50))
        );
        assert_eq!(
            Object::parse("i2c-6", "i2c-dev"),
            Some(Object::I2cAccess(6))
        );
        assert_eq!(
            Object::parse("spi2.3", "spi"),
            Some(Object::SpiDevice(2, 3))
        );
        assert_eq!(
            Object::parse("spidev2.3", "spidev"),
            Some(Object::SpiAccess(2, 3))
        );
        for name in ["6-50", "6-00050", "-1-0050", "i2c-", "spi2.3.4"] {
            assert!(Object::parse(name, "i2c").is_none());
        }
        assert_eq!(address(0xa2ff), Some((0x2ff, 10, false)));
        assert_eq!(address(0xb2ff), Some((0x2ff, 10, true)));
        assert_eq!(address(0x1050), Some((0x50, 7, true)));
        assert!(address(0x0080).is_none());
    }
}
