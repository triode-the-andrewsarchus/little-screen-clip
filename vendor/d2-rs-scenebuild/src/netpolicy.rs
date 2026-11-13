// SPDX-License-Identifier: MPL-2.0
//! D2 master lib/netpolicy: public destinations and the validated dial boundary.
//! Resolver/dial injection is explicit; production HTTP uses the system resolver.
use d2_rs_core::CancellationToken;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;

/// netpolicy.isLocalHostname.
pub fn is_local_hostname(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    [
        "localhost",
        "metadata.google.internal",
        "metadata.goog",
        "metadata.azure.internal",
        "instance-data.ec2.internal",
    ]
    .iter()
    .any(|name| host == *name || host.ends_with(&format!(".{name}")))
}

fn v4_in(address: Ipv4Addr, prefix: [u8; 4], bits: u32) -> bool {
    let mask = if bits == 0 {
        0
    } else {
        u32::MAX << (32 - bits)
    };
    u32::from(address) & mask == u32::from(Ipv4Addr::from(prefix)) & mask
}

fn v6_in(address: Ipv6Addr, prefix: &str, bits: u32) -> bool {
    let prefix: Ipv6Addr = prefix.parse().expect("netpolicy prefix");
    let mask = if bits == 0 {
        0
    } else {
        u128::MAX << (128 - bits)
    };
    u128::from(address) & mask == u128::from(prefix) & mask
}

/// netpolicy.isPublicAddress over an Unmap()ped netip.Addr: Go's
/// IsGlobalUnicast/IsPrivate/IsLinkLocal*/IsMulticast predicates followed by
/// the nonPublicPrefixes table.
pub fn is_public_address(address: IpAddr) -> bool {
    let address = match address {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4),
        v4 => v4,
    };
    match address {
        IpAddr::V4(v4) => {
            let non_public: &[([u8; 4], u32)] = &[
                // IsGlobalUnicast/IsUnspecified/IsLoopback/IsPrivate/
                // IsLinkLocal*/IsMulticast and the broadcast address.
                ([0, 0, 0, 0], 32),
                ([255, 255, 255, 255], 32),
                ([127, 0, 0, 0], 8),
                ([10, 0, 0, 0], 8),
                ([172, 16, 0, 0], 12),
                ([192, 168, 0, 0], 16),
                ([169, 254, 0, 0], 16),
                ([224, 0, 0, 0], 4),
                // nonPublicPrefixes
                ([0, 0, 0, 0], 8),
                ([100, 64, 0, 0], 10),
                ([168, 63, 129, 16], 32),
                ([192, 0, 0, 0], 24),
                ([192, 0, 2, 0], 24),
                ([192, 88, 99, 0], 24),
                ([198, 18, 0, 0], 15),
                ([198, 51, 100, 0], 24),
                ([203, 0, 113, 0], 24),
                ([240, 0, 0, 0], 4),
            ];
            !non_public
                .iter()
                .any(|(prefix, bits)| v4_in(v4, *prefix, *bits))
        }
        IpAddr::V6(v6) => {
            let non_public: &[(&str, u32)] = &[
                // IsGlobalUnicast/IsUnspecified/IsLoopback/IsPrivate/
                // IsLinkLocal*/IsMulticast.
                ("::", 128),
                ("::1", 128),
                ("fc00::", 7),
                ("fe80::", 10),
                ("ff00::", 8),
                // nonPublicPrefixes
                ("::", 96),
                ("64:ff9b::", 96),
                ("64:ff9b:1::", 48),
                ("100::", 64),
                ("2001::", 23),
                ("2001:db8::", 32),
                ("2002::", 16),
                ("3fff::", 20),
                ("5f00::", 16),
                ("fec0::", 10),
            ];
            !non_public
                .iter()
                .any(|(prefix, bits)| v6_in(v6, prefix, *bits))
        }
    }
}

/// The netpolicy dial boundary refused a non-public destination.
#[derive(Debug)]
pub struct BlockedAddress;
impl std::fmt::Display for BlockedAddress {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("network policy blocked a non-public address")
    }
}
impl std::error::Error for BlockedAddress {}

