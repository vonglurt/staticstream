#!/bin/sh
# SPDX-License-Identifier: MIT
# Copyright (c) 2026 Paul Richeson
#
# ytq-window-crosscheck.sh -- make ytq-window-crosscheck: the Rust window
# (sstr ytq) beside the Python ytq's curses window, for step 2d of
# docs/phase-2.md.
#
# Each window runs in a pane of a private tmux server, and tmux reads both
# screens back. capture-pane -e writes cells with their attributes in tmux's
# own words, so curses' way of asking for reverse, bold and cyan and ours come
# out the same whenever the cells are the same. Compared, after normalising
# times, pids and the home:
#
#   P  one queue, two downloads under way in another process, a long list: the
#      screen at 110x30, with the selection moved, scrolled, at 70x20 and 50x12
#   K  each key from the top of that queue, the Python window then the Rust
#      one: the screen after it, and what it left in queue.json, the log and
#      the browser
#   W  the window's own worker and watcher: the clipboard at start queued, a
#      copy queued, the screen mid-part, paused and mid-merge, then q -- the
#      download stopped and back in the queue
#   F  Hyprland says another window has focus: a copy is not queued
#   G  Hyprland says this one has: a copy is queued
#
# Nothing here touches the real queue, clipboard, browser or notifications:
# every window has a throwaway HOME and stubs first on its PATH.

set -u
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
SPEC=${SPEC:-$ROOT/tests/reference/ytq.py}
SSTR=${SSTR:-$ROOT/target/release/sstr}
# The ytq under test: the binary, or `sstr ytq`. Left unquoted where it is
# called, so that the two words of the default split into two.
YTQ=${YTQ:-$SSTR ytq}

skip() { printf '  --      ytq-window-crosscheck skipped: %s\n' "$1"; exit 0; }
[ -f "$SPEC" ] || skip "no specification at $SPEC"
command -v python3 >/dev/null 2>&1 || skip "no python3"
python3 -c 'import curses' 2>/dev/null || skip "no curses in python3"
command -v tmux >/dev/null 2>&1 || skip "no tmux"
[ -x "$SSTR" ] || { printf 'ytq-window-crosscheck: no %s -- make build\n' "$SSTR"; exit 2; }

W=$(mktemp -d "${TMPDIR:-/tmp}/sstr-ytq-window.XXXXXX")
SOCK="sstr-ytq-window-$$"
T() { tmux -L "$SOCK" -f "$W/tmux.conf" "$@"; }
cleanup() { T kill-server 2>/dev/null; pkill -f "$W/" 2>/dev/null; rm -rf "$W"; }
trap cleanup EXIT INT TERM
FAILED=0
PASSED=0
ok() { PASSED=$((PASSED + 1)); [ -n "${VERBOSE:-}" ] && printf '  ok      %s\n' "$1"; return 0; }
bad() { FAILED=$((FAILED + 1)); printf '  FAIL    %s\n' "$1"; }
check() { if eval "$2"; then ok "$1"; else bad "$1"; fi; }
same() {
    if cmp -s "$2" "$3"; then ok "$1"; else bad "$1:"; diff -u "$2" "$3" | head -40 | sed 's/^/          /'; fi
}

cat > "$W/tmux.conf" <<'EOF'
set -g status off
set -g window-size manual
set -g remain-on-exit off
EOF

cp "$SPEC" "$W/ytq.py"

STUB="$W/stub"
mkdir -p "$STUB"
cp "$ROOT/tests/standin/yt-dlp" "$STUB/yt-dlp"
printf '#!/bin/sh\n[ -f "$HOME/clipboard" ] || exit 1\ncat "$HOME/clipboard"\n' > "$STUB/wl-paste"
printf '#!/bin/sh\nexit 1\n' > "$STUB/xclip"
printf '#!/bin/sh\nfor last; do :; done\nprintf "%%s\\n---\\n" "$last" >> "$HOME/notify"\n' > "$STUB/notify-send"
for b in brave flatpak xdg-open; do printf '#!/bin/sh\necho "%s $*" >> "$HOME/opened"\n' "$b" > "$STUB/$b"; done
cat > "$STUB/yt-brave" <<'EOF'
#!/bin/sh
for a; do
    if [ "$a" = --path ]; then
        if [ -f "$HOME/no-brave" ]; then echo "yt-brave: no Brave profile 'Default' under ~/.config/BraveSoftware" >&2; exit 1; fi
        echo "$HOME/brave/Default"; exit 0
    fi
