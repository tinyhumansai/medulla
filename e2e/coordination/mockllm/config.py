"""Environment knobs, the request log, and the one deterministic reply rule.

Everything here is dialect-independent: the three wire adapters differ only in
how they *carry* a prompt and a reply, never in what the reply says.
"""

import json
import os
import re
import time

#: Model id advertised by `/v1/models` and echoed back on every reply.
MODEL_ID = os.environ.get("MOCK_LLM_MODEL", "mock-model")
#: The token every assertion greps for. Overridable so a scenario can prove that
#: a *specific* server instance answered.
MARKER = os.environ.get("MOCK_LLM_MARKER", "COORDINATION_OK")
#: JSONL request log. Unset disables logging entirely.
LOG_PATH = os.environ.get("MOCK_LLM_LOG")
#: Longest echo appended to the marker. Keeps assertions and diagnostics short.
MAX_ECHO_CHARS = 120
#: Opening tag of the context blocks a harness injects into the user turn.
CONTEXT_BLOCK_PREFIX = "<system-reminder>"


def log_request(kind, payload):
    """Append one request record to the JSONL log, if one is configured.

    `kind` names the dialect (`chat`, `messages`, `responses`) or `models`, so a
    scenario can assert which wire a harness used. Log failures are swallowed:
    the log is an assertion aid, and a full disk must not fail the harness in a
    way that looks like a protocol bug.
    """
    if not LOG_PATH:
        return
    try:
        with open(LOG_PATH, "a", encoding="utf-8") as handle:
            handle.write(json.dumps({"kind": kind, "at": time.time(), "payload": payload}) + "\n")
    except OSError:
        pass


def collect_text(content):
    """Flatten one message's `content` into plain text.

    Accepts every shape the three dialects use: a bare string, or a list of
    blocks keyed `text` (OpenAI chat, Anthropic) or `input_text`/`output_text`
    (the Responses API). Unknown block types contribute nothing rather than
    raising — a harness is free to send blocks this mock does not model.

    Injected context blocks are dropped. Claude Code prepends a
    `<system-reminder>` block to the user turn, and it is far longer than the
    task — joining it in would push the task past the echo truncation and leave
    every reply looking identical, which is exactly what the marker exists to
    rule out.
    """
    if isinstance(content, str):
        return content
    if not isinstance(content, list):
        return ""
    parts = []
    for part in content:
        text = part if isinstance(part, str) else None
        if isinstance(part, dict) and isinstance(part.get("text"), str):
            text = part["text"]
        if text is None or text.lstrip().startswith(CONTEXT_BLOCK_PREFIX):
            continue
        parts.append(text)
    return " ".join(parts)


def reply_text(prompt):
    """The deterministic assistant reply for `prompt`.

    `MARKER` followed by a whitespace-collapsed, truncated echo of the prompt, so
    an assertion can prove both that the mock answered *and* that the task text
    reached it.
    """
    echo = re.sub(r"\s+", " ", prompt or "").strip()[:MAX_ECHO_CHARS]
    return f"{MARKER} {echo}".strip()
