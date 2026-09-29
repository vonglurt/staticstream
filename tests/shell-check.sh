#!/bin/sh
# SPDX-License-Identifier: MIT
# Copyright (c) 2026 Paul Richeson
#
# shell-check.sh -- make shell-check: where this program hands a line to a
# shell, and that it is nowhere else.
#
# A process is started with an argument list, so that no word of a title or
# a filename is ever read by a shell. The Workspace is the one exception, on
# purpose: a Service is a command line a person can read and retype, so it is
# said and then given to `sh -c` as it was said, every word through
# services::quote. That is two lines of the program. Two more are tests,
# which ask a real shell whether the quoting held.
#
# A new one fails this check. If it is meant, it is added to the list below,
# by hand, by somebody who has read what reaches it.

set -u
ROOT=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$ROOT" || exit 2

WANT='src/workspace/config.rs:2
src/workspace/mod.rs:1
src/workspace/services.rs:1'
GOT=$(grep -rcE 'Command::new\("(/bin/|/usr/bin/)?(sh|bash|ash|dash|zsh)"\)' src | grep -v ':0$' | sort)

if [ "$GOT" = "$WANT" ]; then
    printf '  ok      shell-check: sh -c in the Workspace, twice, and twice in its tests; nowhere else\n'
else
    printf '\033[31merror:\033[0m shell-check: a shell is started somewhere it was not:\n'
    grep -rnE 'Command::new\("(/bin/|/usr/bin/)?(sh|bash|ash|dash|zsh)"\)' src | sed 's/^/          /'
    printf '          wanted, by file:\n%s\n' "$WANT" | sed '2,$s/^src/          src/'
    exit 1
fi
