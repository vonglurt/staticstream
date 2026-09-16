#!/bin/sh
# SPDX-License-Identifier: MIT
# Copyright (c) 2026 Paul Richeson
#
# outer-check.sh -- make outer-check: version 1's outer code, for step 4a of
# docs/phase-4.md.
#
# THE DONE-CONDITION OF THE PHASE, run as an experiment and run both ways:
#
#   Damage a capture so that two records of one group are lost.
#   Version 0 loses them.  Version 1 plays back byte-identical.
#
# The version 0 failure is as much of the result as the version 1 success. A
# battery that only ever passes is measuring the battery: if the damage stopped
# landing where it is aimed -- a smaller capture, a bigger group, a changed
# chunk size -- version 1 would go on passing and nothing would say the test
# had stopped testing. So version 0 is required to FAIL here, and the check
# fails if it does not.
#
# Phase 4 has no Python to compare against for this, and that is by design:
# V-F of the project report says the prototype stays at version 0, so it cannot
# read a version 1 capture. What anchors this instead is the pair of runs.
#
# The thirteen-mode battery at version 1 is step 4b; this is the one case the
# step turns on.

set -u
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
SSTR=${SSTR:-$ROOT/target/release/sstr}
PROTO=${PROTO:-$ROOT/../copal/tools/copal-sstr.py}

skip() { printf '  --      outer-check skipped: %s\n' "$1"; exit 0; }
[ -x "$SSTR" ] || { printf 'outer-check: no %s -- make build\n' "$SSTR"; exit 2; }
command -v python3 >/dev/null 2>&1 || skip "no python3, and damage.py needs it"
[ -f "$PROTO" ] || skip "no $PROTO, and damage.py reads the record framing with it"

W=$(mktemp -d "${TMPDIR:-/tmp}/sstr-outer.XXXXXX")
trap 'rm -rf "$W"' EXIT INT TERM
FAILED=0
PASSED=0
ok() { PASSED=$((PASSED + 1)); [ -n "${VERBOSE:-}" ] && printf '  ok      %s\n' "$1"; return 0; }
bad() { FAILED=$((FAILED + 1)); printf '  FAIL    %s\n' "$1"; }

# A CHUNK SMALL ENOUGH THAT A GROUP IS A GROUP. At the default 64 KiB this
# payload is four records and "records 2 and 4 of one group" is most of the
# capture; at 4 KiB it is about fifty, and the wipe lands inside the first
# group with records on either side of it, which is the case being tested.
head -c 200000 /dev/urandom > "$W/payload.bin"
CHUNK=4096

for v in 0 1; do
    "$SSTR" record "$W/cap.$v.sstr" --input "$W/payload.bin" \
        --type application/octet-stream --chunk "$CHUNK" --format "$v" >/dev/null 2>&1 \
        || { bad "version $v: record failed"; continue; }
done

# The header says which outer code it is, and `sstr verify` prints the header.
for v in 0 1; do
    [ -f "$W/cap.$v.sstr" ] || continue
    if "$SSTR" verify "$W/cap.$v.sstr" >/dev/null 2>&1; then
        ok "version $v: a fresh capture verifies"
    else
        bad "version $v: a fresh capture does not verify"
    fi
done

# The cost of the second row, measured rather than assumed.
if [ -f "$W/cap.0.sstr" ] && [ -f "$W/cap.1.sstr" ]; then
    s0=$(wc -c < "$W/cap.0.sstr")
    s1=$(wc -c < "$W/cap.1.sstr")
    if [ "$s1" -gt "$s0" ]; then
        ok "version 1 costs $(( s1 - s0 )) bytes more than version 0 on 200,000 ($s0 -> $s1)"
    else
        bad "version 1 is not larger than version 0 -- the second row is missing"
    fi
fi

# ---- the experiment --------------------------------------------------------
# damage.py wipe A B destroys data records A and B outright. With a group of
# 16, records 2 and 4 are in the same group.
for v in 0 1; do
    [ -f "$W/cap.$v.sstr" ] || continue
    if ! python3 "$ROOT/tests/damage.py" "$PROTO" wipe "$W/cap.$v.sstr" "$W/hurt.$v.sstr" 2 4 >"$W/damage.$v.log" 2>&1; then
        bad "version $v: damage.py failed: $(tail -1 "$W/damage.$v.log")"
        continue
    fi
    "$SSTR" play "$W/hurt.$v.sstr" -o "$W/out.$v.bin" >/dev/null 2>&1
    exit_code=$?
    rebuilt=$("$SSTR" verify "$W/hurt.$v.sstr" 2>&1 | sed -n 's/.*, \([0-9]*\) records rebuilt from parity/\1/p')
    lost=$("$SSTR" verify "$W/hurt.$v.sstr" 2>&1 | sed -n 's/^lost  *\([0-9]*\) data records.*/\1/p')
    same=no
    cmp -s "$W/payload.bin" "$W/out.$v.bin" && same=yes

    if [ "$v" = 0 ]; then
        # VERSION 0 MUST FAIL. One XOR row cannot answer two holes, and if this
        # ever passes then the damage is no longer landing in one group and the
        # version 1 result below has stopped meaning anything.
        if [ "$same" = no ] && [ "${rebuilt:-0}" = 0 ] && [ "${lost:-0}" = 2 ]; then
            ok "version 0 loses both: exit $exit_code, 0 rebuilt, 2 data records lost"
        else
            bad "version 0 was expected to lose two records and did not (payload same=$same, rebuilt=${rebuilt:-?}, lost=${lost:-?}) -- the damage is not landing in one group any more"
        fi
    else
        if [ "$same" = yes ] && [ "${rebuilt:-0}" = 2 ] && [ "${lost:-1}" = 0 ]; then
            ok "version 1 rebuilds both: exit $exit_code, 2 rebuilt, 0 lost, payload byte-identical"
        else
            bad "version 1 did not rebuild two (payload same=$same, rebuilt=${rebuilt:-?}, lost=${lost:-?}, exit $exit_code)"
        fi
    fi
done

# One lost record is still one lost record, at either version: version 1's
# first row IS version 0's row, so this is the arithmetic that always did it.
for v in 0 1; do
    [ -f "$W/cap.$v.sstr" ] || continue
    python3 "$ROOT/tests/damage.py" "$PROTO" wipe "$W/cap.$v.sstr" "$W/one.$v.sstr" 5 >/dev/null 2>&1 || continue
    "$SSTR" play "$W/one.$v.sstr" -o "$W/one.$v.bin" >/dev/null 2>&1
    if cmp -s "$W/payload.bin" "$W/one.$v.bin"; then
        ok "version $v rebuilds one lost record, as it always did"
    else
        bad "version $v did not rebuild a single lost record"
    fi
done

if [ "$FAILED" -eq 0 ]; then
    printf '  ok      outer-check: %s checks pass\n' "$PASSED"
    exit 0
fi
printf '  FAIL    outer-check: %s of %s failed\n' "$FAILED" "$((PASSED + FAILED))"
exit 1
