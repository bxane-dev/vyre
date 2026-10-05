//! Bounded HTTPS transport for signed manifests. The application currently
//! ships no endpoint or signing keys, so this client is not invoked by the UI.

use crate::relay_manifest::{
    verify_manifest, ManifestError, VerifiedManifest, MAX_SIGNED_MANIFEST_BYTES,
};
use rusqlite::Connection;
use std::{collections::BTreeMap, sync::Mutex, time::Duration};

#[derive(Clone)]
pub struct ManifestFetchPolicy {
    /// Fixed HTTPS URL supplied by trusted application configuration.
    pub endpoint: String,
    /// Host pinned by the same trusted application configuration.
    pub allowed_host: String,
    /// Ed25519 key IDs and public keys pinned by the signed application build.
    pub trusted_keys: BTreeMap<String, [u8; 32]>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ManifestFetchError {
    InvalidConfiguration,
    Transport,
    UnexpectedStatus,
    ResponseTooLarge,
    Storage,
    CacheMismatch,
    Manifest(ManifestError),
}

impl std::fmt::Display for ManifestFetchError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::InvalidConfiguration => "Relay manifest endpoint configuration is invalid.",
            Self::Transport => "Unable to securely fetch the relay manifest.",
            Self::UnexpectedStatus => "Relay manifest service returned an unexpected status.",
            Self::ResponseTooLarge => "Relay manifest response exceeds the size limit.",
            Self::Storage => "Unable to persist or read the relay manifest cache.",
            Self::CacheMismatch => {
                "Cached relay manifest metadata does not match its signed payload."
            }
            Self::Manifest(error) => return error.fmt(formatter),
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for ManifestFetchError {}

/// Fetch, verify, and atomically cache a manifest. The generation is loaded
/// from SQLite before network I/O and checked again during the storage write.
pub async fn fetch_and_cache(
    policy: &ManifestFetchPolicy,
    database: &Mutex<Connection>,
    now_unix: u64,
) -> Result<VerifiedManifest, ManifestFetchError> {
    let minimum_generation = {
        let database = database.lock().map_err(|_| ManifestFetchError::Storage)?;
        crate::storage::relay_manifest_generation(&database)
            .map_err(|_| ManifestFetchError::Storage)?
    };
    let verified = fetch_and_verify(policy, now_unix, minimum_generation).await?;
    {
        let mut database = database.lock().map_err(|_| ManifestFetchError::Storage)?;
        crate::storage::cache_verified_relay_manifest(&mut database, &verified)
            .map_err(|_| ManifestFetchError::Storage)?;
    }
    Ok(verified)
}

/// Load the cached signed envelope only after re-verifying its signature,
/// expiry, signer, and persisted generation metadata.
pub fn load_cached(
    database: &Mutex<Connection>,
    trusted_keys: &BTreeMap<String, [u8; 32]>,
    now_unix: u64,
) -> Result<Option<VerifiedManifest>, ManifestFetchError> {
    let database = database.lock().map_err(|_| ManifestFetchError::Storage)?;
    let Some(cached) = crate::storage::cached_relay_manifest(&database)
        .map_err(|_| ManifestFetchError::Storage)?
    else {
        return Ok(None);
    };
    let verified = verify_manifest(
        cached.envelope_json.as_bytes(),
        trusted_keys,
        now_unix,
        cached.generation.saturating_sub(1),
    )
    .map_err(ManifestFetchError::Manifest)?;
    if verified.manifest().generation != cached.generation
        || verified.signer_key_id() != cached.signer_key_id
    {
        return Err(ManifestFetchError::CacheMismatch);
    }
    Ok(Some(verified))
}

/// Fetch one signed manifest over HTTPS, prohibit redirects, cap the body while
/// streaming, and verify it before returning any relay node data.
pub async fn fetch_and_verify(
    policy: &ManifestFetchPolicy,
    now_unix: u64,
    minimum_generation: u64,
) -> Result<VerifiedManifest, ManifestFetchError> {
    let endpoint = validate_endpoint(&policy.endpoint, &policy.allowed_host)?;
    let client = reqwest::Client::builder()
        .https_only(true)
        .connect_timeout(Duration::from_secs(4))
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| ManifestFetchError::Transport)?;
    let mut response = client
        .get(endpoint)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|_| ManifestFetchError::Transport)?;
    if response.status() != reqwest::StatusCode::OK {
        return Err(ManifestFetchError::UnexpectedStatus);
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_SIGNED_MANIFEST_BYTES as u64)
    {
        return Err(ManifestFetchError::ResponseTooLarge);
    }

    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| ManifestFetchError::Transport)?
    {
        if body.len().saturating_add(chunk.len()) > MAX_SIGNED_MANIFEST_BYTES {
            return Err(ManifestFetchError::ResponseTooLarge);
        }
        body.extend_from_slice(&chunk);
    }

    verify_manifest(&body, &policy.trusted_keys, now_unix, minimum_generation)
        .map_err(ManifestFetchError::Manifest)
}

fn validate_endpoint(
    endpoint: &str,
    allowed_host: &str,
) -> Result<reqwest::Url, ManifestFetchError> {
    if !is_dns_host(allowed_host) {
        return Err(ManifestFetchError::InvalidConfiguration);
    }
    let url =
        reqwest::Url::parse(endpoint).map_err(|_| ManifestFetchError::InvalidConfiguration)?;
    let Some(host) = url.host_str() else {
        return Err(ManifestFetchError::InvalidConfiguration);
    };
    if url.scheme() != "https"
        || !host.eq_ignore_ascii_case(allowed_host)
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(ManifestFetchError::InvalidConfiguration);
    }
    Ok(url)
}

fn is_dns_host(host: &str) -> bool {
    !host.parse::<std::net::IpAddr>().is_ok()
        && !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label.as_bytes()[0].is_ascii_alphanumeric()
                && label.as_bytes()[label.len() - 1].is_ascii_alphanumeric()
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_requires_https_and_exact_pinned_host() {
        assert!(validate_endpoint("https://api.vyre.example/v1/nodes", "api.vyre.example").is_ok());
        for invalid in [
            "http://api.vyre.example/v1/nodes",
            "https://evil.example/v1/nodes",
            "https://user:pass@api.vyre.example/v1/nodes",
            "https://api.vyre.example:444/v1/nodes",
            "https://api.vyre.example/v1/nodes?token=secret",
            "https://api.vyre.example/v1/nodes#fragment",
        ] {
            assert_eq!(
                validate_endpoint(invalid, "api.vyre.example"),
                Err(ManifestFetchError::InvalidConfiguration),
                "{invalid} should be rejected"
            );
        }
    }

    #[test]
    fn endpoint_rejects_malformed_pinned_host() {
        assert_eq!(
            validate_endpoint("https://api.vyre.example/v1/nodes", "bad host"),
            Err(ManifestFetchError::InvalidConfiguration)
        );
    }
}
