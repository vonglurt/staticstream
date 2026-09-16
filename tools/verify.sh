#!/bin/sh
# SPDX-License-Identifier: MIT
# Copyright (c) 2026 Paul Richeson
#
# verify.sh -- "do these binaries run here?", asked properly.
#
# THIS RUNS ON THE FAR MACHINE, and it is shipped inside dist/ so that it gets
# there with the binaries. It needs a shell and nothing else: no cargo, no
# python3, no checkout, no network. That is the point -- a Pi 2B with a copy of
# dist/ on it should be able to answer for itself.
#
# WHY IT IS MORE THAN `sstr --version`. The phase-4 row says "binaries run on
# the Pi 2B and the x86_64 VM". A binary that prints its version has proved it
# can be loaded and little else: not that its arithmetic is right on this
# architecture, not that a capture written here reads back, and not that the
# outer code -- the whole of what version 1 added -- works on a 32-bit ARM.
# Those are the things that differ between machines, so those are the things
# asked. The acceptance files beside this script were written on the machine
# that built these binaries; playing them here is a cross-machine round trip.
#
# Usage:  sh verify.sh [DIR]      (DIR defaults to the directory this is in)

set -u
HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
D=${1:-$HERE}
A="$D/acceptance"
FAILED=0
PASSED=0
ok() { PASSED=$((PASSED + 1)); printf '  ok      %s\n' "$1"; }
bad() { FAILED=$((FAILED + 1)); printf '  FAIL    %s\n' "$1"; }

# The one target directory in here, or the one named. A dist built on one
# machine holds one; a dist built with the cross tools holds several, and the
# machine being stood on picks its own by trying them.
targets=""
for t in "$D"/*/; do
    [ -f "$t/sstr" ] && targets="$targets $(basename "$t")"
done
[ -n "$targets" ] && [ -d "$A" ] || { printf 'verify.sh: no target directories with an sstr in %s\n' "$D"; exit 2; }

W=$(mktemp -d "${TMPDIR:-/tmp}/sstr-verify.XXXXXX")
trap 'rm -rf "$W"' EXIT INT TERM

printf 'verify.sh on %s\n\n' "$(uname -m) $(uname -s)"

for t in $targets; do
    B="$D/$t"
    printf '%s\n' "$t"

    # 1. IT LOADS AND IT SAYS WHAT IT IS. A binary for the wrong architecture
    #    fails here, and says Exec format error rather than anything subtle.
    runs=yes
    for b in sstr ytq sstr-workspace; do
        if v=$("$B/$b" --version 2>&1); then
            ok "$b runs: $v"
        else
            bad "$b did not run: $v"
            runs=no
        fi
    done
    [ "$runs" = yes ] || { printf '\n'; continue; }

    # 2. NO LOADER WAS NEEDED. A dist binary is static, so it must not be
    #    naming an interpreter it hopes to find here.
    if grep -qa 'ld-musl' "$B/sstr"; then
        bad "sstr names a loader -- it is not static, and ran only because this machine happened to have one"
    else
        ok "sstr needs no loader: static"
    fi

    # 3. IT READS WHAT THE OTHER MACHINE WROTE. The acceptance files were
    #    written where these binaries were built.
    if "$B/sstr" play "$A/whole.sstr" -o "$W/whole.out" >/dev/null 2>&1 &&
       cmp -s "$A/payload.bin" "$W/whole.out"; then
        ok "plays the capture built elsewhere, payload byte-identical"
    else
        bad "could not play the capture built on the other machine"
    fi

    # 4. AND THE OUTER CODE WORKS HERE. Two records of one group destroyed;
    #    version 1 rebuilds both. This is the arithmetic most likely to differ
    #    between a 64-bit and a 32-bit machine, and it is the whole of what
    #    version 1 is for.
    if "$B/sstr" play "$A/two-lost.sstr" -o "$W/two.out" >/dev/null 2>&1 &&
       cmp -s "$A/payload.bin" "$W/two.out"; then
        ok "rebuilds two lost records of a group, payload byte-identical"
    else
        bad "did not rebuild two lost records of a group -- the outer code is wrong on this machine"
    fi

    # 5. IT WRITES SOMETHING THIS MACHINE CAN READ BACK. A round trip made
    #    here, so a wrong-endian or wrong-width bug has somewhere to show.
    if "$B/sstr" record "$W/here.sstr" --input "$A/payload.bin" \
            --type application/octet-stream --chunk 4096 >/dev/null 2>&1 &&
       "$B/sstr" verify "$W/here.sstr" >/dev/null 2>&1 &&
       "$B/sstr" play "$W/here.sstr" -o "$W/here.out" >/dev/null 2>&1 &&
       cmp -s "$A/payload.bin" "$W/here.out"; then
        ok "records and plays back its own capture here"
    else
        bad "a capture written on this machine did not read back"
    fi
    printf '\n'
done

if [ "$FAILED" -eq 0 ]; then
    printf '%s checks pass. Paste this out; it is the phase-4 evidence.\n' "$PASSED"
    exit 0
fi
printf '%s of %s failed.\n' "$FAILED" "$((PASSED + FAILED))"
exit 1
