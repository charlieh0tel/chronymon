.PHONY: hooks

# Use the repository's git hooks, once per clone: commit-msg refuses a
# message crediting an AI assistant (AGENTS.md).
hooks:
	git config core.hooksPath .githooks
