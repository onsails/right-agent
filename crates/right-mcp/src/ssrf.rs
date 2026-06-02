//! SSRF hardening helpers for `reqwest` clients that connect to URLs whose
//! host is supplied by user input (e.g. MCP detection/registration).
//!
//! All outbound HTTP from those code paths MUST use [`hardened_client_builder`]
//! so that:
//!
//! 1. DNS resolution filters addresses according to the configured
//!    `NetworkPolicy` (the [`PublicNetworkResolver`]): `PublicOnly` strips all
//!    non-public addresses; `AllowPrivate` additionally permits the operator's
//!    RFC1918/CGNAT/ULA ranges and loopback (for local dev MCP servers) but
//!    never link-local or cloud-metadata.
//! 2. Environment-based proxies are bypassed (`no_proxy`).
//! 3. Redirects are disabled (the server cannot 302 us into the internal
//!    network).
//!
//! Callers add their own timeouts on top of the returned builder.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::str::FromStr as _;
use std::sync::Arc;

/// Substring embedded in the error returned by [`PublicNetworkResolver`] when
/// no resolved address survives the active `NetworkPolicy` filter. Callers
/// match on this marker to distinguish the SSRF-policy rejection from generic
/// DNS failures without echoing the rejected URL back to the user.
pub const PUBLIC_DNS_ERROR_MARKER: &str = "MCP detection DNS resolved to a non-public address";

/// Outbound-connection trust tier. `PublicOnly` is the historical SSRF-hardened
/// behaviour (globally-routable only). `AllowPrivate` additionally permits the
/// operator's own private LAN / Tailscale / ULA ranges and loopback — used only
/// for the operator-supplied base URL, never for server-supplied metadata URLs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkPolicy {
    PublicOnly,
    AllowPrivate,
}

/// RFC1918 + RFC6598 CGNAT (Tailscale) + IPv6 ULA — the operator's own private
/// network. Shared with the detection gate in credentials.rs so both agree on "private".
pub(crate) fn is_user_private_lan(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let n = u32::from(v4);
            n >> 24 == 10
                || in_ipv4_cidr(n, Ipv4Addr::new(172, 16, 0, 0), 12)
                || in_ipv4_cidr(n, Ipv4Addr::new(192, 168, 0, 0), 16)
                || in_ipv4_cidr(n, Ipv4Addr::new(100, 64, 0, 0), 10)
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_user_private_lan(IpAddr::V4(v4));
            }
            in_ipv6_cidr(u128::from(v6), Ipv6Addr::from_str("fc00::").unwrap(), 7)
        }
    }
}

/// Cloud instance-metadata addresses. Denied in EVERY tier. Two of these fall
/// inside otherwise-allowed families (`100.100.100.200` in CGNAT, `fd00:ec2::254`
/// in ULA), so the deny-list is essential — range membership alone would re-open
/// them to a DNS-rebinding pivot on the AllowPrivate base-URL client.
fn is_cloud_metadata(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4 == Ipv4Addr::new(169, 254, 169, 254) || v4 == Ipv4Addr::new(100, 100, 100, 200)
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_cloud_metadata(IpAddr::V4(v4));
            }
            v6 == Ipv6Addr::from_str("fd00:ec2::254").unwrap()
        }
    }
}

/// Is `ip` permitted as a connection target under `policy`?
pub fn ip_allowed(ip: IpAddr, policy: NetworkPolicy) -> bool {
    if is_cloud_metadata(ip) {
        return false;
    }
    match policy {
        NetworkPolicy::PublicOnly => is_public_ip(ip),
        NetworkPolicy::AllowPrivate => {
            is_public_ip(ip) || is_user_private_lan(ip) || is_loopback_addr(ip)
        }
    }
}

/// Loopback check that folds IPv4-mapped IPv6 (`::ffff:127.0.0.1`) so it matches
/// the bare IPv4 form. `Ipv6Addr::is_loopback` alone only catches `::1`.
pub(crate) fn is_loopback_addr(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback(),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return v4.is_loopback();
            }
            v6.is_loopback()
        }
    }
}

/// `reqwest` DNS resolver that strips addresses disallowed by `policy`. If
/// nothing remains, resolution fails with [`PUBLIC_DNS_ERROR_MARKER`].
#[derive(Debug, Clone, Copy)]
pub struct PublicNetworkResolver {
    policy: NetworkPolicy,
}

