//! Bringing a host up: address families, endpoint formatting, and the lock that
//! closes the enroll-then-connect window.
//!
//! Pure and fast — no pty, no bridge, no runtime.

#[test]
fn an_ipv6_literal_is_bracketed_when_joined_to_a_port() {
    // `format!("{host}:{port}")` turns `::1` into `::1:9`, which parses as a
    // different address entirely — so an IPv6 host would be dialled somewhere
    // else rather than failing loudly.
    assert_eq!(crate::remote::serve::join_endpoint("::1", 9), "[::1]:9");
    assert_eq!(
        crate::remote::serve::join_endpoint("2001:db8::1", 60000),
        "[2001:db8::1]:60000"
    );
    // Already bracketed, a hostname, and IPv4 are all left alone.
    assert_eq!(crate::remote::serve::join_endpoint("[::1]", 9), "[::1]:9");
    assert_eq!(
        crate::remote::serve::join_endpoint("tower.local", 22),
        "tower.local:22"
    );
    assert_eq!(
        crate::remote::serve::join_endpoint("10.0.0.2", 22),
        "10.0.0.2:22"
    );
}

#[test]
fn the_unrouted_placeholder_matches_the_binds_family() {
    // The daemon needs *an* endpoint before it has heard from its client, and
    // the link resolves one against the socket's family — so a v4 placeholder
    // under a v6 bind would refuse to start rather than simply never be used.
    let v4 = std::net::SocketAddr::from(([0, 0, 0, 0], 0));
    let v6 = std::net::SocketAddr::from(([0u16; 8], 0));
    assert!(crate::remote::serve::unrouted_placeholder(v4)
        .parse::<std::net::SocketAddr>()
        .unwrap()
        .is_ipv4());
    assert!(!crate::remote::serve::unrouted_placeholder(v6)
        .parse::<std::net::SocketAddr>()
        .unwrap()
        .is_ipv4());
}

#[test]
fn a_second_bootstrap_waits_for_the_first_handoff() {
    // The window this guards: `enroll_client` releases `node.lock` so
    // `Link::connect` can take it, and a second bootstrap arriving in between
    // would write a fresh pair key over the one the first daemon is starting
    // with — leaving it running under a key `node.json` no longer records.
    let dir = tempfile::tempdir().expect("a scratch dir");
    let held = crate::remote::serve::hold_bootstrap(dir.path()).expect("the first hold");

    let path = dir.path().to_path_buf();
    let second = std::thread::spawn(move || {
        let _lock = crate::remote::serve::hold_bootstrap(&path).expect("the second hold");
        std::time::Instant::now()
    });

    // Long enough that a non-blocking implementation would finish first.
    std::thread::sleep(std::time::Duration::from_millis(200));
    let released = std::time::Instant::now();
    drop(held);

    let acquired = second.join().expect("the second bootstrap finishes");
    assert!(
        acquired >= released,
        "the second bootstrap took the lock before the first released it"
    );
}