done
exit 1
EOF
# Hyprland's answer: another window (pid 1 is no one's terminal), or the one
# whose child is asking -- which is the ytq.
cat > "$STUB/hyprctl" <<'EOF'
#!/bin/sh
if [ "${FOCUS:-}" = elsewhere ]; then echo '{"pid": 1, "class": "brave-browser"}'; else printf '{"pid": %s, "class": "foot"}\n' "$PPID"; fi
EOF
chmod +x "$STUB"/*

home() { d="$W/$1"; rm -rf "$d"; mkdir -p "$d/.config/ytq" "$d/out" "$d/.local/share/ytq"; printf 'DIR=%s/out\nOUTPUT=mp4\n' "$d" > "$d/.config/ytq/config"; echo "$d"; }
Q() { echo "$1/.local/share/ytq/queue.json"; }
put_queue() { cp "$1" "$(Q "$2").new" && mv "$(Q "$2").new" "$(Q "$2")"; }
wait_for() { n=0; while [ ! -e "$1" ] && [ $n -lt 400 ]; do sleep 0.05; n=$((n + 1)); done; [ -e "$1" ]; }

# window <session> <side> <home> <cols> <rows> [NAME=value...]
window() {
    s=$1 side=$2 h=$3 cols=$4 rows=$5
    shift 5
    if [ "$side" = py ]; then prog="python3 '$W/ytq.py'"; else prog="$YTQ"; fi
    T new-session -d -s "$s" -x "$cols" -y "$rows" \
        "env -u XDG_CONFIG_HOME -u XDG_DATA_HOME -u XDG_STATE_HOME -u DISPLAY -u WAYLAND_DISPLAY -u HYPRLAND_INSTANCE_SIGNATURE -u DBUS_SESSION_BUS_ADDRESS HOME='$h' PATH='$STUB:$PATH' TERM=xterm-256color PYTHONIOENCODING=utf-8 $* $prog 2>> '$h/window.err'"
}
screen() { T capture-pane -p -e -t "$1"; }
until_screen() { n=0; while ! T capture-pane -p -t "$1" 2>/dev/null | grep -q -- "$2"; do n=$((n + 1)); [ $n -gt 200 ] && return 1; sleep 0.1; done; }
until_gone() { n=0; while T has-session -t "=$1" 2>/dev/null; do n=$((n + 1)); [ $n -gt 200 ] && return 1; sleep 0.1; done; }
until_queue() { # <home> <id> <status>
    n=0
    until python3 -c 'import json,sys; q=json.load(open(sys.argv[1])); sys.exit(0 if any(sys.argv[2] in i["url"] and i["status"] == sys.argv[3] for i in q) else 1)' "$(Q "$1")" "$2" "$3" 2>/dev/null; do
        n=$((n + 1)); [ $n -gt 300 ] && return 1; sleep 0.1
    done
}
# send <session> keys...: tmux key names, or LIT:text typed as it is.
send() {
    s=$1; shift
    for k; do
        case $k in
            LIT:*) T send-keys -t "$s" -l "${k#LIT:}" ;;
            *) T send-keys -t "$s" "$k" ;;
        esac
    done
}
rep() { i=0; while [ $i -lt "$2" ]; do printf '%s ' "$1"; i=$((i + 1)); done; }
# Blanks at the end of a row that carry only a foreground colour look like no
# blanks at all. ncurses writes them or erases them by what each costs in
# bytes -- 7 of them at 50 columns written yellow, 67 at 110 erased -- and the
# Rust window never writes them, so they are dropped here. Reverse blanks,
# which show, are compared as they are.
ESC=$(printf '\033')
norm_screen() {
    sed -E -e "s|$1|HOME|g" -e 's/\(for [0-9:]+\)/(for T)/g' -e 's/started [0-9:]+ ago/started T ago/g' -e 's/pid [0-9]+/pid P/g' \
        -e "s/ +(${ESC}\\[39m)\$/\\1/"
}
norm_queue() {
    python3 -c '
import json, sys
try: q = json.load(open(sys.argv[1]))
except OSError: q = []
def whole(v):
    if isinstance(v, float) and v.is_integer(): return int(v)
    if isinstance(v, dict): return {k: whole(x) for k, x in v.items()}
    if isinstance(v, list): return [whole(x) for x in v]
    return v
q = whole(q)
for i in q: i.pop("added", None)
print(json.dumps(q, indent=1, sort_keys=True, ensure_ascii=False).replace(sys.argv[2], "HOME"))' "$(Q "$1")" "$1"
}
norm_log() {
    # A download stopped by q may or may not get to log its end before the
    # process is gone, in either ytq: those lines are left out.
    sed -E -e "s|$1|HOME|g" -e 's/^[0-9-]+ [0-9:]+ \[[0-9]+\] //' -e '/took run\.lock/d' \
        -e '/stopped while|yt-dlp exited|deleted from the queue while downloading/d' \
        -e 's/\(pid [0-9]+\)/(pid P)/' -e 's/checked in [0-9.]+ s/checked in T s/' -e 's/after [0-9:]+/after T/g' \
        -e 's/took [0-9:]+\)/took T)/g' -e 's/, [0-9:]+ in([,;])/, T in\1/' "$1/.local/share/ytq/ytq.log" 2>/dev/null
}

echo "  --      ytq-window-crosscheck: $YTQ beside the window of $SPEC, in tmux $(tmux -V | cut -d' ' -f2)"

# The live records come from a real run against the stand-in: one entry
# mid-part, the same one mid-merge.
S=$(home setup)
mkdir -p "$S/gates"
env -u XDG_CONFIG_HOME -u XDG_DATA_HOME HOME="$S" PATH="$STUB:$PATH" $YTQ add --no-run 'https://www.youtube.com/watch?v=GATEDAAAAAA' > /dev/null 2>&1
env -u XDG_CONFIG_HOME -u XDG_DATA_HOME HOME="$S" PATH="$STUB:$PATH" STANDIN_GATES="$S/gates" $YTQ run --quiet > "$S/run.out" 2>&1 &
RUNNER=$!
wait_for "$S/gates/at.1" || bad "setup: the stand-in never reached gate 1"
sleep 1.3; cp "$(Q "$S")" "$W/part.json"; touch "$S/gates/go.1"
wait_for "$S/gates/at.2" || bad "setup: the stand-in never reached gate 2"
sleep 1.3; cp "$(Q "$S")" "$W/merge.json"; touch "$S/gates/go.2"
wait "$RUNNER"
python3 - "$W/part.json" "$W/merge.json" "$W/panel.json" "$W/keys.json" <<'EOF'
import json, sys
yt = "https://www.youtube.com/watch?v="
part = dict(json.load(open(sys.argv[1]))[0], added=1.0)
merge = dict(json.load(open(sys.argv[2]))[0], url=yt + "MERGINGAAAA", title="A Second — Video, merging", added=1.5)
def entry(vid, status, title="", quality="", error="", file="", added=0.0):
    return {"url": vid if "://" in vid else yt + vid, "title": title, "status": status, "quality": quality, "attempts": 0,
            "cookie_tried": False, "error": error, "added": added, "file": file, "progress": "", "live": {}}
common = [
    entry("COOKIEAAAAA", "cookies", "Needs a sign-in", "720p mp4", "ERROR: [youtube] COOKIEAAAAA: Sign in to confirm you're not a bot. Use --cookies-from-browser or --cookies for the authentication.", added=2.0),
    entry("RCOOKIEAAAA", "retry-cookies", "Retrying with cookies", "1080p mp4", added=3.0),
    entry("https://example.com/a/page", "checking", added=4.0),
    entry("QUEUEDAAAAA", "queued", "Queued — café, one", "1080p mp4", added=5.0),
    entry("RETRYAAAAAA", "retry", "Retry me", "1080p mp4", "ERROR: [download] Got error: HTTP Error 500: Internal Server Error", added=6.0),
    entry("DONEAAAAAAA", "done", "Done and dusted", "1080p mp4", file="/srv/videos/Fake-Done_and_dusted_DONEAAAAAAA.mp4", added=7.0),
    entry("FAILEDAAAAA", "failed", "Failed twice", "480p mp4", "ERROR: [download] Got error: HTTP Error 403: Forbidden", added=8.0),
    entry("REJECTEDAAA", "rejected", "", "", "ERROR: [youtube] REJECTEDAAA: Video unavailable. This video has been removed by the uploader", added=9.0),
]
many = [entry("MANY%05dAA" % n, "done", "Old download number %d" % n, "360p mp4", file="/srv/videos/old-%d.mp4" % n, added=10.0 + n) for n in range(14)]
json.dump([part, merge] + common + many, open(sys.argv[3], "w"), indent=1)
json.dump([part] + common, open(sys.argv[4], "w"), indent=1)
EOF

# P and K share one HOME, and run.lock is held by a process standing in for a
# runner elsewhere, so neither window downloads, and the downloads stay put.
SH=$(home shared)
put_queue "$W/panel.json" "$SH"
python3 -c '
import fcntl, os, sys, time
f = open(sys.argv[1], "a+"); fcntl.flock(f, fcntl.LOCK_EX); f.seek(0); f.truncate(); f.write("%d\n" % os.getpid()); f.flush()
time.sleep(3600)' "$SH/.local/share/ytq/run.lock" &
HOLDER=$!
window py py "$SH" 110 30
window rs rs "$SH" 110 30
until_screen py ' ytq ' || bad "P: the Python window never drew"
until_screen rs ' ytq ' || bad "P: the Rust window never drew"
sleep 1.2
shot() { # <name> <label>
    screen py | norm_screen "$SH" > "$W/$1.py"
    screen rs | norm_screen "$SH" > "$W/$1.rs"
    same "$2" "$W/$1.py" "$W/$1.rs"
}
both() { send py "$@"; send rs "$@"; }
resize() { T resize-window -t py -x "$1" -y "$2"; T resize-window -t rs -x "$1" -y "$2"; sleep 1; }
shot P1 "P: the screen at 110x30, two downloads and 24 entries"
check "P: the Rust window drew the header, both panels and the list" \
    'grep -q " ytq  watching the clipboard" "$W/P1.rs" && grep -q "merging" "$W/P1.rs" && grep -q "Old download number" "$W/P1.rs"'
both j j j; sleep 0.8; shot P2 "P: the selection moved"
# shellcheck disable=SC2046
both $(rep j 40); sleep 1; shot P3 "P: scrolled to the end"
resize 70 20; shot P4 "P: at 70x20"
# shellcheck disable=SC2046
both $(rep k 30); sleep 0.8; shot P5 "P: at 70x20, back at the top"
resize 50 12; shot P6 "P: at 50x12"
resize 110 30

# K: each key, from the keys queue and the top of the list.
TOP=$(rep k 12)
N=0
key_case() { # <label> keys...
    label=$1; shift
    N=$((N + 1))
    for side in py rs; do
        put_queue "$W/keys.json" "$SH"
        rm -f "$SH/opened"
        : > "$SH/.local/share/ytq/ytq.log"
        # shellcheck disable=SC2086
        send "$side" $TOP
        sleep 0.7
        send "$side" "$@"
        sleep 1
        screen "$side" | norm_screen "$SH" > "$W/K$N.screen.$side"
        norm_queue "$SH" > "$W/K$N.queue.$side"
        norm_log "$SH" > "$W/K$N.log.$side"
        cat "$SH/opened" > "$W/K$N.opened.$side" 2>/dev/null || : > "$W/K$N.opened.$side"
        [ -n "$AFTER" ] && { send "$side" $AFTER; sleep 0.5; }
    done
    same "K $label: the screen" "$W/K$N.screen.py" "$W/K$N.screen.rs"
    same "K $label: queue.json" "$W/K$N.queue.py" "$W/K$N.queue.rs"
    same "K $label: the log" "$W/K$N.log.py" "$W/K$N.log.rs"
    same "K $label: the browser" "$W/K$N.opened.py" "$W/K$N.opened.rs"
}
AFTER=
key_case "k: the top of the list" k
key_case "j j j: the checking entry" j j j
key_case "Down Down Up" Down Down Up
key_case "d on what another runner is downloading" d
key_case "d on a queued entry" j j j j d
key_case "Delete on a failed entry" j j j j j j j DC
key_case "r on a failed entry" j j j j j j j r
key_case "r on a rejected entry with no title" j j j j j j j j r
key_case "r on a retry" j j j j j r
key_case "r on the entry being downloaded" r
key_case "c on an entry waiting for a sign-in" j c
touch "$SH/no-brave"
key_case "c with no Brave profile" j c
rm -f "$SH/no-brave"
key_case "c on anything else" j j c
key_case "o" j j j j o
key_case "h once" h
key_case "h h" h h
key_case "x x" x x
key_case "h j h" h j h
key_case "h h h" h h h
key_case "an unknown key clears the message" h z
key_case "a: a Shorts link without https://" a LIT:youtube.com/shorts/SHORTSAAAAA Enter
key_case "a: a link already queued" a LIT:https://youtu.be/QUEUEDAAAAA Enter
key_case "a: not a URL" a "LIT:not a link at all" Enter
key_case "a: nothing typed" a Enter
key_case "a: typed, and corrected with Backspace" a LIT:https://youtu.be/TYPOAAAAAAAXY BSpace BSpace Enter
key_case "p with another runner downloading" p
AFTER=w
key_case "w: the watcher off" w
AFTER=
for side in py rs; do
    : > "$SH/.local/share/ytq/ytq.log"
    send "$side" q
    until_gone "$side" || bad "K q: the $side window is still open"
    norm_log "$SH" > "$W/Kq.log.$side"
done
same "K q: the log" "$W/Kq.log.py" "$W/Kq.log.rs"
check "K q: the window closed, and said so" 'grep -qx "window closed" "$W/Kq.log.rs"'
kill "$HOLDER" 2>/dev/null

# W: the window downloads itself.
for side in py rs; do
    H=$(home "W-$side"); mkdir -p "$H/gates"
    printf 'https://www.youtube.com/watch?v=GATEDAAAAAA\n' > "$H/clipboard"
    window "w$side" "$side" "$H" 110 30 "STANDIN_GATES='$H/gates'"
    wait_for "$H/gates/at.1" || bad "W $side: never reached gate 1"
    sleep 1.5
    screen "w$side" | norm_screen "$H" > "$W/W1.$side"
    printf 'youtu.be/SECONDAAAAA' > "$H/clipboard"
    until_queue "$H" SECONDAAAAA queued || bad "W $side: the copied link was never queued"
    sleep 0.8
    screen "w$side" | norm_screen "$H" > "$W/W2.$side"
    send "w$side" p; sleep 0.8
    screen "w$side" | norm_screen "$H" > "$W/W3.$side"
    touch "$H/gates/go.1"
    wait_for "$H/gates/at.2" || bad "W $side: never reached gate 2"
    sleep 1.5
    screen "w$side" | norm_screen "$H" > "$W/W4.$side"
    send "w$side" q
    until_gone "w$side" || bad "W $side: the window did not close"
    sleep 0.5
    if pgrep -f "$H/out" > /dev/null 2>&1; then echo yes; else echo no; fi > "$W/W.left.$side"
    norm_queue "$H" > "$W/W.queue.$side"
    norm_log "$H" > "$W/W.log.$side"
    (cd "$H/out" && for f in *; do [ -e "$f" ] && echo "$f"; done) > "$W/W.files.$side"
done
same "W: mid-part, the clipboard at start downloading" "$W/W1.py" "$W/W1.rs"
same "W: a copied link queued" "$W/W2.py" "$W/W2.rs"
same "W: paused" "$W/W3.py" "$W/W3.rs"
same "W: mid-merge" "$W/W4.py" "$W/W4.rs"
same "W: after q, queue.json" "$W/W.queue.py" "$W/W.queue.rs"
same "W: after q, the log" "$W/W.log.py" "$W/W.log.rs"
same "W: after q, the files" "$W/W.files.py" "$W/W.files.rs"
same "W: after q, no yt-dlp left" "$W/W.left.py" "$W/W.left.rs"
check "W: the Rust window stopped the merge because it was closed" 'grep -q "while merging, T in, because the window was closed" "$W/W.log.rs"'

# F and G: focus, as Hyprland reports it.
for side in py rs; do
    H=$(home "F-$side")
    printf 'https://www.youtube.com/watch?v=UNAVAILAAAA' > "$H/clipboard"
    window "f$side" "$side" "$H" 110 30 HYPRLAND_INSTANCE_SIGNATURE=test FOCUS=elsewhere
    until_queue "$H" UNAVAILAAAA rejected || bad "F $side: the clipboard at start was not checked"
    sleep 1.5
    printf 'https://youtu.be/NOTQUEUEDAA' > "$H/clipboard"
    sleep 2.5
    screen "f$side" | norm_screen "$H" > "$W/F.screen.$side"
    norm_queue "$H" > "$W/F.queue.$side"
    send "f$side" q; until_gone "f$side"
    norm_log "$H" > "$W/F.log.$side"

    H=$(home "G-$side")
    window "g$side" "$side" "$H" 110 30 HYPRLAND_INSTANCE_SIGNATURE=test FOCUS=here
    until_screen "g$side" ' ytq ' || bad "G $side: the window never drew"
    sleep 1.5
    printf 'https://youtu.be/UNAVAILBBBB' > "$H/clipboard"
    until_queue "$H" UNAVAILBBBB rejected || bad "G $side: the copy was not queued and checked"
    sleep 0.8
    screen "g$side" | norm_screen "$H" > "$W/G.screen.$side"
    norm_queue "$H" > "$W/G.queue.$side"
    send "g$side" q; until_gone "g$side"
    norm_log "$H" > "$W/G.log.$side"
done
same "F: not focused, the screen" "$W/F.screen.py" "$W/F.screen.rs"
same "F: not focused, the copy not queued" "$W/F.queue.py" "$W/F.queue.rs"
same "F: not focused, the log" "$W/F.log.py" "$W/F.log.rs"
check "F: the Rust window said the clipboard is paused" 'grep -q "clipboard: paused (window not focused)" "$W/F.screen.rs"'
same "G: focused, the screen" "$W/G.screen.py" "$W/G.screen.rs"
same "G: focused, the copy queued" "$W/G.queue.py" "$W/G.queue.rs"
same "G: focused, the log" "$W/G.log.py" "$W/G.log.rs"
for f in "$W"/*/window.err; do
    [ -s "$f" ] && { bad "stderr from a window: $f"; sed 's/^/          /' "$f" | head -20; }
done

if [ "$FAILED" -eq 0 ]; then
    printf '  ok      ytq-window-crosscheck: %d comparisons agree\n' "$PASSED"
else
    printf '\033[31merror:\033[0m ytq-window-crosscheck: %d of %d comparisons disagree\n' "$FAILED" $((FAILED + PASSED))
    exit 1
fi
