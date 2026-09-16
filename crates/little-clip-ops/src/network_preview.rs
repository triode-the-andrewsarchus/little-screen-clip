//! Network entity detection and metadata enrichment.
//!
//! Identifies IPv4, IPv6, and MAC addresses in text, classifies IP addresses according to RFC
//! specifications (RFC 1918, RFC 1122, RFC 6598, etc.), identifies well-known public DNS resolvers,
//! performs OUI hardware vendor lookups for MAC addresses, and runs non-blocking ICMP ping reachability checks.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::str::FromStr;
use std::sync::{LazyLock, RwLock};
use std::time::{Duration, Instant};

/// Structured network entity detected from clipboard text.
#[derive(Debug, Clone, PartialEq)]
pub enum NetworkPreview {
    /// An IPv4 or IPv6 network address.
    Ip {
        /// Parsed IP address.
        ip: IpAddr,
        /// RFC classification string (e.g. "RFC 1918 Private Network (192.168.0.0/16)").
        rfc_class: String,
        /// Detailed description or well-known service name if applicable.
        description: Option<String>,
        /// Background ICMP ping reachability status if checked.
        ping_status: Option<String>,
    },
    /// An IEEE 802 MAC hardware address.
    Mac {
        /// Standard colon-delimited uppercase MAC string (`XX:XX:XX:XX:XX:XX`).
        formatted: String,
        /// Identified manufacturer or OUI classification.
        vendor: String,
        /// MAC address type (e.g. UAA, Locally Administered, Multicast).
        mac_type: String,
    },
}

impl NetworkPreview {
    /// Formats the network entity into human-readable detail lines with emoji icons.
    pub fn summary_lines(&self) -> Vec<String> {
        match self {
            NetworkPreview::Ip {
                ip,
                rfc_class,
                description,
                ping_status,
            } => {
                let mut lines = Vec::new();
                lines.push(format!("🌐 IP: {}", ip));
                lines.push(format!("📋 {}", rfc_class));
                if let Some(desc) = description {
                    lines.push(format!("ℹ️ {}", desc));
                }
                if let Some(ping) = ping_status {
                    lines.push(format!("⚡ Ping: {}", ping));
                }
                lines
            }
            NetworkPreview::Mac {
                formatted,
                vendor,
                mac_type,
            } => {
                vec![
                    format!("🔌 MAC: {}", formatted),
                    format!("🏷 Vendor: {}", vendor),
                    format!("🔒 Type: {}", mac_type),
                ]
            }
        }
    }
}

// Thread-safe cache for bounded ping results: IP -> (status, timestamp)
static PING_CACHE: LazyLock<RwLock<HashMap<IpAddr, (String, Instant)>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Asynchronously initiates a 1-second ICMP ping and returns cached result if available.
pub fn check_bounded_ping(ip: IpAddr) -> Option<String> {
    let now = Instant::now();
    // 1. Check cache (valid for 60s)
    if let Ok(cache) = PING_CACHE.read() {
        if let Some((status, ts)) = cache.get(&ip) {
            if now.duration_since(*ts) < Duration::from_secs(60) {
                return Some(status.clone());
            }
        }
    }

    // 2. Spawn background single-shot ping
    let ip_clone = ip;
    std::thread::spawn(move || {
        let cmd_res = std::process::Command::new("ping")
            .arg("-c")
            .arg("1")
            .arg("-W")
            .arg("1")
            .arg(ip_clone.to_string())
            .output();

        let status = match cmd_res {
            Ok(output) if output.status.success() => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                // Extract "time=X.X ms"
                if let Some(t_idx) = stdout.find("time=") {
                    let part = &stdout[t_idx + 5..];
                    let ms_str = part.split_whitespace().next().unwrap_or("ok");
                    format!("Online ({} ms)", ms_str)
                } else {
                    "Online (<1ms)".to_string()
                }
            }
            _ => "Unreachable / No Reply".to_string(),
        };

        if let Ok(mut cache) = PING_CACHE.write() {
            cache.insert(ip_clone, (status, Instant::now()));
        }
    });

    if let Ok(cache) = PING_CACHE.read() {
        cache.get(&ip).map(|(s, _)| s.clone())
    } else {
        None
    }
}

