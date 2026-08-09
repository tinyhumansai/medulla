"""A dependency-free, deterministic mock LLM that speaks all three harness wires.

The coordination e2e suite drives *real* coding CLIs — `opencode`, `claude` and
`codex` — with no provider key and no network egress. Each one reaches its model
over a different HTTP dialect, so one server answers all three:

  - :mod:`openai_chat`      — `POST /v1/chat/completions` (opencode)
  - :mod:`anthropic`        — `POST /v1/messages`          (Claude Code)
  - :mod:`openai_responses` — `POST /v1/responses`         (Codex)

Every reply is the same deterministic string in every dialect::

    COORDINATION_OK <echo of the last user message, whitespace-collapsed>

so a caller asserts the unique `COORDINATION_OK` marker at the end of the
(encrypted) chain regardless of which harness produced it. Every request is
appended as one JSON line to `$MOCK_LLM_LOG`, tagged with the dialect, so an
assertion can also prove *which* wire the task travelled over.

Bind: `127.0.0.1:$MOCK_LLM_PORT` (default 8080). Loopback only; no external I/O.
"""

from .config import MARKER, MODEL_ID, log_request, reply_text
from .server import serve

__all__ = ["MARKER", "MODEL_ID", "log_request", "reply_text", "serve"]
