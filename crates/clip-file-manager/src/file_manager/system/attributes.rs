//! Subsystem-scoped, documented attribute read allowlist and semantic formatting.
use super::model::Value;
use std::{
    io,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Family {
    Usb,
    Pci,
    Drm,
    Tty,
    Vc,
    Block,
    Nvme,
    Net,
    Cpu,
    Numa,
    Hwmon,
    Thermal,
    PowerSupply,
    Powercap,
    Regulator,
    Rfkill,
    Devfreq,
    Remoteproc,
    Rpmsg,
    Ptp,
    Pps,
    ScsiHost,
    Ata,
    Mdio,
    Iio,
    Leds,
    Backlight,
    PlatformProfile,
    Input,
    Hid,
    Sound,
    Video,
    Media,
    I2c,
    Spi,
    Mtd,
    Ubi,
    Rtc,
    Watchdog,
    Typec,
    Thunderbolt,
    Platform,
    Acpi,
    Wmi,
    Dmi,
    Gpio,
    Pwm,
    Module,
    Driver,
    Unknown,
}
impl Family {
    pub fn label(self) -> &'static str {
        match self {
            Self::Usb => "USB",
            Self::Pci => "PCI/PCIe",
            Self::Drm => "DRM display",
            Self::Tty => "Terminal",
            Self::Vc => "Virtual-console screen interface",
            Self::Block => "Block device",
            Self::Nvme => "NVMe controller/subsystem",
            Self::Net => "Network interface",
            Self::Cpu => "CPU",
            Self::Numa => "NUMA",
            Self::Hwmon => "Hardware monitoring",
            Self::Thermal => "Thermal",
            Self::PowerSupply => "Power supply",
            Self::Powercap => "Powercap",
            Self::Regulator => "Regulator",
            Self::Rfkill => "Radio block control",
            Self::Devfreq => "Device frequency",
            Self::Remoteproc => "Remote processor",
            Self::Rpmsg => "Remote messaging",
            Self::Ptp => "PTP clock",
            Self::Pps => "PPS source",
            Self::ScsiHost => "SCSI host",
            Self::Ata => "ATA",
            Self::Mdio => "MDIO",
            Self::Iio => "Industrial I/O",
            Self::Leds => "LED",
            Self::Backlight => "Backlight",
            Self::PlatformProfile => "Platform profile",
            Self::Input => "Input",
            Self::Hid => "HID",
            Self::Sound => "ALSA sound",
            Self::Video => "V4L2 video",
            Self::Media => "Media controller",
            Self::I2c => "I²C",
            Self::Spi => "SPI",
            Self::Mtd => "Flash",
            Self::Ubi => "UBI",
            Self::Rtc => "RTC",
            Self::Watchdog => "Watchdog",
            Self::Typec => "USB Type-C",
            Self::Thunderbolt => "Thunderbolt/USB4",
            Self::Platform => "Firmware/platform",
            Self::Acpi => "ACPI firmware",
            Self::Wmi => "WMI firmware",
            Self::Dmi => "DMI machine identity",
            Self::Gpio => "GPIO",
            Self::Pwm => "PWM",
            Self::Module => "Kernel module",
            Self::Driver => "Driver",
            Self::Unknown => "Kernel",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) enum Format {
    Text,
    Integer,
    Unsigned,
    Bytes,
    Boolean(&'static str, &'static str),
    Scale(f64, &'static str),
    OptionalScale(f64, &'static str),
    UnsignedScale(f64, &'static str),
    Sectors,
    CpuList,
    CpuSpaceList,
    Selection,
    FrequencyKhz,
    FrequencyHz,
    RpmsgAddress,
    PpsTimestamp,
    CacheBytes,
    NegotiatedSpeed,
}
#[derive(Clone, Debug)]
pub(super) struct Rule {
    pub name: String,
    pub label: String,
    pub format: Format,
}
impl Rule {
    pub(super) fn new(name: &str, label: &str, format: Format) -> Self {
        Self {
            name: name.into(),
            label: label.into(),
            format,
        }
    }
}
pub(super) struct Context {
    pub family: Family,
    pub direct: bool,
    pub leaf: String,
    pub path: String,
    /// Requested group relative to the directly registered owning device.
    pub relative: PathBuf,
}
impl Context {
    pub(super) fn is_runtime_power(&self) -> bool {
        self.relative == Path::new("power")
            && !self.direct
            && !matches!(
                self.family,
                Family::Module | Family::Driver | Family::Unknown
            )
    }
    pub(super) fn label(&self) -> Option<&'static str> {
        if self.is_runtime_power() {
            Some("Device power management")
        } else {
            (self.family != Family::Unknown).then(|| self.family.label())
        }
    }
    pub fn discover(path: &Path) -> Self {
        let mut current = Some(path);
        let mut subsystem = None;
        let mut direct = false;
        for _ in 0..4 {
            let Some(directory) = current else {
                break;
            };
            subsystem = directory
                .join("subsystem")
                .canonicalize()
                .ok()
                .and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()));
            if subsystem.is_some() {
                direct = directory == path;
                break;
            }
            current = directory.parent();
        }
        let mut context = Self::from_parts(path, subsystem.as_deref());
        context.direct = direct;
        if subsystem.is_some() {
            context.relative = path
                .strip_prefix(current.unwrap_or(path))
                .unwrap_or(path)
                .into();
        }
        context
    }
    pub fn from_parts(path: &Path, subsystem: Option<&str>) -> Self {
        let family = match subsystem.unwrap_or("") {
            "usb" => Family::Usb,
            "pci" => Family::Pci,
            "drm" => Family::Drm,
            "tty" => Family::Tty,
            "vc" => Family::Vc,
            "block" => Family::Block,
            "nvme" | "nvme-subsystem" => Family::Nvme,
            "net" => Family::Net,
            "cpu" => Family::Cpu,
            "node" => Family::Numa,
            "hwmon" => Family::Hwmon,
            "thermal" => Family::Thermal,
            "power_supply" => Family::PowerSupply,
            "powercap" => Family::Powercap,
            "regulator" => Family::Regulator,
            "rfkill" => Family::Rfkill,
            "devfreq" => Family::Devfreq,
            "remoteproc" => Family::Remoteproc,
            "rpmsg" => Family::Rpmsg,
            "ptp" => Family::Ptp,
            "pps" => Family::Pps,
            "scsi_host" => Family::ScsiHost,
            "ata_port" | "ata_link" | "ata_device" => Family::Ata,
            "mdio_bus" => Family::Mdio,
            "iio" => Family::Iio,
            "leds" => Family::Leds,
            "backlight" => Family::Backlight,
            "platform-profile" => Family::PlatformProfile,
            "input" => Family::Input,
            "hid" | "hidraw" => Family::Hid,
            "sound" => Family::Sound,
            "video4linux" => Family::Video,
            "media" => Family::Media,
            "i2c" | "i2c-dev" => Family::I2c,
            "spi" | "spidev" | "spi_master" | "spi_slave" => Family::Spi,
            "mtd" => Family::Mtd,
            "ubi" => Family::Ubi,
            "rtc" => Family::Rtc,
            "watchdog" => Family::Watchdog,
            "typec" => Family::Typec,
            "thunderbolt" => Family::Thunderbolt,
            "platform" => Family::Platform,
            "acpi" => Family::Acpi,
            "wmi" => Family::Wmi,
            "dmi" => Family::Dmi,
            "gpio" => Family::Gpio,
            "pwm" => Family::Pwm,
            "" if subsystem.is_none() => fallback_family(path),
            _ => Family::Unknown,
        };
        Self {
            family,
            direct: subsystem.is_some(),
            leaf: path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            path: path.to_string_lossy().into_owned(),
            relative: PathBuf::new(),
        }
    }
}
// Global collections have no subsystem link. Only verified root identities and
// their documented layouts may select an exception to direct registration.
fn fallback_family(path: &Path) -> Family {
    use super::boot_firmware::same_object;
    if path
        .ancestors()
        .take(3)
        .any(|ancestor| same_object(ancestor, Path::new("/sys/module")))
    {
        return Family::Module;
    }
    let cpu = Path::new("/sys/devices/system/cpu");
    if same_object(path, cpu) || same_object(path, &cpu.join("cpufreq")) {
        return Family::Cpu;
    }
    let Some(parent) = path.parent() else {
        return Family::Unknown;
    };
    if same_object(parent, &cpu.join("cpufreq"))
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| super::buses::numbered(name, "policy"))
    {
        return Family::Cpu;
    }
    if parent.file_name().is_some_and(|name| name == "drivers")
        && parent
            .parent()
            .and_then(Path::parent)
            .is_some_and(|root| same_object(root, Path::new("/sys/bus")))
    {
        return Family::Driver;
    }
    Family::Unknown
}

pub(super) fn rules(context: &Context) -> Vec<Rule> {
    use Family::*;
    let path = Path::new(&context.path);
    if super::numa::is_root(path) {
        return super::numa::rules(path);
    }
    if super::illumination::legacy_profile_root(path) {
        return super::illumination::rules(path);
    }
    if let Some(specs) = specialized_specs(context) {
        return materialize_specs(specs);
    }
    let data = match context.family {
        Usb => USB,
        Pci => PCI,
        Drm => DRM,
        Tty => return super::terminal::rules(path),
        Vc => &[("dev", "Console screen device number", Format::Text)],
        Block => BLOCK,
        Nvme => NVME,
        Net => NET,
        Cpu => CPU,
        Numa => return super::numa::rules(path),
        Hwmon => HWMON,
        Thermal => THERMAL,
        PowerSupply => POWER,
        Powercap | Regulator => return super::power_controls::rules(path),
        Rfkill | Devfreq => return super::device_controls::rules(path),
        Remoteproc | Rpmsg => return super::remote::rules(path),
        Ptp | Pps => return super::precision_clock::rules(path),
        ScsiHost | Ata | Mdio => return super::storage_bus::rules(path),
        Iio => return super::iio::rules(path),
        Leds | Backlight | PlatformProfile => return super::illumination::rules(path),
        Input => INPUT,
        Hid => HID,
        Sound => SOUND,
        Video => VIDEO,
        Media => MEDIA,
        I2c | Spi => BUS,
        Mtd => MTD,
        Ubi => UBI,
        Rtc | Watchdog => return super::clock_watchdog::rules(std::path::Path::new(&context.path)),
        Typec | Thunderbolt => return super::connectors::rules(Path::new(&context.path)),
        Platform | Acpi | Wmi | Dmi => firmware_rules(context),
        Gpio | Pwm => return super::pins::rules(std::path::Path::new(&context.path)),
        Module
            if path.parent().is_some_and(|parent| {
                super::boot_firmware::same_object(parent, Path::new("/sys/module"))
            }) =>
        {
            MODULE
        }
        Module => &[],
        Driver | Unknown => &[],
    };
    data.iter()
        .filter_map(|(name, label, format)| {
            let name = Path::new(name).strip_prefix(&context.relative).ok()?;
            Some(Rule::new(name.to_str()?, label, *format))
        })
        .collect()
}

/// Conservative, family-scoped stable identity fields. Unknown fields remain live.
pub(super) fn mostly_static(context: &Context, name: &str) -> bool {
    let names: &[&str] = match context.family {
        Family::Usb => &[
            "idVendor",
            "idProduct",
            "bcdDevice",
            "bcdUSB",
            "manufacturer",
            "product",
            "serial",
        ],
        Family::Pci => &[
            "vendor",
            "device",
            "subsystem_vendor",
            "subsystem_device",
            "class",
            "revision",
        ],
        Family::Nvme => &["model", "serial", "firmware_rev"],
        Family::Dmi => &[
            "sys_vendor",
            "product_name",
            "product_version",
            "product_family",
            "bios_vendor",
            "bios_version",
            "bios_date",
            "bios_release",
            "ec_firmware_release",
            "board_vendor",
            "board_name",
            "board_version",
            "chassis_vendor",
            "chassis_type",
            "chassis_version",
        ],
        _ => &[],
    };
    names.contains(&name)
}

/// Copies a scoped static schema into owned rules for per-observation labels.
pub(super) fn materialize_specs(specs: &[Spec]) -> Vec<Rule> {
    specs
        .iter()
        .map(|(name, label, format)| Rule::new(name, label, *format))
        .collect()
}

// These directories have their own attribute schema, independent of the
// enclosing device's family. Registered devices merely named power are roots.
fn specialized_specs(context: &Context) -> Option<&'static [Spec]> {
    if context.is_runtime_power() {
        return Some(PM);
    }
    match context.family {
        Family::Block
            if !context.direct
                && context.relative.parent() == Some(Path::new("md"))
                && context.leaf.starts_with("dev-") =>
        {
            Some(MD_MEMBER)
        }
        Family::Usb
            if context.leaf.starts_with("ep_")
                && (context.direct || context.relative.components().count() == 1) =>
        {
            Some(USB_ENDPOINT)
        }
        Family::Cpu
            if context.relative.as_os_str().is_empty()
                && (context.leaf == "cpu"
                    || super::boot_firmware::same_object(
                        Path::new(&context.path),
                        Path::new("/sys/devices/system/cpu"),
                    )) =>
        {
            Some(CPU_ROOT)
        }
        Family::Cpu
            if !context.direct
                && super::boot_firmware::same_object(
                    Path::new(&context.path),
                    Path::new("/sys/devices/system/cpu/cpufreq"),
                ) =>
        {
            Some(&[])
        }
        Family::Cpu
            if context.path.contains("/cache/")
                && (context.direct || context.relative.parent() == Some(Path::new("cache"))) =>
        {
            Some(CACHE)
        }
        _ => None,
    }
}