/// Classifies an IPv4 address according to IETF RFC subnet specifications.
pub fn classify_ipv4(ip: Ipv4Addr) -> (&'static str, Option<&'static str>) {
    let [a, b, c, _d] = ip.octets();

    if a == 127 {
        return (
            "RFC 1122 Loopback (localhost)",
            Some("Localhost inter-process loopback"),
        );
    }
    if a == 10 {
        return (
            "RFC 1918 Private Network (10.0.0.0/8)",
            Some("Local Area Network / Private Intranet"),
        );
    }
    if a == 172 && (16..=31).contains(&b) {
        return (
            "RFC 1918 Private Network (172.16.0.0/12)",
            Some("Local Area Network / Docker default"),
        );
    }
    if a == 192 && b == 168 {
        return (
            "RFC 1918 Private Network (192.168.0.0/16)",
            Some("Home / Office Router LAN subnet"),
        );
    }
    if a == 169 && b == 254 {
        return (
            "RFC 3927 Link-Local (APIPA autoconfig)",
            Some("Self-assigned IP without DHCP server"),
        );
    }
    if a == 100 && (64..=127).contains(&b) {
        return (
            "RFC 6598 Shared Carrier-Grade NAT (CGNAT)",
            Some("ISP Carrier-Grade NAT pool"),
        );
    }
    if a == 192 && b == 0 && c == 2
        || a == 198 && b == 51 && c == 100
        || a == 203 && b == 0 && c == 113
    {
        return (
            "RFC 5737 Documentation Network",
            Some("Reserved for example docs & documentation"),
        );
    }
    if (224..=239).contains(&a) {
        return (
            "RFC 1112 Multicast Address",
            Some("Multicast group broadcast"),
        );
    }
    if a >= 240 {
        if a == 255 && b == 255 && c == 255 {
            return (
                "Limited Broadcast (255.255.255.255)",
                Some("Local subnet broadcast"),
            );
        }
        return ("RFC 1112 Reserved / Future Use", None);
    }
    if a == 0 {
        return (
            "RFC 1122 Current Network (0.0.0.0/8)",
            Some("Unspecified / default bind route"),
        );
    }

    // Well-known public IPs
    match (a, b, c, _d) {
        (8, 8, 8, 8) | (8, 8, 4, 4) => (
            "Public Internet (Google Public DNS)",
            Some("Google Anycast Resolver"),
        ),
        (1, 1, 1, 1) | (1, 0, 0, 1) => (
            "Public Internet (Cloudflare DNS)",
            Some("Cloudflare Privacy-First DNS"),
        ),
        (9, 9, 9, 9) => (
            "Public Internet (Quad9 DNS)",
            Some("Quad9 Security-Blocking DNS"),
        ),
        (208, 67, 222, 222) | (208, 67, 220, 220) => {
            ("Public Internet (Cisco OpenDNS)", Some("OpenDNS Resolver"))
        }
        _ => ("Public Internet (Unicast)", None),
    }
}

/// Classifies an IPv6 address according to IETF RFC subnet specifications.
pub fn classify_ipv6(ip: Ipv6Addr) -> (&'static str, Option<&'static str>) {
    if ip.is_loopback() {
        return ("RFC 4291 IPv6 Loopback (::1)", Some("Localhost loopback"));
    }
    if ip.is_unspecified() {
        return (
            "RFC 4291 IPv6 Unspecified (::)",
            Some("Unspecified / bind-all"),
        );
    }
    let segments = ip.segments();
    if segments[0] & 0xffc0 == 0xfe80 {
        return (
            "RFC 4291 Link-Local IPv6",
            Some("Automatic router/neighbor discovery"),
        );
    }
    if segments[0] & 0xfe00 == 0xfc00 {
        return (
            "RFC 4193 Unique Local Address (ULA)",
            Some("IPv6 private intranet"),
        );
    }
    if segments[0] & 0xff00 == 0xff00 {
        return ("RFC 4291 IPv6 Multicast", None);
    }
    if segments[0] == 0x2001 && segments[1] == 0xdb8 {
        return (
            "RFC 3849 IPv6 Documentation",
            Some("Reserved for documentation"),
        );
    }

    ("Global Unicast IPv6", None)
}

