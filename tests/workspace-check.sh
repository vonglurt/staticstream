#!/bin/sh
# SPDX-License-Identifier: MIT
# Copyright (c) 2026 Paul Richeson
#
# workspace-check.sh -- make workspace-check: the Workspace's Browser, for
# step 3a of docs/phase-3.md.
#
# PHASE 3 HAS NO PYTHON TO COMPARE AGAINST. Phases 1 and 2 each ended in a
# crosscheck against a specification -- tools/copal-sstr.py, then the Python
# ytq now frozen at tests/reference/ytq.py. The Workspace is the one part of
# the project with no prototype, so what replaces the comparison is the
# report's own bar, and for the Browser it is this:
#
#   A COLUMN IS EXACTLY WHAT `ls` WOULD HAVE SHOWN.
#
# The Browser reads the disk and holds no index, so anything else would be a
# database drifting from the folders. Each column read off the real screen is
# compared with `ls` run over the same folder, in the C locale both sort by.
#
#   C  the columns against `ls`, at 110x30, 80x24 and 50x12
#   K  the keys: down, up past the top, open a folder, back out, and the
#      column that appears to the right when one is opened
#   B  nothing is drawn outside the frame: no row past the last, no line
#      wider than the window
#   S  the starting folder: ARCHIVE_DIR from media.conf, and the fallback
#      when it is not there
#
# Nothing here touches the real queue, archive folder or terminal: every run
# has a throwaway HOME and its own private tmux server.

set -u
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)

# A COLUMN IS SLICED OUT OF THE SCREEN BY CHARACTER, SO THE LOCALE IS FIXED
# HERE. The Browser pads every column to a character width, and `cut -c` counts
# characters only in a UTF-8 locale -- under LC_ALL=C it counts bytes, and one
# accented name (the fixture has café.txt, and ytq really does write such
# names) would shift every column right of it and fail the comparison for the
# wrong reason. C.UTF-8 also sorts bytewise, which is the order read_dir_sorted
# puts entries in, so the order being compared does not change.
LC_ALL=C.UTF-8
export LC_ALL
WS=${WS:-$ROOT/target/release/sstr-workspace}

skip() { printf '  --      workspace-check skipped: %s\n' "$1"; exit 0; }
command -v tmux >/dev/null 2>&1 || skip "no tmux"
[ -x "$WS" ] || { printf 'workspace-check: no %s -- make build\n' "$WS"; exit 2; }

W=$(mktemp -d "${TMPDIR:-/tmp}/sstr-workspace.XXXXXX")
SOCK="sstr-workspace-$$"
T() { tmux -L "$SOCK" -f "$W/tmux.conf" "$@"; }
cleanup() { T kill-server 2>/dev/null; rm -rf "$W"; }
trap cleanup EXIT INT TERM
FAILED=0
PASSED=0
ok() { PASSED=$((PASSED + 1)); [ -n "${VERBOSE:-}" ] && printf '  ok      %s\n' "$1"; return 0; }
bad() { FAILED=$((FAILED + 1)); printf '  FAIL    %s\n' "$1"; }
same() {
    if [ "$2" = "$3" ]; then ok "$1"; else
        bad "$1:"
        printf '          expected: %s\n          got:      %s\n' "$2" "$3"
    fi
}

cat > "$W/tmux.conf" <<'EOF'
set -g status off
set -g window-size manual
set -g remain-on-exit off
EOF

# ---- the folders the Browser will show -------------------------------------
H="$W/home"
A="$H/Videos/Archive"
mkdir -p "$H/.config/copal" "$A/SharedVM/deeper" "$A/Notes"
printf 'ARCHIVE_DIR=%s\n' "$A" > "$H/.config/copal/media.conf"
# Names chosen so byte order and a human's order differ: Z before a, and an
# accent past both. The Browser and `ls` must agree anyway.
for f in "Zeta-report.sstr" "alpha.txt" "beta.sstr" "café.txt"; do
    printf 'x' > "$A/$f"
done
printf 'x' > "$A/.hidden"
printf 'x' > "$A/SharedVM/inside.sstr"

# What `ls` shows, in the C locale: one name a line, dot files left out.
ls_names() { (cd "$1" && ls); }

