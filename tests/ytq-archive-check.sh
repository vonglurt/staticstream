#!/bin/sh
# SPDX-License-Identifier: MIT
# Copyright (c) 2026 Paul Richeson
#
# ytq-archive-check.sh -- make ytq-archive-check: step 2c of docs/phase-2.md,
# the Rust ytq archiving what it downloads into Static Stream.
#
#   both      the MP4, the capture and the transcript; the capture verifies and
#             plays back as the MP4, byte for byte
#   sstr      the default: the capture and the transcript, the MP4 gone; it plays
#             back as the MP4 of 'both'; the Notes name the capture; the header
#             carries the source, the note and a stated license
#   keys      SSTR_KEY signs, ~/.ssh/id_ed25519 signs by default, SSTR_KEY= does not
#   elsewhere ARCHIVE_DIR: capture and transcript there, nothing left in DIR
#   refused   an ARCHIVE_DIR that cannot be made: the MP4 stays, and says why
#   mp4       the MP4 and the transcript, no capture -- and ytq-runner-crosscheck
#             holds OUTPUT=mp4 to the Python ytq, comparison by comparison
#   real      jNQXAC9IVRw with both and with sstr, when the network is there
#
# The downloads are the stand-in yt-dlp's (tests/standin), except the real one.

set -u
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
SSTR=${SSTR:-$ROOT/target/release/sstr}
# The ytq under test: the binary, or `sstr ytq`. Left unquoted where it is
# called, so that the two words of the default split into two. $SSTR itself
# stays, for record, play and verify.
YTQ=${YTQ:-$SSTR ytq}
[ -x "$SSTR" ] || { printf 'ytq-archive-check: no %s -- make build\n' "$SSTR"; exit 2; }
command -v python3 >/dev/null 2>&1 || { printf '  --      ytq-archive-check skipped: no python3 for the stand-in yt-dlp\n'; exit 0; }

W=$(mktemp -d "${TMPDIR:-/tmp}/sstr-ytq-archive.XXXXXX")
trap 'rm -rf "$W"' EXIT INT TERM
FAILED=0
PASSED=0
ok() { PASSED=$((PASSED + 1)); [ -n "${VERBOSE:-}" ] && printf '  ok      %s\n' "$1"; return 0; }
bad() { FAILED=$((FAILED + 1)); printf '  FAIL    %s\n' "$1"; }
check() { if eval "$2"; then ok "$1"; else bad "$1"; fi; }

