//! Build a narrowly scoped WireGuard client profile from a verified node and
//! a short-lived session lease. This module does not persist secrets, install
//! a tunnel service, or change Windows routes.

use crate::relay_manifest::VerifiedManifest;
use crate::relay_session::ConsumedSessionGrant;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use ipnet::IpNet;
use std::net::IpAddr;
use zeroize::{Zeroize, Zeroizing};

const MAX_INTERFACE_ADDRESSES: usize = 4;
const MAX_ALLOWED_PREFIXES: usize = 32;

/// Secret session material returned by a future authenticated relay control
/// plane. Values are intentionally not serializable or debuggable.
pub struct TunnelLease {
    node_id: String,
    session_id: String,
    private_key: Zeroizing<String>,
    interface_addresses: Vec<IpNet>,
}

impl TunnelLease {
    /// Bind local private key material to a durably consumed signed grant.
    pub fn from_consumed_grant(
        grant: &ConsumedSessionGrant,
        private_key: Zeroizing<String>,
        interface_addresses: Vec<IpNet>,
    ) -> Result<Self, TunnelConfigError> {
        validate_key(&private_key).map_err(|_| TunnelConfigError::InvalidKey)?;
        let mut decoded = STANDARD
            .decode(private_key.as_str())
            .map_err(|_| TunnelConfigError::InvalidKey)?;
        let private_bytes_result: Result<[u8; 32], _> = decoded.as_slice().try_into();
        decoded.zeroize();
        let private_bytes = private_bytes_result.map_err(|_| TunnelConfigError::InvalidKey)?;
        let private_bytes = Zeroizing::new(private_bytes);
        let secret = x25519_dalek::StaticSecret::from(*private_bytes);
        let public = x25519_dalek::PublicKey::from(&secret);
        if STANDARD.encode(public.to_bytes()) != grant.grant().client_public_key {
            return Err(TunnelConfigError::GrantKeyMismatch);
        }
        Ok(Self {
            node_id: grant.grant().node_id.clone(),
            session_id: grant.grant().session_id.clone(),
            private_key,
            interface_addresses,
        })
    }
}

/// In-memory profile bytes that are cleared on drop. Callers must pass these
/// directly to a protected service API; never log or persist them as plain text.
pub struct TunnelConfig(Vec<u8>);

impl TunnelConfig {
    pub fn as_str(&self) -> &str {
        // The builder only writes ASCII text and validated ASCII base64/IP data.
        std::str::from_utf8(&self.0).expect("WireGuard config is valid UTF-8")
    }
}

impl Drop for TunnelConfig {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TunnelConfigError {
    UnknownNode,
    InvalidKey,
    GrantKeyMismatch,
    GrantExpired,
    InvalidAddresses,
    InvalidAllowedPrefixes,
}

impl std::fmt::Display for TunnelConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::UnknownNode => "Tunnel lease does not match a verified relay node.",
            Self::InvalidKey => "Tunnel lease contains an invalid WireGuard key.",
            Self::GrantKeyMismatch => "Tunnel private key does not match the signed session grant.",
            Self::GrantExpired => "Relay session grant has expired.",
            Self::InvalidAddresses => "Tunnel lease contains invalid interface addresses.",
            Self::InvalidAllowedPrefixes => {
                "Tunnel lease must contain narrow explicit destination prefixes."
            }
        })
    }
}

impl std::error::Error for TunnelConfigError {}

