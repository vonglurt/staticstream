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
# Vendored; see tests/reference/README.md for why.
PROTO=${PROTO:-$ROOT/tests/reference/copal-sstr.py}

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

# ---- B: the whole battery, at both versions (step 4b) ----------------------
# THE ANCHOR IS VERSION 0, AND VERSION 0 IS ANCHORED TO THE PYTHON. There is no
# Python to compare a version 1 capture against -- V-F of the project report
# says the prototype stays at version 0 -- so what this compares is version 1
# against version 0 on the same damage. That is not self-consistency: version 0
# is held to `tools/copal-sstr.py` by the 44 comparisons in crosscheck.sh, so
# the chain is Python <-> version 0 <-> version 1.
#
# THE MEASURE IS THE RECOVERED PAYLOAD, NOT THE `lost` COUNTER, and that is a
# finding rather than a convenience. On a 200 KiB burst version 0 reports "1
# data records lost, 40 of unknown type" and version 1 reports "14 lost, 26
# unknown" -- which reads as a regression and is the opposite of one. `lost`
# means "known to be missing", and a record is known to be missing because a
# surviving parity record's entry table names it. Version 1 could NAME
# thirteen more of them. Both recovered the same 248,448 bytes and neither
# matched. A check asserting on `lost` would have failed the better program.
BW=$(mktemp -d "${TMPDIR:-/tmp}/sstr-battery.XXXXXX")
trap 'rm -rf "$W" "$BW"' EXIT INT TERM
head -c 400000 /dev/urandom > "$BW/payload.bin"
for v in 0 1; do
    "$SSTR" record "$BW/cap.$v.sstr" --input "$BW/payload.bin" \
        --type application/octet-stream --chunk "$CHUNK" --format "$v" >/dev/null 2>&1 || true
done
bsize=$(wc -c < "$BW/cap.0.sstr" 2>/dev/null || echo 0)

# recovers VERSION FILE MODE ARGS...: yes when the payload comes back whole.
recovers() {
    _v=$1; shift
    python3 "$ROOT/tests/damage.py" "$PROTO" "$1" "$BW/cap.$_v.sstr" "$BW/hurt.bin" $2 >/dev/null 2>&1 || { echo damage-failed; return; }
    "$SSTR" play "$BW/hurt.bin" -o "$BW/played.bin" >/dev/null 2>&1
    if cmp -s "$BW/payload.bin" "$BW/played.bin"; then echo yes; else echo no; fi
}

better=0
sameg=no
while IFS='|' read -r label mode margs; do
    [ -n "$label" ] || continue
    margs=$(printf '%s' "$margs" | sed "s/SIZE2/$((bsize / 2))/")
    r0=$(recovers 0 "$mode" "$margs")
    r1=$(recovers 1 "$mode" "$margs")
    case "$r0/$r1" in
        damage-failed/*|*/damage-failed) bad "B $label: damage.py failed" ;;
        # Version 1 must never be the one that loses a payload version 0 kept.
        yes/no) bad "B $label: version 0 recovered the payload and version 1 did not" ;;
        no/yes)
            better=$((better + 1))
            [ "$mode" = wipe ] && [ "$margs" = "2 4" ] && sameg=yes
            ok "B $label: version 1 recovers it, version 0 does not"
            ;;
        *) ok "B $label: both $r0" ;;
    esac
done <<EOF
bit errors 1e-5|ber|0.00001
bit errors 1e-4|ber|0.0001
bit errors 1e-3|ber|0.001
bit errors 1e-2|ber|0.01
4 KiB burst early|burst|200000 4096
4 KiB burst mid-body|burst|SIZE2 4096
200 KiB burst|burst|300000 204800
one record wiped|wipe|5
two wiped, different groups|wipe|2 20
two wiped, same group|wipe|2 4
stream header wiped|wipehead|
truncated at 60 %|truncate|0.6
tampered record|tamper|3
EOF

# AND VERSION 1 MUST WIN SOMEWHERE. A battery in which the two versions always
# agree is a battery whose damage no longer reaches the outer code, and it
# would pass in silence.
if [ "$better" -ge 2 ]; then
    ok "B version 1 recovers $better payloads version 0 loses"
else
    bad "B version 1 was better on $better kinds of damage, expected at least 2 -- the battery has stopped reaching the outer code"
fi
if [ "$sameg" = yes ]; then
    ok "B and one of them is two records of one group, which is the phase's own done-condition"
else
    bad "B 'two wiped, same group' did not separate the versions"
fi

