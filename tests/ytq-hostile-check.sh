#!/bin/sh
# SPDX-License-Identifier: MIT
# Copyright (c) 2026 Paul Richeson
#
# ytq-hostile-check.sh -- make ytq-hostile-check: the ytq that ships, against
# a yt-dlp whose every word was chosen by somebody who means harm
# (tests/standin/yt-dlp-hostile). The standard it holds ytq to is section VI
# of copal's docs/text-safety-lab-report.md, rules T1, T2, T3 and T7.
#
#   command   nothing in a description is run: no file appears that ytq did
#             not mean to write
#   text      no control character but newline and tab in the .txt, the log,
#             the queue or a notification
#   terminal  none in what 'ytq list', 'ytq status' and 'sstr verify' print
#   rows      a newline in a title or an uploader forges no row, in the notes
#             or in 'sstr verify'
#   tags      the MP4 has one chapter, its lyrics and its description whole,
#             and one-line tags of one line -- when ffmpeg is there
#   path      a FILE line that names somebody else's file is refused: the
#             file is not tagged, not captured and not removed
#   operand   every yt-dlp command has -- before its URL
#
# A control character here is what Unicode calls one: U+0000 to U+001F,
# U+007F, and U+0080 to U+009F.
#
# Every condition is a string in single quotes that check() hands to eval, as
# in the other checks here: written in this file, never read from anywhere.
# shellcheck disable=SC2016

set -u
# shellcheck disable=SC1007  # CDPATH is meant to be empty for this one cd
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
SSTR=${SSTR:-$ROOT/target/release/sstr}
# Left unquoted where it is called, so that 'sstr ytq' is two words.
YTQ=${YTQ:-$SSTR ytq}
[ -x "$SSTR" ] || { printf 'ytq-hostile-check: no %s -- make build\n' "$SSTR"; exit 2; }
command -v python3 >/dev/null 2>&1 || { printf '  --      ytq-hostile-check skipped: no python3 for the stand-in yt-dlp\n'; exit 0; }

W=$(mktemp -d "${TMPDIR:-/tmp}/sstr-ytq-hostile.XXXXXX")
trap 'rm -rf "$W"' EXIT INT TERM
FAILED=0
PASSED=0
ok() { PASSED=$((PASSED + 1)); [ -n "${VERBOSE:-}" ] && printf '  ok      %s\n' "$1"; return 0; }
bad() { FAILED=$((FAILED + 1)); printf '  FAIL    %s\n' "$1"; }
check() { if eval "$2"; then ok "$1"; else bad "$1"; fi; }

