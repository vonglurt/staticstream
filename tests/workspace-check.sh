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
#   I  the Inspector (3b): what it shows about a capture is what
#      `sstr verify` says about it, line for line; a folder shows its count
#   T  the Transcript (3c): the band holds the lines `ytq` wrote, in the
#      order ytq.log has them, and goes on holding them across the rename
#      that rotation is -- driven by a real ytq writing a real 4 MiB log
#   V  Services (3d): the report's acceptance test. Press the key, read the
#      command line the Transcript printed BEFORE it ran, run that exact
#      line in a shell with a fresh HOME, and compare what each left --
#      what was printed, what was written, and the exit status
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
SSTR=${SSTR:-$ROOT/target/release/sstr}
# The Transcript follows ytq's log, so the other half of that comparison is
# ytq itself writing it -- not the harness writing something log-shaped.
YTQ=${YTQ:-$ROOT/target/release/ytq}
# Serve binds a port. A high one, and the check steps aside if anything is
# already there rather than failing for a reason that is not the Workspace's.
SERVE_PORT=${SERVE_PORT:-18099}

skip() { printf '  --      workspace-check skipped: %s\n' "$1"; exit 0; }
command -v tmux >/dev/null 2>&1 || skip "no tmux"
[ -x "$WS" ] || { printf 'workspace-check: no %s -- make build\n' "$WS"; exit 2; }
[ -x "$SSTR" ] || { printf 'workspace-check: no %s -- make build\n' "$SSTR"; exit 2; }
[ -x "$YTQ" ] || { printf 'workspace-check: no %s -- make build\n' "$YTQ"; exit 2; }

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
# The report's settings for the Workspace. PLAYER is a stand-in, because a
# Service must never reach the real mpv; SERVE is the port above.
printf 'SERVE=127.0.0.1:%s\n' "$SERVE_PORT" >> "$H/.config/copal/media.conf"
printf 'PLAYER=%s\n' "$ROOT/tests/standin/player" >> "$H/.config/copal/media.conf"
# Names chosen so byte order and a human's order differ: Z before a, and an
# accent past both. The Browser and `ls` must agree anyway.
for f in "Zeta-report.sstr" "alpha.txt" "beta.sstr" "café.txt"; do
    printf 'x' > "$A/$f"
done
printf 'x' > "$A/jawed-Me_at_the_zoo_jNQXAC9IVRw.txt"
printf 'x' > "$A/jawed-Me_at_the_zoo_jNQXAC9IVRw.sstr"
printf 'x' > "$A/.hidden"
printf 'x' > "$A/SharedVM/inside.sstr"
# A capture the Inspector can be held to: `sstr verify` is the other half of
# the comparison, so it has to be one sstr actually wrote.
printf 'the payload of a real capture, recorded for the Inspector to read\n' > "$W/payload.txt"
"$SSTR" record "$A/capture.sstr" --input "$W/payload.txt" --type text/plain \
    --source 'https://example.invalid/a-page' --license 'not stated' \
    --note 'a capture by the check' >/dev/null 2>&1 || bad "could not record a capture to inspect"

# workspace::transcript_rows, in shell. Written twice on purpose, for the
# reason elide below is: the comparison IS the agreement between them, so a
# band that moves without this moving with it fails loudly.
tr_rows() { if [ "$1" -ge 20 ]; then echo 3; else echo 1; fi; }

# What `ls` shows, in the C locale: one name a line, dot files left out.
ls_names() { (cd "$1" && ls); }

# A name as the Browser draws it in a column ROOM characters wide: too long,
# and the middle goes so that the extension stays. This is workspace::elide in
# shell, and the two have to agree -- that is the point of the comparison.
# wc -m and cut -c count characters, not bytes, because LC_ALL is C.UTF-8.
elide() {
    _s=$1; _room=$2
    _n=$(printf '%s' "$_s" | wc -m)
    if [ "$_n" -le "$_room" ]; then printf '%s\n' "$_s"; return; fi
    _ext=""
    case "$_s" in *.*) _ext=${_s##*.} ;; esac
    _el=$(printf '%s' "$_ext" | wc -m)
    if [ -n "$_ext" ] && [ "$_el" -le 8 ] && [ "$_room" -gt $(( _el + 3 )) ]; then
        printf '%s..%s\n' "$(printf '%s' "$_s" | cut -c"1-$(( _room - 2 - _el ))")" "$_ext"
    else
        printf '%s' "$_s" | cut -c"1-$_room"; printf '\n'
    fi
}

