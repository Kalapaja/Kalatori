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
//! 6. **Base URL restriction** - Ensures the URL matches (is the same or a
//!    subdomain of) the allowed base URL if provided
//! 7. **DNS resolution + IP validation** - Performs real-time DNS lookup and
//!    validates that resolved IP addresses are:
//!    - Not loopback (`127.0.0.0/8`, `::1`)
//!    - Not private (`10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`)
//!    - Not link-local (`169.254.0.0/16`) - blocks cloud metadata endpoints
//!    - Not CGNAT (`100.64.0.0/10`) - blocks Alibaba Cloud metadata
//!    - Not documentation/test ranges (`192.0.2.0/24`, `198.51.100.0/24`, etc.)
//!    - Not multicast, broadcast or reserved ranges
//!
//! ## DNS rebinding
//! The attack is mitigated by HTTPS requirement (certificate validation fails
//! if DNS rebinds to localhost).
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

use serde::{
    Deserialize,
    Serialize,
};
use sqlx::encode::IsNull;
use sqlx::error::BoxDynError;
use sqlx::{
    Database,
    Decode,
    Encode,
    Type,
};
use std::net::IpAddr;
use thiserror::Error;
use tokio::net as tokio_net;
use url::{
    Host,
    Url,
};

/// Maximum allowed URL length.
const MAX_URL_LENGTH: usize = 2048;

/// Validates a URL for security concerns described in the
/// [module](crate::utils::url_validation).
pub async fn validate(url: &str) -> Result<ValidatedUrl, UrlValidationError> {
    validate_with_allowed_base_impl(url, None).await
}

/// Validates a URL same as [`validate()`], but also checks that the URL host
/// matches the allowed base domain provided in `allowed_base_domain` (exact
/// match or subdomain).
pub async fn validate_with_allowed_base(
    url: &str,
    allowed_base_domain: &Host<String>,
) -> Result<ValidatedUrl, UrlValidationError> {
    validate_with_allowed_base_impl(
        url,
        Some(EitherDomainsCollection::Domain(
            allowed_base_domain,
        )),
    )
    .await
}

/// Validates a URL same as [`validate()`], but also checks that the URL host
/// matches at least one of the allowed base domains provided in
/// `allowed_base_domains` (exact match or subdomain).
pub async fn validate_with_allowed_base_many(
    url: &str,
    allowed_base_domains: &[Host<String>],
) -> Result<ValidatedUrl, UrlValidationError> {
    validate_with_allowed_base_impl(
        url,
        Some(EitherDomainsCollection::DomainCollection(allowed_base_domains)),
    )
    .await
}

/// Internal implementation of URL validation with optional allowed base domain
/// check.
///
/// Performs DNS resolution to verify that the URL endpoint does not target
/// internal/private infrastructure.
///
/// Returns the [`ValidatedUrl`] struct for the provided URL.
async fn validate_with_allowed_base_impl(
    url: &str,
    allowed_base_domain: Option<EitherDomainsCollection<'_>>,
) -> Result<ValidatedUrl, UrlValidationError> {
    // Length check
    if url.len() > MAX_URL_LENGTH {
        return Err(UrlValidationError::TooLong);
    }

    // Parse URL (using WHATWG standard parser)
    let parsed = Url::parse(url)?;

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

    let host = parsed
        .domain()
        .ok_or(UrlValidationError::UrlHostIsNotDomain)?;

    // Validate against allowed base domain if provided
    if let Some(allowed_base_domain) = allowed_base_domain {
        allowed_base_domain.validate(Host::Domain(host))?;
    }

    // DNS resolution + IP validation for SSRF prevention
    // First resolve hostname to addresses
    let resolved_addrs = tokio_net::lookup_host((host, port))
        .await
        .map_err(
            |e| UrlValidationError::DnsLookupFailed {
                hostname: host.to_string(),
                error: e.to_string(),
            },
        )?;

    // Then validate each resolved IP address
    for ip in resolved_addrs.map(|socket_addr| socket_addr.ip()) {
        let check_result = check_ip_is_global(ip);

        if let Err(reason) = check_result {
            return Err(UrlValidationError::NonGlobalIp {
                hostname: host.to_string(),
                ip: ip.to_string(),
                reason,
            });
        }
    }

    Ok(ValidatedUrl(parsed))
}