/// Looks up the Organizationally Unique Identifier (OUI) hardware vendor name.
pub fn lookup_mac_vendor(oui: &[u8; 3]) -> &'static str {
    match oui {
        // Raspberry Pi
        [0xb8, 0x27, 0xeb] | [0xdc, 0xa6, 0x32] | [0xe4, 0x5f, 0x01] | [0x28, 0xcd, 0xc1] => {
            "Raspberry Pi Foundation"
        }
        // Espressif (ESP32/ESP8266 IoT)
        [0x24, 0x0a, 0xc4]
        | [0x30, 0xae, 0xa4]
        | [0x84, 0xf3, 0xeb]
        | [0xa4, 0xcf, 0x12]
        | [0xc4, 0x4f, 0x33] => "Espressif Systems (ESP32/ESP8266)",
        // Apple
        [0x00, 0x17, 0xf2]
        | [0xac, 0xde, 0x48]
        | [0xf0, 0x18, 0x98]
        | [0x3c, 0x06, 0x30]
        | [0xb8, 0x78, 0x26]
        | [0xf4, 0xd4, 0x88] => "Apple, Inc.",
        // Intel
        [0x00, 0x1b, 0x21] | [0x00, 0x15, 0x00] | [0x34, 0x13, 0xe8] | [0xa4, 0x4c, 0xc8] => {
            "Intel Corporate"
        }
        // Google
        [0x00, 0x1a, 0x11] | [0x3c, 0x5a, 0xb4] | [0xf4, 0xf5, 0xdb] | [0x54, 0x60, 0x09] => {
            "Google, LLC"
        }
        // Microsoft / Hyper-V
        [0x00, 0x15, 0x5d] | [0x00, 0x50, 0xf2] => "Microsoft Corporation (Hyper-V / Xbox)",
        // Cisco
        [0x00, 0x00, 0x0c] | [0x00, 0x01, 0x42] | [0x00, 0x40, 0x96] => "Cisco Systems",
        // Realtek
        [0x00, 0xe0, 0x4c] => "Realtek Semiconductor",
        // VMware
        [0x00, 0x05, 0x69] | [0x00, 0x0c, 0x29] | [0x00, 0x50, 0x56] => "VMware, Inc.",
        // QEMU / KVM virtual
        [0x52, 0x54, 0x00] => "QEMU / KVM Virtual NIC",
        // Dell
        [0x00, 0x14, 0x22] | [0x18, 0x66, 0xda] | [0xb8, 0x2a, 0x72] => "Dell Inc.",
        // Samsung
        [0x00, 0x07, 0xab] | [0x00, 0x12, 0x47] | [0x50, 0x01, 0xd9] => "Samsung Electronics",
        // Sony
        [0x00, 0x01, 0x4a] | [0x00, 0x04, 0x1f] => "Sony Corporation",
        // TP-Link
        [0x50, 0xc7, 0xbf] | [0xe8, 0x48, 0xb8] | [0xb0, 0x4e, 0x26] => "TP-Link Technologies",
        // Netgear
        [0x00, 0x09, 0x5b] | [0x00, 0x1e, 0x2a] | [0xc4, 0x04, 0x15] => "Netgear Inc.",
        // Ubiquiti Networks
        [0x78, 0x8a, 0x20] | [0x24, 0xa4, 0x3c] | [0xb4, 0xfb, 0xe4] => "Ubiquiti Networks",
        // Amazon
        [0xf0, 0xd2, 0xf1] | [0x44, 0x65, 0x0d] | [0x68, 0x54, 0x5a] => "Amazon Technologies",
        _ => "Standard IEEE OUI",
    }
}