# Every name in DIR, as the Browser would draw it ROOM wide.
ls_elided() {
    ls_names "$1" | while IFS= read -r _name; do elide "$_name" "$2"; done
}

# ---- driving the Workspace -------------------------------------------------
# start SIZE [ARGS...]: a Workspace in a pane of the given size, on the
# throwaway home, with no ytq environment to find.
start() {
    _size=$1; shift
    _w=${_size%x*}; _h=${_size#*x}
    T kill-session -t ws 2>/dev/null
    # sstr AND ytq ARE ON THE PATH, because a Service is a command line and
    # a command line names `sstr`, not a path into a build directory. On the
    # guest copal-build puts them in ~/.local/bin; here it is the release
    # directory, and the standins come first so nothing reaches a real player.
    T new-session -d -s ws -x "$_w" -y "$_h" \
        env -i HOME="$H" PATH="$ROOT/tests/standin:$ROOT/target/release:/usr/bin:/bin" TERM=xterm-256color \
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

# The last row the Miller columns and the Inspector own, 1-based. Below it
# are the rule, the Transcript's band and the Services line.
last_col_row() { echo $(( $1 - 2 - $(tr_rows "$1") )); }

# column N WIDTH: the names in column N (1-based) of the screen, one a line.
# The columns start on row 4 and end above the rule; each is a third of the
# window, and the last character of one is its rule.
column() {
    _n=$1; _w=$2; _h=$3
    _cw=$((_w / 3))
    _from=$((_n - 1)); _x0=$((_from * _cw + 1)); _x1=$((_x0 + _cw - 2))
    screen | sed -n "4,$(last_col_row "$_h")p" | cut -c"$_x0-$_x1" \
        | sed 's/[ >|]*$//' | grep -v '^$'
}

# pane3 WIDTH HEIGHT: the Inspector's lines, one a line, trailing blanks gone.
# It is the last third of the window, past the two Miller columns.
pane3() {
    _w=$1; _h=$2
    _cw=$(( _w / 3 ))
    screen | sed -n "4,$(last_col_row "$_h")p" | cut -c"$(( 2 * _cw + 1 ))-$_w" | sed 's/ *$//'
}

# band WIDTH HEIGHT: the Transcript's lines, the label and its indent gone.
# The band is the rows between the lower rule and the Services line, and
# ' Transcript ' is twelve characters wide.
band() {
    _w=$1; _h=$2; _tr=$(tr_rows "$_h")
    screen | sed -n "$(( _h - _tr )),$(( _h - 1 ))p" | cut -c13- | sed 's/ *$//'
}

# transcript::shown, in shell: a log line as the band draws it -- the time out
# of the stamp, and the message. The date and the pid go. This is the second
# writing of that rule, and the check is what holds the two together.
shown() {
    sed 's/^[0-9]\{4\}-[0-9]\{2\}-[0-9]\{2\} \([0-9]\{2\}:[0-9]\{2\}:[0-9]\{2\}\) \[[0-9]\{1,\}\] /\1 /'
}

# What the band should hold, given the last lines of a log: each as `shown`
# draws it, cut where the pane ends.
band_want() {
    _log=$1; _w=$2; _h=$3; _tr=$(tr_rows "$_h")
    tail -n "$_tr" "$_log" | shown | cut -c"1-$(( _w - 12 ))" | sed 's/ *$//'
}

# The entry drawn in reverse video in the deepest column: the selection.
#
# tmux writes the reverse video it captured in whatever SGR form is shortest
# for the cell before it: '\033[7m' after a partial redraw, but '\033[0;7m'
# after a full one. Neither contains the other as text, so the parameters are
# read properly instead of matched as a string: reverse present, and dim
# absent, which is how the selection in the deepest column is told from the
# dim reverse of a parent column's. The first three rows are the title bar
# (bold and reverse), the Shelf and the rule.
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

# Walk to a named entry, however the folder happens to be ordered. Down stops
# at the last entry rather than wrapping, so this goes back to the top first --
# otherwise a second call, for a name above the one already selected, would
# walk into its own bound and never find it.
select_name() {
    _want=$1; _i=0
    while [ "$_i" -lt 12 ]; do keys Up; _i=$(( _i + 1 )); done
    _i=0
    while [ "$(selected)" != "$_want" ]; do
        keys Down; _i=$(( _i + 1 ))
        [ "$_i" -gt 24 ] && return 1
    done
    return 0
}

# ---- C: the columns against ls ---------------------------------------------
for size in 110x30 80x24 50x12; do
    w=${size%x*}; h=${size#*x}
    if ! start "$size"; then bad "C $size: the Workspace did not draw"; continue; fi
    got=$(column 1 "$w" "$h")
    # A column is a third of the window, and a name longer than it is drawn
    # elided -- so what `ls` says is elided the same way before comparing. A
    # column that is not the last is a third less its rule (cw - 1), and the
    # Browser gives a name that less its gap and its folder marker
    # (width - 3), so the room for a name is cw - 4.
    room=$(( w / 3 - 4 ))
    want=$(ls_elided "$A" "$room")
    # A short window shows fewer rows than the folder has; compare what fits.
    rows=$(printf '%s\n' "$got" | grep -c '^' )
    want_head=$(printf '%s\n' "$want" | head -n "$rows")
    same "C $size: the first column is what ls shows" "$want_head" "$got"
    dupes=$(printf '%s\n' "$got" | sort | uniq -d)
    if [ -z "$dupes" ]; then
        ok "C $size: no two entries draw alike"
    else
        bad "C $size: two entries draw alike: $dupes"
    fi
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
    want=$(ls_elided "$A/SharedVM" "$(( 110 / 3 - 4 ))")
    same "K opening a folder fills the column to its right" "$want" "$got"
    if screen | sed -n '1p' | grep -q 'SharedVM'; then ok "K the title bar follows the folder"; else bad "K the title bar did not follow"; fi
    keys Left
    got=$(column 1 110 30)
    want=$(ls_elided "$A" "$(( 110 / 3 - 4 ))")
    rows=$(printf '%s\n' "$got" | grep -c '^')
    same "K backing out returns to the folder above" "$(printf '%s\n' "$want" | head -n "$rows")" "$got"
    stop
else
    bad "K the Workspace did not draw"
fi

# ---- I: the Inspector (3b) -------------------------------------------------
if start 110x30; then
    if select_name "capture.sstr"; then
        # The Inspector's first line is the name, then a blank, then what
        # `sstr verify` prints -- cut to the pane, which is a third of 110.
        room=$(( 110 - 2 * (110 / 3) ))
        got=$(pane3 110 30 | sed -n '3,12p')
        want=$("$SSTR" verify "$A/capture.sstr" | head -10 | cut -c"1-$room" | sed 's/ *$//')
        same "I a capture is what sstr verify says about it" "$want" "$got"
        first=$(pane3 110 30 | sed -n '1p')
        same "I the Inspector names the selection" "capture.sstr" "$first"
        if pane3 110 30 | grep -q 'verify: everything checked out'; then
            ok "I a good capture is said to have checked out"
        else
            bad "I nothing said about the capture verifying"
        fi
    else
        bad "I could not select the capture"
    fi

    if select_name "Notes"; then
        if pane3 110 30 | grep -q '^folder, 0 items$'; then
            ok "I a folder shows its count"
        else
            bad "I a folder did not show its count: $(pane3 110 30 | sed -n '3p')"
        fi
    else
        bad "I could not select the folder"
    fi
    stop
else
    bad "I the Workspace did not draw"
fi

# ---- T: the Transcript (3c) -------------------------------------------------
# THE OTHER HALF OF THIS COMPARISON IS ytq. `ytq add --no-run` writes three
# lines to ytq.log, starts no runner and touches no network, so the pane can
# be held against a log a real ytq really wrote -- which is the same shape of
# anchor `ls` is for the Browser and `sstr verify` is for the Inspector.
LOG="$H/.local/share/ytq/ytq.log"
ytq_() { env -i HOME="$H" PATH="/usr/bin:/bin" "$YTQ" "$@" >/dev/null 2>&1; }

if start 110x30; then
    # Nothing has written to ytq.log yet, so the only line is the Workspace's
    # own -- and it is the command line that opened it, which is the rule
    # Services will keep in 3d.
    last=$(band 110 30 | grep -v '^$' | tail -1)
    case "$last" in
        *"sstr-workspace ~/Videos/Archive") ok "T the Workspace says the command line that opened it" ;;
        *) bad "T the opening line is not a command line: $last" ;;
    esac
    if [ "$(band 110 30 | grep -c '^')" -eq 3 ]; then
        ok "T the band is three rows in a tall window"
    else
        bad "T the band is $(band 110 30 | grep -c '^') rows, not 3"
    fi
    if screen | sed -n "$(( 30 - 1 - 3 ))p" | grep -q '^-\{20,\}$'; then
        ok "T the rule sits above the band"
    else
        bad "T no rule above the band"
    fi

    ytq_ add --no-run 'https://www.youtube.com/watch?v=jNQXAC9IVRw'
    sleep 1.5
    same "T the band holds the lines ytq wrote, in the log's order" \
        "$(band_want "$LOG" 110 30)" "$(band 110 30)"
    stop
else
    bad "T the Workspace did not draw"
fi

if start 50x12; then
    if [ "$(band 50 12 | grep -c '^')" -eq 1 ]; then
        ok "T the band is one row in a short window"
    else
        bad "T the band is $(band 50 12 | grep -c '^') rows in a short window, not 1"
    fi
    ytq_ add --no-run 'https://example.invalid/short-window'
    sleep 1.5
    same "T a short window holds the newest line ytq wrote" \
        "$(band_want "$LOG" 50 12)" "$(band 50 12)"
    stop
else
    bad "T the Workspace did not draw in a short window"
fi

# A ROTATION, DRIVEN BY YTQ'S OWN log(). Past LOG_MAX -- 4 MiB -- the next
# line ytq writes renames ytq.log to ytq.log.1 and starts again. A follower
# that remembers only a byte offset goes silent here: its offset is far past
# the end of the file that now wears the name, so every later read returns
# nothing and the pane freezes with the last thing it happened to have.
#
# WHAT THIS CHECKS IS THAT FOLLOWING SURVIVES THE RENAME. The line written to
# the old file between the last poll and the rename -- the one at the seam --
# cannot be placed at that instant from a shell, and is checked instead by
# a_rotation_loses_nothing_at_the_seam in src/workspace/transcript.rs, which
# can. The two together are the step's done-condition.
if start 110x30; then
    before=$(band 110 30 | tail -1)
    awk 'BEGIN{for(i=0;i<100000;i++) printf "2026-09-16 08:00:00 [1] padding line %d\n", i}' >> "$LOG"
    sleep 1.5
    if [ "$(band 110 30 | tail -1)" != "$before" ]; then
        ok "T a burst of lines is followed, not slept through"
    else
        bad "T the band did not move when the log grew"
    fi
    size=$(wc -c < "$LOG")
    if [ "$size" -gt 4194304 ]; then
        ok "T the log is past LOG_MAX, so ytq's next line rotates it"
    else
        bad "T the log is only $size bytes -- no rotation will happen"
    fi

    ytq_ add --no-run 'https://example.invalid/after-the-seam'
    sleep 2
    if [ -f "$LOG.1" ]; then
        ok "T ytq rotated its log"
    else
        bad "T ytq did not rotate its log"
    fi
    same "T the band follows across the rename" \
        "$(band_want "$LOG" 110 30)" "$(band 110 30)"
    keys Down
    if screen | sed -n '1p' | grep -q 'Workspace'; then
        ok "T the Workspace still answers its keys after a rotation"
    else
        bad "T the Workspace stopped answering after a rotation"
    fi
    stop
else
    bad "T the Workspace did not draw for the rotation"
fi
rm -f "$LOG" "$LOG.1"

# ---- V: Services (3d) -------------------------------------------------------
# THE REPORT'S ACCEPTANCE TEST, run against the real binary:
#
#   1. drive the Workspace to the selection and press the key;
#   2. read the command line the Transcript printed BEFORE it ran;
#   3. run that exact line in a shell, with a fresh HOME;
#   4. compare what each left.
#
# THE WINDOW IS 200 COLUMNS WIDE HERE and that is not cosmetic. A line is
# drawn cut to the pane, and `sstr play LONG -o LONG` names the path twice;
# at 110 columns the check would be reading half a command line and running
# it. The Workspace is being asked what it printed, so it is asked at a size
# where the answer is whole.
VW=200; VH=30
FRESH="$W/fresh-home"; mkdir -p "$FRESH"
# A fresh HOME, and none of the Workspace's environment: a command line that
# only works inside the window it came from is not a command line.
in_a_shell() { env -i HOME="$FRESH" PATH="$ROOT/target/release:/usr/bin:/bin" sh -c "$1" 2>&1; }

# The last command line the Transcript printed. Looked for by its shape
# rather than by counting lines back, so a Service that prints as well as
# runs cannot shift it.
said() { band $VW $VH | grep -v '^$' | cut -c10- | grep -E '^(sstr|cat|mpv|vlc) ' | tail -1; }
# The last line of the band: a Service's outcome.
outcome() { band $VW $VH | grep -v '^$' | cut -c10- | tail -1; }
# What the LAST Service to take the terminal wrote there.
#
# THE PANE STILL HOLDS THE ONE BEFORE IT. Leaving the alternate screen puts
# back the ordinary screen, which is where the previous Service also wrote --
# so a capture of the whole pane is two Services' output run together, and a
# comparison against one command line would fail for a reason that is not
# the Workspace's. The Workspace prints each command line above its own
# output, as a shell shows what was typed, so the last of them is the mark to
# read from.
# AND awk -v IS NOT THE WAY TO HAND IT THE MARK. awk runs escape processing
# over a -v value, so the `\'` in a quoted name -- `'Don'\''t Look Up.sstr'`,
# which is the whole point of the awkward capture -- loses its backslash
# before the comparison, and the mark never matches. grep -F -x compares the
# bytes.
on_the_terminal() {
    _all=$(screen | sed 's/ *$//')
    _n=$(printf '%s\n' "$_all" | grep -n -F -x -- "$1" | tail -1 | cut -d: -f1)
    if [ -n "$_n" ]; then
        printf '%s\n' "$_all" | sed -n "$(( _n + 1 )),\$p"
    else
        printf '%s\n' "$_all"
    fi | grep -v 'press a key to return to the Workspace' | grep -v '^$'
}

# A capture whose name a shell would take apart if it were not quoted. It is
# a name a person can make, and ytq writes names with spaces and apostrophes
# every day.
AWKWARD="Don't Look Up.sstr"
"$SSTR" record "$A/$AWKWARD" --input "$W/payload.txt" --type text/plain \
    --note 'a name that needs quoting' >/dev/null 2>&1 || bad "V could not record the awkward capture"

if start "${VW}x${VH}"; then
    if select_name "capture.sstr"; then
        # -- Verify: a report a person reads, so it takes the terminal --
        keys v; sleep 1.5
        got=$(on_the_terminal "sstr verify $A/capture.sstr")
        keys Enter; sleep 0.8
        line=$(said)
        same "V the line Verify printed is sstr verify on the selection" \
            "sstr verify $A/capture.sstr" "$line"
        want=$(in_a_shell "$line" | sed 's/ *$//' | grep -v '^$')
        same "V Verify equals its own command line, run in a shell" "$want" "$got"
        same "V and the Transcript says how it went" "Verify ok" "$(outcome)"

        # THE LINE IS PRINTED BEFORE IT RUNS, not after. On screen that is an
        # order: the command line is above the outcome, never below it.
        # EACH ROW IS FOUND BY WHAT IT IS, not by which match came first.
        # Asking only whether the first of two matches precedes the second is
        # true however they are ordered, and it passed against a Workspace
        # that said the line after the outcome.
        rows=$(band $VW $VH | grep -v '^$' | cut -c10-)
        cmdrow=$(printf '%s\n' "$rows" | grep -n '^sstr verify ' | tail -1 | cut -d: -f1)
        outrow=$(printf '%s\n' "$rows" | grep -n '^Verify ok$' | tail -1 | cut -d: -f1)
        if [ -n "$cmdrow" ] && [ -n "$outrow" ] && [ "$cmdrow" -lt "$outrow" ]; then
            ok "V the command line is said before the outcome, not after"
        else
            bad "V the command line was not said first: line at row ${cmdrow:-none}, outcome at row ${outrow:-none}"
        fi
    else
        bad "V could not select the capture"
    fi

    # -- Export: a file written, and the same file when the line is typed --
    if select_name "capture.sstr"; then
        keys x; sleep 2
        keys Enter; sleep 0.8
        line=$(said)
        out=$(printf '%s' "$line" | sed 's/.* -o //')
        if [ -f "$out" ]; then
            ok "V Export wrote the file its command line named"
            mv "$out" "$W/from-the-workspace"
            in_a_shell "$line" >/dev/null 2>&1
            if cmp -s "$W/from-the-workspace" "$out"; then
                ok "V Export leaves the same bytes as its command line run in a shell"
            else
                bad "V Export and its command line left different bytes"
            fi
            rm -f "$out" "$W/from-the-workspace"
        else
            bad "V Export named $out and did not write it"
        fi
        # AND IT DID NOT WRITE OVER THE TRANSCRIPT BESIDE THE CAPTURE.
        if [ "$(cat "$A/alpha.txt")" = "x" ]; then
            ok "V Export left the other files alone"
        else
            bad "V Export wrote over something"
        fi
    else
        bad "V could not select the capture to export"
    fi

    # -- A name that needs quoting reaches the shell as one word --
    if select_name "$AWKWARD"; then
        keys v; sleep 1.5
        got=$(on_the_terminal "sstr verify '$A/Don'\\''t Look Up.sstr'")
        keys Enter; sleep 0.8
        line=$(said)
        want=$(in_a_shell "$line" | sed 's/ *$//' | grep -v '^$')
        same "V a name with a space and an apostrophe survives the shell" "$want" "$got"
        if printf '%s' "$got" | grep -q 'stream'; then
            ok "V and it really verified the capture, rather than a shell error"
        else
            bad "V the awkward name did not verify: $got"
        fi
    else
        bad "V could not select $AWKWARD"
    fi

    # -- A folder is opened, not sent a verb --
    if select_name "SharedVM"; then
        if screen | sed -n "${VH}p" | grep -q 'nothing to send this selection'; then
            ok "V a folder is offered no Services"
        else
            bad "V a folder was offered Services: $(screen | sed -n "${VH}p")"
        fi
    else
        bad "V could not select the folder"
    fi
    stop
else
    bad "V the Workspace did not draw"
fi

# -- Serve: what it hands a player equals what the command line hands one --
if command -v curl >/dev/null 2>&1 && ! nc -z 127.0.0.1 "$SERVE_PORT" 2>/dev/null; then
    if start "${VW}x${VH}"; then
        if select_name "capture.sstr"; then
            keys s; sleep 1.5
            line=$(said)
            same "V the line Serve printed is sstr play --serve on the selection" \
                "sstr play $A/capture.sstr --serve 127.0.0.1:$SERVE_PORT" "$line"
            pid=$(outcome | sed -n 's/^Serve started, pid \([0-9]*\)$/\1/p')
            if [ -n "$pid" ]; then
                ok "V Serve says it started, and its pid"
                _n=0
                while ! nc -z 127.0.0.1 "$SERVE_PORT" 2>/dev/null; do
                    _n=$(( _n + 1 )); [ $_n -gt 50 ] && break; sleep 0.1
                done
                curl -s "http://127.0.0.1:$SERVE_PORT/" > "$W/served" 2>/dev/null
                in_a_shell "sstr play $A/capture.sstr" > "$W/played" 2>/dev/null
                if cmp -s "$W/served" "$W/played"; then
                    ok "V what Serve hands a player is what sstr play writes"
                else
                    bad "V the served bytes differ from sstr play's"
                fi
                kill "$pid" 2>/dev/null
                rm -f "$W/served" "$W/played"
            else
                bad "V Serve did not say a pid: $(outcome)"
            fi
        else
            bad "V could not select the capture to serve"
        fi
        stop
    else
        bad "V the Workspace did not draw to serve"
    fi
else
    printf '  --      V Serve not checked: no curl, or something is on port %s\n' "$SERVE_PORT"
fi
rm -f "$A/$AWKWARD"

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
