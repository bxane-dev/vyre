//! Build a narrowly scoped WireGuard client profile from a verified node and
//! a short-lived session lease. This module does not persist secrets, install
//! a tunnel service, or change Windows routes.

use crate::relay_manifest::VerifiedManifest;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use ipnet::IpNet;
use std::net::IpAddr;
use zeroize::{Zeroize, Zeroizing};

const MAX_INTERFACE_ADDRESSES: usize = 4;
const MAX_ALLOWED_PREFIXES: usize = 32;

/// Secret session material returned by a future authenticated relay control
/// plane. Values are intentionally not serializable or debuggable.
pub struct TunnelLease {
    pub node_id: String,
    pub private_key: Zeroizing<String>,
    pub interface_addresses: Vec<IpNet>,
    pub allowed_prefixes: Vec<IpNet>,
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
    InvalidAddresses,
    InvalidAllowedPrefixes,
}

impl std::fmt::Display for TunnelConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::UnknownNode => "Tunnel lease does not match a verified relay node.",
            Self::InvalidKey => "Tunnel lease contains an invalid WireGuard key.",
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
    lease: &TunnelLease,
) -> Result<TunnelConfig, TunnelConfigError> {
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

    if lease.allowed_prefixes.is_empty()
        || lease.allowed_prefixes.len() > MAX_ALLOWED_PREFIXES
        || lease.allowed_prefixes.iter().any(|prefix| {
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
        lease.allowed_prefixes.iter().map(ToString::to_string).collect::<Vec<_>>().join(", "),
    );
    let mut bytes = config.into_bytes();
    let output = TunnelConfig(std::mem::take(&mut bytes));
    bytes.zeroize();
    Ok(output)
}

fn validate_key(key: &str) -> Result<(), ()> {
    let decoded = STANDARD.decode(key).map_err(|_| ())?;
    if decoded.len() != 32 || STANDARD.encode(decoded) != key {
        return Err(());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relay_manifest::{
        verify_manifest, RelayManifest, RelayNode, SignedManifestEnvelope,
    };
    use ed25519_dalek::{Signer, SigningKey};
    use std::collections::BTreeMap;

    const NOW: u64 = 1_800_000_000;

    fn verified_manifest(endpoint_host: &str) -> VerifiedManifest {
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

    fn lease(prefix: &str) -> TunnelLease {
        TunnelLease {
            node_id: "node-a".into(),
            private_key: Zeroizing::new(STANDARD.encode([9u8; 32])),
            interface_addresses: vec!["10.8.0.2/32".parse().unwrap()],
            allowed_prefixes: vec![prefix.parse().unwrap()],
        }
    }

    #[test]
    fn builds_config_from_verified_node_and_explicit_prefix() {
        let config = build_config(
            &verified_manifest("relay.example.net"),
            &lease("203.0.113.9/32"),
        )
        .unwrap();
        assert!(config
            .as_str()
            .contains("Endpoint = relay.example.net:51820"));
        assert!(config.as_str().contains("AllowedIPs = 203.0.113.9/32"));
        assert!(config.as_str().contains("PrivateKey = "));
    }

    #[test]
    fn rejects_default_or_broad_routes_and_unknown_nodes() {
        let manifest = verified_manifest("relay.example.net");
        assert_eq!(
            build_config(&manifest, &lease("0.0.0.0/0")).err().unwrap(),
            TunnelConfigError::InvalidAllowedPrefixes
        );
        assert_eq!(
            build_config(&manifest, &lease("10.0.0.0/8")).err().unwrap(),
            TunnelConfigError::InvalidAllowedPrefixes
        );
        let mut unknown = lease("203.0.113.9/32");
        unknown.node_id = "other".into();
        assert_eq!(
            build_config(&manifest, &unknown).err().unwrap(),
            TunnelConfigError::UnknownNode
        );
    }

    #[test]
    fn renders_ipv6_endpoint_and_scoped_ipv6_prefix() {
        let config =
            build_config(&verified_manifest("2001:db8::1"), &lease("2001:db8:1::/48")).unwrap();
        assert!(config.as_str().contains("Endpoint = [2001:db8::1]:51820"));
        assert!(config.as_str().contains("AllowedIPs = 2001:db8:1::/48"));
    }

    #[test]
    fn rejects_bad_keys_and_empty_prefixes() {
        let manifest = verified_manifest("relay.example.net");
        let mut bad_key = lease("203.0.113.9/32");
        bad_key.private_key = Zeroizing::new("not-a-key".into());
        assert_eq!(
            build_config(&manifest, &bad_key).err().unwrap(),
            TunnelConfigError::InvalidKey
        );
        let mut no_routes = lease("203.0.113.9/32");
        no_routes.allowed_prefixes.clear();
        assert_eq!(
            build_config(&manifest, &no_routes).err().unwrap(),
            TunnelConfigError::InvalidAllowedPrefixes
        );
    }
}