STUB="$W/stub"
mkdir -p "$STUB"
cp "$ROOT/tests/standin/yt-dlp" "$STUB/yt-dlp"
printf '#!/bin/sh\nexit 1\n' > "$STUB/wl-paste"
printf '#!/bin/sh\nexit 1\n' > "$STUB/xclip"
printf '#!/bin/sh\nfor last; do :; done\nprintf "%%s\\n" "$last" >> "$HOME/notify"\n' > "$STUB/notify-send"
chmod +x "$STUB"/*

# ytq <home> <path-dir> args...
ytq() { h=$1 p=$2; shift 2; env -u XDG_CONFIG_HOME -u XDG_DATA_HOME HOME="$h" PATH="$p:$PATH" $YTQ "$@"; }
# home <name> <config lines...>
home() {
    d="$W/$1"; shift; rm -rf "$d"; mkdir -p "$d/.config/ytq" "$d/out"
    printf 'DIR=%s/out\n' "$d" > "$d/.config/ytq/config"
    for line; do printf '%s\n' "$line" >> "$d/.config/ytq/config"; done
    echo "$d"
}
fetch() { # <home> <id> [path]: add and run one video
    ytq "$1" "${3:-$STUB}" add --no-run "https://www.youtube.com/watch?v=$2" > /dev/null 2>&1
    ytq "$1" "${3:-$STUB}" run --quiet > "$1/run.out" 2>&1
}
entry() { python3 -c 'import json,sys; i=json.load(open(sys.argv[1]))[0]; print(i["status"], i["file"].replace(sys.argv[2], "HOME"))' "$1/.local/share/ytq/queue.json" "$1"; }
files() { (cd "$1" && ls -A | tr '\n' ' '); }

echo "  --      ytq-archive-check: $YTQ, archiving to Static Stream"

# both
H=$(home both OUTPUT=both); N=Fake-Fake_Title_ARCHIVEAAAA
fetch "$H" ARCHIVEAAAA
check "both: the MP4, the capture and the transcript ($(files "$H/out"))" '[ "$(files "$H/out")" = "$N.mp4 $N.sstr $N.txt " ]'
check "both: the capture verifies" '"$SSTR" verify "$H/out/$N.sstr" > "$W/both.verify"'
"$SSTR" play "$H/out/$N.sstr" -o "$W/both.play" 2>/dev/null
check "both: the capture plays back as the MP4, byte for byte" 'cmp -s "$W/both.play" "$H/out/$N.mp4"'
check "both: the entry names the MP4" '[ "$(entry "$H")" = "done HOME/out/$N.mp4" ]'
check "both: done, with the transcript" 'grep -qx "done: $N.mp4 + transcript" "$H/notify"'
cp "$H/out/$N.mp4" "$W/reference.mp4"

# sstr, the default
H=$(home sstr)
fetch "$H" ARCHIVEAAAA
check "sstr: the capture and the transcript, the MP4 gone ($(files "$H/out"))" '[ "$(files "$H/out")" = "$N.sstr $N.txt " ]'
check "sstr: the capture verifies" '"$SSTR" verify "$H/out/$N.sstr" > "$W/sstr.verify"'
"$SSTR" play "$H/out/$N.sstr" -o "$W/sstr.play" 2>/dev/null
check "sstr: it plays back as the MP4 that 'both' kept" 'cmp -s "$W/sstr.play" "$W/reference.mp4"'
check "sstr: the entry names the capture" '[ "$(entry "$H")" = "done HOME/out/$N.sstr" ]'
check "sstr: the transcript's Notes name the capture" 'grep -qx "  Video:      $N.sstr" "$H/out/$N.txt"'
check "sstr: done names the capture" 'grep -qx "done: $N.sstr + transcript" "$H/notify"'
check "sstr: the header's source" 'grep -qx "source       https://www.youtube.com/watch?v=ARCHIVEAAAA" "$W/sstr.verify"'
check "sstr: the header's note" 'grep -qx "note         Fake Title — café by Fake Uploader" "$W/sstr.verify"'
check "sstr: the header's content type" 'grep -q "  video/mp4  created " "$W/sstr.verify"'
check "sstr: the log says archived, verified and removed" \
    'grep -q "archived .*read back and verified" "$H/.local/share/ytq/ytq.log" && grep -q "removed .*: the capture holds it" "$H/.local/share/ytq/ytq.log"'
check "sstr: no .part left" '! ls "$H/out"/*.part > /dev/null 2>&1'
H=$(home license)
fetch "$H" ARCHIVECCAA
"$SSTR" verify "$H/out/Fake-Fake_Title_ARCHIVECCAA.sstr" > "$W/cc.verify" 2>&1
check "sstr: a stated license goes into the header" 'grep -qx "license      Creative Commons Attribution license (reuse allowed)" "$W/cc.verify"'
check "sstr: none stated, no license line" '! grep -q "^license" "$W/sstr.verify"'

# keys
H=$(home key "SSTR_KEY=$W/signing/id")
mkdir -p "$W/signing"; ssh-keygen -q -t ed25519 -N '' -C archive@sstr -f "$W/signing/id"
printf 'archive@sstr namespaces="sstr@copal" %s\n' "$(cat "$W/signing/id.pub")" > "$W/allowed"
fetch "$H" ARCHIVEAAAA
"$SSTR" verify "$H/out/$N.sstr" --allowed-signers "$W/allowed" > "$W/signing/id.verify" 2>&1; rc=$?
check "keys: SSTR_KEY signs; trusted, exit $rc" '[ $rc = 0 ] && grep -q "trusted as archive@sstr" "$W/signing/id.verify" && grep -q "^signatures   [1-9][0-9]* good, 0 bad" "$W/signing/id.verify"'
H=$(home defaultkey)
mkdir -p "$H/.ssh"; cp "$W/signing/id" "$H/.ssh/id_ed25519"; cp "$W/signing/id.pub" "$H/.ssh/id_ed25519.pub"
fetch "$H" ARCHIVEAAAA
"$SSTR" verify "$H/out/$N.sstr" --allowed-signers "$W/allowed" > "$W/dkey.verify" 2>&1
check "keys: ~/.ssh/id_ed25519 signs when SSTR_KEY is not set" 'grep -q "trusted as archive@sstr" "$W/dkey.verify"'
H=$(home nokey "SSTR_KEY=")
mkdir -p "$H/.ssh"; cp "$W/signing/id" "$H/.ssh/id_ed25519"; cp "$W/signing/id.pub" "$H/.ssh/id_ed25519.pub"
fetch "$H" ARCHIVEAAAA
"$SSTR" verify "$H/out/$N.sstr" > "$W/nokey.verify" 2>&1
check "keys: SSTR_KEY= leaves it unsigned" 'grep -q "^key          none" "$W/nokey.verify"'

# elsewhere
H=$(home elsewhere "ARCHIVE_DIR=~/archive")
fetch "$H" ARCHIVEAAAA
check "elsewhere: capture and transcript in ARCHIVE_DIR ($(files "$H/archive"))" '[ "$(files "$H/archive")" = "$N.sstr $N.txt " ]'
check "elsewhere: nothing left in DIR" '[ -z "$(files "$H/out")" ]'

# refused
H=$(home refused "ARCHIVE_DIR=~/blocked/archive")
: > "$H/blocked"
fetch "$H" ARCHIVEAAAA
check "refused: the MP4 and the transcript stay in DIR ($(files "$H/out"))" '[ "$(files "$H/out")" = "$N.mp4 $N.txt " ]'
check "refused: the entry is done, naming the MP4" '[ "$(entry "$H")" = "done HOME/out/$N.mp4" ]'
check "refused: the notification says why" 'grep -q "^done: $N.mp4 + transcript (not archived: cannot make " "$H/notify"'
check "refused: the log says the MP4 stays" 'grep -q "not archived, so the MP4 stays" "$H/.local/share/ytq/ytq.log"'

# mp4
H=$(home mp4 OUTPUT=mp4)
fetch "$H" ARCHIVEAAAA
check "mp4: the MP4 and the transcript, no capture ($(files "$H/out"))" '[ "$(files "$H/out")" = "$N.mp4 $N.txt " ]'
check "mp4: yt-dlp is asked nothing more than the Python ytq asks" '! grep -q "after_move:NOTES" "$H/.local/share/ytq/ytq.log"'

# real
if [ "${YTQ_REAL:-1}" != 0 ] && timeout 60 yt-dlp --ignore-config --simulate --no-warnings --print id 'https://www.youtube.com/watch?v=jNQXAC9IVRw' > /dev/null 2>&1; then
    REAL="$W/real"; mkdir -p "$REAL"; cp "$STUB/notify-send" "$STUB/wl-paste" "$STUB/xclip" "$REAL/"
    H=$(home realboth OUTPUT=both)
    fetch "$H" jNQXAC9IVRw "$REAL"
    mp4=$(ls "$H"/out/*.mp4 2>/dev/null | head -1)
    sstr="${mp4%.mp4}.sstr"
    check "real, both: $(files "$H/out")" '[ -f "$mp4" ] && [ -f "$sstr" ] && [ -f "${mp4%.mp4}.txt" ]'
    "$SSTR" play "$sstr" -o "$W/real.play" 2>/dev/null
    check "real, both: the capture verifies and plays back as the MP4" '"$SSTR" verify "$sstr" > "$W/real.verify" && cmp -s "$W/real.play" "$mp4"'
    check "real, both: the header's source and note" \
        'grep -qx "source       https://www.youtube.com/watch?v=jNQXAC9IVRw" "$W/real.verify" && grep -qx "note         Me at the zoo by jawed" "$W/real.verify"'
    H=$(home realsstr)
    fetch "$H" jNQXAC9IVRw "$REAL"
    check "real, sstr: $(files "$H/out")" '[ -z "$(ls "$H"/out/*.mp4 2>/dev/null)" ] && ls "$H"/out/*.sstr > /dev/null 2>&1 && ls "$H"/out/*.txt > /dev/null 2>&1'
    check "real, sstr: the capture verifies" '"$SSTR" verify "$(ls "$H"/out/*.sstr | head -1)" > /dev/null'
else
    printf '  --      real skipped: no network, or YTQ_REAL=0\n'
fi

if [ "$FAILED" -eq 0 ]; then
    printf '  ok      ytq-archive-check: %d checks pass\n' "$PASSED"
else
    printf '\033[31merror:\033[0m ytq-archive-check: %d of %d checks fail\n' "$FAILED" $((FAILED + PASSED))
    for f in "$W"/*/run.out; do :; done
    exit 1
fi
