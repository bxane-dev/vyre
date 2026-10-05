//! Signed, short-lived relay-session authorization and local replay protection.

use crate::relay_manifest::VerifiedManifest;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use ed25519_dalek::{Signature, VerifyingKey};
use ipnet::IpNet;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    net::IpAddr,
};

const DOMAIN: &[u8] = b"VYRE-RELAY-SESSION-V1\0";
const MAX_ENVELOPE_BYTES: usize = 64 * 1024;
const MAX_GRANT_TTL_SECONDS: u64 = 10 * 60;
const MAX_CLOCK_SKEW_SECONDS: u64 = 5 * 60;
const MAX_ALLOWED_PREFIXES: usize = 32;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SignedSessionEnvelope {
    pub key_id: String,
    pub payload: String,
    pub signature: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RelaySessionGrant {
    pub schema_version: u16,
    pub session_id: String,
    pub node_id: String,
    /// Base64 WireGuard client public key to which this grant is bound.
    pub client_public_key: String,
    pub issued_at: u64,
    pub expires_at: u64,
    /// Exact prefixes approved by the control plane for this one-time session.
    pub allowed_prefixes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedSessionGrant {
    key_id: String,
    grant: RelaySessionGrant,
}

/// A verified grant whose one-time use has been durably recorded.
pub struct ConsumedSessionGrant(VerifiedSessionGrant);

impl ConsumedSessionGrant {
    pub fn grant(&self) -> &RelaySessionGrant {
        self.0.grant()
    }
    pub fn key_id(&self) -> &str {
        self.0.key_id()
    }
}

impl VerifiedSessionGrant {
    pub fn grant(&self) -> &RelaySessionGrant {
        &self.grant
    }
    pub fn key_id(&self) -> &str {
        &self.key_id
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionGrantError {
    InvalidEnvelope,
    InvalidKeyId,
    UnknownSigningKey,
    InvalidPinnedKey,
    InvalidBase64,
    InvalidSignature,
    InvalidPayload,
    UnsupportedSchema,
    InvalidSessionId,
    UnknownNode,
    InvalidClientKey,
    InvalidTimeWindow,
    Expired,
    InvalidAllowedPrefixes,
    PrefixScopeMismatch,
    Replay,
}

impl std::fmt::Display for SessionGrantError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidEnvelope => "Signed relay session grant is invalid.",
            Self::InvalidKeyId => "Relay session signing key identifier is invalid.",
            Self::UnknownSigningKey => "Relay session signing key is not trusted.",
            Self::InvalidPinnedKey => "Pinned relay session key is invalid.",
            Self::InvalidBase64 => "Relay session grant has invalid base64 encoding.",
            Self::InvalidSignature => "Relay session signature verification failed.",
            Self::InvalidPayload => "Relay session grant payload is invalid.",
            Self::UnsupportedSchema => "Relay session grant schema is unsupported.",
            Self::InvalidSessionId => "Relay session identifier is invalid.",
            Self::UnknownNode => "Relay session grant names an unverified node.",
            Self::InvalidClientKey => "Relay session grant has an invalid client key.",
            Self::InvalidTimeWindow => "Relay session grant time window is invalid.",
            Self::Expired => "Relay session grant is expired or not yet valid.",
            Self::InvalidAllowedPrefixes => "Relay session grant has invalid destination prefixes.",
            Self::PrefixScopeMismatch => {
                "Relay session grant does not match the requested route scope."
            }
            Self::Replay => "Relay session grant was already consumed.",
        })
    }
}

impl std::error::Error for SessionGrantError {}

