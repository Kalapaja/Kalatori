//! URL validation for order's redirect/image URL with SSRF attack prevention.
//!
//! # Security Model
//!
//! This module validates redirect/image URLs before the server makes outbound
//! HTTP requests to prevent Server-Side Request Forgery (SSRF) and related
//! attacks.
//!
//! ## Validation Checks Performed
//!
//! 1. **Length restriction** - Maximum 2048 characters
//! 2. **URL parsing** - Must conform to WHATWG URL Standard
//! 3. **HTTPS-only** - Rejects `http://`, `file://`, `ftp://`, etc.
//! 4. **Port restriction** - Only allows port 443 (HTTPS default)
//! 5. **No credentials** - Rejects `user:password@host` in URL
//! 6. **Base URL restriction** - Ensures the URL matches (is the same or
//!    belongs to base) the allowed base URL if provided
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
//! ## Image URL extra validation
//! When validating image URLs (`validate_image_with_allowed_base_many`), an
//! additional file-extension check is performed on top of all general checks
//! listed above. Only well-known raster/vector image extensions are accepted
//! (see `ALLOWED_IMAGE_EXTENSIONS`). Unknown extensions (e.g. `.php`, `.html`)
//! are rejected, reducing the attack surface for content-type confusion
//! attacks.
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

use std::net::IpAddr;
use thiserror::Error;
use tokio::net as tokio_net;
use url::{
    Host,
    Url,
};

/// Maximum allowed URL length.
const MAX_URL_LENGTH: usize = 2048;
const VALID_PORT: u16 = 443;

/// Known raster/vector image file extensions accepted for cart item image URLs.
/// All comparisons are case-insensitive.
pub const ALLOWED_IMAGE_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "webp", "svg", "avif", "ico", "bmp", "tiff", "tif",
];

/// Validates a URL that is used as a base URL to check user input URLs
/// belong to the allowlist.
///
/// Base URLs must:
/// - have https scheme
/// - have no credentials
/// - have no port or port 443
/// - have a domain host (not IP)
/// - have no fragment or query components
/// - have a path that ends with a slash
pub fn validate_base_url(url: &Url) -> Result<(), UrlValidationError> {
    validate_url_structure(url, true)?;

    if url.fragment().is_some() || url.query().is_some() {
        return Err(UrlValidationError::BaseHasFragmentOrQuery);
    }

    let url_path = url.path();

    if !url_path.ends_with("/") {
        return Err(UrlValidationError::BaseMissingTrailingSlash);
    }

    Ok(())
}

/// Validates a URL for security concerns described in the
/// [module](crate::utils::url_validation).
pub async fn validate(url: &str) -> Result<(), UrlValidationError> {
    validate_with_allowed_base_impl(url, None, None).await
}

/// Validates a URL same as [`validate()`], but also checks that the URL host
/// matches the allowed base URL provided in `allowed_base_url`.
pub async fn validate_with_allowed_base(
    url: &str,
    allowed_base_url: &Url,
) -> Result<(), UrlValidationError> {
    validate_with_allowed_base_impl(
        url,
        Some(EitherUrlsCollection::Url(
            allowed_base_url,
        )),
        None,
    )
    .await
}

/// Validates a URL same as [`validate()`], but also checks that the URL host
/// matches at least one of the allowed base URLs provided in
/// `allowed_base_urls`.
///
/// When `allowed_file_extensions` is `Some`, the URL path must end with one of
/// the provided extensions (case-insensitive).
pub async fn validate_with_allowed_base_many(
    url: &str,
    allowed_base_urls: &[Url],
    allowed_file_extensions: Option<&[&str]>,
) -> Result<(), UrlValidationError> {
    if allowed_base_urls.is_empty() {
        return Err(UrlValidationError::AllowedBaseImageUrlsEmpty);
    }

    validate_with_allowed_base_impl(
        url,
        Some(EitherUrlsCollection::UrlCollection(
            allowed_base_urls,
        )),
        allowed_file_extensions,
    )
    .await
}