/// Render a WireGuard profile without performing any operating-system action.
/// Default routes and broad prefixes are rejected to limit accidental traffic
/// capture while the tunnel lifecycle and rollback service are still under test.
pub fn build_config(
    manifest: &VerifiedManifest,
    grant: &ConsumedSessionGrant,
    lease: &TunnelLease,
    now_unix: u64,
) -> Result<TunnelConfig, TunnelConfigError> {
    if lease.session_id != grant.grant().session_id || lease.node_id != grant.grant().node_id {
        return Err(TunnelConfigError::GrantKeyMismatch);
    }
    if now_unix > grant.grant().expires_at {
        return Err(TunnelConfigError::GrantExpired);
    }
    let node = manifest
        .manifest()
        .nodes
        .iter()
        .find(|node| node.id == lease.node_id)
        .ok_or(TunnelConfigError::UnknownNode)?;

    validate_key(&lease.private_key).map_err(|_| TunnelConfigError::InvalidKey)?;
    validate_key(&node.public_key).map_err(|_| TunnelConfigError::InvalidKey)?;

    if lease.interface_addresses.is_empty()
        || lease.interface_addresses.len() > MAX_INTERFACE_ADDRESSES
        || lease.interface_addresses.iter().any(|address| {
            address.prefix_len() == 0
                || address.addr().is_unspecified()
                || address.addr().is_multicast()
        })
    {
        return Err(TunnelConfigError::InvalidAddresses);
    }

    let allowed_prefixes = grant
        .grant()
        .allowed_prefixes
        .iter()
        .map(|value| value.parse::<IpNet>())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| TunnelConfigError::InvalidAllowedPrefixes)?;
    if allowed_prefixes.is_empty()
        || allowed_prefixes.len() > MAX_ALLOWED_PREFIXES
        || allowed_prefixes.iter().any(|prefix| {
            prefix.prefix_len() == 0
                || prefix.addr().is_unspecified()
                || prefix.addr().is_multicast()
                || match prefix.addr() {
                    IpAddr::V4(_) => prefix.prefix_len() < 16,
                    IpAddr::V6(_) => prefix.prefix_len() < 32,
                }
        })
    {
        return Err(TunnelConfigError::InvalidAllowedPrefixes);
    }

    let endpoint_host = match node.endpoint_host.parse::<IpAddr>() {
        Ok(IpAddr::V6(address)) => format!("[{address}]"),
        _ => node.endpoint_host.clone(),
    };
    let config = format!(
        "[Interface]\nPrivateKey = {}\nAddress = {}\n\n[Peer]\nPublicKey = {}\nEndpoint = {}:{}\nAllowedIPs = {}\nPersistentKeepalive = 25\n",
        lease.private_key.as_str(),
        lease.interface_addresses.iter().map(ToString::to_string).collect::<Vec<_>>().join(", "),
        node.public_key,
        endpoint_host,
        node.endpoint_port,
        allowed_prefixes.iter().map(ToString::to_string).collect::<Vec<_>>().join(", "),
    );
    let mut bytes = config.into_bytes();
    let output = TunnelConfig(std::mem::take(&mut bytes));
    bytes.zeroize();
    Ok(output)
}

