# CI calls the shared rust-ci.yml; "make ci" runs the same checks locally.

CARGO ?= cargo

.PHONY: all build hooks ci fmt fmt-check clippy test web-deps test-web

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

WEB_TESTS := crates/chronymon-web/tests/browser

# Playwright and its Chromium into the test directory, once, for running
# the browser tests natively.  Without Node, test-web runs them in
# Playwright's container instead and needs none of this.
web-deps:
	cd $(WEB_TESTS) && npm ci --no-fund --no-audit && npm run install-browser

# The web pages in a browser: the real server, serving a synthetic
# database and none.  See tools/test-web.
test-web:
	tools/test-web
