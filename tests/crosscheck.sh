#!/bin/sh
# SPDX-License-Identifier: MIT
# Copyright (c) 2026 Paul Richeson
#
# crosscheck.sh -- make crosscheck: the Rust sstr against the Python prototype
# it replaces, file for file. Phase 1 is done when this passes.
#
#   1. Each writes, both read: random bytes signed, text with --deflate, and an
#      MPEG-TS stream when ffmpeg is here. Both readers must play the payload
#      back identical, print the same verify report line for line, and exit
#      the same.
#   2. The damage battery of the Static Stream lab report, on a capture from
#      each writer: both readers must report the same repairs and losses, exit
#      the same, and play the same bytes.
#   3. The armor: both write the same lines; both receive the same payload,
#      with the same report, through the same noisy line.
#
# The prototype is found beside this checkout in ~/code, or at
# $STATICSTREAM_PROTOTYPE. Without it, or without python3, this says so and
# passes, so a lone checkout still builds.

set -u
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
PROTO=${STATICSTREAM_PROTOTYPE:-$ROOT/../copal/tools/copal-sstr.py}
SSTR=${SSTR:-$ROOT/target/release/sstr}
PY="python3 $PROTO"

[ -f "$PROTO" ] || { printf '  --      crosscheck skipped: no prototype at %s\n' "$PROTO"; exit 0; }
command -v python3 >/dev/null 2>&1 || { printf '  --      crosscheck skipped: no python3\n'; exit 0; }
[ -x "$SSTR" ] || { printf 'crosscheck: no %s -- make build\n' "$SSTR"; exit 2; }

W=$(mktemp -d "${TMPDIR:-/tmp}/sstr-crosscheck.XXXXXX")
trap 'rm -rf "$W"' EXIT INT TERM
FAILED=0
PASSED=0
ok() { PASSED=$((PASSED + 1)); [ -n "${VERBOSE:-}" ] && printf '  ok      %s\n' "$1"; return 0; }
bad() { FAILED=$((FAILED + 1)); printf '  FAIL    %s\n' "$1"; }

ssh-keygen -q -t ed25519 -N '' -C crosscheck@sstr -f "$W/key"
printf 'crosscheck@sstr namespaces="sstr@copal" %s\n' "$(cat "$W/key.pub")" > "$W/allowed"

head -c 1500000 /dev/urandom > "$W/rand.bin"
seq 1 150000 | sed 's/$/ the quick brown fox jumps over the lazy dog/' > "$W/text.bin"
INPUTS="rand text"
if command -v ffmpeg >/dev/null 2>&1; then
    ffmpeg -hide_banner -loglevel error -f lavfi -i testsrc=size=320x240:rate=25 -f lavfi -i sine=frequency=440 \
        -t 4 -c:v mpeg2video -b:v 800k -c:a mp2 -f mpegts "$W/ts.bin" && INPUTS="$INPUTS ts"
fi

writer_args() { # <input kind>
    case $1 in
        rand) echo "--key $W/key --checkpoint-records 16" ;;
        text) echo "--deflate --type text/plain" ;;
        ts) echo "--type video/mp2t --key $W/key" ;;
    esac
}

# compare <label> <capture> [verify args]: both readers play and verify it.
# sh has no local variables, and a function's assignments are the caller's:
# these names start with c_ so they cannot overwrite a loop's $cap.
compare() {
    c_label=$1 c_file=$2
    shift 2
    $PY play "$c_file" -o "$W/py.out" "$@" 2>/dev/null; c_py_play=$?
    "$SSTR" play "$c_file" -o "$W/rs.out" "$@" 2>/dev/null; c_rs_play=$?
    $PY verify "$c_file" --allowed-signers "$W/allowed" "$@" > "$W/py.verify" 2>/dev/null; c_py_ver=$?
    "$SSTR" verify "$c_file" --allowed-signers "$W/allowed" "$@" > "$W/rs.verify" 2>/dev/null; c_rs_ver=$?
    if ! cmp -s "$W/py.out" "$W/rs.out"; then
        bad "$c_label: the two readers played different bytes ($(wc -c < "$W/py.out") vs $(wc -c < "$W/rs.out"))"
    elif [ "$c_py_play" != "$c_rs_play" ] || [ "$c_py_ver" != "$c_rs_ver" ]; then
        bad "$c_label: exit codes differ: play $c_py_play/$c_rs_play, verify $c_py_ver/$c_rs_ver"
    elif ! diff -u "$W/py.verify" "$W/rs.verify" > "$W/diff"; then
        bad "$c_label: verify reports differ:"
        sed 's/^/          /' "$W/diff"
    else
        ok "$c_label (play exit $c_rs_play, verify exit $c_rs_ver)"
    fi
}