pub(super) fn rule(context: &Context, name: &str) -> Option<Rule> {
    rules(context)
        .into_iter()
        .find(|r| r.name == name)
        .or_else(|| super::power_controls::constraint_rule(Path::new(&context.path), name))
        .or_else(|| super::iio::metadata_rule(Path::new(&context.path), name))
        .or_else(|| {
            (context.family == Family::Hwmon && context.direct)
                .then(|| channel_rule(name))
                .flatten()
        })
        .or_else(|| {
            (context.family == Family::Thermal && context.direct)
                .then(|| thermal_rule(context, name))
                .flatten()
        })
}
pub(super) fn sensor_channel(name: &str) -> Option<(&str, &str)> {
    let channel = name.split('_').next()?;
    let boundary = channel.find(|character: char| character.is_ascii_digit())?;
    let (kind, number) = channel.split_at(boundary);
    if number.is_empty()
        || !number.bytes().all(|byte| byte.is_ascii_digit())
        || !matches!(
            kind,
            "temp" | "in" | "curr" | "power" | "energy" | "fan" | "pwm"
        )
    {
        return None;
    }
    Some((channel, kind))
}
fn channel_rule(name: &str) -> Option<Rule> {
    let (channel, kind) = sensor_channel(name)?;
    if kind == "pwm" && channel == name {
        return Some(Rule::new(name, "Fan PWM", Format::Integer));
    }
    let (_, field) = name.split_once('_')?;
    let format = match field {
        "label" => Format::Text,
        "alarm" | "fault" | "crit_alarm" | "min_alarm" | "max_alarm" => {
            Format::Boolean("Yes", "No")
        }
        "emergency_alarm" if kind == "temp" => Format::Boolean("Yes", "No"),
        "emergency_hyst" | "lcrit_hyst" if kind == "temp" => Format::Scale(0.001, "°C"),
        "mode" if kind == "pwm" => Format::Boolean("PWM", "DC"),
        "freq" if kind == "pwm" => Format::UnsignedScale(1.0, "Hz"),
        "enable" if kind == "pwm" => Format::Unsigned,
        "type" | "enable" => Format::Integer,
        "input" | "average" | "min" | "max" | "crit" | "lcrit" | "emergency" | "max_hyst"
        | "crit_hyst" | "min_hyst" => match kind {
            "temp" => Format::Scale(0.001, "°C"),
            "in" => Format::Scale(0.001, "V"),
            "curr" => Format::Scale(0.001, "A"),
            "power" => Format::Scale(0.000001, "W"),
            "energy" => Format::Scale(0.000001, "J"),
            "fan" => Format::Scale(1.0, "RPM"),
            _ => return None,
        },
        _ => return None,
    };
    Some(Rule::new(name, name, format))
}

