//! The line-framed JSON-RPC dialect `codex app-server` speaks.
//!
//! One JSON object per line on stdin and stdout — no `Content-Length` framing.
//! The server omits the `jsonrpc` field on the messages it sends and does not
//! require it on the ones it receives; this module writes it anyway, because a
//! future server that checks would otherwise reject every request and the field
//! costs fourteen bytes.
//!
//! Only what this client needs is modelled. The protocol is large (roughly ninety
//! request methods and seventy notifications) and almost all of it is for an
//! interactive editor; mirroring it here would be a maintenance burden with no
//! caller. Params and results stay [`Value`], and the small number of fields the
//! fold reads are pulled out where they are read.

use serde_json::{json, Value};

/// A JSON-RPC id. Codex accepts numbers and strings; this client only ever mints
/// numbers, but a response is matched on whatever came back.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RequestId {
    /// A numeric id — what this client mints.
    Number(i64),
    /// A string id, accepted because the server may echo one on a request of
    /// its own.
    Text(String),
}

impl RequestId {
    /// Read an id out of a decoded message, if it carries a usable one.
    pub fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::Number(number) => number.as_i64().map(RequestId::Number),
            Value::String(text) => Some(RequestId::Text(text.clone())),
            _ => None,
        }
    }

    /// The id as it goes back on the wire.
    pub fn to_value(&self) -> Value {
        match self {
            RequestId::Number(number) => json!(number),
            RequestId::Text(text) => json!(text),
        }
    }
}

/// A notification from the server: a method name and its params.
#[derive(Debug, Clone)]
pub struct Notification {
    /// The notification method, e.g. `item/completed`.
    pub method: String,
    /// Its params object, or [`Value::Null`] when it carried none.
    pub params: Value,
}

impl Notification {
    /// The thread this notification concerns, when it names one.
    ///
    /// Every per-thread notification carries `threadId` in its params — except
    /// `thread/started`, which nests the id under `thread`. Both are read here so
    /// the connection's fan-out has one rule.
    pub fn thread_id(&self) -> Option<&str> {
        self.params
            .get("threadId")
            .and_then(Value::as_str)
            .or_else(|| {
                self.params
                    .get("thread")
                    .and_then(|thread| thread.get("id"))
                    .and_then(Value::as_str)
            })
    }
}

/// One decoded inbound message.
#[derive(Debug, Clone)]
pub enum Message {
    /// A successful response to a request this client sent.
    Response {
        /// The id being answered.
        id: RequestId,
        /// The result payload.
        result: Value,
    },
    /// An error response to a request this client sent.
    Error {
        /// The id being answered.
        id: RequestId,
        /// The server's error message.
        message: String,
    },
    /// A notification the server pushed.
    Notification(Notification),
    /// A request *from* the server, which must be answered.
    ServerRequest {
        /// The id to answer with.
        id: RequestId,
        /// The requested method, e.g. `item/commandExecution/requestApproval`.
        method: String,
        /// Its params object.
        params: Value,
    },
}

impl Message {
    /// Decode one line.
    ///
    /// Returns `None` for anything that is not a message this client
    /// understands — blank lines, log output that reached stdout, a future
    /// message shape. Dropping it is correct: the alternative is failing a
    /// running turn over a line nobody was waiting for.
    pub fn parse(line: &str) -> Option<Self> {
        let line = line.trim();
        if line.is_empty() {
            return None;
        }
        let value: Value = serde_json::from_str(line).ok()?;
        let id = value.get("id").and_then(RequestId::from_value);
        let method = value.get("method").and_then(Value::as_str);
        let params = value.get("params").cloned().unwrap_or(Value::Null);
        match (id, method) {
            // A request from the server: it has both an id to answer and a
            // method to answer about.
            (Some(id), Some(method)) => Some(Message::ServerRequest {
                id,
                method: method.to_string(),
                params,
            }),
            (Some(id), None) => match value.get("error") {
                Some(error) => Some(Message::Error {
                    id,
                    message: error_message(error),
                }),
                None => Some(Message::Response {
                    id,
                    result: value.get("result").cloned().unwrap_or(Value::Null),
                }),
            },
            (None, Some(method)) => Some(Message::Notification(Notification {
                method: method.to_string(),
                params,
            })),
            (None, None) => None,
        }
    }
}

/// Render a JSON-RPC error object as one line of prose.
fn error_message(error: &Value) -> String {
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("unknown error");
    match error.get("data") {
        Some(Value::Null) | None => message.to_string(),
        Some(data) => format!("{message} ({data})"),
    }
}

/// Serialize an outbound request line (without its trailing newline).
pub fn request_line(id: &RequestId, method: &str, params: Value) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": id.to_value(),
        "method": method,
        "params": params,
    })
    .to_string()
}

/// Serialize an outbound notification line (without its trailing newline).
pub fn notification_line(method: &str, params: Value) -> String {
    json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
    })
    .to_string()
}

/// Serialize a response to a server request (without its trailing newline).
pub fn response_line(id: &RequestId, result: Value) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": id.to_value(),
        "result": result,
    })
    .to_string()
}
