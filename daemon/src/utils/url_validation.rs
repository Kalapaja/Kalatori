//! URL validation for order's callback URL with SSRF attack prevention.
//!
//! # Security Model
//!
//! This module validates callback URLs before the server makes outbound HTTP
//! requests to prevent Server-Side Request Forgery (SSRF) and related attacks.
//!
//! ## Validation Checks Performed
//!
//! 1. **Length restriction** - Maximum 2048 characters
//! 2. **URL parsing** - Must conform to WHATWG URL Standard
//! 3. **HTTPS-only** - Rejects `http://`, `file://`, `ftp://`, etc.
//! 4. **Port restriction** - Only allows port 443 (HTTPS default)
//! 5. **No credentials** - Rejects `user:password@host` in URL
//! 6. **DNS resolution + IP validation** - Performs real-time DNS lookup and
//!    validates that resolved IP addresses are:
//!    - Not loopback (`127.0.0.0/8`, `::1`)
//!    - Not private (`10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`)
//!    - Not link-local (`169.254.0.0/16`) - blocks cloud metadata endpoints
//!    - Not CGNAT (`100.64.0.0/10`) - blocks Alibaba Cloud metadata
//!    - Not documentation/test ranges (`192.0.2.0/24`, `198.51.100.0/24`, etc.)
//!    - Not multicast, broadcast or reserved ranges
//!
//! ## DNS rebinding
//! The attack is mitigated by the following techniques:
//! - HTTPS requirement (certificate validation fails if DNS rebinds to
//!   localhost);
//! - Usage of the resolved IP address for the actual HTTP request (not just
//!   hostname validation).
//!
//! ## Unpleasant characters
//! The `url` crate rejects URLs containing CRLF characters (`\r`, `\n`),
//! preventing HTTP header injection attacks. Additionally, the `url` crate
//! normalizes IP addresses in various formats (hex, octal, decimal) to standard
//! dotted-decimal notation before validation, preventing IP obfuscation
//! techniques and double URL encoding attacks.
//!
//! ## Known Limitations
//! Currently it's expected that the callback URL is a fire and forget endpoint,
//! that doesn't return anything meaningful to the server. Also it's expected
//! that no redirects actually happen. If these assumptions were to change,
//! additional checks must be implemented:
//! - In case of redirect expected, **every redirect target must be
//!   re-validated** through the validation function in the module;
//! - In case of response body processing expected, **a maximum response size
//!   limit** must be implemented to prevent memory exhaustion `DoS` attacks via
//!   huge payloads.

use std::net::{
    IpAddr,
    Ipv4Addr,
};
use thiserror::Error;
use tokio::net as tokio_net;
use url::Url;

/// Maximum allowed URL length.
const MAX_URL_LENGTH: usize = 2048;

/// Validates a URL for security concerns described in the
/// [module](crate::utils::url_validation).
///
/// Performs DNS resolution to verify that the URL endpoint does not target
/// internal/private infrastructure.
/// Host name in the returned `Url` is replaced with the resolved IP address to
/// prevent DNS rebinding attacks.
///
/// Returns the [`Url`] struct for the provided URL.
pub async fn validate(url: &str) -> Result<Url, UrlValidationError> {
    // Length check
    if url.len() > MAX_URL_LENGTH {
        return Err(UrlValidationError::TooLong);
    }

    // Parse URL (using WHATWG standard parser)
    let mut parsed = Url::parse(url)?;

    // HTTPS only - blocks http://, file://, ftp://, data://, javascript:, etc.
    if parsed.scheme() != "https" {
        return Err(UrlValidationError::InvalidScheme(
            parsed.scheme().to_string(),
        ));
    }

    // Port must be 443 (HTTPS default)
    let port = match parsed.port() {
        None | Some(443) => 443, // Default or explicit port 443 - OK
        Some(port) => return Err(UrlValidationError::InvalidPort(port)),
    };

    // No credentials in URL
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(UrlValidationError::HasCredentials);
    }

    // DNS resolution + IP validation for SSRF prevention
    // First resolve hostname to addresses
    let host = parsed
        .host_str()
        .expect("https URLs always have a host");
    let resolved_addrs = tokio_net::lookup_host((host, port))
        .await
        .map_err(
            |e| UrlValidationError::DnsLookupFailed {
                hostname: host.to_string(),
                error: e.to_string(),
            },
        )?;

    // Then validate each resolved IP address
    let mut maybe_ip = None;
    for ip in resolved_addrs.map(|socket_addr| socket_addr.ip()) {
        let IpAddr::V4(ipv4) = ip else {
            continue;
        };

        if let Err(reason) = check_ipv4_is_global(ipv4) {
            return Err(UrlValidationError::NonGlobalIp {
                hostname: host.to_string(),
                ip: ip.to_string(),
                reason,
            });
        }

        maybe_ip.get_or_insert(ip);
    }

    // Update the URL host to the resolved IP address to prevent DNS rebinding
    // attacks.
    let ip = maybe_ip.ok_or(UrlValidationError::NoIpAddresses(
        host.to_string(),
    ))?;
    parsed
        .set_ip_host(ip)
        .unwrap_or_else(|()| unreachable!("HTTPS URLs always support IP hosts"));

    Ok(parsed)
}