/// Checks whether an IP address is globally routable.
///
/// Returns `Ok(())` if the IP is globally routable, or `Err(&'static str)` with
/// the reason if it's not.
fn check_ip_is_global(ip: IpAddr) -> Result<(), &'static str> {
    match ip {
        IpAddr::V4(ipv4) => {
            if ipv4.is_private() {
                return Err("private address (RFC 1918)");
            }
            if ipv4.is_loopback() {
                return Err("loopback address (RFC 1122)");
            }
            if ipv4.is_link_local() {
                return Err(
                    "link-local address (RFC 3927), blocks AWS/GCP/Azure metadata at 169.254.169.254",
                );
            }
            if ipv4.is_documentation() {
                return Err("documentation/test address (RFC 5737)");
            }
            if ipv4.is_multicast() {
                return Err("multicast address (RFC 5771)");
            }
            if ipv4.is_broadcast() {
                return Err("broadcast address (RFC 919)");
            }

            let octets = ipv4.octets();

            if octets[0] == 0 {
                return Err("unspecified/this network (RFC 791)");
            }
            if octets[0] == 100 && (octets[1] & 0xC0) == 64 {
                return Err("CGNAT/shared address space (RFC 6598), blocks Alibaba Cloud metadata");
            }
            if octets[0] == 192 && octets[1] == 0 && octets[2] == 0 {
                return Err("IETF protocol assignments (RFC 6890)");
            }
            if octets[0] == 192 && octets[1] == 88 && octets[2] == 99 {
                return Err("6to4 relay anycast (RFC 7526)");
            }
            if octets[0] == 198 && (octets[1] == 18 || octets[1] == 19) {
                return Err("benchmarking address (RFC 2544)");
            }
            if octets[0] >= 240 {
                return Err("reserved address (240.0.0.0/4)");
            }

            Ok(())
        },
        IpAddr::V6(ipv6) => {
            if ipv6.is_unspecified() {
                return Err("unspecified address (::)");
            }
            if ipv6.is_loopback() {
                return Err("loopback address (::1)");
            }
            if ipv6.is_multicast() {
                return Err("multicast address (ff00::/8)");
            }
            if ipv6.is_unique_local() {
                return Err("unique local address (fc00::/7)");
            }
            if ipv6.is_unicast_link_local() {
                return Err("link-local unicast address (fe80::/10)");
            }

            let octets = ipv6.octets();

            // Manually check deprecated site-local unicast range fec0::/10.
            // Prefix condition: first octet is 0xfe and top two bits of second octet are
            // 0b11.
            if octets[0] == 0xfe && (octets[1] & 0xc0) == 0xc0 {
                return Err("site-local unicast address (fec0::/10)");
            }

            if octets[0] == 0x20 && octets[1] == 0x01 && octets[2] == 0x0d && octets[3] == 0xb8 {
                return Err("documentation address (RFC 3849)");
            }
            if octets[..10]
                .iter()
                .all(|byte| *byte == 0)
                && octets[10] == 0xff
                && octets[11] == 0xff
            {
                return Err("IPv4-mapped IPv6 address (::ffff:0:0/96)");
            }

            if let Some(ipv4) = ipv6.to_ipv4() {
                // Check if the embedded IPv4 address is global
                return check_ip_is_global(IpAddr::V4(ipv4));
            }

            Ok(())
        },
    }
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

    #[error("URL hostname {hostname:?} resolves to non-global IP address {ip}: {reason}")]
    NonGlobalIp {
        hostname: String,
        ip: String,
        reason: &'static str,
    },

    #[error("Provided URL host is not a domain")]
    UrlHostIsNotDomain,

    #[error("URL hostname {hostname:?} is not allowed")]
    HostNotAllowed { hostname: String },
}

/// Wrapper around `Url` that has been validated for security concerns by
/// [`validate()`](validate).
///
/// The type is stored in the database as a string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidatedUrl(Url);

impl ValidatedUrl {
    /// Creates a `ValidatedUrl` by applying full URL validation.
    pub async fn new(url: &str) -> Result<Self, UrlValidationError> {
        validate(url).await
    }

    /// Consumes the `ValidatedUrl` and returns the inner `Url`.
    pub fn into_inner(self) -> Url {
        self.0
    }

