//! Trusted-proxy-aware client IP resolution.

use std::net::{IpAddr, SocketAddr};

use axum::http::HeaderMap;

use crate::config::TrustedProxies;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientIpError {
    /// Peer claimed to be a trusted proxy but supplied unusable `X-Forwarded-For`.
    SpoofedOrMissingForwarded,
}

/// Resolve the client IP.
///
/// - When the immediate peer is **not** in the trusted CIDR list, always use the peer
///   address and ignore `X-Forwarded-For` (prevents spoofing).
/// - When the peer **is** trusted:
///   - Reject multiple `X-Forwarded-For` header fields (ambiguous wire order).
///   - Prefer a single overwritten client IP (`X-Forwarded-For: <client>`), or
///   - Walk the list **right-to-left**, skipping trusted hops, and take the first
///     untrusted address as the client.
///   - Missing/malformed values fail closed.
pub fn resolve_client_ip(
    peer: Option<SocketAddr>,
    headers: &HeaderMap,
    trusted: &TrustedProxies,
) -> Result<IpAddr, ClientIpError> {
    let peer_ip = peer.map(|addr| addr.ip()).unwrap_or_else(|| {
        // Hermetic oneshot tests have no ConnectInfo; treat as loopback direct peer.
        IpAddr::from([127, 0, 0, 1])
    });

    if trusted.is_empty() || !trusted.contains(peer_ip) {
        return Ok(peer_ip);
    }

    let mut xff_values = headers.get_all("x-forwarded-for").iter();
    let first = xff_values
        .next()
        .ok_or(ClientIpError::SpoofedOrMissingForwarded)?;
    // Multiple XFF fields have ambiguous concatenation order across proxies — reject.
    if xff_values.next().is_some() {
        return Err(ClientIpError::SpoofedOrMissingForwarded);
    }
    let forwarded = first
        .to_str()
        .map_err(|_| ClientIpError::SpoofedOrMissingForwarded)?
        .trim();
    if forwarded.is_empty() {
        return Err(ClientIpError::SpoofedOrMissingForwarded);
    }

    let mut parsed: Vec<IpAddr> = Vec::new();
    for part in forwarded.split(',') {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            return Err(ClientIpError::SpoofedOrMissingForwarded);
        }
        parsed.push(
            trimmed
                .parse::<IpAddr>()
                .map_err(|_| ClientIpError::SpoofedOrMissingForwarded)?,
        );
    }
    if parsed.is_empty() {
        return Err(ClientIpError::SpoofedOrMissingForwarded);
    }

    // Strict single overwritten IP from the immediate trusted proxy.
    if parsed.len() == 1 {
        let only = parsed[0];
        if trusted.contains(only) {
            return Err(ClientIpError::SpoofedOrMissingForwarded);
        }
        return Ok(only);
    }

    // Right-to-left trusted-hop walk: skip trusted proxies, first untrusted is client.
    for ip in parsed.into_iter().rev() {
        if !trusted.contains(ip) {
            return Ok(ip);
        }
    }
    Err(ClientIpError::SpoofedOrMissingForwarded)
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, SocketAddr};
    use std::str::FromStr;

    use axum::http::HeaderMap;
    use ipnet::IpNet;

    use super::{resolve_client_ip, ClientIpError};
    use crate::config::TrustedProxies;

    fn headers_xff(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", value.parse().unwrap());
        headers
    }

    #[test]
    fn ignores_xff_from_untrusted_peer() {
        let trusted = TrustedProxies::from_cidrs(vec![IpNet::from_str("10.0.0.0/8").unwrap()]);
        let peer = SocketAddr::from(([203, 0, 113, 10], 443));
        let ip = resolve_client_ip(Some(peer), &headers_xff("1.2.3.4"), &trusted).unwrap();
        assert_eq!(ip, IpAddr::from([203, 0, 113, 10]));
    }

    #[test]
    fn single_overwritten_client_ip_from_trusted_peer() {
        let trusted = TrustedProxies::from_cidrs(vec![IpNet::from_str("10.0.0.0/8").unwrap()]);
        let peer = SocketAddr::from(([10, 0, 0, 2], 443));
        let ip = resolve_client_ip(Some(peer), &headers_xff("198.51.100.7"), &trusted).unwrap();
        assert_eq!(ip, IpAddr::from([198, 51, 100, 7]));
    }

    #[test]
    fn right_to_left_skips_trusted_hops() {
        let trusted = TrustedProxies::from_cidrs(vec![IpNet::from_str("10.0.0.0/8").unwrap()]);
        let peer = SocketAddr::from(([10, 0, 0, 2], 443));
        let ip = resolve_client_ip(
            Some(peer),
            &headers_xff("198.51.100.7, 203.0.113.9, 10.0.0.9"),
            &trusted,
        )
        .unwrap();
        assert_eq!(ip, IpAddr::from([203, 0, 113, 9]));
    }

    #[test]
    fn rejects_multiple_xff_header_fields() {
        let trusted = TrustedProxies::from_cidrs(vec![IpNet::from_str("10.0.0.0/8").unwrap()]);
        let peer = SocketAddr::from(([10, 0, 0, 2], 443));
        let mut headers = HeaderMap::new();
        headers.append("x-forwarded-for", "198.51.100.7".parse().unwrap());
        headers.append("x-forwarded-for", "203.0.113.9".parse().unwrap());
        let err = resolve_client_ip(Some(peer), &headers, &trusted).unwrap_err();
        assert_eq!(err, ClientIpError::SpoofedOrMissingForwarded);
    }

    #[test]
    fn rejects_all_trusted_xff_chain() {
        let trusted = TrustedProxies::from_cidrs(vec![IpNet::from_str("10.0.0.0/8").unwrap()]);
        let peer = SocketAddr::from(([10, 0, 0, 2], 443));
        let err = resolve_client_ip(Some(peer), &headers_xff("10.0.0.8, 10.0.0.9"), &trusted)
            .unwrap_err();
        assert_eq!(err, ClientIpError::SpoofedOrMissingForwarded);
    }

    #[test]
    fn rejects_missing_xff_from_trusted_peer() {
        let trusted = TrustedProxies::from_cidrs(vec![IpNet::from_str("10.0.0.0/8").unwrap()]);
        let peer = SocketAddr::from(([10, 0, 0, 2], 443));
        let err = resolve_client_ip(Some(peer), &HeaderMap::new(), &trusted).unwrap_err();
        assert_eq!(err, ClientIpError::SpoofedOrMissingForwarded);
    }

    #[test]
    fn rejects_malformed_xff_from_trusted_peer() {
        let trusted = TrustedProxies::from_cidrs(vec![IpNet::from_str("10.0.0.0/8").unwrap()]);
        let peer = SocketAddr::from(([10, 0, 0, 2], 443));
        let err = resolve_client_ip(Some(peer), &headers_xff("not-an-ip"), &trusted).unwrap_err();
        assert_eq!(err, ClientIpError::SpoofedOrMissingForwarded);
    }
}
