//! Socket framing tests for the control-plane listener.

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