impl PublicNetworkResolver {
    pub fn new(policy: NetworkPolicy) -> Self {
        Self { policy }
    }
}

impl reqwest::dns::Resolve for PublicNetworkResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_owned();
        let policy = self.policy;
        Box::pin(async move {
            let addrs = tokio::net::lookup_host((host.as_str(), 0))
                .await
                .map_err(|error| Box::new(error) as Box<dyn std::error::Error + Send + Sync>)?;
            let allowed = addrs
                .filter(|addr| ip_allowed(addr.ip(), policy))
                .collect::<Vec<_>>();
            if allowed.is_empty() {
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!("{PUBLIC_DNS_ERROR_MARKER}: {host}"),
                ))
                    as Box<dyn std::error::Error + Send + Sync>);
            }

            Ok(Box::new(allowed.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// Build a `reqwest::ClientBuilder` pre-hardened against SSRF under `policy`.
/// Callers add their own `connect_timeout` / `timeout` and then call `.build()`.
pub fn hardened_client_builder(policy: NetworkPolicy) -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .dns_resolver(Arc::new(PublicNetworkResolver::new(policy)))
}

/// Returns true if `domain` is `localhost` (with optional trailing dot,
/// ASCII-case-insensitive).
pub fn is_localhost_domain(domain: &str) -> bool {
    domain
        .strip_suffix('.')
        .unwrap_or(domain)
        .eq_ignore_ascii_case("localhost")
}

/// Validate a user-/metadata-supplied HTTP URL under `policy` before outbound
/// I/O. IP literals are checked directly; domains are filtered at connect time
/// by [`PublicNetworkResolver`].
pub fn is_allowed_http_url(input: &str, policy: NetworkPolicy) -> bool {
    let Ok(parsed) = reqwest::Url::parse(input) else {
        return false;
    };
    matches!(parsed.scheme(), "http" | "https")
        && parsed.host_str().is_some()
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && parsed.fragment().is_none()
        && parsed.host().is_some_and(|host| match host {
            url::Host::Domain(domain) => match policy {
                NetworkPolicy::PublicOnly => !is_localhost_domain(domain),
                NetworkPolicy::AllowPrivate => true,
            },
            url::Host::Ipv4(ip) => ip_allowed(IpAddr::V4(ip), policy),
            url::Host::Ipv6(ip) => ip_allowed(IpAddr::V6(ip), policy),
        })
}

/// Public-only URL check (historical behaviour). Thin wrapper over
/// [`is_allowed_http_url`] with [`NetworkPolicy::PublicOnly`].
pub fn is_public_http_url(input: &str) -> bool {
    is_allowed_http_url(input, NetworkPolicy::PublicOnly)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_http_url_rejects_loopback_and_private_literals() {
        for url in [
            "http://127.0.0.1:8080/token",
            "http://localhost:8080/token",
            "http://localhost.:8080/token",
            "https://192.168.1.1/token",
            "https://[::1]/token",
            "https://[::ffff:10.0.0.1]/token",
        ] {
            assert!(!is_public_http_url(url), "{url} must be rejected");
        }
    }

    #[test]
    fn public_http_url_rejects_non_http_credentials_and_fragments() {
        for url in [
            "ftp://mcp.example.com/token",
            "https://user@mcp.example.com/token",
            "https://mcp.example.com/token#fragment",
            "not a url",
        ] {
            assert!(!is_public_http_url(url), "{url} must be rejected");
        }
    }

    #[test]
    fn public_http_url_allows_public_hosts() {
        assert!(is_public_http_url("https://mcp.example.com/token"));
        assert!(is_public_http_url("http://8.8.8.8/token"));
    }

    #[test]
    fn allow_private_admits_tailscale_cgnat() {
        // 100.85.147.49 is the obsidian repro address; 100.64.0.0/10 CGNAT.
        for ip in ["100.85.147.49", "100.64.0.0", "100.127.255.255"] {
            let ip: IpAddr = ip.parse().unwrap();
            assert!(
                ip_allowed(ip, NetworkPolicy::AllowPrivate),
                "{ip} must be allowed"
            );
            assert!(
                !ip_allowed(ip, NetworkPolicy::PublicOnly),
                "{ip} must be public-blocked"
            );
        }
    }

    #[test]
    fn allow_private_admits_rfc1918_and_ula() {
        for ip in [
            "10.0.0.5",
            "172.16.9.9",
            "192.168.1.10",
            "fc00::1",
            "fd12:3456::1",
        ] {
            let ip: IpAddr = ip.parse().unwrap();
            assert!(
                ip_allowed(ip, NetworkPolicy::AllowPrivate),
                "{ip} must be allowed"
            );
            assert!(
                !ip_allowed(ip, NetworkPolicy::PublicOnly),
                "{ip} must be public-blocked"
            );
        }
    }

    #[test]
    fn allow_private_blocks_link_local_unspecified_but_allows_loopback() {
        for ip in ["127.0.0.1", "::1", "::ffff:127.0.0.1"] {
            let ip: IpAddr = ip.parse().unwrap();
            assert!(
                ip_allowed(ip, NetworkPolicy::AllowPrivate),
                "loopback {ip} must be allowed under AllowPrivate"
            );
            assert!(
                !ip_allowed(ip, NetworkPolicy::PublicOnly),
                "loopback {ip} must be blocked under PublicOnly"
            );
        }
        for ip in ["169.254.0.5", "fe80::1", "0.0.0.0", "::"] {
            let ip: IpAddr = ip.parse().unwrap();
            assert!(
                !ip_allowed(ip, NetworkPolicy::AllowPrivate),
                "{ip} must stay blocked"
            );
            assert!(
                !ip_allowed(ip, NetworkPolicy::PublicOnly),
                "{ip} must stay blocked"
            );
        }
    }

    #[test]
    fn cloud_metadata_blocked_even_inside_allowed_families() {
        // fd00:ec2::254 is inside ULA fc00::/7; 100.100.100.200 is inside CGNAT 100.64/10.
        for ip in ["169.254.169.254", "100.100.100.200", "fd00:ec2::254"] {
            let ip: IpAddr = ip.parse().unwrap();
            assert!(
                !ip_allowed(ip, NetworkPolicy::AllowPrivate),
                "metadata {ip} must be denied"
            );
            assert!(
                !ip_allowed(ip, NetworkPolicy::PublicOnly),
                "metadata {ip} must be denied"
            );
        }
    }

    #[test]
    fn allow_private_admits_public() {
        let ip: IpAddr = "8.8.8.8".parse().unwrap();
        assert!(ip_allowed(ip, NetworkPolicy::AllowPrivate));
        assert!(ip_allowed(ip, NetworkPolicy::PublicOnly));
    }

    #[tokio::test]
    async fn resolver_constructs_with_policy_and_resolves_public_host() {
        use reqwest::dns::Resolve as _;
        use std::str::FromStr as _;
        let ip: IpAddr = "100.85.147.49".parse().unwrap();
        assert!(ip_allowed(ip, NetworkPolicy::AllowPrivate));
        // Resolver construction must accept a policy.
        let _ = PublicNetworkResolver::new(NetworkPolicy::AllowPrivate);
        let r = PublicNetworkResolver::new(NetworkPolicy::PublicOnly);
        // Public host resolves to >=1 public addr under PublicOnly.
        let addrs = r
            .resolve(reqwest::dns::Name::from_str("example.com").unwrap())
            .await;
        assert!(addrs.is_ok(), "public host must resolve under PublicOnly");
    }

    #[tokio::test]
    async fn public_only_resolver_strips_private_resolving_hostname() {
        use reqwest::dns::Resolve as _;
        use std::str::FromStr as _;

        let public_only = PublicNetworkResolver::new(NetworkPolicy::PublicOnly);
        let stripped = public_only
            .resolve(reqwest::dns::Name::from_str("localhost").unwrap())
            .await;
        assert!(
            stripped.is_err(),
            "localhost resolves to loopback; PublicOnly must strip it and fail resolution"
        );

        let allow_private = PublicNetworkResolver::new(NetworkPolicy::AllowPrivate);
        let admitted = allow_private
            .resolve(reqwest::dns::Name::from_str("localhost").unwrap())
            .await;
        assert!(
            admitted.is_ok(),
            "AllowPrivate must admit a loopback-resolving hostname"
        );
    }
}