/// Checks whether an IPv4 address is globally routable.
///
/// Returns `Ok(())` if the IP is globally routable, or `Err(&'static str)` with
/// the reason if it's not.
fn check_ipv4_is_global(ip: Ipv4Addr) -> Result<(), &'static str> {
    // Use standard library methods where stable
    if ip.is_private() {
        return Err("private address (RFC 1918)");
    }
    if ip.is_loopback() {
        return Err("loopback address (RFC 1122)");
    }
    if ip.is_link_local() {
        return Err(
            "link-local address (RFC 3927), blocks AWS/GCP/Azure metadata at 169.254.169.254",
        );
    }
    if ip.is_documentation() {
        return Err("documentation/test address (RFC 5737)");
    }
    if ip.is_multicast() {
        return Err("multicast address (RFC 5771)");
    }
    if ip.is_broadcast() {
        return Err("broadcast address (RFC 919)");
    }

    let octets = ip.octets();

    // 0.0.0.0/8 - "This" network (RFC 791)
    if octets[0] == 0 {
        return Err("unspecified/this network (RFC 791)");
    }

    // 100.64.0.0/10 - CGNAT / Shared address space (RFC 6598)
    if octets[0] == 100 && (octets[1] & 0xC0) == 64 {
        return Err("CGNAT/shared address space (RFC 6598), blocks Alibaba Cloud metadata");
    }

    // 192.0.0.0/24 - IETF Protocol Assignments (RFC 6890)
    if octets[0] == 192 && octets[1] == 0 && octets[2] == 0 {
        return Err("IETF protocol assignments (RFC 6890)");
    }

    // 192.88.99.0/24 - 6to4 Relay Anycast (RFC 7526, deprecated)
    if octets[0] == 192 && octets[1] == 88 && octets[2] == 99 {
        return Err("6to4 relay anycast (RFC 7526)");
    }

    // 198.18.0.0/15 - Benchmarking (RFC 2544)
    if octets[0] == 198 && (octets[1] == 18 || octets[1] == 19) {
        return Err("benchmarking address (RFC 2544)");
    }

    // 240.0.0.0/4 - Reserved for future use
    if octets[0] >= 240 {
        return Err("reserved address (240.0.0.0/4)");
    }

    Ok(())
}

#[derive(Debug, Error)]
pub enum UrlValidationError {
    #[error("URL exceeds maximum length of 2048 characters")]
    TooLong,