# ---- C: what the second row costs (step 4b) --------------------------------
# THE DESIGNED FIGURE ASSUMES A FULL GROUP. (1 + 32/223) x (1 + 1/16) = 1.2150
# for version 0 and x (1 + 2/16) = 1.2864 for version 1, a ratio of 1.0588 --
# but the parity blob is one padded body per row, so a capture with four
# records in its group pays the whole outer code on four records. Measured on
# 400,000 bytes at 64 KiB chunks the ratio came out 1.1392, and that is not a
# fault in the arithmetic: the group was six records, not sixteen. So this
# measures where groups are full.
head -c 2097152 /dev/urandom > "$BW/big.bin"
for v in 0 1; do
    "$SSTR" record "$BW/big.$v.sstr" --input "$BW/big.bin" \
        --type application/octet-stream --chunk 16384 --format "$v" >/dev/null 2>&1 || true
done
if [ -f "$BW/big.0.sstr" ] && [ -f "$BW/big.1.sstr" ]; then
    b0=$(wc -c < "$BW/big.0.sstr")
    b1=$(wc -c < "$BW/big.1.sstr")
    ratio=$(awk -v a="$b0" -v b="$b1" 'BEGIN{printf "%.4f", b/a}')
    over0=$(awk -v a="$b0" 'BEGIN{printf "%.4f", a/2097152}')
    over1=$(awk -v b="$b1" 'BEGIN{printf "%.4f", b/2097152}')
    inband=$(awk -v r="$ratio" 'BEGIN{print (r > 1.03 && r < 1.10) ? "yes" : "no"}')
    if [ "$inband" = yes ]; then
        ok "C the second row costs $ratio of version 0 (designed 1.0588); overhead $over0 -> $over1 of the payload"
    else
        bad "C the second row costs $ratio of version 0, which is not near the designed 1.0588 ($b0 -> $b1)"
    fi
fi

# ---- D: a version 0 reader, given a version 1 capture -----------------------
# VERSION 1 IS BACKWARD COMPATIBLE, and that was not the plan -- it is what P
# being version 0's row, first, turns out to buy. A version 0 reader takes the
# first padded width of the blob as the XOR and truncates each rebuilt body to
# its own length, so the Q row sitting behind it is bytes that reader never
# reaches. tools/copal-sstr.py therefore plays a version 1 capture, and still
# rebuilds ONE lost record of a group from it. It fails only at two, which is
# precisely what version 1 added.
#
# This is checked here because it is the kind of property that is true by
# accident until someone reorders two rows, and then quietly is not.
head -c 200000 /dev/urandom > "$W/compat.bin"
if "$SSTR" record "$W/compat.sstr" --input "$W/compat.bin" \
    --type application/octet-stream --chunk "$CHUNK" --format 1 >/dev/null 2>&1; then

    python3 "$PROTO" play "$W/compat.sstr" -o "$W/proto.out" >/dev/null 2>&1
    if cmp -s "$W/compat.bin" "$W/proto.out"; then
        ok "D the prototype plays a version 1 capture"
    else
        bad "D the prototype could not play an undamaged version 1 capture"
    fi

    if python3 "$ROOT/tests/damage.py" "$PROTO" wipe "$W/compat.sstr" "$W/compat1.sstr" 5 >/dev/null 2>&1; then
        python3 "$PROTO" play "$W/compat1.sstr" -o "$W/proto1.out" >/dev/null 2>&1
        if cmp -s "$W/compat.bin" "$W/proto1.out"; then
            ok "D and rebuilds one lost record from it, because P is its own row"
        else
            bad "D the prototype could not rebuild one lost record of a version 1 capture"
        fi
    fi

    # AND IT STOPS AT TWO. If this ever succeeds, P and Q are not what this
    # check thinks they are.
    if python3 "$ROOT/tests/damage.py" "$PROTO" wipe "$W/compat.sstr" "$W/compat2.sstr" 2 4 >/dev/null 2>&1; then
        python3 "$PROTO" play "$W/compat2.sstr" -o "$W/proto2.out" >/dev/null 2>&1
        "$SSTR" play "$W/compat2.sstr" -o "$W/rs2.out" >/dev/null 2>&1
        p_same=no; cmp -s "$W/compat.bin" "$W/proto2.out" && p_same=yes
        r_same=no; cmp -s "$W/compat.bin" "$W/rs2.out" && r_same=yes
        if [ "$p_same" = no ] && [ "$r_same" = yes ]; then
            ok "D and stops at two, where version 1's own reader does not"
        else
            bad "D two lost: prototype recovered=$p_same, version 1 reader recovered=$r_same"
        fi
    fi
fi

if [ "$FAILED" -eq 0 ]; then
    printf '  ok      outer-check: %s checks pass\n' "$PASSED"
    exit 0
fi
printf '  FAIL    outer-check: %s of %s failed\n' "$FAILED" "$((PASSED + FAILED))"
exit 1
