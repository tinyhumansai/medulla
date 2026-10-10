#!/usr/bin/env bash
# Initialize the direct pins and all recursive sources required by OpenHuman.
set -euo pipefail
cd "$(dirname "$0")/.."
git submodule update --init --recursive vendor/openhuman vendor/tinyflows vendor/tinyhumans-sdk
