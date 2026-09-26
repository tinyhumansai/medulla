#!/usr/bin/env python3
"""Entrypoint for the mock LLM server.

The implementation lives in the :mod:`mockllm` package next to this file, one
module per harness wire dialect. This stays a script so every scenario keeps
launching it the same way::

    MOCK_LLM_PORT=0 MOCK_LLM_LOG=… python3 mock_llm.py
"""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from mockllm import serve  # noqa: E402  (path must be set before the import)

if __name__ == "__main__":
    serve()