/// Internal implementation of URL validation with optional allowed base url(s)
/// check.
///
/// Performs DNS resolution to verify that the URL endpoint does not target
/// internal/private infrastructure.
async fn validate_with_allowed_base_impl(
    url: &str,
    allowed_base_urls: Option<EitherUrlsCollection<'_>>,
    allowed_file_extensions: Option<&[&str]>,
) -> Result<(), UrlValidationError> {
    // Length check
    if url.len() > MAX_URL_LENGTH {
        return Err(UrlValidationError::TooLong);
    }

    // Parse URL (using WHATWG standard parser)
    let parsed = Url::parse(url)?;

    // Structural checks (scheme, port, credentials).
    // If `allowed_base_urls` is provided, also check that the host is a domain.
    validate_url_structure(&parsed, allowed_base_urls.is_some())?;

    // Validate against allowed base url if provided
    if let Some(allowed_base_urls) = allowed_base_urls {
        allowed_base_urls.validate(&parsed)?;
    }

    // File extension check.
    if let Some(extensions) = allowed_file_extensions {
        validate_url_file_extension(&parsed, extensions)?;
    }

    // Check ips
    match parsed.host() {
        Some(Host::Domain(domain)) => {
            // DNS resolution + IP validation for SSRF prevention
            // First resolve hostname to addresses
            let resolved_addrs = tokio_net::lookup_host((domain, VALID_PORT))
                .await
                .map_err(|e| {
                    tracing::debug!("DNS lookup failed for {domain:?}: {e}");

                    UrlValidationError::DnsLookupFailed {
                        hostname: domain.to_string(),
                    }
                })?;

            // Then validate each resolved IP address
            for ip in resolved_addrs.map(|socket_addr| socket_addr.ip()) {
                let check_result = check_ip_is_global(ip);

                if let Err(reason) = check_result {
                    return Err(UrlValidationError::NonGlobalIp {
                        hostname: Some(domain.to_string()),
                        ip: ip.to_string(),
                        reason,
                    });
                }
            }
        },
        // Be specific about variants
        Some(ip @ (Host::Ipv4(_) | Host::Ipv6(_))) => {
            let ip = host_to_ip(ip).expect("ip hosts must be convertible to IpAddr");
            check_ip_is_global(ip).map_err(
                |reason| UrlValidationError::NonGlobalIp {
                    hostname: None,
                    ip: ip.to_string(),
                    reason,
                },
            )?;
        },
        None => {
            return Err(UrlValidationError::UrlHasNoHost(
                url.to_string(),
            ));
        },
    }

    Ok(())
}

/// Validates the URL structure and components.
///
/// Checks:
/// - Scheme is HTTPS
/// - Port is 443 or not specified
/// - No credentials in the URL
/// - If `check_domain` is true, ensures the host is a domain (not IP).
fn validate_url_structure(
    url: &Url,
    check_domain: bool,
) -> Result<(), UrlValidationError> {
    if url.scheme() != "https" {
        return Err(UrlValidationError::InvalidScheme(
            url.scheme().to_string(),
        ));
    }

    if let Some(port) = url.port()
        && port != VALID_PORT
    {
        return Err(UrlValidationError::InvalidPort(port));
    }

    if !url.username().is_empty() || url.password().is_some() {
        return Err(UrlValidationError::HasCredentials);
    }

    if check_domain && url.domain().is_none() {
        return Err(UrlValidationError::UrlHostIsNotDomain);
    }

    Ok(())
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

            // Reject IPv4-compatible IPv6 addresses (::/96, excluding ::,
            // which is already handled by `is_unspecified` above).
            if octets[..12]
                .iter()
                .all(|byte| *byte == 0)
                && octets[12..]
                    .iter()
                    .any(|byte| *byte != 0)
            {
                return Err("IPv4-compatible IPv6 address (::/96)");
            }

            Ok(())
        },
    }
}

// Helper enum to allow passing either a single allowed base URL or
// a collection of allowed base URLs to the validation function without
// unnecessary allocations.
enum EitherUrlsCollection<'a> {
    Url(&'a Url),
    UrlCollection(&'a [Url]),
}

impl EitherUrlsCollection<'_> {
    fn validate(
        self,
        checking_url: &Url,
    ) -> Result<(), UrlValidationError> {
        match self {
            EitherUrlsCollection::Url(url) => {
                if !is_within_allowed_base(checking_url, url) {
                    return Err(UrlValidationError::UrlNotAllowed(
                        checking_url.to_string(),
                    ));
                }
            },
            EitherUrlsCollection::UrlCollection(urls) => {
                debug_assert!(
                    !urls.is_empty(),
                    "UrlCollection should never be empty, this is checked at the public API boundary"
                );

                let is_allowed = urls
                    .iter()
                    .any(|url| is_within_allowed_base(checking_url, url));

                if !is_allowed {
                    return Err(UrlValidationError::UrlNotAllowed(
                        checking_url.to_string(),
                    ));
                }
            },
        }

        Ok(())
    }
}

