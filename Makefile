# SPDX-License-Identifier: MIT
# Copyright (c) 2026 Paul Richeson
#
# staticstream -- Static Stream, the sstr command and its Workspace.
#
#   make                 this list
#   make check           what a commit must pass
#   make run ARGS=paths  the sstr command, with arguments
#
# Everything except `tools` and `dist` needs nothing but cargo. copal-build
# compiles this checkout on machines that never reach the internet, and it
# calls cargo, not make, so no step here may be one the build depends on.
# Nothing here writes inside a tracked file either: a modified tracked file
# makes `git pull --ff-only` refuse every update after it.

CARGO ?= cargo
# cargo install writes binaries into $(ROOT)/bin; ~/.local/bin is on Copal's PATH.
ROOT ?= $(HOME)/.local
ARGS ?=

RED = \033[31m
OFF = \033[0m

.DEFAULT_GOAL := help
.PHONY: help build run workspace test deps check install tools dist clean

help: ## this list
	@printf 'staticstream -- make TARGET\n\n'
	@grep -E '^[a-z-]+: .*## ' $(MAKEFILE_LIST) | sed 's/:.*## /|/' | awk -F'|' '{printf "  make %-10s %s\n", $$1, $$2}'

build: ## release build of every crate; also writes Cargo.lock the first time
	$(CARGO) build --release --workspace

run: ## the sstr command:  make run ARGS='paths'
	$(CARGO) run --release --quiet -p staticstream-cli -- $(ARGS)

workspace: ## the terminal Workspace
	$(CARGO) run --release --quiet -p staticstream-workspace -- $(ARGS)

test: ## the tests, including the constants against tools/copal-sstr.py
	$(CARGO) test --workspace --quiet

deps: ## prove Cargo.lock names no crate from outside this repository
	@test -f Cargo.lock || { printf '$(RED)error:$(OFF) no Cargo.lock -- run make build once, and commit it\n'; exit 1; }
	@if grep -q '^source = ' Cargo.lock; then \
	    printf '$(RED)error:$(OFF) Cargo.lock names crates from outside this repository:\n'; \
	    grep -B2 '^source = ' Cargo.lock | sed -n 's/^name = /  /p'; exit 1; fi
	@printf '  ok      no external crates: %s packages, all in this workspace\n' "$$(grep -c '^name = ' Cargo.lock)"

# The ytq crosschecks run against the binary, not `sstr ytq`: step 2e's bar is
# that the whole crosscheck passes with `ytq` in the Python one's place, so
# that is what a commit must pass. Run by hand (make ytq-crosscheck) they take
# their default and exercise `sstr ytq` instead, so both spellings are covered.
YTQ_BIN = $(CURDIR)/target/release/ytq

check: deps ## what a commit must pass: no external crates, the tests, an offline release build, the crosscheck
	$(CARGO) test --workspace --offline --locked --quiet
	$(CARGO) build --release --workspace --offline --locked
	@sh tests/crosscheck.sh
	@YTQ='$(YTQ_BIN)' sh tests/ytq-crosscheck.sh
	@YTQ='$(YTQ_BIN)' sh tests/ytq-runner-crosscheck.sh
	@YTQ='$(YTQ_BIN)' sh tests/ytq-archive-check.sh
	@YTQ='$(YTQ_BIN)' sh tests/ytq-window-crosscheck.sh
	@printf '  ok      check passed\n'

crosscheck: ## the Rust sstr against tools/copal-sstr.py in ../copal: both directions, damage, armor
	@$(CARGO) build --release --workspace --offline --locked --quiet
	@VERBOSE=1 sh tests/crosscheck.sh

ytq-crosscheck: ## the Rust ytq against the Python ytq of tests/reference: urls, settings, queue, clipboard
	@$(CARGO) build --release --workspace --offline --locked --quiet
	@VERBOSE=1 sh tests/ytq-crosscheck.sh

# YTQ_REAL=0 leaves out the real download of jNQXAC9IVRw; without a network it is left out anyway.
ytq-runner-crosscheck: ## the Rust runner against the Python ytq's: downloads, stops, cookies, retries, transcripts
	@$(CARGO) build --release --workspace --offline --locked --quiet
	@VERBOSE=1 sh tests/ytq-runner-crosscheck.sh

ytq-archive-check: ## the Rust ytq archiving downloads into Static Stream: OUTPUT, ARCHIVE_DIR, SSTR_KEY
	@$(CARGO) build --release --workspace --offline --locked --quiet
	@VERBOSE=1 sh tests/ytq-archive-check.sh

# Needs tmux and Python's curses; without either it is skipped.
ytq-window-crosscheck: ## the Rust window beside the Python ytq's curses window, in tmux: screens, keys, worker, watcher
	@$(CARGO) build --release --workspace --offline --locked --quiet
	@VERBOSE=1 sh tests/ytq-window-crosscheck.sh

# ytq is installed from here since step 2e: it does everything the Python one
# did, and that one is retired from copal-prep.sh. $(ROOT)/bin comes before
# /usr/local/bin on Copal's PATH, so this is the ytq that Super+Shift+Y and
# every `ytq` typed in a shell now reach.
install: ## sstr, sstr-workspace and ytq into ~/.local/bin (ROOT=DIR for DIR/bin)
	$(CARGO) install --locked --offline --root $(ROOT) --path crates/staticstream-cli
	$(CARGO) install --locked --offline --root $(ROOT) --path crates/staticstream-workspace
	$(CARGO) install --locked --offline --root $(ROOT) --path crates/staticstream-ytq

tools: ## cargo-make and cargo-zigbuild, for make dist: apk on Alpine, else cargo install
	@if command -v apk >/dev/null 2>&1; then doas apk add cargo-make cargo-zigbuild; \
	 else $(CARGO) install --locked cargo-make cargo-zigbuild; fi

dist: ## release binaries for each target in Makefile.toml, into dist/
	@command -v cargo-make >/dev/null 2>&1 || { printf '$(RED)error:$(OFF) no cargo-make -- make tools\n'; exit 1; }
	$(CARGO) make dist

clean: ## remove target/ and dist/
	$(CARGO) clean
	rm -rf dist
