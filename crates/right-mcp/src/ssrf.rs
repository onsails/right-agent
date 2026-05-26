//! SSRF hardening helpers for `reqwest` clients that connect to URLs whose
//! host is supplied by user input (e.g. MCP detection/registration).
//!
//! All outbound HTTP from those code paths MUST use [`hardened_client_builder`]
//! so that:
//!
//! 1. DNS resolution rejects private/loopback/link-local addresses (the
//!    [`PublicNetworkResolver`]).
//! 2. Environment-based proxies are bypassed (`no_proxy`).
//! 3. Redirects are disabled (the server cannot 302 us into the internal
//!    network).
//!
//! Callers add their own timeouts on top of the returned builder.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::str::FromStr as _;
use std::sync::Arc;

/// Substring embedded in the error returned by [`PublicNetworkResolver`] when
/// every resolved address is in a private/loopback/link-local range. Callers
/// match on this marker to distinguish the SSRF-policy rejection from generic
/// DNS failures without echoing the rejected URL back to the user.
pub const PUBLIC_DNS_ERROR_MARKER: &str = "MCP detection DNS resolved to a non-public address";

/// `reqwest` DNS resolver that strips private/loopback/link-local addresses
/// from a hostname's resolved set. If nothing public remains, resolution
/// fails with [`PUBLIC_DNS_ERROR_MARKER`] in the error message.
#[derive(Debug, Clone, Copy)]
pub struct PublicNetworkResolver;

impl reqwest::dns::Resolve for PublicNetworkResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let addrs = tokio::net::lookup_host((host.as_str(), 0))
                .await
                .map_err(|error| Box::new(error) as Box<dyn std::error::Error + Send + Sync>)?;
            let public_addrs = addrs
                .filter(|addr| is_public_ip(addr.ip()))
                .collect::<Vec<_>>();
            if public_addrs.is_empty() {
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!("{PUBLIC_DNS_ERROR_MARKER}: {host}"),
                ))
                    as Box<dyn std::error::Error + Send + Sync>);
            }

            Ok(Box::new(public_addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// Build a `reqwest::ClientBuilder` pre-hardened against SSRF. Callers add
/// their own `connect_timeout` / `timeout` and then call `.build()`.
pub fn hardened_client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .dns_resolver(Arc::new(PublicNetworkResolver))
}

pub fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_public_ipv4(ip),
        IpAddr::V6(ip) => is_public_ipv6(ip),
    }
}

pub fn is_public_ipv4(ip: Ipv4Addr) -> bool {
    let ip = u32::from(ip);
    !([0, 10, 127].iter().any(|octet| ip >> 24 == *octet)
        || in_ipv4_cidr(ip, Ipv4Addr::new(100, 64, 0, 0), 10)
        || in_ipv4_cidr(ip, Ipv4Addr::new(169, 254, 0, 0), 16)
        || in_ipv4_cidr(ip, Ipv4Addr::new(172, 16, 0, 0), 12)
        || in_ipv4_cidr(ip, Ipv4Addr::new(192, 0, 0, 0), 24)
        || in_ipv4_cidr(ip, Ipv4Addr::new(192, 0, 2, 0), 24)
        || in_ipv4_cidr(ip, Ipv4Addr::new(192, 88, 99, 0), 24)
        || in_ipv4_cidr(ip, Ipv4Addr::new(192, 168, 0, 0), 16)
        || in_ipv4_cidr(ip, Ipv4Addr::new(198, 18, 0, 0), 15)
        || in_ipv4_cidr(ip, Ipv4Addr::new(198, 51, 100, 0), 24)
        || in_ipv4_cidr(ip, Ipv4Addr::new(203, 0, 113, 0), 24)
        || in_ipv4_cidr(ip, Ipv4Addr::new(224, 0, 0, 0), 4)
        || in_ipv4_cidr(ip, Ipv4Addr::new(240, 0, 0, 0), 4))
}

pub fn is_public_ipv6(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_public_ipv4(v4);
    }

    let ip = u128::from(ip);
    !(ip == 0
        || ip == 1
        || in_ipv6_cidr(ip, Ipv6Addr::from_str("64:ff9b::").unwrap(), 96)
        || in_ipv6_cidr(ip, Ipv6Addr::from_str("64:ff9b:1::").unwrap(), 48)
        || in_ipv6_cidr(ip, Ipv6Addr::from_str("100::").unwrap(), 64)
        || in_ipv6_cidr(ip, Ipv6Addr::from_str("2001:2::").unwrap(), 48)
        || in_ipv6_cidr(ip, Ipv6Addr::from_str("2001:db8::").unwrap(), 32)
        || in_ipv6_cidr(ip, Ipv6Addr::from_str("2002::").unwrap(), 16)
        || in_ipv6_cidr(ip, Ipv6Addr::from_str("fc00::").unwrap(), 7)
        || in_ipv6_cidr(ip, Ipv6Addr::from_str("fe80::").unwrap(), 10)
        || in_ipv6_cidr(ip, Ipv6Addr::from_str("ff00::").unwrap(), 8))
}

fn in_ipv4_cidr(ip: u32, base: Ipv4Addr, prefix: u32) -> bool {
    let mask = u32::MAX << (32 - prefix);
    (ip & mask) == (u32::from(base) & mask)
}

fn in_ipv6_cidr(ip: u128, base: Ipv6Addr, prefix: u32) -> bool {
    let mask = u128::MAX << (128 - prefix);
    (ip & mask) == (u128::from(base) & mask)
}
