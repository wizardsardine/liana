//! Regression test for the TLS-handshake timeout bound in
//! `phone_signer::transport::dial_tls`.
//!
//! Failure mode this guards against: a phone (or attacker) that
//! accepts the TCP socket but never speaks TLS hangs the
//! `connector.connect(sni, tcp).await` indefinitely. Before the
//! handshake bound was added, this would stall the discovery loop's
//! per-phone dial future forever (preventing the cooldown from ever
//! being recorded) and freeze the pairing wizard with no error.
//!
//! Runs in real time (~750 ms per `CONNECT_TIMEOUT`). Paused-time
//! auto-advance would fire the timer before the real-I/O TCP connect
//! completes, so we can't shortcut the wait the way the `sign_tx`
//! timeout test does.

use std::net::Ipv4Addr;

use tokio::net::TcpListener;

use coincube_gui::phone_signer::{identity::DesktopIdentity, transport::PairedTransport};

fn fresh_desktop_identity() -> DesktopIdentity {
    let mut path = std::env::temp_dir();
    path.push(format!("coincube-handshake-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&path).expect("mkdir tempdir");
    let dir = coincube_gui::dir::CoincubeDirectory::new(path);
    coincube_gui::phone_signer::identity::load_or_create(&dir).expect("identity")
}

#[tokio::test]
async fn connect_unpinned_returns_error_when_phone_stalls_handshake() {
    // Bind a listener and accept the TCP socket but never speak
    // TLS. The handshake should time out within the production
    // `CONNECT_TIMEOUT` (750 ms), not hang. Also enforce an outer
    // test-level cap so a regression that re-introduces the hang
    // fails the test fast instead of stalling CI.
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    let phone_task = tokio::spawn(async move {
        let (_tcp, _peer) = listener.accept().await.expect("accept");
        // Hold the TCP socket open without speaking TLS. Sleep for
        // longer than the test's outer cap so the desktop is the
        // one that times out, not us.
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
    });

    let identity = fresh_desktop_identity();
    // Outer cap: the production `CONNECT_TIMEOUT` is 750 ms, so 5 s
    // is plenty of slack while still failing fast if the handshake
    // hangs.
    let res = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        PairedTransport::connect_unpinned(addr, &identity),
    )
    .await
    .expect("connect_unpinned must return within the outer cap");

    let err = res.expect_err("expected handshake timeout error");
    let msg = match err {
        async_hwi::Error::Device(m) => m,
        other => panic!("expected Device(timeout), got {:?}", other),
    };
    assert!(
        msg.to_lowercase().contains("timeout"),
        "error message should mention timeout, got: {}",
        msg,
    );

    // Cancel the stalled phone task so it doesn't outlive the test;
    // its 30 s sleep would otherwise sit in the runtime past the
    // assertion. `abort()` signals cancellation and the JoinHandle
    // then drops naturally without tripping clippy's
    // `let_underscore_future`.
    phone_task.abort();
}
