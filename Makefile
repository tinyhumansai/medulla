.DEFAULT_GOAL := help

.PHONY: help init fmt clippy test build check ci

help: ## Show available targets
	@awk 'BEGIN {FS = ":.*## "; print "Usage: make <target>\n"} /^[a-zA-Z_-]+:.*## / {printf "  %-10s %s\n", $$1, $$2}' $(MAKEFILE_LIST)

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