pub fn verify_session_grant(
    envelope_bytes: &[u8],
    trusted_keys: &BTreeMap<String, [u8; 32]>,
    manifest: &VerifiedManifest,
    requested_prefixes: &[IpNet],
    now_unix: u64,
) -> Result<VerifiedSessionGrant, SessionGrantError> {
    if envelope_bytes.is_empty() || envelope_bytes.len() > MAX_ENVELOPE_BYTES {
        return Err(SessionGrantError::InvalidEnvelope);
    }
    let envelope: SignedSessionEnvelope =
        serde_json::from_slice(envelope_bytes).map_err(|_| SessionGrantError::InvalidEnvelope)?;
    if !is_token(&envelope.key_id, 64) {
        return Err(SessionGrantError::InvalidKeyId);
    }
    let pinned = trusted_keys
        .get(&envelope.key_id)
        .ok_or(SessionGrantError::UnknownSigningKey)?;
    let verifying_key =
        VerifyingKey::from_bytes(pinned).map_err(|_| SessionGrantError::InvalidPinnedKey)?;
    let payload = decode_canonical(&envelope.payload)?;
    let signature_bytes = decode_canonical(&envelope.signature)?;
    let signature =
        Signature::from_slice(&signature_bytes).map_err(|_| SessionGrantError::InvalidSignature)?;
    let mut signed_message =
        Vec::with_capacity(DOMAIN.len() + envelope.key_id.len() + 1 + payload.len());
    signed_message.extend_from_slice(DOMAIN);
    signed_message.extend_from_slice(envelope.key_id.as_bytes());
    signed_message.push(0);
    signed_message.extend_from_slice(&payload);
    verifying_key
        .verify_strict(&signed_message, &signature)
        .map_err(|_| SessionGrantError::InvalidSignature)?;

    let grant: RelaySessionGrant =
        serde_json::from_slice(&payload).map_err(|_| SessionGrantError::InvalidPayload)?;
    if grant.schema_version != 1 {
        return Err(SessionGrantError::UnsupportedSchema);
    }
    if !is_token(&grant.session_id, 64) {
        return Err(SessionGrantError::InvalidSessionId);
    }
    if !manifest
        .manifest()
        .nodes
        .iter()
        .any(|node| node.id == grant.node_id)
    {
        return Err(SessionGrantError::UnknownNode);
    }
    let client_key = decode_canonical(&grant.client_public_key)
        .map_err(|_| SessionGrantError::InvalidClientKey)?;
    if client_key.len() != 32 {
        return Err(SessionGrantError::InvalidClientKey);
    }
    if grant.issued_at > now_unix.saturating_add(MAX_CLOCK_SKEW_SECONDS)
        || grant.expires_at <= grant.issued_at
        || grant.expires_at - grant.issued_at > MAX_GRANT_TTL_SECONDS
    {
        return Err(SessionGrantError::InvalidTimeWindow);
    }
    if now_unix > grant.expires_at
        || grant.issued_at > now_unix.saturating_add(MAX_CLOCK_SKEW_SECONDS)
    {
        return Err(SessionGrantError::Expired);
    }

    let granted = parse_prefixes(&grant.allowed_prefixes)?;
    let requested = requested_prefixes.iter().cloned().collect::<BTreeSet<_>>();
    if granted != requested {
        return Err(SessionGrantError::PrefixScopeMismatch);
    }

    Ok(VerifiedSessionGrant {
        key_id: envelope.key_id,
        grant,
    })
}

/// Atomically records one-time consumption before a caller starts a tunnel.
/// Entries remain until expiry so a consumed grant cannot be replayed after a
/// restart. This client-side guard complements server-side expiry/revocation.
pub fn consume_once(
    database: &mut Connection,
    verified: &VerifiedSessionGrant,
    now_unix: u64,
) -> Result<ConsumedSessionGrant, SessionGrantError> {
    if now_unix > verified.grant.expires_at {
        return Err(SessionGrantError::Expired);
    }
    let expires_at = i64::try_from(verified.grant.expires_at)
        .map_err(|_| SessionGrantError::InvalidTimeWindow)?;
    let now = i64::try_from(now_unix).map_err(|_| SessionGrantError::InvalidTimeWindow)?;
    let transaction = database
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|_| SessionGrantError::Replay)?;
    transaction
        .execute(
            "DELETE FROM relay_grant_replay WHERE expires_at < ?1",
            [now],
        )
        .map_err(|_| SessionGrantError::Replay)?;
    let inserted = transaction.execute(
        "INSERT OR IGNORE INTO relay_grant_replay(session_id,expires_at,accepted_at) VALUES(?1,?2,?3)",
        params![verified.grant.session_id, expires_at, chrono::Utc::now().to_rfc3339()],
    ).map_err(|_| SessionGrantError::Replay)?;
    if inserted != 1 {
        return Err(SessionGrantError::Replay);
    }
    transaction
        .commit()
        .map_err(|_| SessionGrantError::Replay)?;
    Ok(ConsumedSessionGrant(verified.clone()))
}

fn parse_prefixes(values: &[String]) -> Result<BTreeSet<IpNet>, SessionGrantError> {
    if values.is_empty() || values.len() > MAX_ALLOWED_PREFIXES {
        return Err(SessionGrantError::InvalidAllowedPrefixes);
    }
    let mut prefixes = BTreeSet::new();
    for value in values {
        let prefix = value
            .parse::<IpNet>()
            .map_err(|_| SessionGrantError::InvalidAllowedPrefixes)?;
        if prefix.to_string() != *value
            || prefix.prefix_len() == 0
            || prefix.addr().is_unspecified()
            || prefix.addr().is_multicast()
            || match prefix.addr() {
                IpAddr::V4(_) => prefix.prefix_len() < 16,
                IpAddr::V6(_) => prefix.prefix_len() < 32,
            }
            || !prefixes.insert(prefix)
        {
            return Err(SessionGrantError::InvalidAllowedPrefixes);
        }
    }
    Ok(prefixes)
}