fn validate_key(key: &str) -> Result<(), ()> {
    let mut decoded = STANDARD.decode(key).map_err(|_| ())?;
    let valid = decoded.len() == 32 && STANDARD.encode(&decoded) == key;
    decoded.zeroize();
    if !valid {
        return Err(());
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::relay_manifest::{
        verify_manifest, RelayManifest, RelayNode, SignedManifestEnvelope,
    };
    use ed25519_dalek::{Signer, SigningKey};
    use std::collections::BTreeMap;

    const NOW: u64 = 1_800_000_000;

    pub(crate) fn verified_manifest(endpoint_host: &str) -> VerifiedManifest {
        let signer = SigningKey::from_bytes(&[42; 32]);
        let manifest = RelayManifest {
            schema_version: 1,
            generation: 1,
            issued_at: NOW - 10,
            expires_at: NOW + 3000,
            nodes: vec![RelayNode {
                id: "node-a".into(),
                region: "test-region".into(),
                endpoint_host: endpoint_host.into(),
                endpoint_port: 51820,
                public_key: STANDARD.encode([7u8; 32]),
                protocol: "wireguard-udp".into(),
            }],
        };
        let payload = serde_json::to_vec(&manifest).unwrap();
        let mut message = b"VYRE-RELAY-MANIFEST-V1\0".to_vec();
        message.extend_from_slice(b"test-key\0");
        message.extend_from_slice(&payload);
        let envelope = SignedManifestEnvelope {
            key_id: "test-key".into(),
            payload: STANDARD.encode(payload),
            signature: STANDARD.encode(signer.sign(&message).to_bytes()),
        };
        verify_manifest(
            &serde_json::to_vec(&envelope).unwrap(),
            &BTreeMap::from([("test-key".into(), signer.verifying_key().to_bytes())]),
            NOW,
            0,
        )
        .unwrap()
    }

    fn lease(grant: &ConsumedSessionGrant) -> TunnelLease {
        TunnelLease::from_consumed_grant(
            grant,
            Zeroizing::new(STANDARD.encode([9u8; 32])),
            vec!["10.8.0.2/32".parse().unwrap()],
        )
        .unwrap()
    }

    pub(crate) fn coordinator_fixture() -> TunnelConfig {
        let (manifest, _database, grant) = crate::relay_session::tests::consumed_grant_fixture();
        let lease = lease(&grant);
        build_config(&manifest, &grant, &lease, NOW).unwrap()
    }

    #[test]
    fn builds_config_from_verified_node_and_explicit_prefix() {
        let (manifest, _database, grant) = crate::relay_session::tests::consumed_grant_fixture();
        let lease = lease(&grant);
        let config = build_config(&manifest, &grant, &lease, NOW).unwrap();
        assert!(config
            .as_str()
            .contains("Endpoint = relay.example.net:51820"));
        assert!(config.as_str().contains("AllowedIPs = 203.0.113.9/32"));
        assert!(config.as_str().contains("PrivateKey = "));
    }

    #[test]
    fn rejects_default_or_broad_routes_in_the_signed_grant() {
        let manifest = verified_manifest("relay.example.net");
        for prefix in ["0.0.0.0/0", "10.0.0.0/8"] {
            let (bytes, trusted) =
                crate::relay_session::tests::signed_grant(NOW + 300, vec![prefix.into()]);
            let result = crate::relay_session::verify_session_grant(
                &bytes,
                &trusted,
                &manifest,
                &[prefix.parse().unwrap()],
                NOW,
            );
            assert_eq!(
                result.err(),
                Some(crate::relay_session::SessionGrantError::InvalidAllowedPrefixes)
            );
        }
    }

    #[test]
    fn renders_ipv6_endpoint_and_scoped_ipv6_prefix() {
        let (manifest, _db, grant) =
            crate::relay_session::tests::consumed_grant_for_prefix("2001:db8:1::/48");
        let lease = lease(&grant);
        let config = build_config(&manifest, &grant, &lease, NOW).unwrap();
        assert!(config
            .as_str()
            .contains("Endpoint = relay.example.net:51820"));
        assert!(config.as_str().contains("AllowedIPs = 2001:db8:1::/48"));
    }

    #[test]
    fn rejects_bad_or_unbound_keys_and_expired_grants() {
        let (manifest, _database, grant) = crate::relay_session::tests::consumed_grant_fixture();
        let bad_key = TunnelLease::from_consumed_grant(
            &grant,
            Zeroizing::new("not-a-key".into()),
            vec!["10.8.0.2/32".parse().unwrap()],
        );
        assert_eq!(bad_key.err().unwrap(), TunnelConfigError::InvalidKey);
        let mismatched_lease = TunnelLease::from_consumed_grant(
            &grant,
            Zeroizing::new(STANDARD.encode([8u8; 32])),
            vec!["10.8.0.2/32".parse().unwrap()],
        );
        assert_eq!(
            mismatched_lease.err().unwrap(),
            TunnelConfigError::GrantKeyMismatch
        );
        assert_eq!(
            build_config(&manifest, &grant, &lease(&grant), NOW + 600)
                .err()
                .unwrap(),
            TunnelConfigError::GrantExpired
        );
    }
}