# ---- driving the Workspace -------------------------------------------------
# start SIZE [ARGS...]: a Workspace in a pane of the given size, on the
# throwaway home, with no ytq environment to find.
start() {
    _size=$1; shift
    _w=${_size%x*}; _h=${_size#*x}
    T kill-session -t ws 2>/dev/null
    T new-session -d -s ws -x "$_w" -y "$_h" \
        env -i HOME="$H" PATH="/usr/bin:/bin" TERM=xterm-256color \
        "$WS" "$@"
    _n=0
    while ! T capture-pane -p -t ws 2>/dev/null | grep -q 'Workspace'; do
        _n=$((_n + 1)); [ $_n -gt 100 ] && return 1; sleep 0.1
    done
    sleep 0.2
}
keys() { for k in "$@"; do T send-keys -t ws "$k"; sleep 0.15; done; }
screen() { T capture-pane -p -t ws; }
stop() { T send-keys -t ws q 2>/dev/null; sleep 0.2; T kill-session -t ws 2>/dev/null; }

# column N WIDTH: the names in column N (1-based) of the screen, one a line.
# The columns start on row 4 and end four rows from the bottom; each is a
# third of the window, and the last character of one is its rule.
column() {
    _n=$1; _w=$2; _h=$3
    _cw=$((_w / 3))
    _from=$((_n - 1)); _x0=$((_from * _cw + 1)); _x1=$((_x0 + _cw - 2))
    screen | sed -n "4,$((_h - 3))p" | cut -c"$_x0-$_x1" \
        | sed 's/[ >|]*$//' | grep -v '^$'
}

# ---- C: the columns against ls ---------------------------------------------
for size in 110x30 80x24 50x12; do
    w=${size%x*}; h=${size#*x}
    if ! start "$size"; then bad "C $size: the Workspace did not draw"; continue; fi
    got=$(column 1 "$w" "$h")
    # A column is a third of the window, and a name longer than it is cut to
    # fit -- so what `ls` says is cut the same way before comparing. A column
    # that is not the last is a third less its rule (cw - 1), and the Browser
    # gives a name that less its gap and its folder marker (width - 3), so the
    # room for a name is cw - 4.
    room=$(( w / 3 - 4 ))
    want=$(ls_names "$A" | cut -c"1-$room" | sed 's/ *$//')
    # A short window shows fewer rows than the folder has; compare what fits.
    rows=$(printf '%s\n' "$got" | grep -c '^' )
    want_head=$(printf '%s\n' "$want" | head -n "$rows")
    same "C $size: the first column is what ls shows" "$want_head" "$got"
    stop
done

# ---- B: nothing outside the frame ------------------------------------------
if start 80x24; then
    long=$(screen | awk '{ if (length($0) > 80) print NR }' | head -1)
    if [ -z "$long" ]; then ok "B no line is wider than the window"; else bad "B line $long is wider than 80"; fi
    rows=$(screen | grep -c '^')
    if [ "$rows" -le 24 ]; then ok "B nothing is drawn past the last row"; else bad "B $rows rows drawn in a 24-row window"; fi
    if screen | sed -n '24p' | grep -q 'Services'; then ok "B the Services line is the last row"; else bad "B the Services line is not the last row"; fi
    if screen | sed -n '1p' | grep -q 'Workspace'; then ok "B the title bar is the first row"; else bad "B the title bar is not the first row"; fi
    stop
else
    bad "B the Workspace did not draw"
fi

# ---- K: the keys -----------------------------------------------------------
if start 110x30; then
    first=$(ls_names "$A" | sed -n '1p')
    second=$(ls_names "$A" | sed -n '2p')

    # The selection is drawn in reverse, which capture-pane -e writes as an
    # escape; the name under it is what we check moved.
    # tmux writes the reverse video it captured in whatever SGR form is
    # shortest for the cell before it: '\033[7m' after a partial redraw, but
    # '\033[0;7m' after a full one. Neither contains the other as text, so the
    # parameters are read properly instead of matched as a string: reverse
    # present, and dim absent, which is how the selection in the deepest column
    # is told from the dim reverse of a parent column's. The first three rows
    # are the title bar (bold and reverse), the Shelf and the rule, and the
    # columns start under them.
    selected() {
        T capture-pane -p -e -t ws | awk -v e="$(printf '\033')" '
            NR <= 3 { next }
            {
                s = $0
                while (match(s, e "\\[[0-9;]*m")) {
                    params = substr(s, RSTART + 2, RLENGTH - 3)
                    rest = substr(s, RSTART + RLENGTH)
                    rev = 0; dim = 0
                    n = split(params, p, ";")
                    for (i = 1; i <= n; i++) {
                        if (p[i] == "7") rev = 1
                        if (p[i] == "2") dim = 1
                    }
                    if (rev && !dim) {
                        m = index(rest, e)
                        print (m ? substr(rest, 1, m - 1) : rest)
                        exit
                    }
                    s = rest
                }
            }' | sed 's/[ >|]*$//'
    }

    same "K the first entry is selected at the start" "$first" "$(selected)"
    keys Down
    same "K down moves to the second entry" "$second" "$(selected)"
    keys Up Up
    same "K up past the top stays on the first" "$first" "$(selected)"

    # SharedVM is a folder; opening it puts a column to its right.
    n=$(ls_names "$A" | grep -n '^SharedVM$' | cut -d: -f1)
    i=1; while [ "$i" -lt "$n" ]; do keys Down; i=$((i + 1)); done
    same "K the folder is selected" "SharedVM" "$(selected)"
    keys Right
    got=$(column 2 110 30)
    want=$(ls_names "$A/SharedVM" | cut -c"1-$(( 110 / 3 - 4 ))" | sed 's/ *$//')
    same "K opening a folder fills the column to its right" "$want" "$got"
    if screen | sed -n '1p' | grep -q 'SharedVM'; then ok "K the title bar follows the folder"; else bad "K the title bar did not follow"; fi
    keys Left
    got=$(column 1 110 30)
    want=$(ls_names "$A" | cut -c"1-$(( 110 / 3 - 4 ))" | sed 's/ *$//')
    rows=$(printf '%s\n' "$got" | grep -c '^')
    same "K backing out returns to the folder above" "$(printf '%s\n' "$want" | head -n "$rows")" "$got"
    stop
else
    bad "K the Workspace did not draw"
fi

# ---- S: where it starts ----------------------------------------------------
if start 80x24; then
    if screen | sed -n '1p' | grep -q 'Archive'; then ok "S it starts at ARCHIVE_DIR from media.conf"; else bad "S it did not start at ARCHIVE_DIR"; fi
    stop
else
    bad "S the Workspace did not draw"
fi

mv "$A" "$A.gone"
if start 80x24; then
    if screen | grep -q 'no archive folder'; then ok "S a missing archive folder is said in the Transcript"; else bad "S nothing was said about the missing archive folder"; fi
    stop
else
    bad "S the Workspace did not draw without an archive folder"
fi
mv "$A.gone" "$A"

# ---- the count -------------------------------------------------------------
if [ "$FAILED" -eq 0 ]; then
    printf '  ok      workspace-check: %s checks pass\n' "$PASSED"
    exit 0
fi
printf '  FAIL    workspace-check: %s of %s failed\n' "$FAILED" "$((PASSED + FAILED))"
exit 1
