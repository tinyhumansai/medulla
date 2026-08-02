//! Socket framing tests for the control-plane listener.

mod hub_ops;
mod registry;

#[cfg(unix)]
#[tokio::test]
async fn an_endless_oversized_line_is_bounded_before_it_is_fully_read() {
    use std::io::Cursor;

    let payload = vec![b'x'; super::MAX_FRAME_BYTES * 2];
    let mut reader = tokio::io::BufReader::new(Cursor::new(payload.clone()));

    let frame = super::read_frame(&mut reader).await.unwrap();

    assert!(frame.is_none(), "an oversized frame closes the connection");
    assert!(
        reader.get_ref().position() < payload.len() as u64,
        "the reader must stop before consuming the unbounded line"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn blank_frames_do_not_renew_the_handshake_deadline() {
    use std::sync::Arc;
    use std::time::Duration;

    use tokio::io::AsyncWriteExt;

    let (server_stream, mut client_stream) = tokio::net::UnixStream::pair().unwrap();
    let (_shutdown, shutdown) = tokio::sync::watch::channel(false);
    let server = super::connection::serve_connection_with_handshake_timeout(
        server_stream,
        Arc::new(super::super::tests::FakeFleet::new()),
        crate::control_socket::GrantRegistry::default(),
        super::TaskRegistry::new(),
        shutdown,
        Duration::from_millis(50),
    );
    let spam = tokio::spawn(async move {
        loop {
            if client_stream.write_all(b"\n").await.is_err() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });

    tokio::time::timeout(Duration::from_millis(200), server)
        .await
        .expect("blank frames must not keep an unauthenticated connection alive")
        .expect("the deadline closes the connection cleanly");
    spam.await.unwrap();
}
