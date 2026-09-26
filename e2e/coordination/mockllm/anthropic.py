"""The Anthropic Messages dialect — what Claude Code speaks.

Claude Code is pointed at a custom endpoint with `ANTHROPIC_BASE_URL` plus
`ANTHROPIC_AUTH_TOKEN` (see `crate::protocol::env::router_env`), and posts
`POST /v1/messages?beta=true` with `Authorization: Bearer <token>`. It always
streams, and it reads the documented event sequence:

    message_start → content_block_start → content_block_delta…
                  → content_block_stop → message_delta → message_stop

A text-only reply is enough to end an ordinary turn. A prompt containing the
state-probe marker instead gets a real `Bash` tool-use block, so the terminal
suite can drive Claude from working into a permission wait without touching a
live model.
"""

import os
import time

from .config import MODEL_ID, collect_text

#: Request paths this dialect answers. `count_tokens` is answered separately by
#: the server because it is a different response shape, not a completion.
PATHS = ("/v1/messages", "/messages")
#: Token-counting probe some clients issue before a turn.
COUNT_TOKENS_PATHS = ("/v1/messages/count_tokens", "/messages/count_tokens")
#: The `kind` this dialect's requests are logged under.
LOG_KIND = "messages"

_MESSAGE_ID = "msg_mock"
_TOOL_USE_ID = "toolu_state_probe"
STATE_PROBE_MARKER = "MEDULLA_STATE_PROBE"


def extract_prompt(body):
    """The text of the last `user` message, or empty when there is none.

    Claude Code sends system context as `system` messages interleaved with the
    conversation, so the search is restricted to the `user` role exactly as the
    other dialects do — otherwise the echo would be a system reminder rather than
    the task.
    """
    for message in reversed(body.get("messages") or []):
        if message.get("role") == "user":
            return collect_text(message.get("content"))
    return ""


def log_payload(path, body, reply):
    """The record written to the request log for one messages request.

    The `messages` key is deliberately spelled the same as the chat dialect's, so
    an assertion looking for the task text works unchanged across harnesses.
    """
    return {
        "path": path,
        "stream": bool(body.get("stream")),
        "model": body.get("model"),
        "reply": reply,
        "messages": body.get("messages"),
    }


def requests_state_probe(body):
    """Whether this is the probe's initial request rather than its tool result."""
    messages = body.get("messages") or []
    user_text = "\n".join(
        collect_text(message.get("content"))
        for message in messages
        if message.get("role") == "user"
    )
    has_marker = any(
        message.get("role") == "user"
        and STATE_PROBE_MARKER in collect_text(message.get("content"))
        for message in messages
    )
    has_result = any(
        isinstance(block, dict) and block.get("type") == "tool_result"
        for message in messages
        for block in (message.get("content") if isinstance(message.get("content"), list) else [])
    )
    # Claude issues a concurrent title-generation request containing the task
    # text. It is metadata, not the agent turn, and must remain text-only.
    is_title_request = "Write the title in the predominant language" in user_text
    return has_marker and not has_result and not is_title_request


def _message(reply, stop_reason):
    return {
        "id": _MESSAGE_ID,
        "type": "message",
        "role": "assistant",
        "model": MODEL_ID,
        "content": [{"type": "text", "text": reply}] if reply else [],
        "stop_reason": stop_reason,
        "stop_sequence": None,
        "usage": {"input_tokens": 1, "output_tokens": 1},
    }


def unary(reply):
    """The non-streaming `message` body, for a client that asked for one."""
    return _message(reply, "end_turn")


def tool_unary():
    """A unary Bash request for clients that do not stream."""
    message = _message("", "tool_use")
    message["content"] = [
        {
            "type": "tool_use",
            "id": _TOOL_USE_ID,
            "name": "Bash",
            "input": {
                "command": "touch claude-state-probe.txt",
                "description": "Create the deterministic state-probe file",
            },
        }
    ]
    return message


def count_tokens(body):
    """A constant answer to the token-counting probe.

    The number is never asserted on — it exists so a client that budgets before
    a turn gets a well-formed reply instead of a 404 it would report as an
    endpoint misconfiguration.
    """
    prompt = extract_prompt(body)
    return {"input_tokens": max(1, len(prompt) // 4)}


def stream(reply):
    """The documented Anthropic SSE event sequence for one text block.

    Yields `(event_name, payload)` pairs. Unlike the chat dialect, every event
    carries an `event:` line — the Anthropic SDK dispatches on it.
    """
    yield "message_start", {"type": "message_start", "message": _message("", None)}
    yield (
        "content_block_start",
        {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}},
    )
    yield (
        "content_block_delta",
        {
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "text_delta", "text": reply},
        },
    )
    yield "content_block_stop", {"type": "content_block_stop", "index": 0}
    yield (
        "message_delta",
        {
            "type": "message_delta",
            "delta": {"stop_reason": "end_turn", "stop_sequence": None},
            "usage": {"output_tokens": 1},
        },
    )
    yield "message_stop", {"type": "message_stop"}


def tool_stream():
    """An Anthropic SSE tool request that makes Claude ask for Bash permission."""
    delay_ms = int(os.environ.get("MOCK_LLM_TOOL_DELAY_MS", "0") or "0")
    if delay_ms > 0:
        time.sleep(delay_ms / 1000)
    yield "message_start", {"type": "message_start", "message": _message("", None)}
    yield (
        "content_block_start",
        {
            "type": "content_block_start",
            "index": 0,
            "content_block": {
                "type": "tool_use",
                "id": _TOOL_USE_ID,
                "name": "Bash",
                "input": {},
            },
        },
    )
    yield (
        "content_block_delta",
        {
            "type": "content_block_delta",
            "index": 0,
            "delta": {
                "type": "input_json_delta",
                "partial_json": '{"command":"touch claude-state-probe.txt","description":"Create the deterministic state-probe file"}',
            },
        },
    )
    yield "content_block_stop", {"type": "content_block_stop", "index": 0}
    yield (
        "message_delta",
        {
            "type": "message_delta",
            "delta": {"stop_reason": "tool_use", "stop_sequence": None},
            "usage": {"output_tokens": 1},
        },
    )
    yield "message_stop", {"type": "message_stop"}
