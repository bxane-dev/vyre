//! Validation for signed relay-node manifests. This code has no network or
//! Windows routing side effects; callers provide pinned public keys and time.

use base64::{engine::general_purpose::STANDARD, Engine as _};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

const DOMAIN: &[u8] = b"VYRE-RELAY-MANIFEST-V1\0";
pub const MAX_SIGNED_MANIFEST_BYTES: usize = 400 * 1024;
const MAX_PAYLOAD_BYTES: usize = 256 * 1024;
const MAX_NODES: usize = 64;
const MAX_MANIFEST_TTL_SECONDS: u64 = 24 * 60 * 60;
const MAX_CLOCK_SKEW_SECONDS: u64 = 5 * 60;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SignedManifestEnvelope {
    pub key_id: String,
    /// Base64 of the exact UTF-8 JSON payload bytes covered by the signature.
    pub payload: String,
    /// Base64 of a 64-byte Ed25519 signature.
    pub signature: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RelayManifest {
    pub schema_version: u16,
    pub generation: u64,
    pub issued_at: u64,
    pub expires_at: u64,
    pub nodes: Vec<RelayNode>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RelayNode {
    pub id: String,
    pub region: String,
    pub endpoint_host: String,
    pub endpoint_port: u16,
    /// Base64 of the 32-byte WireGuard public key.
    pub public_key: String,
    pub protocol: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedManifest {
    signer_key_id: String,
    manifest: RelayManifest,
    signed_envelope: Vec<u8>,
}

impl VerifiedManifest {
    pub fn signer_key_id(&self) -> &str {
        &self.signer_key_id
    }

    pub fn manifest(&self) -> &RelayManifest {
        &self.manifest
    }

    pub(crate) fn signed_envelope(&self) -> &[u8] {
        &self.signed_envelope
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ManifestError {
    EnvelopeTooLarge,
    InvalidEnvelope,
    InvalidKeyId,
    UnknownSigningKey,
    InvalidPinnedKey,
    InvalidBase64,
    InvalidSignatureEncoding,
    InvalidSignature,
    PayloadTooLarge,
    InvalidPayload,
    UnsupportedSchema,
    GenerationRollback,
    InvalidTimeWindow,
    ManifestExpired,
    GenerationOutOfRange,
    TooManyNodes,
    NoNodes,
    InvalidNodeId,
    InvalidRegion,
    InvalidEndpoint,
    InvalidPort,
    InvalidPublicKey,
    UnsupportedProtocol,
    DuplicateNodeId,
    DuplicateNodeKey,
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::EnvelopeTooLarge => "Signed relay manifest exceeds the size limit.",
            Self::InvalidEnvelope => "Signed relay manifest envelope is invalid.",
            Self::InvalidKeyId => "Manifest signing key identifier is invalid.",
            Self::UnknownSigningKey => "Manifest signing key is not trusted.",
            Self::InvalidPinnedKey => "Pinned manifest verification key is invalid.",
            Self::InvalidBase64 => "Manifest contains invalid or non-canonical base64.",
            Self::InvalidSignatureEncoding => "Manifest signature must contain exactly 64 bytes.",
            Self::InvalidSignature => "Manifest signature verification failed.",
            Self::PayloadTooLarge => "Manifest payload exceeds the size limit.",
            Self::InvalidPayload => "Manifest payload is invalid.",
            Self::UnsupportedSchema => "Manifest schema version is unsupported.",
            Self::GenerationRollback => {
                "Manifest generation is stale or has already been accepted."
            }
            Self::InvalidTimeWindow => "Manifest time window is invalid.",
            Self::ManifestExpired => "Manifest has expired or is not yet valid.",
            Self::GenerationOutOfRange => "Manifest generation is outside the supported range.",
            Self::TooManyNodes => "Manifest contains too many nodes.",
            Self::NoNodes => "Manifest contains no relay nodes.",
            Self::InvalidNodeId => "Relay node identifier is invalid.",
            Self::InvalidRegion => "Relay node region is invalid.",
            Self::InvalidEndpoint => "Relay endpoint host is invalid.",
            Self::InvalidPort => "Relay endpoint port is invalid.",
            Self::InvalidPublicKey => "Relay public key is invalid.",
            Self::UnsupportedProtocol => "Relay protocol is not supported.",
            Self::DuplicateNodeId => "Manifest contains duplicate relay node identifiers.",
            Self::DuplicateNodeKey => "Manifest contains duplicate relay public keys.",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for ManifestError {}

/// Verify an Ed25519 signature, parse the signed payload, and validate every
/// field before a caller can treat any node as an available candidate.
///
/// `trusted_keys` must come from the signed VYRE release, never from the
/// manifest or a remote response. `minimum_generation` is the highest
/// generation already accepted from this trust domain; persist the accepted
/// generation after this function succeeds to prevent replay of older data.
pub fn verify_manifest(
    envelope_bytes: &[u8],
    trusted_keys: &BTreeMap<String, [u8; 32]>,
    now_unix: u64,
    minimum_generation: u64,
) -> Result<VerifiedManifest, ManifestError> {
    if envelope_bytes.len() > MAX_SIGNED_MANIFEST_BYTES {
        return Err(ManifestError::EnvelopeTooLarge);
    }
    let envelope: SignedManifestEnvelope =
        serde_json::from_slice(envelope_bytes).map_err(|_| ManifestError::InvalidEnvelope)?;
    if !is_token(&envelope.key_id, 64) {
        return Err(ManifestError::InvalidKeyId);
    }
    let key_bytes = trusted_keys
        .get(&envelope.key_id)
        .ok_or(ManifestError::UnknownSigningKey)?;
    let verifying_key =
        VerifyingKey::from_bytes(key_bytes).map_err(|_| ManifestError::InvalidPinnedKey)?;

    let payload_limit_base64 = MAX_PAYLOAD_BYTES.div_ceil(3) * 4;
    if envelope.payload.len() > payload_limit_base64 {
        return Err(ManifestError::PayloadTooLarge);
    }
    let payload_bytes = decode_canonical_base64(&envelope.payload)?;
    if payload_bytes.len() > MAX_PAYLOAD_BYTES {
        return Err(ManifestError::PayloadTooLarge);
    }
    let signature_bytes = decode_canonical_base64(&envelope.signature)?;
    let signature = Signature::from_slice(&signature_bytes)
        .map_err(|_| ManifestError::InvalidSignatureEncoding)?;

    let mut signed_message =
        Vec::with_capacity(DOMAIN.len() + envelope.key_id.len() + 1 + payload_bytes.len());
    signed_message.extend_from_slice(DOMAIN);
    signed_message.extend_from_slice(envelope.key_id.as_bytes());
    signed_message.push(0);
    signed_message.extend_from_slice(&payload_bytes);
    verifying_key
        .verify_strict(&signed_message, &signature)
        .map_err(|_| ManifestError::InvalidSignature)?;

    let manifest: RelayManifest =
        serde_json::from_slice(&payload_bytes).map_err(|_| ManifestError::InvalidPayload)?;
    validate_manifest(&manifest, now_unix, minimum_generation)?;
    Ok(VerifiedManifest {
        signer_key_id: envelope.key_id,
        manifest,
        signed_envelope: envelope_bytes.to_vec(),
    })
}

fn validate_manifest(
    manifest: &RelayManifest,
    now_unix: u64,
    minimum_generation: u64,
) -> Result<(), ManifestError> {
    if manifest.schema_version != 1 {
        return Err(ManifestError::UnsupportedSchema);
    }
    if manifest.generation <= minimum_generation {
        return Err(ManifestError::GenerationRollback);
    }
    if manifest.generation > i64::MAX as u64 {
        return Err(ManifestError::GenerationOutOfRange);
    }
    if manifest.issued_at >= manifest.expires_at
        || manifest.expires_at - manifest.issued_at > MAX_MANIFEST_TTL_SECONDS
    {
        return Err(ManifestError::InvalidTimeWindow);
    }
    if manifest.issued_at > now_unix.saturating_add(MAX_CLOCK_SKEW_SECONDS)
        || manifest.expires_at <= now_unix
    {
        return Err(ManifestError::ManifestExpired);
    }
    if manifest.nodes.is_empty() {
        return Err(ManifestError::NoNodes);
    }
    if manifest.nodes.len() > MAX_NODES {
        return Err(ManifestError::TooManyNodes);
    }

    let mut node_ids = HashSet::with_capacity(manifest.nodes.len());
    let mut public_keys = HashSet::with_capacity(manifest.nodes.len());
    for node in &manifest.nodes {
        if !is_slug(&node.id, 64) {
            return Err(ManifestError::InvalidNodeId);
        }
        if !node_ids.insert(node.id.to_ascii_lowercase()) {
            return Err(ManifestError::DuplicateNodeId);
        }
        if !is_slug(&node.region, 64) {
            return Err(ManifestError::InvalidRegion);
        }
        if !is_endpoint_host(&node.endpoint_host) {
            return Err(ManifestError::InvalidEndpoint);
        }
        if node.endpoint_port == 0 {
            return Err(ManifestError::InvalidPort);
        }
        if node.protocol != "wireguard-udp" {
            return Err(ManifestError::UnsupportedProtocol);
        }
        let public_key = decode_canonical_base64(&node.public_key)
            .map_err(|_| ManifestError::InvalidPublicKey)?;
        let public_key: [u8; 32] = public_key
            .try_into()
            .map_err(|_| ManifestError::InvalidPublicKey)?;
        if !public_keys.insert(public_key) {
            return Err(ManifestError::DuplicateNodeKey);
        }
    }
    Ok(())
}

fn decode_canonical_base64(value: &str) -> Result<Vec<u8>, ManifestError> {
    let decoded = STANDARD
        .decode(value)
        .map_err(|_| ManifestError::InvalidBase64)?;
    if STANDARD.encode(&decoded) != value {
        return Err(ManifestError::InvalidBase64);
    }
    Ok(decoded)
}

fn is_token(value: &str, max_length: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn is_slug(value: &str, max_length: usize) -> bool {
    if value.is_empty() || value.len() > max_length {
        return false;
    }
    let bytes = value.as_bytes();
    bytes[0].is_ascii_alphanumeric()
        && bytes[bytes.len() - 1].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
}

fn is_endpoint_host(value: &str) -> bool {
    if value.is_empty() || value.len() > 253 || value.trim() != value {
        return false;
    }
    if value.parse::<std::net::IpAddr>().is_ok() {
        return true;
    }
    value.split('.').all(|label| {
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
    use ed25519_dalek::{Signer, SigningKey};

    const TEST_NOW: u64 = 1_800_000_000;
    const KEY_ID: &str = "vyre-test-key-1";

    fn sample_manifest() -> RelayManifest {
        RelayManifest {
            schema_version: 1,
            generation: 1,
            issued_at: TEST_NOW - 10,
            expires_at: TEST_NOW + 3600,
            nodes: vec![RelayNode {
                id: "eu-frankfurt-1".into(),
                region: "eu-central".into(),
                endpoint_host: "relay.example.net".into(),
                endpoint_port: 51820,
                public_key: STANDARD.encode([7u8; 32]),
                protocol: "wireguard-udp".into(),
            }],
        }
    }

    fn signed_envelope(manifest: &RelayManifest) -> (Vec<u8>, BTreeMap<String, [u8; 32]>) {
        let signing_key = SigningKey::from_bytes(&[42u8; 32]);
        let payload = serde_json::to_vec(manifest).unwrap();
        let mut signed_message = Vec::new();
        signed_message.extend_from_slice(DOMAIN);
        signed_message.extend_from_slice(KEY_ID.as_bytes());
        signed_message.push(0);
        signed_message.extend_from_slice(&payload);
        let signature = signing_key.sign(&signed_message);
        let envelope = SignedManifestEnvelope {
            key_id: KEY_ID.into(),
            payload: STANDARD.encode(payload),
            signature: STANDARD.encode(signature.to_bytes()),
        };
        let trusted = BTreeMap::from([(KEY_ID.into(), signing_key.verifying_key().to_bytes())]);
        (serde_json::to_vec(&envelope).unwrap(), trusted)
    }

    #[test]
    fn verifies_trusted_signature_and_valid_node() {
        let (bytes, trusted) = signed_envelope(&sample_manifest());
        let verified = verify_manifest(&bytes, &trusted, TEST_NOW, 0).unwrap();
        assert_eq!(verified.signer_key_id(), KEY_ID);
        assert_eq!(verified.manifest().nodes[0].id, "eu-frankfurt-1");
    }

    #[test]
    fn rejects_tampering_unknown_keys_and_replayed_generations() {
        let manifest = sample_manifest();
        let (bytes, trusted) = signed_envelope(&manifest);
        let mut envelope: SignedManifestEnvelope = serde_json::from_slice(&bytes).unwrap();
        let mut payload = STANDARD.decode(&envelope.payload).unwrap();
        payload[0] ^= 1;
        envelope.payload = STANDARD.encode(payload);
        let tampered = serde_json::to_vec(&envelope).unwrap();
        assert_eq!(
            verify_manifest(&tampered, &trusted, TEST_NOW, 0),
            Err(ManifestError::InvalidSignature)
        );

        let (bytes, trusted) = signed_envelope(&manifest);
        assert_eq!(
            verify_manifest(&bytes, &BTreeMap::new(), TEST_NOW, 0),
            Err(ManifestError::UnknownSigningKey)
        );
        assert_eq!(
            verify_manifest(&bytes, &trusted, TEST_NOW, 1),
            Err(ManifestError::GenerationRollback)
        );
    }

    #[test]
    fn rejects_expired_and_far_future_manifests() {
        let mut expired = sample_manifest();
        expired.expires_at = TEST_NOW;
        let (bytes, trusted) = signed_envelope(&expired);
        assert_eq!(
            verify_manifest(&bytes, &trusted, TEST_NOW, 0),
            Err(ManifestError::ManifestExpired)
        );

        let mut future = sample_manifest();
        future.issued_at = TEST_NOW + MAX_CLOCK_SKEW_SECONDS + 1;
        future.expires_at = future.issued_at + 100;
        let (bytes, trusted) = signed_envelope(&future);
        assert_eq!(
            verify_manifest(&bytes, &trusted, TEST_NOW, 0),
            Err(ManifestError::ManifestExpired)
        );
    }

    #[test]
    fn rejects_malformed_node_fields_and_unsupported_versions() {
        let mut bad_node = sample_manifest();
        bad_node.nodes[0].endpoint_host = "bad host".into();
        let (bytes, trusted) = signed_envelope(&bad_node);
        assert_eq!(
            verify_manifest(&bytes, &trusted, TEST_NOW, 0),
            Err(ManifestError::InvalidEndpoint)
        );

        let mut bad_protocol = sample_manifest();
        bad_protocol.nodes[0].protocol = "unknown".into();
        let (bytes, trusted) = signed_envelope(&bad_protocol);
        assert_eq!(
            verify_manifest(&bytes, &trusted, TEST_NOW, 0),
            Err(ManifestError::UnsupportedProtocol)
        );

        let mut bad_schema = sample_manifest();
        bad_schema.schema_version = 2;
        let (bytes, trusted) = signed_envelope(&bad_schema);
        assert_eq!(
            verify_manifest(&bytes, &trusted, TEST_NOW, 0),
            Err(ManifestError::UnsupportedSchema)
        );
    }

    #[test]
    fn rejects_duplicate_ids_keys_and_long_lived_manifests() {
        let mut duplicate = sample_manifest();
        duplicate.nodes[0].id = "eu-amsterdam-2".into();
        duplicate.nodes.push(RelayNode {
            id: "eu-amsterdam-2".into(),
            region: "eu-west".into(),
            endpoint_host: "relay2.example.net".into(),
            endpoint_port: 51820,
            public_key: STANDARD.encode([8u8; 32]),
            protocol: "wireguard-udp".into(),
        });
        let (bytes, trusted) = signed_envelope(&duplicate);
        assert_eq!(
            verify_manifest(&bytes, &trusted, TEST_NOW, 0),
            Err(ManifestError::DuplicateNodeId)
        );

        let mut duplicate_key = sample_manifest();
        duplicate_key.nodes.push(RelayNode {
            id: "eu-amsterdam-2".into(),
            region: "eu-west".into(),
            endpoint_host: "relay2.example.net".into(),
            endpoint_port: 51820,
            public_key: duplicate_key.nodes[0].public_key.clone(),
            protocol: "wireguard-udp".into(),
        });
        let (bytes, trusted) = signed_envelope(&duplicate_key);
        assert_eq!(
            verify_manifest(&bytes, &trusted, TEST_NOW, 0),
            Err(ManifestError::DuplicateNodeKey)
        );

        let mut long_lived = sample_manifest();
        long_lived.expires_at = long_lived.issued_at + MAX_MANIFEST_TTL_SECONDS + 1;
        let (bytes, trusted) = signed_envelope(&long_lived);
        assert_eq!(
            verify_manifest(&bytes, &trusted, TEST_NOW, 0),
            Err(ManifestError::InvalidTimeWindow)
        );
    }
}
