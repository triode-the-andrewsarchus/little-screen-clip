// SPDX-License-Identifier: MPL-2.0
//! Native client ownership boundary from lib/netpolicy/netpolicy.go:48–125.
//! Rust's standard transport uses a builder callback for caller-specific hooks;
//! the public-only policy discards it and retains only explicit safe options.
use super::netpolicy::{
    BlockedAddress, PublicResolver, host_is_blocked, validate_public_addresses,
};
use d2_rs_core::CancellationToken;
use std::{future::Future, net::SocketAddr, pin::Pin, sync::Arc, time::Duration};

pub type HttpFuture<'a> =
    Pin<Box<dyn Future<Output = Result<reqwest::Response, String>> + Send + 'a>>;

/// Arbitrary caller transport. Public-only policy cannot constrain its sockets.
pub trait RoundTripper: Send + Sync {
    fn round_trip(&self, request: reqwest::Request) -> HttpFuture<'_>;
}

pub type ConfigureTransport =
    Arc<dyn Fn(reqwest::ClientBuilder) -> reqwest::ClientBuilder + Send + Sync>;
#[derive(Clone, Default)]
pub enum Transport {
    #[default]
    Default,
    /// Caller proxy, DNS, redirect and connector hooks. Ignored in public-only mode.
    Standard(ConfigureTransport),
    Custom(Arc<dyn RoundTripper>),
}

#[derive(Clone)]
pub struct HttpClientOptions {
    pub timeout: Duration,
    pub connect_timeout: Duration,
    pub redirect_limit: usize,
    pub transport: Transport,
}
impl Default for HttpClientOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(60),
            connect_timeout: Duration::from_secs(30),
            redirect_limit: 10,
            transport: Transport::Default,
        }
    }
}

#[derive(Clone, Copy, Default)]
pub struct Policy {
    pub allow_private_networks: bool,
}