fn thermal_rule(context: &Context, name: &str) -> Option<Rule> {
    if !context.leaf.starts_with("thermal_zone") {
        return None;
    }
    if let Some(tail) = name.strip_prefix("trip_point_") {
        let (id, field) = tail.split_once('_')?;
        if id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        let (label, format) = match field {
            "temp" => ("Temperature", Format::Scale(0.001, "°C")),
            "hyst" => ("Hysteresis", Format::Scale(0.001, "°C")),
            "type" => ("Type", Format::Text),
            _ => return None,
        };
        return Some(Rule::new(name, &format!("Trip {id} {label}"), format));
    }
    let id = name.strip_prefix("cdev")?.strip_suffix("_trip_point")?;
    if id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some(Rule::new(
        name,
        &format!("Cooling device {id} trip binding"),
        Format::Integer,
    ))
}
pub(super) fn parse(rule: &Rule, raw: &str) -> io::Result<(Value, String)> {
    let raw = raw.trim();
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "Attribute does not match its documented type",
        )
    };
    let result = match rule.format {
        Format::Text => (Value::Text(raw.into()), raw.into()),
        Format::Unsigned | Format::Bytes => {
            let value = raw.parse::<u64>().map_err(|_| invalid())?;
            (
                Value::Integer(value.into()),
                if matches!(rule.format, Format::Bytes) {
                    super::super::filesystem::format_size(value)
                } else {
                    value.to_string()
                },
            )
        }
        Format::Integer => {
            let value = raw.parse::<i128>().map_err(|_| invalid())?;
            (Value::Integer(value), value.to_string())
        }
        Format::Boolean(yes, no) => {
            let value = match raw {
                "1" => true,
                "0" => false,
                _ => return Err(invalid()),
            };
            (Value::Boolean(value), if value { yes } else { no }.into())
        }
        Format::OptionalScale(_, _) if raw == "constraint not defined" => {
            (Value::Text(raw.into()), raw.into())
        }
        Format::Scale(scale, unit)
        | Format::OptionalScale(scale, unit)
        | Format::UnsignedScale(scale, unit) => {
            let raw_value = if matches!(rule.format, Format::UnsignedScale(_, _)) {
                raw.parse::<u64>().map_err(|_| invalid())? as f64
            } else {
                raw.parse::<i64>().map_err(|_| invalid())? as f64
            };
            let value = raw_value * scale;
            (
                Value::Measurement {
                    value,
                    unit: unit.into(),
                },
                format!("{} {unit}", decimal(value)),
            )
        }
        Format::Sectors => {
            let sectors = raw.parse::<u64>().map_err(|_| invalid())?;
            let bytes = sectors.checked_mul(512).ok_or_else(invalid)?;
            (
                Value::Integer(bytes.into()),
                super::super::filesystem::format_size(bytes),
            )
        }
        Format::PpsTimestamp => (
            Value::Text(raw.into()),
            super::precision_clock::timestamp(raw)?,
        ),
        Format::RpmsgAddress => {
            let value = raw
                .parse::<u32>()
                .or_else(|_| raw.parse::<i32>().map(|value| value as u32))
                .map_err(|_| invalid())?;
            let display = if value == u32::MAX {
                "Unassigned / any-peer sentinel (0xffffffff)".into()
            } else {
                format!("0x{value:08x} ({value})")
            };
            (Value::Integer(value.into()), display)
        }
        Format::NegotiatedSpeed => {
            let mbps = raw.parse::<i64>().map_err(|_| invalid())?;
            if mbps < 0 {
                (Value::Text("Unknown".into()), "Unknown".into())
            } else {
                (
                    Value::Measurement {
                        value: mbps as f64,
                        unit: "Mb/s".into(),
                    },
                    format!("{mbps} Mb/s"),
                )
            }
        }
        Format::CacheBytes => {
            let bytes = raw
                .strip_suffix('K')
                .and_then(|value| value.parse::<u64>().ok())
                .and_then(|value| value.checked_mul(1024))
                .ok_or_else(invalid)?;
            (
                Value::Measurement {
                    value: bytes as f64,
                    unit: "bytes".into(),
                },
                super::super::filesystem::format_size(bytes),
            )
        }
        Format::FrequencyKhz | Format::FrequencyHz => {
            let raw_value = raw.parse::<u64>().map_err(|_| invalid())?;
            let hz = if matches!(rule.format, Format::FrequencyKhz) {
                raw_value.checked_mul(1000).ok_or_else(invalid)?
            } else {
                raw_value
            };
            (
                Value::Measurement {
                    value: hz as f64,
                    unit: "Hz".into(),
                },
                if matches!(rule.format, Format::FrequencyKhz) {
                    format!("{} GHz", decimal(hz as f64 / 1_000_000_000.0))
                } else {
                    let (unit, divisor) = [
                        ("GHz", 1_000_000_000u64),
                        ("MHz", 1_000_000),
                        ("kHz", 1000),
                        ("Hz", 1),
                    ]
                    .into_iter()
                    .find(|(_, divisor)| hz >= *divisor)
                    .unwrap_or(("Hz", 1));
                    format!("{} {unit}", decimal(hz as f64 / divisor as f64))
                },
            )
        }
        Format::CpuList | Format::CpuSpaceList => {
            let list = if matches!(rule.format, Format::CpuSpaceList) {
                raw.split_whitespace().collect::<Vec<_>>().join(",")
            } else {
                raw.to_string()
            };
            let ranges = cpu_list(&list)?;
            let count = ranges
                .iter()
                .map(|(a, b)| u64::from(*b) - u64::from(*a) + 1)
                .sum::<u64>();
            (
                Value::CpuList(ranges),
                format!("{count} logical CPUs · {raw}"),
            )
        }
        Format::Selection => selection(raw)?,
    };
    Ok(result)
}
/// Parses one optional bracketed current value without guessing through bad markers.
fn selection(raw: &str) -> io::Result<(Value, String)> {
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "Malformed supported-value selection",
        )
    };
    let mut available = Vec::new();
    let mut selected = None;
    for token in raw.split_whitespace() {
        let marked = token
            .strip_prefix('[')
            .and_then(|value| value.strip_suffix(']'));
        let value = marked.unwrap_or(token);
        if value.is_empty() || value.contains(['[', ']']) {
            return Err(invalid());
        }
        if marked.is_some() && selected.replace(value.to_string()).is_some() {
            return Err(invalid());
        }
        available.push(value.to_string());
    }
    let display = selected.as_ref().map_or_else(
        || available.join(" / "),
        |value| format!("{value} · available: {}", available.join(" / ")),
    );
    Ok((
        Value::Selection {
            selected,
            available,
        },
        display,
    ))
}

