"""The OpenAI Responses dialect — what Codex speaks.

Codex dropped the chat-completions wire in 0.146, so a routed Codex run posts
`POST /v1/responses` with `stream: true`. That is also the `wire_api` Medulla's
own provider block declares (see `crate::codex_overrides`), which is why this is
the dialect a `codexOverrides` preset lands on.

The prompt lives in an `input` array of items rather than `messages`, and the
reply is an `output` array of items. Codex reads the streamed deltas for its
live display and the terminal `response.completed` event for the final turn.
"""

from .config import collect_text

#: Request paths this dialect answers.
PATHS = ("/v1/responses", "/responses")
#: The `kind` this dialect's requests are logged under.
LOG_KIND = "responses"

_RESPONSE_ID = "resp_mock"
_ITEM_ID = "msg_mock"


def extract_prompt(body):
    """The text of the last user input item, or empty when there is none.

    `input` may be a bare string (the API's shorthand) or a list of items. Only
    `user` items are considered so the echo is the task rather than the
    developer instructions Codex sends alongside it.
    """
    payload = body.get("input")
    if isinstance(payload, str):
        return payload
    if not isinstance(payload, list):
        return ""
    for item in reversed(payload):
        if not isinstance(item, dict):
            continue
        if item.get("type") not in (None, "message"):
            continue
        if item.get("role") != "user":
            continue
        return collect_text(item.get("content"))
    return ""


def log_payload(path, body, reply):
    """The record written to the request log for one responses request.

    The prompt items are logged under `messages` as well as `input`: every
    assertion in the suite greps one key for the task text, and duplicating it
    here is what keeps those assertions harness-agnostic.
    """
    return {
        "path": path,
        "stream": bool(body.get("stream")),
        "model": body.get("model"),
        "reply": reply,
        "input": body.get("input"),
        "messages": body.get("input"),
        "instructions": body.get("instructions"),
    }


def _output_item(reply, status):
    return {
        "type": "message",
        "id": _ITEM_ID,
        "status": status,
        "role": "assistant",
        "content": [{"type": "output_text", "text": reply, "annotations": []}] if reply else [],
    }


def _response(reply, status, output):
    return {
        "id": _RESPONSE_ID,
        "object": "response",
        "status": status,
        "model": None,
        "output": output,
        "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2},
    }


def unary(reply):
    """The non-streaming `response` body, for a client that asked for one."""
    return _response(reply, "completed", [_output_item(reply, "completed")])


def stream(reply):
    """The SSE event sequence for one completed assistant message.

    Yields `(event_name, payload)` pairs. Both the `event:` line and the
    payload's own `type` field are populated: Codex dispatches on the latter,
    while the OpenAI SDKs read the former.
    """
    yield (
        "response.created",
        {"type": "response.created", "response": _response("", "in_progress", [])},
    )
    yield (
        "response.output_item.added",
        {
            "type": "response.output_item.added",
            "output_index": 0,
            "item": _output_item("", "in_progress"),
        },
    )
    yield (
        "response.output_text.delta",
        {
            "type": "response.output_text.delta",
            "item_id": _ITEM_ID,
            "output_index": 0,
            "content_index": 0,
            "delta": reply,
        },
    )
    yield (
        "response.output_item.done",
        {
            "type": "response.output_item.done",
            "output_index": 0,
            "item": _output_item(reply, "completed"),
        },
    )
    yield (
        "response.completed",
        {
            "type": "response.completed",
            "response": _response(reply, "completed", [_output_item(reply, "completed")]),
        },
    )
