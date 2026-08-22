"""The HTTP surface: route a request to its dialect, answer it, log it.

One `ThreadingHTTPServer` serves every dialect at once, so a harness only has to
be pointed at this address — the path it posts to selects the wire, and no
scenario has to know which port belongs to which CLI.
"""

import json
import os
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from . import anthropic, openai_chat, openai_responses
from .config import MODEL_ID, log_request, reply_text

#: Every completion dialect, in dispatch order.
DIALECTS = (openai_chat, anthropic, openai_responses)


class Handler(BaseHTTPRequestHandler):
    # HTTP/1.1 for chunk-free streaming with Content-Length on unary replies, but
    # every response closes its connection: pooled keep-alive connections race the
    # AI SDK's client and can deadlock the (single-request-per-thread) server.
    protocol_version = "HTTP/1.1"

    def log_message(self, *_args):
        pass  # silence default stderr access log

    # ── plumbing ────────────────────────────────────────────────────────────
    def _path(self):
        return self.path.split("?", 1)[0]

    def _body(self):
        length = int(self.headers.get("Content-Length", "0") or "0")
        raw = self.rfile.read(length) if length else b""
        try:
            return json.loads(raw.decode("utf-8")) if raw else {}
        except (ValueError, UnicodeDecodeError):
            return {}

    def _send_json(self, obj, status=200):
        data = json.dumps(obj).encode("utf-8")
        self.close_connection = True
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(data)

    def _send_stream(self, events):
        """Write an SSE stream of `(event_name, payload)` pairs.

        A payload that is already a string is written verbatim — that is how the
        chat dialect emits its `[DONE]` sentinel.
        """
        self.close_connection = True
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Connection", "close")
        self.end_headers()
        for name, payload in events:
            if name:
                self.wfile.write(f"event: {name}\n".encode("utf-8"))
            body = payload if isinstance(payload, str) else json.dumps(payload)
            self.wfile.write(f"data: {body}\n\n".encode("utf-8"))
            self.wfile.flush()

    # ── routing ─────────────────────────────────────────────────────────────
    def do_GET(self):
        path = self._path()
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
        path = self._path()
        body = self._body()

        # Answered before the dialect scan: it shares the Anthropic prefix but is
        # a token count, not a completion.
        if path in anthropic.COUNT_TOKENS_PATHS:
            log_request("count_tokens", {"path": self.path})
            self._send_json(anthropic.count_tokens(body))
            return

        for dialect in DIALECTS:
            if path in dialect.PATHS:
                self._complete(dialect, body)
                return
        self._send_json({"error": f"not found: {path}"}, status=404)

    def _complete(self, dialect, body):
        """Answer one completion request in `dialect`, logging it first.

        The client's `User-Agent` is logged alongside the payload because it is
        what tells the two transports apart: a CLI run reaches here from the
        harness itself, an ACP run from the ACP server's own SDK. Without it a
        transport that silently fell back to the other one would still pass
        every assertion about the reply.
        """
        if dialect is anthropic and anthropic.requests_state_probe(body):
            reply = "[tool_use Bash: state probe]"
            payload = dialect.log_payload(self.path, body, reply)
            payload["user_agent"] = self.headers.get("User-Agent", "")
            log_request(dialect.LOG_KIND, payload)
            if body.get("stream"):
                self._send_stream(dialect.tool_stream())
            else:
                self._send_json(dialect.tool_unary())
            return

        reply = reply_text(dialect.extract_prompt(body))
        payload = dialect.log_payload(self.path, body, reply)
        payload["user_agent"] = self.headers.get("User-Agent", "")
        log_request(dialect.LOG_KIND, payload)
        if body.get("stream"):
            self._send_stream(dialect.stream(reply))
        else:
            self._send_json(dialect.unary(reply))


def serve():
    """Bind `127.0.0.1:$MOCK_LLM_PORT` and serve until interrupted.

    Prints the bound address (port 0 picks a free one) so a wrapper script can
    capture the real port before pointing a harness at it.
    """
    port = int(os.environ.get("MOCK_LLM_PORT", "8080"))
    server = ThreadingHTTPServer(("127.0.0.1", port), Handler)
    sys.stdout.write(f"mock_llm listening on http://127.0.0.1:{server.server_address[1]}\n")
    sys.stdout.flush()
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