    /// Creates a `ValidatedUrl` without validation, for use in tests only.
    #[cfg(test)]
    pub fn new_unchecked(url: &str) -> Self {
        Self(Url::parse(url).expect("test URL must be parseable"))
    }
}

impl<DB> Type<DB> for ValidatedUrl
where
    DB: Database,
    String: Type<DB>,
{
    fn type_info() -> DB::TypeInfo {
        <String as Type<DB>>::type_info()
    }

    fn compatible(ty: &DB::TypeInfo) -> bool {
        <String as Type<DB>>::compatible(ty)
    }
}

impl<'q, DB> Encode<'q, DB> for ValidatedUrl
where
    DB: Database,
    String: Encode<'q, DB>,
{
    fn encode_by_ref(
        &self,
        buf: &mut <DB as Database>::ArgumentBuffer<'q>,
    ) -> Result<IsNull, BoxDynError> {
        <String as Encode<'q, DB>>::encode(self.0.to_string(), buf)
    }
}

// Decode implementation doesn't re-validate the URL as we assume that all URLs
// in the database have been validated before insertion. TODO: add validation if
// DNS look-up is removed?
impl<'r, DB> Decode<'r, DB> for ValidatedUrl
where
    DB: Database,
    String: Decode<'r, DB>,
{
    fn decode(value: <DB as Database>::ValueRef<'r>) -> Result<Self, BoxDynError> {
        let url = String::decode(value)?;
        let parsed = Url::parse(&url)?;
        Ok(Self(parsed))
    }
}

// Helper enum to allow passing either a single allowed base domain or
// a collection of allowed base domains to the validation function without
// unnecessary allocations.
enum EitherDomainsCollection<'a> {
    Domain(&'a Host<String>),
    DomainCollection(&'a [Host<String>]),
}

impl EitherDomainsCollection<'_> {
    fn validate(
        self,
        checking_host: Host<&str>,
    ) -> Result<(), UrlValidationError> {
        let Host::Domain(checking_host) = checking_host else {
            return Err(UrlValidationError::UrlHostIsNotDomain);
        };

        match self {
            EitherDomainsCollection::Domain(domain) => {
                let Host::Domain(domain) = domain else {
                    unreachable!("Allowed base URLs must have domain host, not URL")
                };

                if !is_same_or_subdomain(checking_host, domain) {
                    return Err(UrlValidationError::HostNotAllowed {
                        hostname: checking_host.to_string(),
                    });
                }
            },
            EitherDomainsCollection::DomainCollection(domains) => {
                let mut has_allowed_domains = false;

                let is_allowed = domains
                    .iter()
                    .filter_map(|host| match host {
                        Host::Domain(domain) => Some(domain.as_str()),
                        _ => None,
                    })
                    .inspect(|_| has_allowed_domains = true)
                    .any(|domain| is_same_or_subdomain(checking_host, domain));

                if has_allowed_domains && !is_allowed {
                    return Err(UrlValidationError::HostNotAllowed {
                        hostname: checking_host.to_string(),
                    });
                }
            },
        }

        Ok(())
    }
}

fn is_same_or_subdomain(
    checking_host: &str,
    allowed_domain: &str,
) -> bool {
    checking_host == allowed_domain
        || checking_host
            .strip_suffix(allowed_domain)
            .is_some_and(|prefix| prefix.ends_with('.'))
}

#[cfg(test)]
mod tests {
    use std::net::Ipv6Addr;

    use super::*;

    #[tokio::test]
    #[ntest::timeout(5_000)]
    async fn valid_https_urls() {
        let urls = [
            "https://example.com/webhook",
            "https://example.com:443/path",
            "https://example.com/path?order=123&status=paid",
        ];
        for url in urls {
            assert!(validate(url).await.is_ok());
        }
    }

    #[tokio::test]
    async fn validate_with_allowed_domains_rejects_disallowed_host() {
        let error = validate_with_allowed_base_many(
            "https://evil.com/webhook",
            &[Host::Domain("example.com".to_string())],
        )
        .await
        .expect_err("host must be rejected");

        assert!(matches!(
            error,
            UrlValidationError::HostNotAllowed {
                hostname
            } if hostname == "evil.com"
        ));
    }

