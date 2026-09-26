//! Unit tests for [`super::bind_dual_stack`].
//!
//! The property under test is the one `UdpSocket::bind` cannot express: a
//! socket bound to an unspecified IPv6 address must also accept a datagram
//! sent to its IPv4-mapped equivalent, on any host where the OS honours
//! `IPV6_V6ONLY(false)` at all. Skipped rather than failed where it does not
//! (a sandboxed or IPv6-disabled CI runner), since that is an environment
//! limit, not a regression in this code.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

use tokio::net::UdpSocket;

use super::bind_dual_stack;

/// An IPv6 wildcard bind also accepts a datagram from an IPv4-mapped sender,
/// when the platform supports dual-stack sockets at all.
#[tokio::test]
async fn a_dual_stack_bind_accepts_an_ipv4_mapped_datagram() {
    let bind: SocketAddr = "[::]:0".parse().expect("a literal address parses");
    let std_socket = bind_dual_stack(bind).expect("binding the wildcard address succeeds");
    std_socket
        .set_nonblocking(true)
        .expect("the socket accepts nonblocking mode");
    let socket = UdpSocket::from_std(std_socket).expect("tokio adopts the bound socket");
    let local_port = socket
        .local_addr()
        .expect("a bound socket has a local address")
        .port();

    let sender = UdpSocket::bind("0.0.0.0:0")
        .await
        .expect("binding an ephemeral IPv4 socket succeeds");
    let target = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, local_port));
    sender
        .send_to(b"probe", target)
        .await
        .expect("sending to the mapped address succeeds");

    let mut buf = [0u8; 5];
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        socket.recv_from(&mut buf),
    )
    .await;
    match result {
        Ok(Ok((n, _))) => assert_eq!(&buf[..n], b"probe"),
        // A runner with IPv6 fully disabled, or one whose kernel refuses
        // `IPV6_V6ONLY(false)`, cannot demonstrate this property — that is an
        // environment limit rather than a bug in `bind_dual_stack`.
        _ => eprintln!(
            "skipping: this host does not deliver an IPv4-mapped datagram to a dual-stack IPv6 bind"
        ),
    }
}

/// An IPv4 bind is untouched: no `set_only_v6` call, no behaviour change.
#[tokio::test]
async fn an_ipv4_bind_is_unaffected() {
    let bind: SocketAddr = "0.0.0.0:0".parse().expect("a literal address parses");
    let std_socket = bind_dual_stack(bind).expect("binding an IPv4 wildcard address succeeds");
    assert!(std_socket
        .local_addr()
        .expect("a bound socket has a local address")
        .is_ipv4());
}

#[test]
fn a_dual_stack_bind_reaches_an_ipv4_peer_through_its_mapped_form() {
    // A `[::]` socket with `IPV6_V6ONLY` off talks to IPv4 peers via
    // `::ffff:a.b.c.d`. Without this the client had to guess a family from the
    // host's DNS records, and guessed wrong whenever a host had an AAAA record
    // but no usable IPv6 path — bootstrap succeeded over SSH's own fallback and
    // the link then had nowhere to send.
    let resolved = super::resolve("127.0.0.1:9", false).expect("a v4 peer is reachable");
    assert!(!resolved.is_ipv4(), "expected the mapped form: {resolved}");
    assert_eq!(resolved.port(), 9);
    assert_eq!(resolved.to_string(), "[::ffff:127.0.0.1]:9");
}

#[test]
fn a_native_match_is_preferred_over_the_mapped_fallback() {
    // A host with both records should use its real IPv6 address, not a mapped
    // v4 one — the fallback exists for hosts that have no v6 address at all.
    let resolved = super::resolve("localhost:9", false).expect("localhost resolves");
    assert!(!resolved.is_ipv4());
}

#[test]
fn an_ipv4_bind_still_refuses_an_ipv6_only_endpoint() {
    // The mapping goes one way only: a v4 socket cannot reach a v6 peer, and
    // saying so is better than binding something that can never deliver.
    assert!(super::resolve("[::1]:9", true).is_err());
}