    #[error("URL parse error: {0}")]
    ParseError(#[from] url::ParseError),

    #[error("URL must use HTTPS scheme, got {0:?}")]
    InvalidScheme(String),

    #[error("URL port must be 443 (HTTPS default), got {0}")]
    InvalidPort(u16),

    #[error("URL must not contain credentials")]
    HasCredentials,

    #[error("DNS lookup failed for {hostname:?}: {error}")]
    DnsLookupFailed { hostname: String, error: String },

    #[error("DNS lookup for {0:?} returned no IPv4 addresses")]
    NoIpAddresses(String),

    #[error("URL hostname {hostname:?} resolves to non-global IP address {ip}: {reason}")]
    NonGlobalIp {
        hostname: String,
        ip: String,
        reason: &'static str,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ntest::timeout(5_000)]
    async fn valid_https_urls() {
        let urls = [
            "https://example.com/webhook",
            "https://example.com:443/path",
            "https://example.com/path?order=123&status=paid",
            "https://8.8.8.8/webhook",
            "https://1.1.1.1/webhook",
            "https://93.184.216.34/webhook",
        ];
        for url in urls {
            assert!(validate(url).await.is_ok());
        }
    }

    #[tokio::test]
    async fn reject_other_schemes() {
        let urls = [
            "http://example.com/webhook",
            "ftp://example.com/file",
            "file:///etc/passwd",
            "data:text/html,<h1>hi</h1>",
            "javascript:alert(1)",
        ];

        for url in urls {
            let err = validate(url).await.unwrap_err();
            assert!(matches!(
                err,
                UrlValidationError::InvalidScheme(_)
            ));
        }
    }

    #[tokio::test]
    async fn reject_credentials() {
        let urls = [
            "https://user@example.com/webhook",
            "https://user:pass@example.com/webhook",
        ];

        for url in urls {
            let err = validate(url).await.unwrap_err();
            assert!(matches!(
                err,
                UrlValidationError::HasCredentials
            ));
        }

        // %40 = @, which may be used to obscure credentials
        let err = validate("https://user%40example.com/webhook")
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            UrlValidationError::ParseError(_)
        ));
    }

    #[tokio::test]
    async fn reject_localhost_variants() {
        let urls = ["https://localhost/webhook", "https://0.0.0.0/webhook"];

        for url in urls {
            let err = validate(url).await.unwrap_err();
            assert!(matches!(
                err,
                UrlValidationError::NonGlobalIp { .. }
            ));
        }
    }

    #[tokio::test]
    async fn reject_local_and_internal_ips() {
        let urls = [
            "https://10.0.0.1/webhook",        // RFC 1918 private
            "https://172.16.0.1/webhook",      // RFC 1918 private
            "https://192.168.1.1/webhook",     // RFC 1918 private
            "https://169.254.169.254/webhook", // Link-local (cloud metadata)
            "https://127.0.0.1/webhook",       // Loopback
        ];

        for url in urls {
            let err = validate(url).await.unwrap_err();
            assert!(matches!(
                err,
                UrlValidationError::NonGlobalIp { .. }
            ));
        }
    }