fn decimal(value: f64) -> String {
    format!("{value:.6}")
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}
pub(super) fn cpu_list(raw: &str) -> io::Result<Vec<(u32, u32)>> {
    let mut ranges = Vec::new();
    if raw.is_empty() {
        return Ok(ranges);
    }
    for part in raw.split(',') {
        let (first, last) = part.split_once('-').unwrap_or((part, part));
        let first = first
            .parse::<u32>()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Invalid CPU list"))?;
        let last = last
            .parse::<u32>()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Invalid CPU list"))?;
        if first > last || last > 1_048_575 || ranges.len() >= 4096 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid or oversized CPU list",
            ));
        }
        ranges.push((first, last));
    }
    Ok(merge_cpu_ranges(ranges))
}
pub(super) fn merge_cpu_ranges(mut ranges: Vec<(u32, u32)>) -> Vec<(u32, u32)> {
    ranges.sort_unstable();
    let mut merged: Vec<(u32, u32)> = Vec::new();
    for (first, last) in ranges {
        if let Some(previous) = merged
            .last_mut()
            .filter(|(_, end)| first <= end.saturating_add(1))
        {
            previous.1 = previous.1.max(last);
        } else {
            merged.push((first, last));
        }
    }
    merged
}

use Format::{Boolean as B, CpuList as C, Integer as I, Scale as S, Text as T};
type Spec = (&'static str, &'static str, Format);
const USB: &[Spec] = &[
    ("serial", "USB serial number", T),
    ("product", "USB product", T),
    ("manufacturer", "USB manufacturer", T),
    ("idVendor", "USB vendor ID", T),
    ("idProduct", "USB product ID", T),
    ("speed", "Negotiated USB speed", S(1.0, "Mb/s")),
    ("busnum", "Bus", I),
    ("devnum", "Device address", I),
    ("bcdUSB", "USB version", T),
    ("bNumInterfaces", "Interfaces", I),
    ("bNumConfigurations", "Configurations", I),
    ("bInterfaceNumber", "Interface number", T),
    ("bInterfaceClass", "Interface class", T),
    ("bInterfaceSubClass", "Interface subclass", T),
    ("bInterfaceProtocol", "Interface protocol", T),
    ("bAlternateSetting", "Alternate setting", T),
    ("bNumEndpoints", "Endpoint count", T),
    ("bConfigurationValue", "Active configuration", I),
    ("bMaxPower", "Reported configuration maximum power", T),
    ("bmAttributes", "Active configuration attributes", T),
    ("power/control", "USB power policy", T),
    ("power/runtime_status", "USB power state", T),
];
const USB_ENDPOINT: &[Spec] = &[
    ("bEndpointAddress", "Endpoint address", T),
    ("bmAttributes", "Endpoint transfer attributes", T),
    ("wMaxPacketSize", "Maximum packet descriptor", T),
    ("bInterval", "Interval descriptor", T),
    ("interval", "Polling interval", T),
];
const PCI: &[Spec] = &[
    ("resource", "PCI resource ranges", T),
    ("revision", "PCI revision", T),
    ("vendor", "PCI vendor ID", T),
    ("device", "PCI device ID", T),
    ("class", "PCI class", T),
    ("subsystem_vendor", "Subsystem vendor ID", T),
    ("subsystem_device", "Subsystem device ID", T),
    ("current_link_speed", "Negotiated link speed", T),
    (
        "current_link_width",
        "Negotiated link width",
        Format::Unsigned,
    ),
    ("max_link_speed", "Maximum link speed", T),
    ("max_link_width", "Maximum link width", Format::Unsigned),
    ("numa_node", "NUMA node", I),
    ("irq", "IRQ", I),
];
const DRM: &[Spec] = &[
    ("status", "Connection status", T),
    ("enabled", "Encoder association", T),
    ("modes", "Advertised mode names", T),
    ("dpms", "Reported DPMS property", T),
    ("connector_id", "Connector ID", I),
    ("dev", "Device number", T),
];
// Documented MD member metadata; reading does not assemble, fail or remove members.
const MD_MEMBER: &[Spec] = &[
    ("state", "MD member state", T),
    ("slot", "MD member role", T),
];
const BLOCK: &[Spec] = &[
    ("md/level", "RAID level", T),
    ("md/array_state", "Array state", T),
    ("md/raid_disks", "Configured RAID roles", Format::Unsigned),
    ("md/degraded", "Degraded RAID roles", Format::Unsigned),
    ("md/sync_action", "Recovery/check action", T),
    ("md/sync_completed", "Recovery/check sectors", T),
    ("dev", "Device number", T),
    ("diskseq", "Disk sequence", I),
    ("size", "Capacity", Format::Sectors),
    ("ro", "Read only", B("Yes", "No")),
    ("partition", "Partition number", I),
    ("start", "Start", Format::Sectors),
    ("dm/name", "Mapped name", T),
    ("dm/uuid", "Mapped UUID", T),
    ("loop/backing_file", "Backing file", T),
    ("loop/offset", "Backing offset", S(1.0, "bytes")),
    ("loop/sizelimit", "Size limit", S(1.0, "bytes")),
    ("loop/autoclear", "Autoclear", B("Yes", "No")),
    ("loop/dio", "Direct I/O", B("Yes", "No")),
    ("loop/partscan", "Partition scanning", B("Yes", "No")),
    ("device/model", "Storage model", T),
    ("device/serial", "Storage serial", T),
    ("device/rev", "Storage revision", T),
    ("device/firmware_rev", "Firmware revision", T),
    ("device/transport", "Storage transport", T),
    ("device/state", "Controller/device state", T),
    ("wwid", "World-wide identifier", T),
    ("nsid", "Namespace ID", I),
    ("uuid", "Namespace UUID", T),
    ("eui", "Namespace EUI", T),
    ("nguid", "Namespace NGUID", T),
    (
        "queue/discard_max_bytes",
        "Maximum discard",
        S(1.0, "bytes"),
    ),
    (
        "queue/discard_granularity",
        "Discard granularity",
        S(1.0, "bytes"),
    ),
    ("queue/nr_requests", "Request queue limit", I),
    (
        "queue/logical_block_size",
        "Logical sector",
        S(1.0, "bytes"),
    ),
    (
        "queue/physical_block_size",
        "Physical sector",
        S(1.0, "bytes"),
    ),
    ("queue/rotational", "Rotational", B("Yes", "No")),
    ("queue/scheduler", "I/O scheduler", Format::Selection),
];
const NVME: &[Spec] = &[
    ("model", "Storage model", T),
    ("serial", "Storage serial", T),
    ("firmware_rev", "Firmware revision", T),
    ("cntlid", "Controller ID", I),
    ("cntrltype", "Controller type", T),
    ("dctype", "Discovery controller type", T),
    ("state", "Controller state", T),
    ("transport", "Storage transport", T),
    ("subsysnqn", "Subsystem NQN", T),
    ("address", "Transport address", T),
    ("numa_node", "NUMA node", I),
    ("queue_count", "Controller queues", I),
    ("sqsize", "Submission queue size", I),
];
const NET: &[Spec] = &[
    ("operstate", "Operational state", T),
    ("carrier", "Carrier", B("Up", "Down")),
    ("address", "MAC address", T),
    ("mtu", "MTU", Format::Unsigned),
    ("speed", "Negotiated speed", Format::NegotiatedSpeed),
    ("duplex", "Duplex", T),
    ("ifindex", "Interface index", Format::Unsigned),
    ("iflink", "Linked interface index", Format::Unsigned),
    ("type", "Interface type", I),
];
const CPU_ROOT: &[Spec] = &[
    ("online", "Online CPUs", C),
    ("present", "Present CPUs", C),
    ("possible", "Possible CPUs", C),
    ("offline", "Offline CPUs", C),
    ("isolated", "Isolated CPUs", C),
    ("nohz_full", "Full tickless CPUs", C),
];
const CPU: &[Spec] = &[
    ("online", "Online", B("Yes", "No")),
    ("topology/physical_package_id", "Package", I),
    ("topology/core_id", "Core", I),
    ("topology/die_id", "Die", I),
    ("topology/cluster_id", "Cluster", I),
    ("topology/core_cpus_list", "Core CPUs (SMT)", C),
    ("topology/package_cpus_list", "Package CPUs", C),
    ("topology/thread_siblings_list", "SMT siblings", C),
    ("topology/core_siblings_list", "Core siblings", C),
    (
        "scaling_cur_freq",
        "Requested/current policy frequency",
        Format::FrequencyKhz,
    ),
    (
        "cpuinfo_cur_freq",
        "Hardware frequency",
        Format::FrequencyKhz,
    ),
    (
        "scaling_min_freq",
        "Minimum policy frequency",
        Format::FrequencyKhz,
    ),
    (
        "scaling_max_freq",
        "Maximum policy frequency",
        Format::FrequencyKhz,
    ),
    ("scaling_governor", "Governor", T),
    ("scaling_available_governors", "Available governors", T),
    ("scaling_driver", "Scaling driver", T),
    (
        "cpuinfo_min_freq",
        "Minimum hardware frequency",
        Format::FrequencyKhz,
    ),
    (
        "cpuinfo_max_freq",
        "Maximum hardware frequency",
        Format::FrequencyKhz,
    ),
    ("related_cpus", "Policy CPUs", Format::CpuSpaceList),
    ("affected_cpus", "Affected CPUs", Format::CpuSpaceList),
];
const HWMON: &[Spec] = &[
    ("name", "Monitoring driver", T),
    ("update_interval", "Update interval", S(1.0, "ms")),
];
const THERMAL: &[Spec] = &[
    ("type", "Thermal type", T),
    ("temp", "Temperature", S(0.001, "°C")),
    ("mode", "Mode", T),
    ("policy", "Policy", T),
    ("cur_state", "Cooling state", Format::Unsigned),
    ("max_state", "Maximum cooling state", Format::Unsigned),
];
const POWER: &[Spec] = &[
    ("type", "Supply type", T),
    ("status", "Status", T),
    ("capacity", "Capacity", S(1.0, "%")),
    ("present", "Present", B("Yes", "No")),
    ("online", "Online", B("Yes", "No")),
    ("energy_now", "Remaining energy", S(0.000001, "Wh")),
    ("energy_full", "Full energy", S(0.000001, "Wh")),
    ("energy_full_design", "Design energy", S(0.000001, "Wh")),
    ("charge_now", "Remaining charge", S(0.000001, "Ah")),
    ("charge_full", "Full charge", S(0.000001, "Ah")),
    ("charge_full_design", "Design charge", S(0.000001, "Ah")),
    ("voltage_now", "Voltage", S(0.000001, "V")),
    ("current_now", "Current", S(0.000001, "A")),
    ("power_now", "Power", S(0.000001, "W")),
    ("temp", "Temperature", S(0.1, "°C")),
    ("health", "Kernel health state", T),
    ("manufacturer", "Manufacturer", T),
    ("model_name", "Model", T),
];
const INPUT: &[Spec] = &[
    ("name", "Input name", T),
    ("phys", "Physical path", T),
    ("id/vendor", "Input vendor ID", T),
    ("id/product", "Input product ID", T),
    ("id/bustype", "Input bus ID", T),
    ("capabilities/ev", "Event capabilities", T),
    ("capabilities/key", "Key capabilities", T),
    ("capabilities/rel", "Relative axes", T),
    ("capabilities/abs", "Absolute axes", T),
    ("capabilities/sw", "Switch capabilities", T),
    ("capabilities/led", "LED capabilities", T),
    ("capabilities/ff", "Force feedback capabilities", T),
    ("id/version", "Input device version", T),
    ("uniq", "Input unique ID", T),
    ("properties", "Input properties", T),
];
const HID: &[Spec] = &[("uevent", "HID kernel metadata", T)];
const VIDEO: &[Spec] = &[
    ("name", "Video device name", T),
    ("index", "Video registration index", I),
];
const MEDIA: &[Spec] = &[("model", "Media controller model", T)];
const SOUND: &[Spec] = &[("id", "ALSA card ID", T)];
const BUS: &[Spec] = &[
    ("name", "Registered device name", T),
    ("modalias", "Module alias", T),
];
const MTD: &[Spec] = &[
    ("name", "Flash name", T),
    ("type", "Flash type", T),
    ("size", "Capacity", Format::Bytes),
    ("offset", "Partition offset", Format::Bytes),
    ("erasesize", "Major erase size", Format::Bytes),
    ("writesize", "Write size", Format::Bytes),
    ("oobsize", "OOB size", Format::Bytes),
    ("oobavail", "Available OOB bytes", Format::Bytes),
    ("ecc_strength", "ECC strength", Format::Unsigned),
    ("ecc_step_size", "ECC step size", Format::Bytes),
    ("ecc_failures", "ECC failures (aggregate)", Format::Unsigned),
    (
        "corrected_bits",
        "Corrected bits (aggregate)",
        Format::Unsigned,
    ),
    ("bad_blocks", "Bad blocks (aggregate)", Format::Unsigned),
    ("bbt_blocks", "Reserved BBT blocks", Format::Unsigned),
    ("numeraseregions", "Erase regions", Format::Unsigned),
    ("flags", "Kernel flash flags", T),
];
const UBI: &[Spec] = &[
    ("mtd_num", "Backing MTD number", Format::Unsigned),
    ("name", "Volume name", T),
    ("type", "Volume type", T),
    (
        "total_eraseblocks",
        "Good physical eraseblocks",
        Format::Unsigned,
    ),
    (
        "avail_eraseblocks",
        "Available logical eraseblocks",
        Format::Unsigned,
    ),
    ("eraseblock_size", "Logical eraseblock size", Format::Bytes),
    ("min_io_size", "Minimum I/O size", Format::Bytes),
    (
        "data_bytes",
        "Data bytes (static payload; dynamic capacity)",
        Format::Bytes,
    ),
    (
        "reserved_ebs",
        "Reserved volume eraseblocks",
        Format::Unsigned,
    ),
    ("max_ec", "Maximum erase counter", Format::Unsigned),
    (
        "bad_peb_count",
        "Bad physical eraseblocks",
        Format::Unsigned,
    ),
    (
        "reserved_for_bad",
        "Reserved bad-block pool",
        Format::Unsigned,
    ),
    ("volumes_count", "Kernel volume count", Format::Unsigned),
    ("max_vol_count", "Maximum volume count", Format::Unsigned),
    ("alignment", "Volume alignment", Format::Unsigned),
    ("ro_mode", "Kernel read-only mode", B("Yes", "No")),
    (
        "corrupted",
        "Kernel volume corruption flag",
        B("Set", "Clear"),
    ),
    ("upd_marker", "Kernel update marker", B("Set", "Clear")),
];
pub(super) const RTC: &[Spec] = &[
    ("name", "Clock identity", T),
    ("date", "RTC date", T),
    ("time", "RTC time (timezone unspecified)", T),
    ("wakealarm", "Wake alarm", T),
    ("since_epoch", "RTC seconds since epoch", I),
    ("hctosys", "Provided system time at boot", B("Yes", "No")),
    ("range", "RTC supported epoch range", T),
    ("offset", "RTC clock adjustment (ppb)", I),
];
pub(super) const WATCHDOG: &[Spec] = &[
    ("identity", "Watchdog identity", T),
    ("state", "Watchdog kernel active state", T),
    ("timeout", "Watchdog timeout (s)", Format::Unsigned),
    ("timeleft", "Watchdog time left (s)", Format::Unsigned),
    ("pretimeout", "Watchdog pretimeout (s)", Format::Unsigned),
    ("nowayout", "Watchdog nowayout", B("Set", "Clear")),
    ("bootstatus", "Watchdog boot-status bits", T),
    ("status", "Watchdog reported status bits", T),
    ("options", "Watchdog supported-option bits", T),
    ("fw_version", "Watchdog firmware version", T),
];
const PLATFORM: &[Spec] = &[("modalias", "Module alias", T)];
fn firmware_rules(context: &Context) -> &'static [Spec] {
    match (context.family, context.direct) {
        (Family::Wmi, true) => WMI,
        (Family::Acpi, true) => ACPI,
        (Family::Platform, true) => PLATFORM,
        (Family::Dmi, true) => DMI,
        _ => &[],
    }
}
const WMI: &[Spec] = &[
    ("guid", "WMI interface GUID", T),
    ("modalias", "Module alias", T),
    ("object_id", "WMI object ID", T),
    ("notify_id", "WMI notification ID", T),
    ("instance_count", "WMI instance count", Format::Unsigned),
    ("expensive", "WMI expensive-method flag", B("Yes", "No")),
    ("setable", "WMI data-block writable flag", B("Yes", "No")),
];
const DMI: &[Spec] = &[
    ("sys_vendor", "System manufacturer", T),
    ("product_name", "System product/model", T),
    ("product_version", "System product version", T),
    ("product_family", "System product family", T),
    ("bios_vendor", "BIOS/UEFI vendor", T),
    ("bios_version", "BIOS/UEFI version", T),
    ("bios_date", "BIOS/UEFI release date", T),
    ("bios_release", "BIOS/UEFI release", T),
    (
        "ec_firmware_release",
        "Embedded-controller firmware release",
        T,
    ),
    ("board_vendor", "Board manufacturer", T),
    ("board_name", "Board model", T),
    ("board_version", "Board version", T),
    ("chassis_vendor", "Chassis manufacturer", T),
    ("chassis_type", "Chassis type code", Format::Unsigned),
    ("chassis_version", "Chassis version", T),
];
// Cached enumeration identity only. cid/status/description/hrv evaluate methods.
const ACPI: &[Spec] = &[
    ("hid", "ACPI hardware ID", T),
    ("modalias", "ACPI cached enumeration alias", T),
    ("path", "ACPI namespace path", T),
    ("uid", "ACPI unique ID", T),
    ("adr", "ACPI cached bus address", T),
];
const MODULE: &[Spec] = &[
    ("version", "Module version", T),
    ("srcversion", "Module source version checksum", T),
    ("refcnt", "Reference count", I),
    ("taint", "Module taint", T),
    ("initstate", "Initialization state", T),
];

