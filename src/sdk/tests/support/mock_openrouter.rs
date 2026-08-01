//! A stand-in for OpenRouter: records exactly what reached it, and can answer
//! with a deliberately slow SSE stream.
//!
//! Recording the *received* request is the whole point. The attribution rewrite
//! is only meaningful if it is observed from the far side of the proxy — a test
//! that inspected the header map before it was sent would pass even if the
//! forward dropped it on the floor.
//!
//! Offline and deterministic: binds `127.0.0.1:0`, speaks enough HTTP/1.1 to
//! serve the proxy (including chunked request bodies, which is how a streamed
//! forward arrives), and nothing more.

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

/// One request as the upstream actually received it.
#[derive(Debug, Clone, Default)]
pub struct RecordedRequest {
    /// Request method, e.g. `POST`.
    pub method: String,
    /// Request target including any query string.
    pub target: String,
    /// Every header, lowercased, in arrival order (duplicates preserved).
    pub headers: Vec<(String, String)>,
    /// The decoded request body.
    pub body: String,
}

impl RecordedRequest {
    /// The first value of `name`, if present.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    /// How many times `name` appears.
    pub fn header_count(&self, name: &str) -> usize {
        self.headers.iter().filter(|(key, _)| key == name).count()
    }
}

/// How the mock answers.
#[derive(Debug, Clone)]
pub enum Reply {
    /// A single JSON body.
    Json(String),
    /// A chunked `text/event-stream`, pausing `gap` between the two chunks so a
    /// caller can prove the response was relayed incrementally rather than
    /// buffered to completion first.
    SlowStream {
        /// Written immediately.
        first: String,
        /// Written after `gap`.
        second: String,
        /// The pause between them.
        gap: Duration,
    },
}

/// A running mock upstream.
pub struct MockOpenRouter {
    /// The API root to hand the proxy, e.g. `http://127.0.0.1:PORT/api`.
    pub root: String,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    accept: JoinHandle<()>,
}

impl Drop for MockOpenRouter {
    fn drop(&mut self) {
        self.accept.abort();
    }
}

impl MockOpenRouter {
    /// Bind an ephemeral loopback port and answer every request with `reply`.
    pub async fn start(reply: Reply) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mock");
        let address = listener.local_addr().expect("mock address");
        let requests: Arc<Mutex<Vec<RecordedRequest>>> = Arc::default();
        let accept_requests = Arc::clone(&requests);
        let accept = tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    return;
                };
                let requests = Arc::clone(&accept_requests);
                let reply = reply.clone();
                tokio::spawn(async move {
                    let _ = serve(socket, requests, reply).await;
                });
            }
        });
        Self {
            root: format!("http://{address}/api"),
            requests,
            accept,
        }
    }

    /// Everything received so far.
    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// The single request received, panicking if that is not the count.
    pub fn only_request(&self) -> RecordedRequest {
        let requests = self.requests();
        assert_eq!(requests.len(), 1, "expected exactly one upstream request");
        requests.into_iter().next().expect("one request")
    }
}

/// Read one request, record it, and write the configured reply.
async fn serve(
    mut socket: TcpStream,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    reply: Reply,
) -> std::io::Result<()> {
    let mut buffer: Vec<u8> = Vec::new();
    let head_end = loop {
        if let Some(index) = find_head_end(&buffer) {
            break index;
        }
        let mut chunk = [0u8; 4096];
        let read = socket.read(&mut chunk).await?;
        if read == 0 {
            return Ok(());
        }
        buffer.extend_from_slice(&chunk[..read]);
    };

    let head = String::from_utf8_lossy(&buffer[..head_end]).to_string();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let target = parts.next().unwrap_or_default().to_string();

    let mut headers = Vec::new();
    let mut lookup: HashMap<String, String> = HashMap::new();
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim().to_string();
        lookup.entry(name.clone()).or_insert_with(|| value.clone());
        headers.push((name, value));
    }

    // Everything after the head is the start of the body.
    let mut rest = buffer[head_end + 4..].to_vec();
    let body = if lookup
        .get("transfer-encoding")
        .is_some_and(|value| value.eq_ignore_ascii_case("chunked"))
    {
        // A streamed forward arrives chunked, since its length is unknown when
        // the request head is written.
        read_chunked(&mut socket, &mut rest).await?
    } else {
        let length: usize = lookup
            .get("content-length")
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        while rest.len() < length {
            let mut chunk = [0u8; 4096];
            let read = socket.read(&mut chunk).await?;
            if read == 0 {
                break;
            }
            rest.extend_from_slice(&chunk[..read]);
        }
        rest.truncate(length);
        rest
    };

    requests
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(RecordedRequest {
            method,
            target,
            headers,
            body: String::from_utf8_lossy(&body).to_string(),
        });

    match reply {
        Reply::Json(payload) => {
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                payload.len()
            );
            socket.write_all(response.as_bytes()).await?;
            socket.flush().await?;
        }
        Reply::SlowStream { first, second, gap } => {
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
                )
                .await?;
            write_chunk(&mut socket, &first).await?;
            tokio::time::sleep(gap).await;
            write_chunk(&mut socket, &second).await?;
            socket.write_all(b"0\r\n\r\n").await?;
            socket.flush().await?;
        }
    }
    Ok(())
}

/// Index of the `\r\n\r\n` terminating the request head.
fn find_head_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

/// Write one HTTP chunk and flush it, so the far side sees it now.
async fn write_chunk(socket: &mut TcpStream, payload: &str) -> std::io::Result<()> {
    socket
        .write_all(format!("{:x}\r\n{payload}\r\n", payload.len()).as_bytes())
        .await?;
    socket.flush().await
}

/// Decode a chunked request body, reading more from the socket as needed.
async fn read_chunked(socket: &mut TcpStream, buffer: &mut Vec<u8>) -> std::io::Result<Vec<u8>> {
    let mut body = Vec::new();
    loop {
        // Chunk size line.
        let line = loop {
            if let Some(index) = buffer.windows(2).position(|window| window == b"\r\n") {
                let line = String::from_utf8_lossy(&buffer[..index]).to_string();
                buffer.drain(..index + 2);
                break line;
            }
            let mut chunk = [0u8; 4096];
            let read = socket.read(&mut chunk).await?;
            if read == 0 {
                return Ok(body);
            }
            buffer.extend_from_slice(&chunk[..read]);
        };
        let size = usize::from_str_radix(line.trim().split(';').next().unwrap_or("0").trim(), 16)
            .unwrap_or(0);
        if size == 0 {
            return Ok(body);
        }
        // Chunk payload plus its trailing CRLF.
        while buffer.len() < size + 2 {
            let mut chunk = [0u8; 4096];
            let read = socket.read(&mut chunk).await?;
            if read == 0 {
                return Ok(body);
            }
            buffer.extend_from_slice(&chunk[..read]);
        }
        body.extend_from_slice(&buffer[..size]);
        buffer.drain(..size + 2);
    }
}
