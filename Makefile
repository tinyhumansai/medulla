.DEFAULT_GOAL := help

.PHONY: help init fmt clippy test build check ci e2e-image e2e-docker e2e-live

# Tag for the containerized e2e harness image (see e2e/coordination/Dockerfile).
E2E_IMAGE ?= medulla-e2e:latest
# Every suite runs fully network-isolated: the whole stack is loopback.
E2E_RUN = docker run --rm --network none $(E2E_IMAGE) bash

help: ## Show available targets
	@awk 'BEGIN {FS = ":.*## "; print "Usage: make <target>\n"} /^[a-zA-Z0-9_-]+:.*## / {printf "  %-12s %s\n", $$1, $$2}' $(MAKEFILE_LIST)

init: ## Initialize submodules, Rust tooling, dependencies, and Git hooks
	git submodule update --init --recursive
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

e2e-image: ## Build the containerized e2e harness image (slow cold; needs submodules)
	docker build -t $(E2E_IMAGE) -f e2e/coordination/Dockerfile .

e2e-docker: e2e-image ## Run every offline e2e suite in containers (round trip, functional, multi-agent)
	$(E2E_RUN) /app/e2e/coordination/run.sh
	$(E2E_RUN) /app/e2e/coordination/tests.sh
	$(E2E_RUN) /app/e2e/coordination/tests_multi.sh

# Deliberately NOT wired into `ci`: this one bills a real OpenRouter key and
# talks to real staging. It runs on the host (not in a container) so it can use
# your ambient credentials, and refuses to start without E2E_LIVE=1.
e2e-live: ## Run the live staging + OpenRouter suite (needs E2E_LIVE=1 and OPENROUTER_API_KEY)
	bash e2e/coordination/run-live.sh