echo "  --      crosscheck: $SSTR against $PROTO"

# 1. Each writes, both read, and the payload is the original.
for kind in $INPUTS; do
    for w in py rs; do
        if [ $w = py ]; then
            $PY record "$W/$kind.$w.sstr" --input "$W/$kind.bin" $(writer_args $kind)
        else
            "$SSTR" record "$W/$kind.$w.sstr" --input "$W/$kind.bin" $(writer_args $kind)
        fi
        compare "$kind, written by $w" "$W/$kind.$w.sstr"
        cmp -s "$W/rs.out" "$W/$kind.bin" && ok "$kind, written by $w: payload is the original" \
            || bad "$kind, written by $w: payload is not the original"
    done
done

# 2. The damage battery, on a signed random capture from each writer.
for w in py rs; do
    cap="$W/rand.$w.sstr"
    size=$(wc -c < "$cap")
    while IFS='|' read -r label mode margs; do
        [ -n "$label" ] || continue
        rm -f "$W/hurt.sstr"
        if ! python3 "$ROOT/tests/damage.py" "$PROTO" "$mode" "$cap" "$W/hurt.sstr" $margs > "$W/damage.log" 2>&1; then
            bad "$w capture, $label: damage.py failed: $(tail -1 "$W/damage.log")"
            continue
        fi
        compare "$w capture, $label" "$W/hurt.sstr"
    done <<EOF
bit errors 1e-5|ber|0.00001
bit errors 1e-4|ber|0.0001
bit errors 1e-3|ber|0.001
bit errors 1e-2|ber|0.01
4 KiB burst early|burst|200000 4096
4 KiB burst mid-body|burst|$((size / 2)) 4096
200 KiB burst|burst|300000 204800
one record wiped|wipe|5
two wiped, different groups|wipe|2 20
two wiped, same group|wipe|2 4
stream header wiped|wipehead|
truncated at 60 %|truncate|0.6
tampered record|tamper|3
EOF
done

# 3. The armor, and a noisy line.
cap="$W/ts.py.sstr"
[ -f "$cap" ] || cap="$W/rand.py.sstr"
$PY armor "$cap" > "$W/py.armor"
"$SSTR" armor "$cap" > "$W/rs.armor"
cmp -s "$W/py.armor" "$W/rs.armor" && ok "armor: identical lines ($(wc -l < "$W/rs.armor"))" || bad "armor: the lines differ"
while IFS='|' read -r label drop ber junk; do
    [ -n "$label" ] || continue
    python3 "$ROOT/tests/noise.py" "$drop" "$ber" "$junk" < "$W/py.armor" > "$W/line"
    $PY recv "$W/line" > "$W/py.recv" 2> "$W/py.recv.err"; py_rc=$?
    "$SSTR" recv "$W/line" > "$W/rs.recv" 2> "$W/rs.recv.err"; rs_rc=$?
    $PY recv "$W/line" --verify 2> "$W/py.rv" >/dev/null
    "$SSTR" recv "$W/line" --verify 2> "$W/rs.rv" >/dev/null
    if ! cmp -s "$W/py.recv" "$W/rs.recv"; then
        bad "recv, $label: different payloads ($(wc -c < "$W/py.recv") vs $(wc -c < "$W/rs.recv"))"
    elif [ "$py_rc" != "$rs_rc" ]; then
        bad "recv, $label: exit $py_rc vs $rs_rc"
    elif ! diff -u "$W/py.rv" "$W/rs.rv" > "$W/diff"; then
        bad "recv, $label: reports differ:"
        sed 's/^/          /' "$W/diff"
    else
        ok "recv, $label (exit $rs_rc)"
    fi
done <<EOF
clean|0|0|0
3 % of lines dropped, console noise|0.03|0|40
bit errors 1e-4 on the line|0|0.0001|50
10 % of lines dropped|0.10|0|0
EOF
python3 "$ROOT/tests/noise.py" 0.03 0 0 < "$W/rs.armor" > "$W/line"
"$SSTR" unarmor "$W/line" --erasures "$W/rs.er" > "$W/rs.un" 2>/dev/null
$PY unarmor "$W/line" --erasures "$W/py.er" > "$W/py.un" 2>/dev/null
cmp -s "$W/py.un" "$W/rs.un" && cmp -s "$W/py.er" "$W/rs.er" && ok "unarmor: same bytes, same erasure ranges" \
    || bad "unarmor: bytes or erasure ranges differ"

if [ "$FAILED" -eq 0 ]; then
    printf '  ok      crosscheck: %d comparisons agree\n' "$PASSED"
else
    printf '\033[31merror:\033[0m crosscheck: %d of %d comparisons disagree\n' "$FAILED" $((FAILED + PASSED))
    exit 1
fi
