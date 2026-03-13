use hmac::{
    Hmac,
    Mac,
};
use http::{
    HeaderValue,
    Method,
};
use secrecy::{
    ExposeSecret,
    SecretSlice,
};
use sha2::Sha256;

/// HMAC-SHA256 signature validator
pub(crate) type HmacSha256 = Hmac<Sha256>;

pub const SIGNATURE_HEADER: &str = "X-KALATORI-SIGNATURE";
pub const TIMESTAMP_HEADER: &str = "X-KALATORI-TIMESTAMP";

/// Configuration for HMAC validation middleware
#[derive(Clone)]
pub struct HmacConfig {
    /// The secret key used for HMAC calculation
    pub(crate) secret_key: SecretSlice<u8>,
    /// Maximum age of the request in seconds (prevents replay attacks)
    pub(crate) max_age_seconds: u64,
}

impl HmacConfig {
    pub fn new(
        secret_key: impl Into<SecretSlice<u8>>,
        max_age_seconds: u64,
    ) -> Self {
        Self {
            secret_key: secret_key.into(),
            max_age_seconds,
        }
    }
}

/// Calculates HMAC-SHA256
fn calculate_hmac(
    secret_key: &SecretSlice<u8>,
    method: &str,
    path: &str,
    query_or_body: &[u8],
    timestamp: &str,
) -> Hmac<Sha256> {
    let mut mac = HmacSha256::new_from_slice(secret_key.expose_secret())
        .expect("HMAC can take key of any size");

    mac.update(method.as_bytes());
    mac.update(b"\n");
    mac.update(path.as_bytes());
    mac.update(b"\n");
    mac.update(query_or_body);
    mac.update(b"\n");
    mac.update(timestamp.as_bytes());

    mac
}

fn sorted_query_string(query: &str) -> String {
    let mut pairs: Vec<(&str, &str)> = query
        .split('&')
        .filter_map(|pair| {
            let mut split = pair.splitn(2, '=');
            let key = split.next()?;
            let value = split.next().unwrap_or("");
            Some((key, value))
        })
        .collect();

    pairs.sort_by(|a, b| a.0.cmp(b.0));

    pairs
        .into_iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&")
}

/// Creates HMAC from request parts. Returns `None` for unsupported methods.
pub(crate) fn hmac_from_request_parts(
    config: &HmacConfig,
    method: &Method,
    path: &str,
    query_params: Option<&str>,
    body_bytes: &[u8],
    timestamp: &str,
) -> Option<Hmac<Sha256>> {
    let sorted_params = sorted_query_string(query_params.unwrap_or(""));

    let query_or_body = match *method {
        Method::GET => sorted_params.as_bytes(),
        Method::POST => body_bytes,
        _ => return None,
    };

    Some(calculate_hmac(
        &config.secret_key,
        method.as_str(),
        path,
        query_or_body,
        timestamp,
    ))
}

pub(crate) fn timestamp_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("System time before UNIX epoch")
        .as_secs()
}

// TODO: it's used in example only. Probably will be better to move it out of
// here
/// Computes a hex-encoded HMAC-SHA256 webhook signature using the same
/// algorithm as production webhook delivery.
///
/// This is useful for generating test vectors or verifying signatures
/// outside of the `reqwest` middleware flow.
///
/// **Note**: This function always signs the raw `body` bytes and is intended
/// for POST webhooks only. For GET requests (which sign sorted query
/// parameters instead of the body), use [`hmac_from_request_parts`].
///
/// The signed message is: `{method}\n{path}\n{body}\n{timestamp}`
pub fn compute_webhook_signature(
    secret: &[u8],
    method: &str,
    path: &str,
    body: &[u8],
    timestamp: &str,
) -> String {
    let secret_key: SecretSlice<u8> = secret.to_vec().into();
    let mac = calculate_hmac(
        &secret_key,
        method,
        path,
        body,
        timestamp,
    );
    const_hex::encode(mac.finalize().into_bytes())
}

pub fn add_headers_to_reqwest(
    config: &HmacConfig,
    request: &mut reqwest::Request,
) {
    let timestamp = timestamp_secs().to_string();

    let signature = hmac_from_request_parts(
        config,
        request.method(),
        request.url().path(),
        request.url().query(),
        request
            .body()
            .and_then(|b| b.as_bytes())
            .unwrap_or(&[]),
        &timestamp,
    )
    .unwrap();

    let encoded_signature = const_hex::encode(signature.finalize().into_bytes());

    let headers = request.headers_mut();

    headers.insert(
        TIMESTAMP_HEADER,
        HeaderValue::from_str(&timestamp).unwrap(),
    );
    headers.insert(
        SIGNATURE_HEADER,
        HeaderValue::from_str(&encoded_signature).unwrap(),
    );
}