/// Literal-address and local-name half of netpolicy.resolvePublicAddresses.
/// Hyper dials IP literals without consulting the resolver, so the initial
/// request and every redirect target are checked here.
pub fn host_is_blocked(url: &reqwest::Url) -> bool {
    let Some(host) = url.host_str() else {
        return true;
    };
    let literal = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    match literal.parse::<IpAddr>() {
        Ok(address) => !is_public_address(address),
        Err(_) => is_local_hostname(host.trim_end_matches('.')),
    }
}

/// Complete-answer validation from resolvePublicAddresses. Preserve answer order,
/// unmap IPv4 addresses and reject the whole resolution before any connection.
pub fn validate_public_addresses(
    addresses: impl IntoIterator<Item = IpAddr>,
) -> Result<Vec<IpAddr>, String> {
    let mut unique = Vec::new();
    for address in addresses {
        let address = match address {
            IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4),
            v4 => v4,
        };
        if !is_public_address(address) {
            return Err(BlockedAddress.to_string());
        }
        if !unique.contains(&address) {
            unique.push(address);
        }
    }
    if unique.is_empty() {
        return Err("network policy: destination resolved to no addresses".into());
    }
    Ok(unique)
}

/// Native resolvePublicAddresses with an explicit resolver for caller-owned I/O.
pub fn resolve_public_addresses(
    network: &str,
    host: &str,
    mut lookup: impl FnMut(&str, &str) -> Result<Vec<IpAddr>, String>,
) -> Result<Vec<IpAddr>, String> {
    let host = host.strip_suffix('.').unwrap_or(host);
    if is_local_hostname(host) {
        return Err(BlockedAddress.to_string());
    }
    let addresses = if let Ok(literal) = host.parse::<IpAddr>() {
        vec![literal]
    } else {
        let lookup_network = if network.ends_with('4') {
            "ip4"
        } else if network.ends_with('6') {
            "ip6"
        } else {
            "ip"
        };
        lookup(lookup_network, host)?
    };
    validate_public_addresses(addresses)
}

/// Native validatedDialContext. Only validated numeric addresses reach dial;
/// every invocation resolves again and cancellation wins after a failed dial.
pub fn validated_dial<T>(
    network: &str,
    host: &str,
    port: &str,
    cancel: Option<&CancellationToken>,
    lookup: impl FnMut(&str, &str) -> Result<Vec<IpAddr>, String>,
    mut dial: impl FnMut(&str, &str) -> Result<T, String>,
) -> Result<T, String> {
    let addresses = resolve_public_addresses(network, host, lookup)?;
    let mut errors = Vec::new();
    for address in addresses {
        let destination = match address {
            IpAddr::V4(_) => format!("{address}:{port}"),
            IpAddr::V6(_) => format!("[{address}]:{port}"),
        };
        match dial(network, &destination) {
            Ok(connection) => return Ok(connection),
            Err(error) => errors.push(error),
        }
        if let Some(cause) = cancel.and_then(CancellationToken::error) {
            return Err(cause.to_string());
        }
    }
    Err(errors.join("\n"))
}

/// Each connection receives a fresh complete-answer check. Hyper receives only
/// validated numeric addresses; proxies and caller hooks are disabled upstream.
pub struct PublicResolver;
impl reqwest::dns::Resolve for PublicResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name
            .as_str()
            .strip_suffix('.')
            .unwrap_or(name.as_str())
            .to_owned();
        Box::pin(async move {
            if is_local_hostname(&host) {
                return Err(Box::new(BlockedAddress) as Box<dyn std::error::Error + Send + Sync>);
            }
            let answers = tokio::net::lookup_host((host.as_str(), 0))
                .await
                .map_err(|error| Box::new(error) as Box<dyn std::error::Error + Send + Sync>)?;
            let unique = validate_public_addresses(answers.map(|address| address.ip())).map_err(
                |error| {
                    if error == BlockedAddress.to_string() {
                        Box::new(BlockedAddress) as Box<dyn std::error::Error + Send + Sync>
                    } else {
                        error.into()
                    }
                },
            )?;
            Ok(Box::new(
                unique
                    .into_iter()
                    .map(|address| SocketAddr::new(address, 0)),
            ) as reqwest::dns::Addrs)
        })
    }
}