fn decode_canonical(value: &str) -> Result<Vec<u8>, SessionGrantError> {
    let decoded = STANDARD
        .decode(value)
        .map_err(|_| SessionGrantError::InvalidBase64)?;
    if STANDARD.encode(&decoded) != value {
        return Err(SessionGrantError::InvalidBase64);
    }
    Ok(decoded)
}

fn is_token(value: &str, maximum: usize) -> bool {
    (16..=maximum).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::{relay_tunnel::tests::verified_manifest, storage};
    use ed25519_dalek::{Signer, SigningKey};
    use std::path::Path;

    const NOW: u64 = 1_800_000_000;

    pub(crate) fn signed_grant(
        expires_at: u64,
        prefixes: Vec<String>,
    ) -> (Vec<u8>, BTreeMap<String, [u8; 32]>) {
        let signer = SigningKey::from_bytes(&[0x43; 32]);
        let grant = RelaySessionGrant {
            schema_version: 1,
            session_id: "grant-session-012345".into(),
            node_id: "node-a".into(),
            client_public_key: STANDARD.encode(
                x25519_dalek::PublicKey::from(&x25519_dalek::StaticSecret::from([9u8; 32]))
                    .to_bytes(),
            ),
            issued_at: NOW - 10,
            expires_at,
            allowed_prefixes: prefixes,
        };
        let payload = serde_json::to_vec(&grant).unwrap();
        let mut message = DOMAIN.to_vec();
        message.extend_from_slice(b"test-session-key\0");
        message.extend_from_slice(&payload);
        let envelope = SignedSessionEnvelope {
            key_id: "test-session-key".into(),
            payload: STANDARD.encode(payload),
            signature: STANDARD.encode(signer.sign(&message).to_bytes()),
        };
        (
            serde_json::to_vec(&envelope).unwrap(),
            BTreeMap::from([("test-session-key".into(), signer.verifying_key().to_bytes())]),
        )
    }

    fn requested() -> Vec<IpNet> {
        vec!["203.0.113.9/32".parse().unwrap()]
    }

    pub(crate) fn consumed_grant_fixture() -> (VerifiedManifest, Connection, ConsumedSessionGrant) {
        consumed_grant_for_prefix("203.0.113.9/32")
    }

    pub(crate) fn consumed_grant_for_prefix(
        prefix: &str,
    ) -> (VerifiedManifest, Connection, ConsumedSessionGrant) {
        let manifest = verified_manifest("relay.example.net");
        let (bytes, trusted) = signed_grant(NOW + 300, vec![prefix.into()]);
        let requested = vec![prefix.parse::<IpNet>().unwrap()];
        let verified = verify_session_grant(&bytes, &trusted, &manifest, &requested, NOW).unwrap();
        let mut database = storage::open(Path::new(":memory:")).unwrap();
        let consumed = consume_once(&mut database, &verified, NOW).unwrap();
        (manifest, database, consumed)
    }

    #[test]
    fn verifies_signed_node_bound_short_lived_scoped_grant() {
        let (bytes, trusted) = signed_grant(NOW + 300, vec!["203.0.113.9/32".into()]);
        let manifest = verified_manifest("relay.example.net");
        let verified =
            verify_session_grant(&bytes, &trusted, &manifest, &requested(), NOW).unwrap();
        assert_eq!(verified.grant().session_id, "grant-session-012345");
        assert_eq!(verified.grant().node_id, "node-a");
    }

    #[test]
    fn rejects_expired_and_scope_mismatched_grants() {
        let manifest = verified_manifest("relay.example.net");
        let (expired, trusted) = signed_grant(NOW - 1, vec!["203.0.113.9/32".into()]);
        assert_eq!(
            verify_session_grant(&expired, &trusted, &manifest, &requested(), NOW),
            Err(SessionGrantError::Expired)
        );
        let (wrong_scope, trusted) = signed_grant(NOW + 300, vec!["203.0.113.10/32".into()]);
        assert_eq!(
            verify_session_grant(&wrong_scope, &trusted, &manifest, &requested(), NOW),
            Err(SessionGrantError::PrefixScopeMismatch)
        );
    }

    #[test]
    fn consumed_grant_cannot_be_replayed_after_restart() {
        let (bytes, trusted) = signed_grant(NOW + 300, vec!["203.0.113.9/32".into()]);
        let verified = verify_session_grant(
            &bytes,
            &trusted,
            &verified_manifest("relay.example.net"),
            &requested(),
            NOW,
        )
        .unwrap();
        let path =
            std::env::temp_dir().join(format!("vyre-grant-replay-{}.sqlite", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut database = storage::open(&path).unwrap();
        let _consumed = consume_once(&mut database, &verified, NOW).unwrap();
        drop(database);
        let mut database = storage::open(&path).unwrap();
        assert!(matches!(
            consume_once(&mut database, &verified, NOW),
            Err(SessionGrantError::Replay)
        ));
        drop(database);
        let _ = std::fs::remove_file(path);
    }
}
