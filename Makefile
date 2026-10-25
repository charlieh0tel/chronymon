# CI calls the shared rust-ci.yml; "make ci" runs the same checks locally.

CARGO ?= cargo

.PHONY: all build hooks ci fmt fmt-check clippy test

all: build

build:
	$(CARGO) build --workspace --all-targets

# Use the repository's git hooks, once per clone: commit-msg refuses a
# message crediting an AI assistant (AGENTS.md).
hooks:
	git config core.hooksPath .githooks

ci: fmt-check clippy test

fmt:
	$(CARGO) fmt --all

fmt-check:
	$(CARGO) fmt --all -- --check

clippy:
	$(CARGO) clippy --workspace --all-targets --all-features -- -D warnings

test:
	$(CARGO) test --workspace