/// Build the public-only half of newHTTPClient without exposing bypass hooks.
pub fn public_client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .no_proxy()
        .dns_resolver(Arc::new(PublicResolver))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_answers_unmap_deduplicate_and_reject_before_dial() {
        let answers = ["8.8.8.8", "::ffff:8.8.8.8", "1.1.1.1"].map(|a| a.parse().unwrap());
        assert_eq!(
            validate_public_addresses(answers).unwrap(),
            vec![
                "8.8.8.8".parse::<IpAddr>().unwrap(),
                "1.1.1.1".parse().unwrap()
            ]
        );
        let result: Result<(), String> = validated_dial(
            "tcp",
            "assets.example",
            "443",
            None,
            |_, _| {
                Ok(vec![
                    "8.8.8.8".parse().unwrap(),
                    "127.0.0.1".parse().unwrap(),
                ])
            },
            |_, _| panic!("mixed answers must never dial"),
        );
        assert_eq!(result.unwrap_err(), BlockedAddress.to_string());
    }

    #[test]
    fn lookup_family_local_names_literals_and_empty_answers() {
        for (network, family) in [("tcp", "ip"), ("tcp4", "ip4"), ("tcp6", "ip6")] {
            resolve_public_addresses(network, "assets.example.", |got, host| {
                assert_eq!((got, host), (family, "assets.example"));
                Ok(vec!["8.8.8.8".parse().unwrap()])
            })
            .unwrap();
        }
        for name in ["LOCALHOST", "x.metadata.goog", "localhost."] {
            assert_eq!(
                resolve_public_addresses("tcp", name, |_, _| panic!("local name must not resolve"))
                    .unwrap_err(),
                BlockedAddress.to_string()
            );
        }
        assert_eq!(
            resolve_public_addresses("tcp", "8.8.8.8.", |_, _| panic!("literal must not resolve"))
                .unwrap(),
            vec!["8.8.8.8".parse::<IpAddr>().unwrap()]
        );
        assert_eq!(
            resolve_public_addresses("tcp", "assets.example", |_, _| Ok(vec![])).unwrap_err(),
            "network policy: destination resolved to no addresses"
        );
    }

    #[test]
    fn dial_preserves_order_joined_errors_success_and_cancellation() {
        let answers = || {
            vec![
                "8.8.8.8".parse().unwrap(),
                "2606:4700:4700::1111".parse().unwrap(),
            ]
        };
        let mut dials = Vec::new();
        let result: Result<(), String> = validated_dial(
            "tcp6",
            "assets.example",
            "443",
            None,
            |_, _| Ok(answers()),
            |network, address| {
                assert_eq!(network, "tcp6");
                dials.push(address.to_owned());
                Err(format!("failed {address}"))
            },
        );
        assert_eq!(dials, vec!["8.8.8.8:443", "[2606:4700:4700::1111]:443"]);
        assert_eq!(
            result.unwrap_err(),
            "failed 8.8.8.8:443\nfailed [2606:4700:4700::1111]:443"
        );
        let token = CancellationToken::new();
        let result: Result<(), String> = validated_dial(
            "tcp",
            "assets.example",
            "80",
            Some(&token),
            |_, _| Ok(answers()),
            |_, _| {
                token.cancel();
                Err("dial failed".into())
            },
        );
        assert_eq!(result.unwrap_err(), "context canceled");
        assert_eq!(
            validated_dial(
                "tcp",
                "8.8.8.8",
                "80",
                None,
                |_, _| panic!("literal"),
                |_, _| Ok(42)
            )
            .unwrap(),
            42
        );
    }
}
