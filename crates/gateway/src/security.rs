//! Security utilities for the gateway.
//!
//! Provides constant-time token comparison and SSRF prevention via private
//! network detection.

use std::net::IpAddr;

/// Constant-time byte comparison to prevent timing attacks on secret tokens.
///
/// Returns `true` only when both slices have the same length and identical
/// contents. The comparison always examines every byte regardless of where
/// the first difference is, so an attacker cannot infer partial matches from
/// response timing.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

/// Check whether a URL points to a private/internal network endpoint.
///
/// Parses the URL, resolves the hostname via DNS, and returns `true` if any
/// resolved address falls in a private, loopback, link-local, or otherwise
/// reserved range. This prevents SSRF attacks where an attacker registers a
/// service pointing at internal infrastructure or cloud metadata endpoints.
///
/// # Rejected ranges
///
/// - `127.0.0.0/8` — IPv4 loopback
/// - `10.0.0.0/8` — RFC 1918 private
/// - `172.16.0.0/12` — RFC 1918 private
/// - `192.168.0.0/16` — RFC 1918 private
/// - `169.254.0.0/16` — link-local (cloud metadata)
/// - `0.0.0.0/8` — "this" network
/// - `::1` — IPv6 loopback
/// - `fe80::/10` — IPv6 link-local
/// - `localhost` hostname (rejected before DNS resolution)
pub async fn is_private_endpoint(url: &str) -> Result<bool, String> {
    let parsed = url::Url::parse(url).map_err(|e| format!("invalid URL: {e}"))?;

    let host = parsed
        .host_str()
        .ok_or_else(|| "URL has no host".to_string())?;

    // Reject localhost hostname before DNS resolution
    if host.eq_ignore_ascii_case("localhost") {
        return Ok(true);
    }

    // If the host is already an IP address, check it directly without DNS
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(is_private_ip(ip));
    }

    let port = parsed.port_or_known_default().unwrap_or(443);
    let lookup_addr = format!("{host}:{port}");

    // Attempt DNS resolution. If it fails, the host is unresolvable — not
    // a private address. The actual HTTP request will fail later anyway.
    let addrs = match tokio::net::lookup_host(&lookup_addr).await {
        Ok(addrs) => addrs,
        Err(_) => return Ok(false),
    };

    for addr in addrs {
        if is_private_ip(addr.ip()) {
            return Ok(true);
        }
    }

    Ok(false)
}

/// Returns `true` if the IP address is in a private, loopback, link-local,
/// or otherwise reserved range.
fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let octets = v4.octets();
            // 127.0.0.0/8 — loopback
            octets[0] == 127
            // 10.0.0.0/8 — private
            || octets[0] == 10
            // 172.16.0.0/12 — private
            || (octets[0] == 172 && (16..=31).contains(&octets[1]))
            // 192.168.0.0/16 — private
            || (octets[0] == 192 && octets[1] == 168)
            // 169.254.0.0/16 — link-local (cloud metadata)
            || (octets[0] == 169 && octets[1] == 254)
            // 0.0.0.0/8 — "this" network
            || octets[0] == 0
        }
        IpAddr::V6(v6) => {
            // ::1 — IPv6 loopback
            v6 == std::net::Ipv6Addr::LOCALHOST
            // fe80::/10 — IPv6 link-local
            || (v6.segments()[0] & 0xffc0) == 0xfe80
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // constant_time_eq
    // -----------------------------------------------------------------------

    #[test]
    fn test_constant_time_eq_identical() {
        assert!(constant_time_eq(b"secret-token", b"secret-token"));
    }

    #[test]
    fn test_constant_time_eq_different() {
        assert!(!constant_time_eq(b"secret-token", b"wrong-token!"));
    }

    #[test]
    fn test_constant_time_eq_different_lengths() {
        assert!(!constant_time_eq(b"short", b"longer-string"));
    }

    #[test]
    fn test_constant_time_eq_empty() {
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn test_constant_time_eq_single_bit_difference() {
        // 'A' (0x41) vs 'B' (0x42) — single bit difference
        assert!(!constant_time_eq(b"A", b"B"));
    }

    // -----------------------------------------------------------------------
    // is_private_ip
    // -----------------------------------------------------------------------

    #[test]
    fn test_loopback_v4() {
        assert!(is_private_ip("127.0.0.1".parse().unwrap()));
        assert!(is_private_ip("127.255.255.255".parse().unwrap()));
    }

    #[test]
    fn test_private_10() {
        assert!(is_private_ip("10.0.0.1".parse().unwrap()));
        assert!(is_private_ip("10.255.255.255".parse().unwrap()));
    }

    #[test]
    fn test_private_172() {
        assert!(is_private_ip("172.16.0.1".parse().unwrap()));
        assert!(is_private_ip("172.31.255.255".parse().unwrap()));
        // 172.15.x.x is NOT private
        assert!(!is_private_ip("172.15.0.1".parse().unwrap()));
        // 172.32.x.x is NOT private
        assert!(!is_private_ip("172.32.0.1".parse().unwrap()));
    }

    #[test]
    fn test_private_192_168() {
        assert!(is_private_ip("192.168.0.1".parse().unwrap()));
        assert!(is_private_ip("192.168.255.255".parse().unwrap()));
    }

    #[test]
    fn test_link_local_v4() {
        assert!(is_private_ip("169.254.169.254".parse().unwrap()));
        assert!(is_private_ip("169.254.0.1".parse().unwrap()));
    }

    #[test]
    fn test_zero_network() {
        assert!(is_private_ip("0.0.0.0".parse().unwrap()));
        assert!(is_private_ip("0.0.0.1".parse().unwrap()));
    }

    #[test]
    fn test_public_v4() {
        assert!(!is_private_ip("8.8.8.8".parse().unwrap()));
        assert!(!is_private_ip("1.1.1.1".parse().unwrap()));
        assert!(!is_private_ip("203.0.113.1".parse().unwrap()));
    }

    #[test]
    fn test_loopback_v6() {
        assert!(is_private_ip("::1".parse().unwrap()));
    }

    #[test]
    fn test_link_local_v6() {
        assert!(is_private_ip("fe80::1".parse().unwrap()));
        assert!(is_private_ip("fe80::abcd:ef01:2345:6789".parse().unwrap()));
    }

    #[test]
    fn test_public_v6() {
        assert!(!is_private_ip("2607:f8b0:4004:800::200e".parse().unwrap()));
    }

    // -----------------------------------------------------------------------
    // is_private_endpoint
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_localhost_hostname_rejected() {
        assert!(is_private_endpoint("https://localhost:8080/api")
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn test_loopback_ip_rejected() {
        assert!(is_private_endpoint("https://127.0.0.1:8080/api")
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn test_metadata_endpoint_rejected() {
        // 169.254.169.254 is the cloud metadata endpoint
        assert!(
            is_private_endpoint("http://169.254.169.254/latest/meta-data")
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn test_private_10_rejected() {
        assert!(is_private_endpoint("https://10.0.0.1/internal")
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn test_private_192_168_rejected() {
        assert!(is_private_endpoint("https://192.168.1.1/admin")
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn test_invalid_url_returns_error() {
        assert!(is_private_endpoint("not-a-url").await.is_err());
    }

    #[tokio::test]
    async fn test_no_host_returns_error() {
        assert!(is_private_endpoint("file:///etc/passwd").await.is_err());
    }
}
