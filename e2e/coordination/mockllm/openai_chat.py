"""The OpenAI Chat Completions dialect — what `opencode` speaks.

opencode reaches its provider through `@ai-sdk/openai-compatible`, which posts
`POST /v1/chat/completions` and reads either a JSON body or an SSE stream of
`chat.completion.chunk` objects terminated by `data: [DONE]`.
"""

import time

from .config import MODEL_ID, collect_text

#: Request paths this dialect answers. The bare form exists because a `baseURL`
#: configured without the `/v1` suffix is a common and harmless mistake.
PATHS = ("/v1/chat/completions", "/chat/completions")
#: The `kind` this dialect's requests are logged under.
LOG_KIND = "chat"


def extract_prompt(body):
    """The text of the last `user` message, or empty when there is none."""
    for message in reversed(body.get("messages") or []):
        if message.get("role") == "user":
            return collect_text(message.get("content"))
    return ""


def log_payload(path, body, reply):
    """The record written to the request log for one chat request."""
    return {
        "path": path,
        "stream": bool(body.get("stream")),
        "model": body.get("model"),
        "reply": reply,
        "messages": body.get("messages"),
    }


def unary(reply):
    """The non-streaming `chat.completion` body."""
    return {
        "id": "chatcmpl-mock",
        "object": "chat.completion",
        "created": int(time.time()),
        "model": MODEL_ID,
        "choices": [
            {
                "index": 0,
                "message": {"role": "assistant", "content": reply},
                "finish_reason": "stop",
            }
        ],
        "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2},
    }


def _chunk(delta, finish_reason=None):
    return {
        "id": "chatcmpl-mock",
        "object": "chat.completion.chunk",
        "created": int(time.time()),
        "model": MODEL_ID,
        "choices": [{"index": 0, "delta": delta, "finish_reason": finish_reason}],
    }


def stream(reply):
    """The SSE event sequence: a role delta, the content, then a stop chunk.

    Yields `(event_name, payload)` pairs; `event_name` is None because this
    dialect's stream carries no `event:` lines. A trailing `[DONE]` sentinel is
    emitted as a raw payload string.
    """
    yield None, _chunk({"role": "assistant"})
    yield None, _chunk({"content": reply})
    yield None, _chunk({}, finish_reason="stop")
    yield None, "[DONE]"