fn is_within_allowed_base(
    checking_url: &Url,
    allowed_base_url: &Url,
) -> bool {
    if checking_url.scheme() != allowed_base_url.scheme()
        || checking_url.host_str() != allowed_base_url.host_str()
        || checking_url.port_or_known_default() != allowed_base_url.port_or_known_default()
    {
        return false;
    }

    let mut candidate_segments = checking_url
        .path_segments()
        .expect("internal error: validated URLs must be able to provide path segments");

    let allowed_base_segments = allowed_base_url
        .path_segments()
        .expect("internal error: allowed base URLs must have path segments");

    // Base always has a trailing slash, so it always has at least one path segment
    // (even if it's empty). We filter out these empty segments to avoid the
    // following issues: base - https://example.com/api/   (path segments: ["api", ""])
    // candidate - https://example.com/api (path segments: ["api"])
    for base_segment in allowed_base_segments.filter(|s| !s.is_empty()) {
        match candidate_segments.next() {
            Some(segment) if segment == base_segment => {},
            _ => return false,
        }
    }

    true
}

fn host_to_ip<T>(host: Host<T>) -> Option<IpAddr> {
    match host {
        Host::Domain(_) => None,
        Host::Ipv4(ipv4) => Some(ipv4.into()),
        Host::Ipv6(ipv6) => Some(ipv6.into()),
    }
}

fn validate_url_file_extension(
    url: &Url,
    allowed: &[&str],
) -> Result<(), UrlValidationError> {
    let url_last_segment = url
        .path_segments()
        .and_then(|segments| {
            segments
                .filter(|s| !s.is_empty())
                .next_back()
        });

    let filename = match url_last_segment {
        Some(name) => name,
        None => return Err(UrlValidationError::FileInUrlNotFound),
    };

    let ext = match filename.rsplit('.').next() {
        Some(ext) if ext != filename => ext,
        _ => return Err(UrlValidationError::FileInUrlHasNoExtension),
    };

    if allowed
        .iter()
        .any(|&a| a.eq_ignore_ascii_case(ext))
    {
        Ok(())
    } else {
        let expected = allowed
            .iter()
            .map(|&s| s.to_string())
            .collect();

        Err(
            UrlValidationError::UnsupportedFileExtension {
                expected,
                actual: ext.to_string(),
            },
        )
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
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

    #[error("DNS lookup failed for {hostname:?}")]
    DnsLookupFailed { hostname: String },

    #[error("URL hostname {hostname:?} resolves to non-global IP address {ip}: {reason}")]
    NonGlobalIp {
        hostname: Option<String>,
        ip: String,
        reason: &'static str,
    },

    #[error("No host component found in the url - {0}")]
    UrlHasNoHost(String),

    #[error("Provided URL host is not a domain")]
    UrlHostIsNotDomain,

    #[error("URL {0:?} is not allowed")]
    UrlNotAllowed(String),

    #[error("Base URL must not contain fragment or query components")]
    BaseHasFragmentOrQuery,

    #[error("Base URL must have a path that ends with a slash")]
    BaseMissingTrailingSlash,

    #[error("Allowed base image URLs list is empty")]
    AllowedBaseImageUrlsEmpty,

    #[error("URL has no file in the path")]
    FileInUrlNotFound,

    #[error("URL file has no extension")]
    FileInUrlHasNoExtension,

    #[error(
        "File in URL has unsupported extension - {actual}; \
        expected one of: {expected:?}"
    )]
    UnsupportedFileExtension {
        expected: Vec<String>,
        actual: String,
    },
}

#[cfg(test)]
mod tests {
    use std::net::Ipv6Addr;

    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

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
    async fn validate_with_allowed_urls_rejects_disallowed_host() {
        let error = validate_with_allowed_base_many(
            "https://evil.com/webhook",
            &[url("https://example.com")],
            None,
        )
        .await
        .expect_err("host must be rejected");

        assert!(matches!(
            error,
            UrlValidationError::UrlNotAllowed(url) if url == "https://evil.com/webhook"
        ));
    }