    #[tokio::test]
    async fn reject_loopback() {
        assert!(
            validate("https://127.0.0.1/webhook")
                .await
                .is_err()
        );
        assert!(
            validate("https://127.255.255.255/webhook")
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn reject_rfc1918_private() {
        let urls = [
            // 10.0.0.0/8
            "https://10.0.0.1/webhook",
            "https://10.255.255.255/webhook",
            // 172.16.0.0/12
            "https://172.16.0.1/webhook",
            "https://172.31.255.255/webhook",
            // 192.168.0.0/16
            "https://192.168.1.1/webhook",
        ];

        for url in urls {
            let err = validate(url).await.unwrap_err();
            assert!(matches!(
                err,
                UrlValidationError::NonGlobalIp { .. }
            ));
        }
    }

    #[tokio::test]
    async fn reject_cgnat() {
        // 100.64.0.0/10 - CGNAT (RFC 6598), also covers Alibaba Cloud metadata
        // 100.100.100.200
        let urls = [
            "https://100.64.0.1/webhook",
            "https://100.100.100.200/webhook",
            "https://100.127.255.255/webhook",
        ];

        for url in urls {
            let err = validate(url).await.unwrap_err();
            assert!(matches!(
                err,
                UrlValidationError::NonGlobalIp { .. }
            ));
        }
    }

    #[tokio::test]
    async fn reject_link_local_and_cloud_metadata() {
        // 169.254.0.0/16 - covers AWS/GCP/Azure metadata at 169.254.169.254
        let urls = [
            "https://169.254.169.254/latest/meta-data/",
            "https://169.254.0.1/webhook",
        ];

        for url in urls {
            let err = validate(url).await.unwrap_err();
            assert!(matches!(
                err,
                UrlValidationError::NonGlobalIp { .. }
            ));
        }
    }

    #[tokio::test]
    async fn reject_documentation_and_test_nets() {
        let urls = [
            // 192.0.2.0/24 - TEST-NET-1
            "https://192.0.2.1/webhook",
            // 198.51.100.0/24 - TEST-NET-2
            "https://198.51.100.1/webhook",
            // 203.0.113.0/24 - TEST-NET-3
            "https://203.0.113.1/webhook",
        ];

        for url in urls {
            let err = validate(url).await.unwrap_err();
            assert!(matches!(
                err,
                UrlValidationError::NonGlobalIp { .. }
            ));
        }
    }

    #[tokio::test]
    async fn reject_benchmarking() {
        // 198.18.0.0/15
        let urls = [
            "https://198.18.0.1/webhook",
            "https://198.19.255.255/webhook",
        ];

        for url in urls {
            let err = validate(url).await.unwrap_err();
            assert!(matches!(
                err,
                UrlValidationError::NonGlobalIp { .. }
            ));
        }
    }

    #[tokio::test]
    async fn reject_ietf_and_6to4() {
        let urls = ["https://192.0.0.1/webhook", "https://192.88.99.1/webhook"];

        for url in urls {
            let err = validate(url).await.unwrap_err();
            assert!(matches!(
                err,
                UrlValidationError::NonGlobalIp { .. }
            ));
        }
    }

    #[tokio::test]
    async fn reject_multicast_and_reserved() {
        let urls = [
            "https://224.0.0.1/webhook",
            "https://239.255.255.255/webhook",
            "https://240.0.0.1/webhook",
            "https://255.255.255.255/webhook",
        ];

        for url in urls {
            let err = validate(url).await.unwrap_err();
            assert!(matches!(
                err,
                UrlValidationError::NonGlobalIp { .. }
            ));
        }
    }

    #[test]
    fn cgnat_boundary() {
        // 100.63.255.255 is just below CGNAT range - should be allowed
        assert!(check_ipv4_is_global("100.63.255.255".parse().unwrap()).is_ok());
        // 100.64.0.0 is start of CGNAT - should be blocked
        assert!(check_ipv4_is_global("100.64.0.0".parse().unwrap()).is_err());
        // 100.127.255.255 is end of CGNAT - should be blocked
        assert!(check_ipv4_is_global("100.127.255.255".parse().unwrap()).is_err());
        // 100.128.0.0 is just above CGNAT range - should be allowed
        assert!(check_ipv4_is_global("100.128.0.0".parse().unwrap()).is_ok());
    }

    #[tokio::test]
    async fn reject_non_https_ports() {
        let urls = [
            "https://example.com:8080/webhook",
            "https://example.com:80/webhook",
            "https://example.com:22/webhook",
        ];

        for url in urls {
            let err = validate(url).await.unwrap_err();
            assert!(matches!(
                err,
                UrlValidationError::InvalidPort(_)
            ));
        }
    }

    #[tokio::test]
    async fn reject_too_long() {
        let long_url = format!(
            "https://example.com/{}",
            "a".repeat(2050)
        );
        let err = validate(&long_url).await.unwrap_err();
        assert!(matches!(
            err,
            UrlValidationError::TooLong
        ));
    }

    #[tokio::test]
    async fn reject_obfuscated_ips() {
        // These are all 127.0.0.1 in different representations.
        // The `url` crate normalizes them before we see host_str().
        assert!(
            validate("https://0x7f000001/webhook")
                .await
                .is_err()
        );
        assert!(
            validate("https://2130706433/webhook")
                .await
                .is_err()
        );
        assert!(
            validate("https://0177.0.0.1/webhook")
                .await
                .is_err()
        );
    }
}