/// The injectable I/O half is separate from the policy. Lookup answers are
/// validated in full before the numeric dial callback receives any address.
/// Production construction always uses the system resolver and normal sockets.
pub struct ClientIo {
    pub resolver: Arc<dyn reqwest::dns::Resolve>,
    pub dial: Arc<dyn Fn(SocketAddr) -> Result<SocketAddr, String> + Send + Sync>,
}
struct ValidatedResolver(ClientIo);
impl reqwest::dns::Resolve for ValidatedResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let lookup = self.0.resolver.resolve(name);
        let dial = self.0.dial.clone();
        Box::pin(async move {
            let addresses = validate_public_addresses(lookup.await?.map(|a| a.ip())).map_err(
                |error| -> Box<dyn std::error::Error + Send + Sync> {
                    if error == BlockedAddress.to_string() {
                        Box::new(BlockedAddress)
                    } else {
                        error.into()
                    }
                },
            )?;
            let mut targets = Vec::new();
            for ip in addresses {
                targets.push(dial(SocketAddr::new(ip, 0))?);
            }
            Ok(Box::new(targets.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

enum ClientTransport {
    Standard(reqwest::Client),
    Custom(Arc<dyn RoundTripper>),
}
pub struct HttpClient {
    transport: ClientTransport,
    policy: Policy,
}

pub fn new_http_client(
    base: Option<&HttpClientOptions>,
    policy: Policy,
) -> Result<HttpClient, String> {
    new_http_client_with_io(base, policy, None)
}

/// Deterministic resolver/dial injection, equivalent to upstream newHTTPClient.
/// The dial callback is policy-owned, never a retained caller transport hook.
pub fn new_http_client_with_io(
    base: Option<&HttpClientOptions>,
    policy: Policy,
    io: Option<ClientIo>,
) -> Result<HttpClient, String> {
    let options = base.cloned().unwrap_or_default();
    if let Transport::Custom(transport) = &options.transport {
        if !policy.allow_private_networks {
            return Err("public-only network policy requires a standard HTTP transport".into());
        }
        return Ok(HttpClient {
            transport: ClientTransport::Custom(transport.clone()),
            policy,
        });
    }
    let limit = options.redirect_limit;
    let mut builder = reqwest::Client::builder()
        .timeout(options.timeout)
        .connect_timeout(options.connect_timeout)
        .redirect(reqwest::redirect::Policy::custom(move |attempt| {
            if !policy.allow_private_networks && host_is_blocked(attempt.url()) {
                attempt.error(BlockedAddress)
            } else if attempt.previous().len() >= limit {
                attempt.error(format!("stopped after {limit} redirects"))
            } else {
                attempt.follow()
            }
        }));
    if policy.allow_private_networks {
        if let Transport::Standard(configure) = options.transport {
            builder = configure(builder);
        }
    } else {
        builder = builder.no_proxy();
        builder = if let Some(io) = io {
            builder.dns_resolver(Arc::new(ValidatedResolver(io)))
        } else {
            builder.dns_resolver(Arc::new(PublicResolver))
        };
    }
    Ok(HttpClient {
        transport: ClientTransport::Standard(builder.build().map_err(|e| e.to_string())?),
        policy,
    })
}

impl HttpClient {
    pub fn uses_custom_transport(&self, transport: &Arc<dyn RoundTripper>) -> bool {
        matches!(&self.transport, ClientTransport::Custom(current) if Arc::ptr_eq(current, transport))
    }

    pub async fn get(
        &self,
        url: &str,
        cancel: Option<&CancellationToken>,
    ) -> Result<reqwest::Response, String> {
        if let Some(error) = cancel.and_then(CancellationToken::error) {
            return Err(error.to_string());
        }
        let url = reqwest::Url::parse(url).map_err(|e| e.to_string())?;
        if !self.policy.allow_private_networks && host_is_blocked(&url) {
            return Err(BlockedAddress.to_string());
        }
        let request = reqwest::Request::new(reqwest::Method::GET, url);
        let request = async {
            match &self.transport {
                ClientTransport::Standard(client) => client.execute(request).await.map_err(|e| {
                    use std::error::Error;
                    let mut cause = e.source();
                    while let Some(inner) = cause {
                        if inner.downcast_ref::<BlockedAddress>().is_some() {
                            return BlockedAddress.to_string();
                        }
                        cause = inner.source();
                    }
                    e.to_string()
                }),
                ClientTransport::Custom(transport) => transport.round_trip(request).await,
            }
        };
        let cancelled = async {
            loop {
                if let Some(error) = cancel.and_then(CancellationToken::error) {
                    return error.to_string();
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        };
        let result = tokio::select! {
            result = request => result,
            error = cancelled => Err(error),
        };
        if let Some(error) = cancel.and_then(CancellationToken::error) {
            return Err(error.to_string());
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Custom(Arc<AtomicUsize>, bool);
    impl RoundTripper for Custom {
        fn round_trip(&self, _: reqwest::Request) -> HttpFuture<'_> {
            self.0.fetch_add(1, Ordering::SeqCst);
            let pending = self.1;
            Box::pin(async move {
                if pending {
                    std::future::pending::<()>().await;
                }
                Err("stopped by test".into())
            })
        }
    }
    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }
    #[test]
    fn private_opt_in_shares_and_invokes_custom_transport() {
        let calls = Arc::new(AtomicUsize::new(0));
        let transport: Arc<dyn RoundTripper> = Arc::new(Custom(calls.clone(), false));
        let options = HttpClientOptions {
            transport: Transport::Custom(transport.clone()),
            ..Default::default()
        };
        assert!(new_http_client(Some(&options), Policy::default()).is_err());
        let client = new_http_client(
            Some(&options),
            Policy {
                allow_private_networks: true,
            },
        )
        .unwrap();
        assert!(client.uses_custom_transport(&transport));
        assert_eq!(
            runtime()
                .block_on(client.get("http://127.0.0.1/", None))
                .unwrap_err(),
            "stopped by test"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(Arc::strong_count(&transport), 3);
    }
    struct MixedResolver;
    impl reqwest::dns::Resolve for MixedResolver {
        fn resolve(&self, _: reqwest::dns::Name) -> reqwest::dns::Resolving {
            Box::pin(async {
                Ok(Box::new(
                    [
                        "93.184.216.34:0".parse::<SocketAddr>().unwrap(),
                        "127.0.0.1:0".parse().unwrap(),
                    ]
                    .into_iter(),
                ) as reqwest::dns::Addrs)
            })
        }
    }
    #[test]
    fn mixed_answers_cannot_reach_numeric_dial_or_caller_hooks() {
        let calls = Arc::new(AtomicUsize::new(0));
        let hook_calls = calls.clone();
        let options = HttpClientOptions {
            transport: Transport::Standard(Arc::new(move |builder| {
                hook_calls.fetch_add(100, Ordering::SeqCst);
                builder
            })),
            ..Default::default()
        };
        let dial_calls = calls.clone();
        let io = ClientIo {
            resolver: Arc::new(MixedResolver),
            dial: Arc::new(move |address| {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(address)
            }),
        };
        let client = new_http_client_with_io(Some(&options), Policy::default(), Some(io)).unwrap();
        assert_eq!(
            runtime()
                .block_on(client.get("http://assets.example/", None))
                .unwrap_err(),
            BlockedAddress.to_string()
        );
        assert_eq!(dial_calls.load(Ordering::SeqCst), 0);
        // A retained callback increments the same counter by 100; either bypass fails.
        let Transport::Standard(callback) = &options.transport else {
            panic!()
        };
        assert_eq!(Arc::strong_count(callback), 1);
    }
    #[test]
    fn cancellation_interrupts_an_active_custom_request() {
        let calls = Arc::new(AtomicUsize::new(0));
        let options = HttpClientOptions {
            transport: Transport::Custom(Arc::new(Custom(calls.clone(), true))),
            ..Default::default()
        };
        let client = new_http_client(
            Some(&options),
            Policy {
                allow_private_networks: true,
            },
        )
        .unwrap();
        let cancel = CancellationToken::new();
        let signal = cancel.clone();
        let thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            signal.cancel();
        });
        assert_eq!(
            runtime()
                .block_on(client.get("http://127.0.0.1/", Some(&cancel)))
                .unwrap_err(),
            "context canceled"
        );
        thread.join().unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