/// Parses a string into 6 MAC bytes if formatted as colon/dash/dot delimited hex.
pub fn parse_mac_address(text: &str) -> Option<[u8; 6]> {
    let clean: String = text.chars().filter(|c| c.is_ascii_hexdigit()).collect();

    if clean.len() != 12 {
        return None;
    }

    // Verify format matches XX:XX:XX:XX:XX:XX, XX-XX-XX-XX-XX-XX, or XXXX.XXXX.XXXX
    let t = text.trim();
    let has_colons = t.split(':').count() == 6;
    let has_dashes = t.split('-').count() == 6;
    let has_dots = t.split('.').count() == 3;

    if !has_colons && !has_dashes && !has_dots && t.len() != 12 {
        return None;
    }

    let mut bytes = [0u8; 6];
    for i in 0..6 {
        bytes[i] = u8::from_str_radix(&clean[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(bytes)
}

/// Attempts to parse text into an enriched `NetworkPreview` (IP or MAC address).
pub fn detect_network_preview(text: &str) -> Option<NetworkPreview> {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.len() > 60 {
        return None;
    }

    // 1. IP Address
    if let Ok(ip) = IpAddr::from_str(trimmed) {
        let (rfc_class, desc) = match ip {
            IpAddr::V4(v4) => {
                let (c, d) = classify_ipv4(v4);
                (c.to_string(), d.map(|s| s.to_string()))
            }
            IpAddr::V6(v6) => {
                let (c, d) = classify_ipv6(v6);
                (c.to_string(), d.map(|s| s.to_string()))
            }
        };

        let ping_status = check_bounded_ping(ip);

        return Some(NetworkPreview::Ip {
            ip,
            rfc_class,
            description: desc,
            ping_status,
        });
    }

    // 2. MAC Address
    if let Some(bytes) = parse_mac_address(trimmed) {
        let formatted = format!(
            "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5]
        );

        let is_multicast = (bytes[0] & 0x01) != 0;
        let is_local = (bytes[0] & 0x02) != 0;

        let mac_type = if is_local {
            "Locally Administered (Private/Randomized MAC)".to_string()
        } else if is_multicast {
            "Multicast MAC Address".to_string()
        } else {
            "Universally Administered Unicast (UAA)".to_string()
        };

        let vendor = if is_local {
            "Randomized / Private".to_string()
        } else {
            lookup_mac_vendor(&[bytes[0], bytes[1], bytes[2]]).to_string()
        };

        return Some(NetworkPreview::Mac {
            formatted,
            vendor,
            mac_type,
        });
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_network_preview_ip() {
        let loopback = detect_network_preview("127.0.0.1").expect("127.0.0.1");
        match loopback {
            NetworkPreview::Ip { ip, rfc_class, .. } => {
                assert_eq!(ip, IpAddr::from_str("127.0.0.1").unwrap());
                assert!(rfc_class.contains("Loopback"));
            }
            _ => panic!("Expected IP preview"),
        }

        let private_ip = detect_network_preview("192.168.1.1").expect("192.168.1.1");
        match private_ip {
            NetworkPreview::Ip { rfc_class, .. } => {
                assert!(rfc_class.contains("RFC 1918 Private Network"));
            }
            _ => panic!("Expected IP preview"),
        }

        let google_dns = detect_network_preview("8.8.8.8").expect("8.8.8.8");
        match google_dns {
            NetworkPreview::Ip {
                rfc_class,
                description,
                ..
            } => {
                assert!(rfc_class.contains("Google Public DNS"));
                assert_eq!(description, Some("Google Anycast Resolver".to_string()));
            }
            _ => panic!("Expected IP preview"),
        }
    }

    #[test]
    fn test_network_preview_mac() {
        let rpi_mac = detect_network_preview("B8:27:EB:12:34:56").expect("Raspberry Pi MAC");
        match rpi_mac {
            NetworkPreview::Mac {
                formatted, vendor, ..
            } => {
                assert_eq!(formatted, "B8:27:EB:12:34:56");
                assert_eq!(vendor, "Raspberry Pi Foundation");
            }
            _ => panic!("Expected MAC preview"),
        }

        let local_mac = detect_network_preview("02:00:00:00:00:01").expect("Local MAC");
        match local_mac {
            NetworkPreview::Mac { mac_type, .. } => {
                assert!(mac_type.contains("Locally Administered"));
            }
            _ => panic!("Expected MAC preview"),
        }
    }
}