    #[tokio::test]
    async fn reject_non_domain_hosts() {
        let urls = [
            "https://8.8.8.8/webhook",
            "https://1.1.1.1/webhook",
            "https://93.184.216.34/webhook",
            "https://[::1]/webhook",
            "https://[2001:db8::1]/webhook",
        ];

        for url in urls {
            let err = validate(url).await.unwrap_err();
            assert!(matches!(
                err,
                UrlValidationError::UrlHostIsNotDomain
            ));
        }
    }

    #[test]
    fn reject_non_global_ipv6_ranges() {
        let ipv6 = [
            "::",
            "::1",
            "ff02::1",
            "fc00::1",
            "fd12:3456:789a::1",
            "fe80::1",
            "fec0::1",
            "2001:db8::1",
            "::ffff:127.0.0.1",
        ];

        for value in ipv6 {
            let ip: Ipv6Addr = value.parse().unwrap();
            assert!(check_ip_is_global(IpAddr::V6(ip)).is_err());
        }
    }

    #[test]
    fn reject_ipv6_site_local_boundaries() {
        let start: Ipv6Addr = "fec0::".parse().unwrap();
        let end: Ipv6Addr = "feff:ffff:ffff:ffff:ffff:ffff:ffff:ffff"
            .parse()
            .unwrap();

        assert!(check_ip_is_global(IpAddr::V6(start)).is_err());
        assert!(check_ip_is_global(IpAddr::V6(end)).is_err());
    }

    #[test]
    fn accept_global_ipv6() {
        let ip: Ipv6Addr = "2606:4700:4700::1111".parse().unwrap();
        assert!(check_ip_is_global(IpAddr::V6(ip)).is_ok());
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
        let localhost_err = validate("https://localhost/webhook")
            .await
            .unwrap_err();
        assert!(matches!(
            localhost_err,
            UrlValidationError::NonGlobalIp { .. }
        ));

        assert!(check_ip_is_global("0.0.0.0".parse().unwrap()).is_err());
    }

    #[test]
    fn reject_local_and_internal_ips() {
        let ips: [IpAddr; 5] = [
            "10.0.0.1".parse().unwrap(),        // RFC 1918 private
            "172.16.0.1".parse().unwrap(),      // RFC 1918 private
            "192.168.1.1".parse().unwrap(),     // RFC 1918 private
            "169.254.169.254".parse().unwrap(), // Link-local (cloud metadata)
            "127.0.0.1".parse().unwrap(),       // Loopback
        ];

        for ip in ips {
            assert!(check_ip_is_global(ip).is_err());
        }
    }

    #[test]
    fn reject_loopback() {
        assert!(check_ip_is_global("127.0.0.1".parse().unwrap()).is_err());
        assert!(check_ip_is_global("127.255.255.255".parse().unwrap()).is_err());
    }

    #[test]
    fn reject_rfc1918_private() {
        let ips: [IpAddr; 5] = [
            // 10.0.0.0/8
            "10.0.0.1".parse().unwrap(),
            "10.255.255.255".parse().unwrap(),
            // 172.16.0.0/12
            "172.16.0.1".parse().unwrap(),
            "172.31.255.255".parse().unwrap(),
            // 192.168.0.0/16
            "192.168.1.1".parse().unwrap(),
        ];

        for ip in ips {
            assert!(check_ip_is_global(ip).is_err());
        }
    }

    #[test]
    fn reject_cgnat() {
        // 100.64.0.0/10 - CGNAT (RFC 6598), also covers Alibaba Cloud metadata
        // 100.100.100.200
        let ips: [IpAddr; 3] = [
            "100.64.0.1".parse().unwrap(),
            "100.100.100.200".parse().unwrap(),
            "100.127.255.255".parse().unwrap(),
        ];

        for ip in ips {
            assert!(check_ip_is_global(ip).is_err());
        }
    }

    #[test]
    fn reject_link_local_and_cloud_metadata() {
        // 169.254.0.0/16 - covers AWS/GCP/Azure metadata at 169.254.169.254
        let ips: [IpAddr; 2] = [
            "169.254.169.254".parse().unwrap(),
            "169.254.0.1".parse().unwrap(),
        ];

        for ip in ips {
            assert!(check_ip_is_global(ip).is_err());
        }
    }

