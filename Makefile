.DEFAULT_GOAL := help

.PHONY: help init fmt clippy test build check ci e2e-image e2e-image-base \
        e2e-docker e2e-docker-all e2e-live e2e-remote-hosts

# Tag for the containerized e2e harness image (see e2e/coordination/Dockerfile).
E2E_IMAGE ?= medulla-e2e:latest
# Which coding CLI the suites drive: opencode, claude or codex. One image bakes
# all three, so switching is a run-time choice and needs no rebuild.
E2E_HARNESS ?= opencode
# Every suite runs fully network-isolated: the whole stack is loopback.
E2E_RUN = docker run --rm --network none -e E2E_HARNESS=$(E2E_HARNESS) $(E2E_IMAGE) bash

help: ## Show available targets
	@awk 'BEGIN {FS = ":.*## "; print "Usage: make <target>\n"} /^[a-zA-Z0-9_-]+:.*## / {printf "  %-12s %s\n", $$1, $$2}' $(MAKEFILE_LIST)

init: ## Initialize submodules, Rust tooling, dependencies, and Git hooks
	bash scripts/init-submodules.sh
	rustup component add rustfmt clippy
	cargo fetch --locked
	git config core.hooksPath .githooks
	@echo "Repository initialized; pre-push hooks are active."

fmt: ## Check Rust formatting
	cargo fmt --all -- --check

clippy: ## Run Clippy with warnings denied
	cargo clippy --locked --all-targets -- -D warnings

test: ## Run the offline test suite
	cargo test --locked

build: ## Build all targets
	cargo build --locked --all-targets

check: fmt clippy ## Run the pre-push checks

ci: fmt clippy test build ## Run the complete CI gate locally

e2e-image: ## Build the containerized e2e harness image (needs submodules; pulls the tools base)
	IMAGE=$(firstword $(subst :, ,$(E2E_IMAGE))) TAGS=$(word 2,$(subst :, ,$(E2E_IMAGE))) \
	  bash e2e/coordination/build-image.sh

# The ~600 MB of coding CLIs the harness image layers onto. Published to GHCR and
# pulled by default, so this is only needed when bumping a CLI version or when
# GHCR is unreachable — then point Dockerfile's BASE_IMAGE at what it builds.
e2e-image-base: ## Build the e2e tools base image (tmux, node, opencode/claude/codex)
	bash e2e/coordination/build-image.sh --base

e2e-docker: e2e-image ## Run every offline e2e suite in containers for E2E_HARNESS (default: opencode)
	$(E2E_RUN) /app/e2e/coordination/run.sh
	$(E2E_RUN) /app/e2e/coordination/tests.sh
	$(E2E_RUN) /app/e2e/coordination/tests_multi.sh
	$(E2E_RUN) /app/e2e/coordination/tests_acp.sh
	$(E2E_RUN) /app/e2e/coordination/tests_tui.sh

# Builds once, then loops: the image is the same for every harness, so a rebuild
# per leg would only pay docker's cache-check cost three times over.
e2e-docker-all: e2e-image ## Run every offline e2e suite against all three coding CLIs
	@for harness in opencode claude codex; do \
	  for suite in run tests tests_multi tests_acp tests_tui; do \
	    echo "==> $$harness / $$suite"; \
	    docker run --rm --network none -e E2E_HARNESS=$$harness $(E2E_IMAGE) \
	      bash /app/e2e/coordination/$$suite.sh || exit 1; \
	  done; \
	done

# Deliberately NOT wired into `ci`: this one bills a real OpenRouter key and
# talks to real staging. It runs on the host (not in a container) so it can use
# your ambient credentials, and refuses to start without E2E_LIVE=1.
e2e-live: ## Run the live staging + OpenRouter suite (needs E2E_LIVE=1 and OPENROUTER_API_KEY)
	bash e2e/coordination/run-live.sh

# Two containers on one bridge network, which is the smallest arrangement that
# can tell the truth about remote hosts: the in-process tests prove the protocol,
# but only a second machine proves the SSH bootstrap and the UDP path. Mounts
# `target/debug` rather than rebuilding, so the base image must match this
# machine's glibc — see e2e/remote-hosts/Dockerfile.
e2e-remote-hosts: ## Run the remote-host suite: real sshd, real UDP, two containers
	cargo build -p medulla-tui --bin medulla
	docker compose -f e2e/remote-hosts/compose.yaml build
	docker compose -f e2e/remote-hosts/compose.yaml up \
	  --abort-on-container-exit --exit-code-from client --force-recreate; \
	status=$$?; \
	docker compose -f e2e/remote-hosts/compose.yaml down -v; \
	exit $$status

.PHONY: e2e-native
e2e-native: ## Run native OpenHuman coordination against the loopback mock LLM
	E2E_HARNESS=openhuman E2E_SMOKE=0 bash e2e/coordination/run.sh
