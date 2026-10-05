//! Loopback WireGuard data-plane test. This runs two user-space endpoints on
//! ephemeral localhost UDP sockets and never creates a tunnel adapter/routes.

use boringtun::{
    noise::{Tunn, TunnResult},
    x25519::{PublicKey, StaticSecret},
};
use std::{
    net::{Ipv4Addr, UdpSocket},
    time::Duration,
};

#[test]
fn loopback_wireguard_handshake_and_encrypted_packet_echo() {
    let client_secret = StaticSecret::from([0x11; 32]);
    let client_public = PublicKey::from(&client_secret);
    let relay_secret = StaticSecret::from([0x22; 32]);
    let relay_public = PublicKey::from(&relay_secret);
    let mut client = Tunn::new(client_secret, relay_public, None, None, 1, None);
    let mut relay = Tunn::new(relay_secret, client_public, None, None, 2, None);

    let client_socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let relay_socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    client_socket
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    relay_socket
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let relay_addr = relay_socket.local_addr().unwrap();
    let mut client_out = [0u8; 2048];
    let mut relay_out = [0u8; 2048];

    let initiation = match client.format_handshake_initiation(&mut client_out, false) {
        TunnResult::WriteToNetwork(packet) => packet.to_vec(),
        other => panic!("expected handshake initiation, got {other:?}"),
    };
    client_socket.send_to(&initiation, relay_addr).unwrap();

    let mut network_packet = [0u8; 2048];
    let (received, source) = relay_socket.recv_from(&mut network_packet).unwrap();
    let response = match relay.decapsulate(
        Some(source.ip()),
        &network_packet[..received],
        &mut relay_out,
    ) {
        TunnResult::WriteToNetwork(packet) => packet.to_vec(),
        other => panic!("expected handshake response, got {other:?}"),
    };
    relay_socket.send_to(&response, source).unwrap();

    let (received, source) = client_socket.recv_from(&mut network_packet).unwrap();
    let keepalive = match client.decapsulate(
        Some(source.ip()),
        &network_packet[..received],
        &mut client_out,
    ) {
        TunnResult::WriteToNetwork(packet) => packet.to_vec(),
        other => panic!("expected handshake confirmation, got {other:?}"),
    };
    client_socket.send_to(&keepalive, relay_addr).unwrap();

    let (received, source) = relay_socket.recv_from(&mut network_packet).unwrap();
    assert!(matches!(
        relay.decapsulate(
            Some(source.ip()),
            &network_packet[..received],
            &mut relay_out,
        ),
        TunnResult::Done
    ));

    let plain_ip_packet = ipv4_udp_packet(b"vyre-loopback-wireguard-test");
    let encrypted = match client.encapsulate(&plain_ip_packet, &mut client_out) {
        TunnResult::WriteToNetwork(packet) => packet.to_vec(),
        other => panic!("expected encrypted transport packet, got {other:?}"),
    };
    assert!(!contains_bytes(&encrypted, b"vyre-loopback-wireguard-test"));
    client_socket.send_to(&encrypted, relay_addr).unwrap();

    let (received, source) = relay_socket.recv_from(&mut network_packet).unwrap();
    let (decrypted_packet, source_ip) = match relay.decapsulate(
        Some(source.ip()),
        &network_packet[..received],
        &mut relay_out,
    ) {
        TunnResult::WriteToTunnelV4(packet, source_ip) => (packet.to_vec(), source_ip),
        other => panic!("expected authenticated IPv4 tunnel packet, got {other:?}"),
    };
    assert_eq!(source_ip, Ipv4Addr::new(10, 8, 0, 2));
    assert_eq!(decrypted_packet, plain_ip_packet);

    let encrypted_echo = match relay.encapsulate(&decrypted_packet, &mut relay_out) {
        TunnResult::WriteToNetwork(packet) => packet.to_vec(),
        other => panic!("expected encrypted relay response, got {other:?}"),
    };
    relay_socket.send_to(&encrypted_echo, source).unwrap();
    let (received, _) = client_socket.recv_from(&mut network_packet).unwrap();
    match client.decapsulate(None, &network_packet[..received], &mut client_out) {
        TunnResult::WriteToTunnelV4(packet, source_ip) => {
            assert_eq!(source_ip, Ipv4Addr::new(10, 8, 0, 2));
            assert_eq!(packet, plain_ip_packet);
        }
        other => panic!("expected authenticated relay response, got {other:?}"),
    }
}

fn ipv4_udp_packet(payload: &[u8]) -> Vec<u8> {
    let udp_len = 8 + payload.len();
    let total_len = 20 + udp_len;
    let mut packet = vec![0u8; total_len];
    packet[0] = 0x45;
    packet[2..4].copy_from_slice(&(total_len as u16).to_be_bytes());
    packet[4..6].copy_from_slice(&7u16.to_be_bytes());
    packet[8] = 64;
    packet[9] = 17;
    packet[12..16].copy_from_slice(&[10, 8, 0, 2]);
    packet[16..20].copy_from_slice(&[203, 0, 113, 9]);
    let checksum = ipv4_checksum(&packet[..20]);
    packet[10..12].copy_from_slice(&checksum.to_be_bytes());
    packet[20..22].copy_from_slice(&40000u16.to_be_bytes());
    packet[22..24].copy_from_slice(&27015u16.to_be_bytes());
    packet[24..26].copy_from_slice(&(udp_len as u16).to_be_bytes());
    packet[28..].copy_from_slice(payload);
    packet
}

fn ipv4_checksum(header: &[u8]) -> u16 {
    let sum = header.chunks_exact(2).fold(0u32, |sum, pair| {
        sum + u16::from_be_bytes([pair[0], pair[1]]) as u32
    });
    !(sum.wrapping_add((sum >> 16) & 0xffff) as u16)
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}