    #[test]
    fn reject_documentation_and_test_nets() {
        let ips: [IpAddr; 3] = [
            // 192.0.2.0/24 - TEST-NET-1
            "192.0.2.1".parse().unwrap(),
            // 198.51.100.0/24 - TEST-NET-2
            "198.51.100.1".parse().unwrap(),
            // 203.0.113.0/24 - TEST-NET-3
            "203.0.113.1".parse().unwrap(),
        ];

        for ip in ips {
            assert!(check_ip_is_global(ip).is_err());
        }
    }

    #[test]
    fn reject_benchmarking() {
        // 198.18.0.0/15
        let ips: [IpAddr; 2] = [
            "198.18.0.1".parse().unwrap(),
            "198.19.255.255".parse().unwrap(),
        ];

        for ip in ips {
            assert!(check_ip_is_global(ip).is_err());
        }
    }

    #[test]
    fn reject_ietf_and_6to4() {
        let ips: [IpAddr; 2] = ["192.0.0.1".parse().unwrap(), "192.88.99.1".parse().unwrap()];

        for ip in ips {
            assert!(check_ip_is_global(ip).is_err());
        }
    }

    #[test]
    fn reject_multicast_and_reserved() {
        let ips: [IpAddr; 4] = [
            "224.0.0.1".parse().unwrap(),
            "239.255.255.255".parse().unwrap(),
            "240.0.0.1".parse().unwrap(),
            "255.255.255.255".parse().unwrap(),
        ];

        for ip in ips {
            assert!(check_ip_is_global(ip).is_err());
        }
    }

    #[test]
    fn cgnat_boundary() {
        // 100.63.255.255 is just below CGNAT range - should be allowed
        assert!(
            check_ip_is_global(IpAddr::V4(
                "100.63.255.255".parse().unwrap()
            ))
            .is_ok()
        );
        // 100.64.0.0 is start of CGNAT - should be blocked
        assert!(
            check_ip_is_global(IpAddr::V4(
                "100.64.0.0".parse().unwrap()
            ))
            .is_err()
        );
        // 100.127.255.255 is end of CGNAT - should be blocked
        assert!(
            check_ip_is_global(IpAddr::V4(
                "100.127.255.255".parse().unwrap()
            ))
            .is_err()
        );
        // 100.128.0.0 is just above CGNAT range - should be allowed
        assert!(
            check_ip_is_global(IpAddr::V4(
                "100.128.0.0".parse().unwrap()
            ))
            .is_ok()
        );
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

    #[test]
    fn domain_collection_accepts_single_exact_match_of_three() {
        let domains = [
            Host::Domain("first.example.com".to_string()),
            Host::Domain("match.example.com".to_string()),
            Host::Domain("third.example.com".to_string()),
        ];

        let result = EitherDomainsCollection::DomainCollection(&domains)
            .validate(Host::Domain("match.example.com"));

        assert!(result.is_ok());
    }

    #[test]
    fn domain_collection_accepts_single_subdomain_match_of_three() {
        let domains = [
            Host::Domain("first.example.com".to_string()),
            Host::Domain("cdn.example.com".to_string()),
            Host::Domain("third.example.com".to_string()),
        ];

        let result = EitherDomainsCollection::DomainCollection(&domains)
            .validate(Host::Domain("img.cdn.example.com"));

        assert!(result.is_ok());
    }

    #[test]
    fn domain_collection_rejects_when_none_match() {
        let domains = [
            Host::Domain("first.example.com".to_string()),
            Host::Domain("second.example.com".to_string()),
            Host::Domain("third.example.com".to_string()),
        ];

        let result = EitherDomainsCollection::DomainCollection(&domains)
            .validate(Host::Domain("example.com"));

        assert!(matches!(
            result,
            Err(UrlValidationError::HostNotAllowed {
                hostname
            }) if hostname == "example.com"
        ));
    }

    #[test]
    fn domain_collection_rejects_prefix_without_dot_separator() {
        let domains = [Host::Domain("example.com".to_string())];

        let result = EitherDomainsCollection::DomainCollection(&domains)
            .validate(Host::Domain("evilexample.com"));

        assert!(matches!(
            result,
            Err(UrlValidationError::HostNotAllowed {
                hostname
            }) if hostname == "evilexample.com"
        ));
    }
}
