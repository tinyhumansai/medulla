#!/usr/bin/env python3
"""A dependency-free, deterministic OpenAI-compatible mock LLM server.

Used by the coordination e2e suite so the real `opencode` CLI can run end to end
without any real provider key or network egress. It answers just enough of the
OpenAI HTTP surface for the `@ai-sdk/openai-compatible` provider opencode uses:

  - GET  /v1/models            → a one-model catalog.
  - POST /v1/chat/completions  → a scripted reply, streaming (SSE) or not.

Every reply is deterministic: the assistant content is

    COORDINATION_OK <echo of the last user message, whitespace-collapsed>

so a caller can assert the unique `COORDINATION_OK` marker at the end of the
(encrypted) chain. Every request is appended as one JSON line to the log file
given by MOCK_LLM_LOG (if set) for later assertions.

Bind: 127.0.0.1:$MOCK_LLM_PORT (default 8080). Loopback only; no external I/O.
"""

import json
import os
import re
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

MODEL_ID = os.environ.get("MOCK_LLM_MODEL", "mock-model")
MARKER = os.environ.get("MOCK_LLM_MARKER", "COORDINATION_OK")
LOG_PATH = os.environ.get("MOCK_LLM_LOG")


def log_request(kind, payload):
    if not LOG_PATH:
        return
    try:
        with open(LOG_PATH, "a", encoding="utf-8") as handle:
            handle.write(json.dumps({"kind": kind, "at": time.time(), "payload": payload}) + "\n")
    except OSError:
        pass


def last_user_message(body):
    messages = body.get("messages") or []
    for message in reversed(messages):
        if message.get("role") == "user":
            content = message.get("content")
            if isinstance(content, str):
                return content
            if isinstance(content, list):
                parts = []
                for part in content:
                    if isinstance(part, dict) and isinstance(part.get("text"), str):
                        parts.append(part["text"])
                return " ".join(parts)
    return ""


def reply_text(body):
    echo = re.sub(r"\s+", " ", last_user_message(body)).strip()
    # Keep the echo short and marker-adjacent so assertions stay simple.
    echo = echo[:120]
    return f"{MARKER} {echo}".strip()


def chat_completion_object(content, finish_reason="stop"):
    return {
        "id": "chatcmpl-mock",
        "object": "chat.completion",
        "created": int(time.time()),
        "model": MODEL_ID,
        "choices": [
            {
                "index": 0,
                "message": {"role": "assistant", "content": content},
                "finish_reason": finish_reason,
            }
        ],
        "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2},
    }


def chunk_object(delta, finish_reason=None):
    return {
        "id": "chatcmpl-mock",
        "object": "chat.completion.chunk",
        "created": int(time.time()),
        "model": MODEL_ID,
        "choices": [{"index": 0, "delta": delta, "finish_reason": finish_reason}],
    }


class Handler(BaseHTTPRequestHandler):
    # HTTP/1.1 for chunk-free streaming with Content-Length on unary replies, but
    # every response closes its connection: pooled keep-alive connections race the
    # AI SDK's client and can deadlock the (single-request-per-thread) server.
    protocol_version = "HTTP/1.1"

    def log_message(self, *_args):
        pass  # silence default stderr access log

    def _send_json(self, obj, status=200):
        data = json.dumps(obj).encode("utf-8")
        self.close_connection = True
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        path = self.path.split("?", 1)[0]
        if path in ("/v1/models", "/models"):
            log_request("models", {"path": self.path})
            self._send_json(
                {
                    "object": "list",
                    "data": [{"id": MODEL_ID, "object": "model", "owned_by": "mock"}],
                }
            )
            return
        self._send_json({"error": "not found"}, status=404)

    def do_POST(self):
        path = self.path.split("?", 1)[0]
        length = int(self.headers.get("Content-Length", "0") or "0")
        raw = self.rfile.read(length) if length else b""
        try:
            body = json.loads(raw.decode("utf-8")) if raw else {}
        except (ValueError, UnicodeDecodeError):
            body = {}

        if path not in ("/v1/chat/completions", "/chat/completions"):
            self._send_json({"error": "not found"}, status=404)
            return

        content = reply_text(body)
        log_request(
            "chat",
            {
                "path": self.path,
                "stream": bool(body.get("stream")),
                "model": body.get("model"),
                "reply": content,
                "messages": body.get("messages"),
            },
        )

        if body.get("stream"):
            self._stream(content)
        else:
            self._send_json(chat_completion_object(content))

    def _stream(self, content):
        self.close_connection = True
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Connection", "close")
        self.end_headers()

        def write_event(obj):
            self.wfile.write(f"data: {json.dumps(obj)}\n\n".encode("utf-8"))
            self.wfile.flush()

        # role delta, then the content in a single chunk, then a stop chunk.
        write_event(chunk_object({"role": "assistant"}))
        write_event(chunk_object({"content": content}))
        write_event(chunk_object({}, finish_reason="stop"))
        self.wfile.write(b"data: [DONE]\n\n")
        self.wfile.flush()


def main():
    port = int(os.environ.get("MOCK_LLM_PORT", "8080"))
    server = ThreadingHTTPServer(("127.0.0.1", port), Handler)
    # Print the bound address so a wrapper script can capture the real port.
    sys.stdout.write(f"mock_llm listening on http://127.0.0.1:{server.server_address[1]}\n")
    sys.stdout.flush()
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