STUB="$W/stub"
mkdir -p "$STUB"
cp "$ROOT/tests/standin/yt-dlp-hostile" "$STUB/yt-dlp"
printf '#!/bin/sh\nexit 1\n' > "$STUB/wl-paste"
printf '#!/bin/sh\nexit 1\n' > "$STUB/xclip"
printf '#!/bin/sh\nfor last; do :; done\nprintf "%%s\\n" "$last" >> "$HOME/notify"\n' > "$STUB/notify-send"
# A download that wants a sign-in opens a browser: these, and never a real one.
for b in brave flatpak xdg-open; do printf '#!/bin/sh\necho "%s $*" >> "$HOME/opened"\n' "$b" > "$STUB/$b"; done
chmod +x "$STUB"/*

# controls FILE: how many control characters it holds, newline and tab aside.
# strings FILE:  the same, of every string in a JSON file -- which writes an
#                escape as six printable characters, so its bytes say nothing.
# tags FILE:     what ffprobe finds in an MP4, a fact to a line.
cat > "$W/look.py" <<'PY'
import json, subprocess, sys

def controls(s):
    return sum(1 for c in s if (ord(c) < 0x20 and c not in "\n\t") or 0x7f <= ord(c) <= 0x9f)

def walk(v):
    if isinstance(v, str):
        return controls(v)
    if isinstance(v, dict):
        return sum(walk(k) + walk(x) for k, x in v.items())
    if isinstance(v, list):
        return sum(walk(x) for x in v)
    return 0

what, path = sys.argv[1], sys.argv[2]
if what == "controls":
    print(controls(open(path, "rb").read().decode("utf-8", "replace")))
elif what == "strings":
    print(walk(json.load(open(path, encoding="utf-8"))))
elif what == "tags":
    p = subprocess.run(["ffprobe", "-v", "error", "-show_entries", "format_tags:chapter_tags=title",
                        "-of", "json", path], capture_output=True, text=True)
    d = json.loads(p.stdout or "{}")
    t = {k.lower(): v for k, v in d.get("format", {}).get("tags", {}).items()}
    ch = d.get("chapters", [])
    ends = lambda k, w: "absent" if k not in t else ("whole" if t[k].rstrip().endswith(w) else "cut")
    print("chapters", len(ch))
    print("lyrics", ends("lyrics", "END-OF-CAPTIONS"))
    print("description", ends("description", "END-OF-DESCRIPTION"))
    print("title-lines", len(t.get("title", "").split("\n")))
    print("artist-lines", len(t.get("artist", "").split("\n")))
    print("controls", walk(t) + walk([c.get("tags", {}) for c in ch]))
PY
controls() { python3 "$W/look.py" controls "$1"; }
strings() { python3 "$W/look.py" strings "$1"; }

# ytq <home> args...: run from inside the home, so that a command a
# description managed to run would leave its file where the check looks.
ytq() {
    h=$1; shift
    # shellcheck disable=SC2086  # $YTQ is 'sstr ytq' by default: two words
    (cd "$h" && env -u XDG_CONFIG_HOME -u XDG_DATA_HOME -u DISPLAY -u WAYLAND_DISPLAY -u HYPRLAND_INSTANCE_SIGNATURE \
        HOME="$h" PATH="$STUB:$PATH" HOSTILE_MP4="$W/real.mp4" \
        HOSTILE_AWAY="$W/away/precious.mp4" HOSTILE_NEAR="$h/out/Other-Video_OTHERVIDEO1.mp4" $YTQ "$@")
}
# home <name> <config lines...>
home() {
    d="$W/$1"; shift; rm -rf "$d"; mkdir -p "$d/.config/ytq" "$d/out"
    printf 'DIR=%s/out\n' "$d" > "$d/.config/ytq/config"
    for line; do printf '%s\n' "$line" >> "$d/.config/ytq/config"; done
    echo "$d"
}
fetch() { # <home> <id>: add and run one video
    ytq "$1" add --no-run "https://www.youtube.com/watch?v=$2" > /dev/null 2>&1
    ytq "$1" run --quiet > "$1/run.out" 2>&1
}
# shellcheck disable=SC2012  # the names are ytq's: A-Z a-z 0-9 - _ and a dot
files() { (cd "$1" && ls -A | tr '\n' ' '); }
count() { grep -c -e "$1" "$2"; }

echo "  --      ytq-hostile-check: $YTQ, against a yt-dlp that means harm"

# A real MP4 for ffmpeg to tag: a second of test card, and of silence.
TAGS=0
if command -v ffmpeg >/dev/null 2>&1 && command -v ffprobe >/dev/null 2>&1; then
    ffmpeg -nostdin -v error -y -f lavfi -i testsrc=d=1:s=64x64:r=5 -f lavfi -i anullsrc=d=1 -shortest \
        -c:v mpeg4 -c:a aac "$W/real.mp4" 2>/dev/null && TAGS=1
fi

N=Up-Honest_title_HOSTILEAAAA
H=$(home mp4 OUTPUT=mp4)
fetch "$H" HOSTILEAAAA
fetch "$H" HOSTILEFAIL
fetch "$H" HOSTILESIGN
# And a URL that is one to URL_RE, with an escape sequence for a path.
ytq "$H" add --no-run "$(printf 'https://example.com/\033]0;PWNED-BY-A-URL\007')" > /dev/null 2>&1
TXT="$H/out/$N.txt"
LOG="$H/.local/share/ytq/ytq.log"
ytq "$H" list > "$W/list.out" 2>&1
ytq "$H" status > "$W/status.out" 2>&1

# command
check "command: one download finished, one failed, and one waits on a sign-in" \
    'grep -q "^done: $N.mp4" "$H/notify" && grep -q "^failed " "$W/list.out" && grep -q "^  sign-in " "$W/status.out"'
check "command: the browser opened was the stand-in, on the video and nothing else" \
    '[ "$(cat "$H/opened")" = "brave https://www.youtube.com/watch?v=HOSTILESIGN" ]'
check "command: a URL with an escape sequence in it is refused, and said to be" \
    '! grep -q "example.com" "$H/.local/share/ytq/queue.json" && grep -q "^not a URL: https://example.com/" "$H/notify"'
check "command: nothing a description asked for was run" '[ -z "$(find "$W" -name "PWNED*")" ]'
check "command: the folder holds the video and its text, and nothing else ($(files "$H/out"))" \
    '[ "$(files "$H/out")" = "$N.mp4 $N.txt " ]'

# text
check "text: the .txt holds no control character ($(controls "$TXT"))" '[ "$(controls "$TXT")" = 0 ]'
check "text: nor the log ($(controls "$LOG"))" '[ "$(controls "$LOG")" = 0 ]'
check "text: nor a notification ($(controls "$H/notify"))" '[ "$(controls "$H/notify")" = 0 ]'
check "text: nor any string in the queue ($(strings "$H/.local/share/ytq/queue.json"))" \
    '[ "$(strings "$H/.local/share/ytq/queue.json")" = 0 ]'
check "text: the description is whole" 'grep -q "END-OF-DESCRIPTION" "$TXT"'
check "text: the transcript is whole" 'grep -q "END-OF-CAPTIONS" "$TXT"'

# terminal
check "terminal: 'ytq list' prints no control character ($(controls "$W/list.out"))" '[ "$(controls "$W/list.out")" = 0 ]'
check "terminal: nor 'ytq status' ($(controls "$W/status.out"))" '[ "$(controls "$W/status.out")" = 0 ]'

# rows
check "rows: the title is the first line, and the second is blank" \
    'sed -n 1p "$TXT" | grep -q "^Honest" && [ -z "$(sed -n 2p "$TXT")" ] && [ "$(sed -n 3p "$TXT")" = Notes ]'
check "rows: no line of the notes starts with a forged License ($(count "^[Ll]icense" "$TXT"))" '[ "$(count "^[Ll]icense" "$TXT")" = 0 ]'
check "rows: one License row, and it is the site's" \
    '[ "$(count "^  License: " "$TXT")" = 1 ] && grep -qx "  License:    not stated" "$TXT"'
check "rows: no line starts with a forged URL ($(count "^URL:" "$TXT"))" '[ "$(count "^URL:" "$TXT")" = 0 ]'
check "rows: one URL row, and it is the video's" \
    '[ "$(count "^  URL: " "$TXT")" = 1 ] && grep -qx "  URL:        https://www.youtube.com/watch?v=HOSTILEAAAA" "$TXT"'
check "rows: one chapter in the notes" \
    '[ "$(sed -n "/^Chapters\$/,/^\$/p" "$TXT" | grep -c "^  ")" = 1 ]'

# tags
if [ "$TAGS" = 1 ]; then
    python3 "$W/look.py" tags "$H/out/$N.mp4" > "$W/tags" 2>&1
    tag() { sed -n "s/^$1 //p" "$W/tags"; }
    check "tags: one chapter in the MP4 ($(tag chapters))" '[ "$(tag chapters)" = 1 ]'
    check "tags: the lyrics are there, and whole ($(tag lyrics))" '[ "$(tag lyrics)" = whole ]'
    check "tags: the description is whole ($(tag description))" '[ "$(tag description)" = whole ]'
    check "tags: the title is one line ($(tag title-lines))" '[ "$(tag title-lines)" = 1 ]'
    check "tags: the artist is one line ($(tag artist-lines))" '[ "$(tag artist-lines)" = 1 ]'
    check "tags: no control character in any tag ($(tag controls))" '[ "$(tag controls)" = 0 ]'
else
    printf '  --      tags skipped: no ffmpeg and ffprobe to make and read an MP4\n'
fi

# the capture's header, and what 'sstr verify' prints from it
H=$(home both OUTPUT=both SSTR_KEY=)
fetch "$H" HOSTILEAAAA
"$SSTR" verify "$H/out/$N.sstr" > "$W/verify.out" 2>&1; rc=$?
check "verify: the capture verifies, exit $rc" '[ $rc = 0 ]'
check "terminal: 'sstr verify' prints no control character ($(controls "$W/verify.out"))" '[ "$(controls "$W/verify.out")" = 0 ]'
check "rows: 'sstr verify' has one note row" '[ "$(count "^note " "$W/verify.out")" = 1 ]'
check "rows: and no license row, the site having stated none ($(count "^[Ll]icense" "$W/verify.out"))" \
    '[ "$(count "^[Ll]icense" "$W/verify.out")" = 0 ]'
check "rows: and no forged URL ($(count "^URL:" "$W/verify.out"))" '[ "$(count "^URL:" "$W/verify.out")" = 0 ]'
check "command: nothing was run by the second download either" '[ -z "$(find "$W" -name "PWNED*")" ]'

# path: OUTPUT=sstr, which removes the file it was told it downloaded.
H=$(home path SSTR_KEY=)
mkdir -p "$W/away"
for f in "$W/away/precious.mp4" "$H/out/Other-Video_OTHERVIDEO1.mp4"; do
    if [ "$TAGS" = 1 ]; then cp "$W/real.mp4" "$f"; else printf 'not a video, but somebody wants it\n' > "$f"; fi
    cp "$f" "$f.kept"
done
mv "$H/out/Other-Video_OTHERVIDEO1.mp4.kept" "$W/away/other.kept"
fetch "$H" HOSTILEAWAY
fetch "$H" HOSTILENEAR
LOG="$H/.local/share/ytq/ytq.log"
check "path: a file outside the folder is as it was" 'cmp -s "$W/away/precious.mp4" "$W/away/precious.mp4.kept"'
check "path: another video in the folder is as it was" 'cmp -s "$H/out/Other-Video_OTHERVIDEO1.mp4" "$W/away/other.kept"'
check "path: neither was captured ($(files "$H/out"))" \
    '[ "$(files "$H/out")" = "Other-Video_OTHERVIDEO1.mp4 Up-Honest_title_HOSTILEAWAY.sstr Up-Honest_title_HOSTILEAWAY.txt Up-Honest_title_HOSTILENEAR.sstr Up-Honest_title_HOSTILENEAR.txt " ] && [ "$(files "$W/away")" = "other.kept precious.mp4 precious.mp4.kept " ]'
check "path: the log says each was refused, and why ($(count "is not this download.s, and it is left alone" "$LOG"))" \
    'grep -q "left alone: $W/away/precious.mp4 -- it is not in " "$LOG" && grep -q "left alone: $H/out/Other-Video_OTHERVIDEO1.mp4 -- its name does not end in HOSTILENEAR" "$LOG"'

# operand: a command is logged as 'run: ...', and ends in its URL -- on a
# later line of the log when an argument of its own holds a newline.
cat "$W"/*/.local/share/ytq/ytq.log > "$W/all.log"
runs=$(grep -c "run: " "$W/all.log")
ends=$(grep -c " -- 'https://www.youtube.com/watch?v=HOSTILE[A-Z]*'\$" "$W/all.log")
check "operand: every yt-dlp command in the logs has -- before its URL ($ends of $runs)" '[ "$runs" -gt 0 ] && [ "$ends" = "$runs" ]'

if [ "$FAILED" -eq 0 ]; then
    printf '  ok      ytq-hostile-check: %d checks pass\n' "$PASSED"
else
    printf '\033[31merror:\033[0m ytq-hostile-check: %d of %d checks fail\n' "$FAILED" $((FAILED + PASSED))
    exit 1
fi