const PM: &[Spec] = &[
    ("control", "Runtime PM policy", T),
    ("runtime_status", "Runtime power state", T),
    ("autosuspend_delay_ms", "Autosuspend delay", S(1.0, "ms")),
    ("wakeup", "Wakeup policy", T),
    ("runtime_enabled", "Runtime PM enabled state", T),
    ("runtime_usage", "Runtime PM usage count", I),
    (
        "runtime_active_kids",
        "Runtime PM children usage count",
        Format::Unsigned,
    ),
    (
        "runtime_active_time",
        "Runtime active time",
        Format::UnsignedScale(1.0, "ms"),
    ),
    (
        "runtime_suspended_time",
        "Runtime suspended time",
        Format::UnsignedScale(1.0, "ms"),
    ),
    ("wakeup_count", "Signaled wakeup events", Format::Unsigned),
    (
        "wakeup_active_count",
        "Completed wakeup processing",
        Format::Unsigned,
    ),
    (
        "wakeup_abort_count",
        "Wakeup sleep-transition aborts",
        Format::Unsigned,
    ),
    (
        "wakeup_expire_count",
        "Expired wakeup timeouts",
        Format::Unsigned,
    ),
    (
        "wakeup_active",
        "Wakeup event processing",
        B("Active", "Inactive"),
    ),
    (
        "wakeup_total_time_ms",
        "Total wakeup processing time",
        Format::UnsignedScale(1.0, "ms"),
    ),
];
const CACHE: &[Spec] = &[
    ("level", "Cache level", I),
    ("type", "Cache type", T),
    ("size", "Cache size", Format::CacheBytes),
    ("ways_of_associativity", "Associativity", I),
    ("number_of_sets", "Sets", I),
    ("coherency_line_size", "Line size", S(1.0, "bytes")),
    ("shared_cpu_list", "Shared CPUs", C),
];

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unknown_subsystems_do_not_inherit_schemas_from_path_names() {
        for component in [
            "cpu",
            "cpufreq",
            "net",
            "hwmon",
            "thermal",
            "power_supply",
            "input",
            "video4linux",
            "block",
            "nvme",
            "module",
            "drivers",
        ] {
            let path = format!("/sys/devices/mystery/{component}/leaf");
            let context = Context::from_parts(Path::new(&path), Some("unrecognized"));
            assert_eq!(context.family, Family::Unknown, "{path}");
            assert_eq!(context.label(), None);
            assert!(rules(&context).is_empty());
        }
        let cpu = Context::from_parts(Path::new("/sys/devices/processor0"), Some("cpu"));
        assert_eq!(cpu.family, Family::Cpu);
        assert!(rule(&cpu, "online").is_some());
    }

    #[test]
    fn unknown_power_groups_do_not_request_runtime_getters() {
        let unknown = Context::from_parts(Path::new("/sys/devices/mystery/power"), None);
        assert!(!unknown.is_runtime_power());
        assert!(rules(&unknown).is_empty());

        let (_temp, root, mut reads, _) = super::super::collector::registered_fixture(
            "unrecognized",
            "net",
            &[("ifindex", "7"), ("operstate", "up")],
        );
        for path in [root.clone(), root.join("power")] {
            std::fs::create_dir_all(&path).unwrap();
            let context = Context::discover(&path);
            assert_eq!(context.family, Family::Unknown);
            assert_eq!(context.direct, path == root);
            assert_eq!(context.label(), None);
            for rule in rules(&context) {
                reads.fact(&path, &rule);
            }
        }
        assert_eq!(reads.attempts().count(), 0);
    }

    #[test]
    fn global_root_aliases_keep_identity_scoped_schemas() {
        let temp = tempfile::tempdir().unwrap();
        for (name, source, family) in [
            ("processors", "/sys/devices/system/cpu", Family::Cpu),
            ("loaded", "/sys/module", Family::Module),
        ] {
            let source = Path::new(source);
            if !source.is_dir() {
                // An unmounted/unavailable global hierarchy cannot authorize a schema.
                assert_eq!(fallback_family(source), Family::Unknown);
                continue;
            }
            let alias = temp.path().join(name);
            std::os::unix::fs::symlink(source, &alias).unwrap();
            let context = Context::discover(&alias);
            assert_eq!(context.family, family);
            if family == Family::Cpu {
                let online = rule(&context, "online").unwrap();
                assert!(matches!(
                    parse(&online, "0-3").unwrap().0,
                    Value::CpuList(_)
                ));
            } else {
                assert!(
                    rules(&context).is_empty(),
                    "module inventory is not a module object"
                );
                let module = Context::from_parts(&alias.join("example"), None);
                assert!(rule(&module, "version").is_some());
                let nested = Context::from_parts(&alias.join("example/parameters"), None);
                assert!(rules(&nested).is_empty());
            }
        }
    }

    #[test]
    fn unregistered_familiar_names_do_not_select_global_or_device_schemas() {
        let temp = tempfile::tempdir().unwrap();
        for component in [
            "cpu", "cpufreq", "module", "drivers", "hwmon", "net", "block",
        ] {
            let path = temp.path().join(component).join("device0");
            std::fs::create_dir_all(path.join("power")).unwrap();
            for path in [&path, &path.join("power")] {
                let context = Context::discover(path);
                assert_eq!(context.family, Family::Unknown, "{}", path.display());
                assert!(rules(&context).is_empty());
                assert!(rule(&context, "temp1_input").is_none());
            }
        }
    }

    #[test]
    fn inherited_schemas_require_the_documented_owner_relative_group() {
        for (subsystem, attribute) in [
            ("net", "ifindex"),
            ("hwmon", "temp1_input"),
            ("block", "logical_block_size"),
        ] {
            let (_temp, root, mut reads, _) =
                super::super::collector::registered_fixture(subsystem, "device0", &[]);
            for group in ["unrelated", "unrelated/power", "unrelated/queue"] {
                let path = root.join(group);
                std::fs::create_dir_all(&path).unwrap();
                let context = Context::discover(&path);
                assert!(rules(&context).is_empty(), "{subsystem}: {group}");
                assert!(rule(&context, attribute).is_none(), "{subsystem}: {group}");
                for rule in rules(&context) {
                    reads.fact(&path, &rule);
                }
            }
            assert_eq!(reads.attempts().count(), 0);
            let power = root.join("power");
            std::fs::create_dir(&power).unwrap();
            assert!(rule(&Context::discover(&power), "runtime_status").is_some());
            if subsystem == "block" {
                assert!(rule(&Context::discover(&root), "ro").is_some());
                let queue = root.join("queue");
                std::fs::create_dir(&queue).unwrap();
                let context = Context::discover(&queue);
                assert!(rule(&context, "logical_block_size").is_some());
                assert!(rule(&context, "ro").is_none());
            }
        }
    }

    #[test]
    fn runtime_power_accounting_is_scoped_typed_and_not_an_event_count() {
        let mut context =
            Context::from_parts(Path::new("/sys/devices/virtual/net/lo/power"), Some("net"));
        context.direct = false;
        context.relative = "power".into();
        assert_eq!(context.label(), Some("Device power management"));
        for name in [
            "runtime_active_time",
            "runtime_suspended_time",
            "wakeup_total_time_ms",
        ] {
            let rule = rule(&context, name).unwrap();
            assert_eq!(
                parse(&rule, "1234").unwrap(),
                (
                    Value::Measurement {
                        value: 1234.0,
                        unit: "ms".into()
                    },
                    "1234 ms".into()
                )
            );
            for invalid in ["-1", "", "1.5"] {
                assert!(parse(&rule, invalid).is_err());
            }
        }
        for name in [
            "wakeup_count",
            "wakeup_active_count",
            "wakeup_abort_count",
            "wakeup_expire_count",
            "runtime_active_kids",
        ] {
            let rule = rule(&context, name).unwrap();
            assert_eq!(parse(&rule, "0").unwrap().0, Value::Integer(0));
            assert!(parse(&rule, "-1").is_err());
            assert!(
                parse(&rule, "").is_err(),
                "empty disabled wakeup counters must not become zero"
            );
        }
        let delay = rule(&context, "autosuspend_delay_ms").unwrap();
        assert_eq!(parse(&delay, "-1").unwrap().1, "-1 ms");
        let active = rule(&context, "wakeup_active").unwrap();
        assert_eq!(parse(&active, "1").unwrap().0, Value::Boolean(true));
        assert!(parse(&active, "2").is_err());
        context.direct = true;
        assert_eq!(context.label(), Some("Network interface"));
        assert!(rule(&context, "runtime_active_time").is_none());
        let module = Context::from_parts(Path::new("/sys/module/power"), None);
        assert_eq!(module.label(), Some("Kernel module"));
        assert!(rule(&module, "runtime_active_time").is_none());
    }
    fn formatted(family: Family, path: &str, name: &str, raw: &str) -> String {
        let context = Context {
            family,
            direct: true,
            leaf: Path::new(path)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into(),
            path: path.into(),
            relative: PathBuf::new(),
        };
        parse(&rule(&context, name).unwrap(), raw).unwrap().1
    }
    #[test]
    fn nvme_passive_rules_exclude_secrets_controls_and_identify_triggering_reads() {
        let controller = Context::from_parts(Path::new("/sys/class/nvme/nvme0"), Some("nvme"));
        assert_eq!(controller.family, Family::Nvme);
        assert!(rule(&controller, "model").is_some());
        assert!(rule(&controller, "state").is_some());
        for name in [
            "dhchap_secret",
            "dhchap_ctrl_secret",
            "reset_controller",
            "rescan_controller",
            "delete_controller",
            "tls_key",
        ] {
            assert!(
                rule(&controller, name).is_none(),
                "{name} must remain metadata-only"
            );
        }
        let namespace = Context::from_parts(Path::new("/sys/block/nvme0n1"), Some("block"));
        assert!(rule(&namespace, "nsid").is_some());
        assert!(
            rule(&namespace, "nuse").is_none(),
            "nuse triggers an Identify Namespace command"
        );
    }
    #[test]
    fn temperature_units_depend_on_subsystem_and_attribute() {
        assert_eq!(
            formatted(
                Family::PowerSupply,
                "/sys/class/power_supply/BAT0",
                "temp",
                "475"
            ),
            "47.5 °C"
        );
        assert_eq!(
            formatted(
                Family::Thermal,
                "/sys/class/thermal/thermal_zone0",
                "temp",
                "47500"
            ),
            "47.5 °C"
        );
        assert_eq!(
            formatted(
                Family::Hwmon,
                "/sys/class/hwmon/hwmon0",
                "temp1_input",
                "-12500"
            ),
            "-12.5 °C"
        );
        let unknown = Context::from_parts(Path::new("/sys/devices/unknown"), None);
        assert!(rule(&unknown, "temp").is_none());
        assert!(channel_rule("unknown1_label").is_none());
        assert!(channel_rule("unknown1_alarm").is_none());
    }
    #[test]
    fn charge_energy_and_small_measurements_keep_their_units() {
        assert_eq!(
            formatted(
                Family::PowerSupply,
                "/sys/class/power_supply/BAT0",
                "energy_now",
                "42000000"
            ),
            "42 Wh"
        );
        assert_eq!(
            formatted(
                Family::PowerSupply,
                "/sys/class/power_supply/BAT0",
                "charge_now",
                "4200000"
            ),
            "4.2 Ah"
        );
        assert_eq!(
            formatted(
                Family::PowerSupply,
                "/sys/class/power_supply/BAT0",
                "current_now",
                "1"
            ),
            "0.000001 A"
        );
    }
    #[test]
    fn block_and_cpu_attributes_format_without_size_guesses() {
        assert_eq!(
            formatted(Family::Block, "/sys/class/block/test0", "size", "2048"),
            "1.0 MiB"
        );
        assert_eq!(
            formatted(
                Family::Cpu,
                "/sys/devices/system/cpu/cpufreq/policy0",
                "scaling_cur_freq",
                "4200000"
            ),
            "4.2 GHz"
        );
        assert_eq!(
            formatted(Family::Cpu, "/sys/devices/system/cpu", "online", "0-3,8-11"),
            "8 logical CPUs · 0-3,8-11"
        );
        assert_eq!(
            formatted(Family::Cpu, "/sys/devices/system/cpu/cpu6", "online", "0"),
            "No"
        );
    }
    #[test]
    fn nested_containers_do_not_inherit_unrelated_attribute_names() {
        let mut queue = Context::from_parts(
            Path::new("/sys/devices/virtual/block/test0/queue"),
            Some("block"),
        );
        queue.direct = false;
        queue.relative = "queue".into();
        assert!(rule(&queue, "ro").is_none());
        assert!(rule(&queue, "logical_block_size").is_some());
        let parameters = Context::from_parts(Path::new("/sys/module/hwmon/parameters"), None);
        assert_eq!(parameters.family, Family::Module);
        assert!(rule(&parameters, "temp1_input").is_none());
    }
    #[test]
    fn pci_payloads_and_display_framebuffers_are_not_passive_attributes() {
        let pci = Context::from_parts(
            Path::new("/sys/devices/pci0000:00/0000:00:01.0"),
            Some("pci"),
        );
        assert!(rule(&pci, "resource").is_some());
        for attribute in ["config", "resource0", "resource1", "rom", "reset", "remove"] {
            assert!(rule(&pci, attribute).is_none(), "{attribute}");
        }
        let drm = Context::from_parts(Path::new("/sys/class/drm/card0-DP-1"), Some("drm"));
        assert!(rule(&drm, "modes").is_some());
        for attribute in ["framebuffer", "force", "edid_override"] {
            assert!(rule(&drm, attribute).is_none(), "{attribute}");
        }
    }
    #[test]
    fn kernel_cache_kibibytes_are_formatted_without_unit_guessing() {
        assert_eq!(
            formatted(
                Family::Cpu,
                "/sys/devices/system/cpu/cpu0/cache/index3",
                "size",
                "32768K"
            ),
            "32.0 MiB"
        );
        let context = Context::from_parts(
            Path::new("/sys/devices/system/cpu/cpu0/cache/index3"),
            Some("cpu"),
        );
        let rule = rule(&context, "size").unwrap();
        assert!(parse(&rule, "32M").is_err());
        assert!(parse(&rule, "18446744073709551615K").is_err());
    }
    #[test]
    fn thermal_trip_registry_is_scoped_and_excludes_emulation_controls() {
        let zone = Context::from_parts(
            Path::new("/sys/class/thermal/thermal_zone0"),
            Some("thermal"),
        );
        assert_eq!(
            parse(&rule(&zone, "trip_point_0_temp").unwrap(), "80000")
                .unwrap()
                .1,
            "80 °C"
        );
        assert!(rule(&zone, "trip_point_0_type").is_some());
        assert!(rule(&zone, "cdev0_trip_point").is_some());
        for name in [
            "emul_temp",
            "trip_point_bad_temp",
            "trip_point_0_unknown",
            "cdevx_trip_point",
        ] {
            assert!(rule(&zone, name).is_none(), "{name}");
        }
        let cooling = Context::from_parts(
            Path::new("/sys/class/thermal/cooling_device0"),
            Some("thermal"),
        );
        assert!(rule(&cooling, "trip_point_0_temp").is_none());
    }
    #[test]
    fn supported_value_selections_preserve_options_and_reject_ambiguous_markers() {
        let rule = Rule::new("queue/scheduler", "I/O scheduler", Format::Selection);
        let (value, display) = parse(&rule, "none mq-deadline [kyber] bfq\n").unwrap();
        assert_eq!(
            value,
            Value::Selection {
                selected: Some("kyber".into()),
                available: vec![
                    "none".into(),
                    "mq-deadline".into(),
                    "kyber".into(),
                    "bfq".into()
                ],
            }
        );
        assert_eq!(
            display,
            "kyber · available: none / mq-deadline / kyber / bfq"
        );
        assert_eq!(
            parse(&rule, "simple_ondemand performance").unwrap().0,
            Value::Selection {
                selected: None,
                available: vec!["simple_ondemand".into(), "performance".into()]
            }
        );
        for raw in [
            "[none] [kyber]",
            "none [kyber",
            "none kyber]",
            "[]",
            "[[kyber]]",
            "no[ne]",
            "[kyber]tail",
        ] {
            let fact = super::super::model::Fact::attribute(
                &rule.label,
                "/fixture/queue/scheduler".into(),
                "documented sysfs attribute",
                Ok(raw.into()),
                |value| parse(&rule, value),
            );
            assert_eq!(
                fact.outcome,
                super::super::model::Outcome::Malformed,
                "{raw}"
            );
            assert_eq!(fact.raw.as_deref(), Some(raw));
            assert!(fact.semantic.is_none());
            assert!(fact.value.is_none());
        }
    }

    #[test]
    fn malformed_lists_and_booleans_do_not_become_empty_or_false() {
        assert!(cpu_list("0-3,,8").is_err());
        assert!(cpu_list("8-3").is_err());
        assert!(cpu_list("0-4294967295").is_err());
        assert_eq!(cpu_list("0-3,2-5,8").unwrap(), vec![(0, 5), (8, 8)]);
        let rule = Rule::new("online", "Online", Format::Boolean("Yes", "No"));
        assert!(parse(&rule, "unknown").is_err());
        assert!(parse(&rule, "-1").is_err());
    }
    #[test]
    fn negative_network_speed_is_unknown_and_capabilities_are_not_active_state() {
        assert_eq!(
            formatted(Family::Net, "/sys/class/net/test0", "speed", "-1"),
            "Unknown"
        );
        assert!(
            rules(&Context::from_parts(
                Path::new("/sys/class/net/test0"),
                Some("net")
            ))
            .iter()
            .any(|r| r.label == "Negotiated speed")
        );
    }
}
