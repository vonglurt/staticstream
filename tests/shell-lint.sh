#!/bin/sh
# SPDX-License-Identifier: MIT
# Copyright (c) 2026 Paul Richeson
#
# shell-lint.sh -- make shell-lint, and part of make check: shellcheck over
# every shell script here, the checks themselves included. They are what
# says the program is right, so they are held to the same standard as it is
# -- section VI of copal's docs/text-safety-lab-report.md.
#
#   anything shellcheck says  fails: an error, a warning or a note
#   /tmp/name.$$              fails: a name that can be guessed; use mktemp
#   a script with no set -u   fails
#
# A finding that is wrong is answered where it is raised, with
#   # shellcheck disable=SC2086  # and the reason, in a few words
# so that the next reader is told why, and the one after that can disagree.
#
# Without shellcheck this is skipped, and says so.

set -eu
cd "$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)" || exit 1

if ! command -v shellcheck >/dev/null 2>&1; then
    printf '  --      shell-lint skipped: no shellcheck here  (apk add shellcheck)\n'
    exit 0
fi

list=$(mktemp)
said=$(mktemp)
trap 'rm -f "$list" "$said"' EXIT

# By what a file says it is, not by its name: a stand-in has no extension.
find tests tools -type f ! -name '._*' ! -path '*/__pycache__/*' | sort | while read -r f; do
    case "$(head -n 1 "$f" 2>/dev/null)" in
        '#!/bin/sh'*|'#!/usr/bin/env sh'*) printf '%s\n' "$f" ;;
    esac
done > "$list"
files=$(wc -l < "$list" | tr -d ' ')
[ "$files" -gt 0 ] || { printf '\033[31merror:\033[0m shell-lint: found no scripts to read\n'; exit 1; }

if xargs grep -nE '^[[:space:]]*[^#[:space:]].*/tmp/[A-Za-z0-9._-]+\.\$\$' < "$list" > "$said" 2>/dev/null; then
    printf '\033[31merror:\033[0m shell-lint: a file in /tmp with a name that can be guessed; use mktemp\n'
    sed 's/^/          /' "$said" | cut -c1-150
    exit 1
fi

unset_ok=$(while read -r f; do
    grep -qE '^[[:space:]]*set -[a-z]*u' "$f" || printf '%s\n' "$f"
done < "$list")
if [ -n "$unset_ok" ]; then
    printf '\033[31merror:\033[0m shell-lint: a script with no set -u:\n'
    printf '%s\n' "$unset_ok" | sed 's/^/          /'
    exit 1
fi

# It exits 1 when it has anything to say, which is what is being asked.
if xargs shellcheck -f gcc < "$list" > "$said" 2>&1; then
    printf '  ok      shell-lint: %s scripts, and shellcheck has nothing to say\n' "$files"
else
    printf '\033[31merror:\033[0m shell-lint: %s findings in %s scripts\n' "$(grep -c ': ' "$said")" "$files"
    sed 's/^/          /' "$said"
    exit 1
fi