    #[tokio::test]
    async fn validate_with_allowed_base_accepts_matching_url() {
        // Exercises the EitherUrlsCollection::Url (single) variant via the public API.
        let result = validate_with_allowed_base(
            "https://example.com/checkout",
            &url("https://example.com"),
        )
        .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn validate_with_allowed_base_rejects_different_host() {
        let result = validate_with_allowed_base(
            "https://evil.com/webhook",
            &url("https://example.com"),
        )
        .await;

        assert!(matches!(
            result,
            Err(UrlValidationError::UrlNotAllowed(_))
        ));
    }

    #[test]
    fn reject_non_domain_hosts() {
        let urls = [
            url("https://8.8.8.8/webhook"),
            url("https://1.1.1.1/webhook"),
            url("https://93.184.216.34/webhook"),
            url("https://[::1]/webhook"),
            url("https://[2001:db8::1]/webhook"),
        ];

        for url in urls {
            let err = validate_url_structure(&url, true).unwrap_err();
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
    fn url_collection_accepts_single_exact_match_of_three() {
        let urls = [
            url("https://first.example.com"),
            url("https://match.example.com"),
            url("https://third.example.com"),
        ];

        let result =
            EitherUrlsCollection::UrlCollection(&urls).validate(&url("https://match.example.com"));

        assert!(result.is_ok());
    }

    #[test]
    fn url_collection_accepts_url_under_allowed_base_path() {
        let allowed = [
            url("https://first.example.com"),
            url("https://image.cdn.example.com"),
            url("https://third.example.com"),
        ];

        let result = EitherUrlsCollection::UrlCollection(&allowed).validate(&url(
            "https://image.cdn.example.com/storage/image-123.png",
        ));

        assert!(result.is_ok());
    }

    #[test]
    fn url_collection_rejects_url_sharing_path_prefix_without_segment_boundary() {
        // "https://example.com/path-evil" must NOT match allowed "https://example.com/path".
        // The suffix "-evil" does not start with '/', '?', or '#'.
        let allowed = [url("https://example.com/path")];

        let result = EitherUrlsCollection::UrlCollection(&allowed)
            .validate(&url("https://example.com/path-evil"));

        assert!(matches!(
            result,
            Err(UrlValidationError::UrlNotAllowed(_))
        ));
    }

    #[test]
    fn url_single_variant_accepts_exact_match() {
        let result = EitherUrlsCollection::Url(&url("https://example.com/checkout"))
            .validate(&url("https://example.com/checkout"));

        assert!(result.is_ok());
    }

    #[test]
    fn url_single_variant_accepts_sub_path() {
        let result = EitherUrlsCollection::Url(&url("https://example.com/checkout")).validate(
            &url("https://example.com/checkout/order/42"),
        );

        assert!(result.is_ok());
    }

    #[test]
    fn url_single_variant_rejects_different_host() {
        let result = EitherUrlsCollection::Url(&url("https://example.com/checkout"))
            .validate(&url("https://evil.com/checkout"));

        assert!(matches!(
            result,
            Err(UrlValidationError::UrlNotAllowed(_))
        ));
    }

    #[test]
    #[should_panic = "UrlCollection should never be empty, this is checked at the public API boundary"]
    fn empty_url_collection_accepts_any_url() {
        #[expect(let_underscore_drop)]
        let _ =
            EitherUrlsCollection::UrlCollection(&[]).validate(&url("https://anything.com/path"));
    }

    #[test]
    fn url_collection_rejects_when_none_match() {
        let domains = [
            url("https://first.example.com"),
            url("https://second.example.com"),
            url("https://third.example.com"),
        ];

        let result =
            EitherUrlsCollection::UrlCollection(&domains).validate(&url("https://hexample.com"));

        assert!(matches!(
            result,
            Err(UrlValidationError::UrlNotAllowed(url)) if url == "https://hexample.com/"
        ));
    }

    #[test]
    fn url_collection_rejects_host_sharing_only_a_common_prefix() {
        // "https://evilexample.com" must NOT match allowed "https://example.com".
        let domains = [url("https://example.com")];

        let result =
            EitherUrlsCollection::UrlCollection(&domains).validate(&url("https://evilexample.com"));

        assert!(matches!(
            result,
            Err(UrlValidationError::UrlNotAllowed(url)) if url == "https://evilexample.com/"
        ));
    }

    #[test]
    fn validate_base_url_rejects_fragments_and_queries() {
        assert!(matches!(
            validate_base_url(&url(
                "https://example.com/path/?query=1"
            )),
            Err(UrlValidationError::BaseHasFragmentOrQuery)
        ));
        assert!(matches!(
            validate_base_url(&url(
                "https://example.com/path/#fragment"
            )),
            Err(UrlValidationError::BaseHasFragmentOrQuery)
        ));
    }

    #[test]
    fn validate_base_url_rejects_missing_trailing_slash() {
        assert!(matches!(
            validate_base_url(&url("https://example.com/path")),
            Err(UrlValidationError::BaseMissingTrailingSlash)
        ));
    }

    #[test]
    fn validate_base_url_accepts_valid_base_urls() {
        assert!(validate_base_url(&url("https://example.com/")).is_ok());
        assert!(validate_base_url(&url("https://example.com/path/")).is_ok());
    }

    #[test]
    fn validate_base_url_rejects_credentials() {
        assert!(matches!(
            validate_base_url(&url("https://user:pass@example.com/")),
            Err(UrlValidationError::HasCredentials)
        ));
        assert!(matches!(
            validate_base_url(&url("https://user@example.com/")),
            Err(UrlValidationError::HasCredentials)
        ));
    }

    #[test]
    fn validate_base_url_rejects_non_443_port() {
        assert!(matches!(
            validate_base_url(&url("https://example.com:8080/")),
            Err(UrlValidationError::InvalidPort(8080))
        ));
    }

    #[test]
    fn validate_base_url_accepts_explicit_port_443() {
        // Port 443 is the HTTPS default and must be accepted.
        assert!(validate_base_url(&url("https://example.com:443/")).is_ok());
    }

    #[tokio::test]
    async fn validate_accepts_globally_routable_ip() {
        // When no allowed_base is given (validate, not validate_with_allowed_base*),
        // check_domain is false so a bare IP is allowed as long as it is globally
        // routable. 93.184.216.34 is example.com's IP - a real,
        // globally-routable address.
        let result = validate("https://93.184.216.34/webhook").await;
        assert!(
            result.is_ok(),
            "globally-routable IP should be accepted: {result:?}"
        );
    }

    #[test]
    fn reject_ipv4_compatible_ipv6() {
        // ::1.2.3.4 is represented as ::102:304 in hex.
        // These are IPv4-compatible IPv6 addresses in the ::/96 range.
        let cases: &[&str] = &[
            "::1.2.3.4", // ::ffff:0:0/96 IPv4-compatible
            "::102:304", // same address in hex
            "::7f00:1",  // ::127.0.0.1 - loopback wrapped
        ];
        for addr in cases {
            let ip: std::net::Ipv6Addr = addr
                .parse()
                .unwrap_or_else(|_| panic!("parse {addr}"));
            assert!(
                check_ip_is_global(IpAddr::V6(ip)).is_err(),
                "IPv4-compatible IPv6 {addr} should be rejected"
            );
        }
    }

    #[test]
    fn is_within_allowed_base_treats_implicit_and_explicit_443_as_equal() {
        // https://example.com and https://example.com:443 refer to the same
        // origin; is_within_allowed_base must match either way.
        let base_implicit = url("https://example.com/");
        let base_explicit = url("https://example.com:443/");
        let candidate = url("https://example.com/path");

        assert!(
            is_within_allowed_base(&candidate, &base_implicit),
            "implicit port 443 in base should match candidate"
        );
        assert!(
            is_within_allowed_base(&candidate, &base_explicit),
            "explicit port 443 in base should match candidate"
        );

        // And the candidate with explicit :443 should match an implicit base.
        let candidate_explicit = url("https://example.com:443/path");
        assert!(
            is_within_allowed_base(&candidate_explicit, &base_implicit),
            "explicit port 443 in candidate should match implicit base"
        );
    }

    #[tokio::test]
    async fn validate_with_allowed_base_many_empty_slice() {
        // An empty allowlist imposes no host restriction — any otherwise-valid
        // URL is accepted.
        let result =
            validate_with_allowed_base_many("https://example.com/webhook", &[], None).await;
        assert!(matches!(
            result,
            Err(UrlValidationError::AllowedBaseImageUrlsEmpty)
        ));
    }

    // ----- check_image_extension -----

    #[test]
    fn image_extension_accepts_known_extensions() {
        let valid = [
            "https://cdn.example.com/product.jpg",
            "https://cdn.example.com/product.jpeg",
            "https://cdn.example.com/product.png",
            "https://cdn.example.com/product.gif",
            "https://cdn.example.com/product.webp",
            "https://cdn.example.com/product.svg",
            "https://cdn.example.com/product.avif",
            "https://cdn.example.com/product.ico",
            "https://cdn.example.com/product.bmp",
            "https://cdn.example.com/product.tiff",
            "https://cdn.example.com/product.tif",
            // Case-insensitive
            "https://cdn.example.com/product.JPG",
            "https://cdn.example.com/product.PNG",
            // Nested path
            "https://cdn.example.com/store/items/product.webp",
            // Query string and fragment must not interfere with extension detection
            "https://cdn.example.com/product.png?v=42",
            "https://cdn.example.com/product.png#section",
        ];

        for raw in valid {
            assert!(
                validate_url_file_extension(&url(raw), ALLOWED_IMAGE_EXTENSIONS).is_ok(),
                "expected ok for {raw}"
            );
        }
    }

    #[test]
    fn image_extension_rejects_unknown_extensions() {
        let invalid = [
            (
                "https://cdn.example.com/file.php",
                "php",
            ),
            (
                "https://cdn.example.com/file.html",
                "html",
            ),
            ("https://cdn.example.com/file.js", "js"),
            (
                "https://cdn.example.com/file.exe",
                "exe",
            ),
            (
                "https://cdn.example.com/file.pdf",
                "pdf",
            ),
        ];

        for (raw, expected_ext) in invalid {
            match validate_url_file_extension(&url(raw), ALLOWED_IMAGE_EXTENSIONS) {
                Err(UrlValidationError::UnsupportedFileExtension {
                    actual: extension, ..
                }) => {
                    assert_eq!(
                        extension, expected_ext,
                        "wrong extension reported for {raw}"
                    );
                },
                other => panic!("expected UnsupportedImageExtension for {raw}, got {other:?}"),
            }
        }
    }

    #[test]
    fn image_extension_rejects_missing_extension() {
        // No dot in the last segment → extension is empty string → rejected.
        let no_ext = [
            (
                "https://cdn.example.com/image",
                UrlValidationError::FileInUrlHasNoExtension,
            ),
            (
                "https://cdn.example.com/",
                UrlValidationError::FileInUrlNotFound,
            ),
        ];

        for (raw, expected_err) in no_ext {
            assert!(
                matches!(
                    validate_url_file_extension(&url(raw), ALLOWED_IMAGE_EXTENSIONS),
                    Err(err) if err == expected_err
                ),
                "expected UnsupportedImageExtension with empty extension for {raw}"
            );
        }
    }

    #[tokio::test]
    #[ntest::timeout(5_000)]
    async fn validate_with_allowed_base_many_accepts_valid_image_url() {
        let bases = [url("https://example.com/")];
        assert!(
            validate_with_allowed_base_many(
                "https://example.com/products/shirt.png",
                &bases,
                Some(ALLOWED_IMAGE_EXTENSIONS),
            )
            .await
            .is_ok()
        );
    }

    #[tokio::test]
    async fn validate_with_allowed_base_many_rejects_unknown_image_extension() {
        let bases = [url("https://example.com/")];
        let err = validate_with_allowed_base_many(
            "https://example.com/products/script.php",
            &bases,
            Some(ALLOWED_IMAGE_EXTENSIONS),
        )
        .await
        .unwrap_err();

        assert!(
            matches!(
                err,
                UrlValidationError::UnsupportedFileExtension { .. }
            ),
            "expected UnsupportedImageExtension, got: {err}"
        );
    }

    #[tokio::test]
    async fn validate_with_allowed_base_many_rejects_disallowed_host_with_extension_check() {
        let bases = [url("https://example.com/")];
        let err = validate_with_allowed_base_many(
            "https://evil.com/image.png",
            &bases,
            Some(ALLOWED_IMAGE_EXTENSIONS),
        )
        .await
        .unwrap_err();

        assert!(
            matches!(
                err,
                UrlValidationError::UrlNotAllowed(_)
            ),
            "expected UrlNotAllowed, got: {err}"
        );
    }

    #[tokio::test]
    async fn validate_with_allowed_base_many_with_extension_check_empty_slice() {
        let result = validate_with_allowed_base_many(
            "https://cdn.example.com/img.png",
            &[],
            Some(ALLOWED_IMAGE_EXTENSIONS),
        )
        .await;
        assert!(matches!(
            result,
            Err(UrlValidationError::AllowedBaseImageUrlsEmpty)
        ));
    }
}
