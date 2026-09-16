#!/bin/sh
# SPDX-License-Identifier: MIT
# Copyright (c) 2026 Paul Richeson
#
# dist.sh -- make dist: the release binaries, for this machine and for the
# other targets of V-E when the tools for them are here. Step 4c of
# docs/phase-4.md.
#
# IT ALWAYS BUILDS WHAT IT CAN. The Makefile is the front door and it works
# with nothing but cargo, so `make dist` on any Copal machine produces that
# machine's three binaries and a manifest -- and then says, exactly, which of
# the other targets it could not build and the one command that would supply
# what is missing. A release tool that refuses to do the part it can do
# because it cannot do the rest is a tool people stop running.
#
# THE NATIVE TARGET IS NOT SPELLED THE WAY THE REPORT SPELLS IT. V-E names
# aarch64-unknown-linux-musl; Alpine's rustc calls this same machine
# aarch64-alpine-linux-musl. They are one target with two names, and asking
# cargo for the first on a machine that IS the second sends it looking for a
# standard library that is not installed -- to cross-compile to where it
# already is. So the native build is plain `cargo build --release`, labelled
# with rustc's own host triple, and the cross set is V-E's list less whichever
# of them this machine turns out to be.

# EVERY TARGET HERE IS musl, AND A RELEASE BINARY IS STATIC. Rust's
# *-unknown-linux-musl targets link statically already; Alpine's rust patches
# its OWN triple to link musl dynamically, so the binary this machine builds
# by default needs /lib/ld-musl-*.so.1 at the other end. That is right for a
# machine inside Copal and wrong for a binary handed to one that is not, which
# is the whole point of a dist. It costs about 130 KB a binary -- musl itself
# -- and it is not applied to `make build` or to what copal-build installs,
# where Alpine's default is the right one.
set -u
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
CARGO=${CARGO:-cargo}
RUSTFLAGS="${RUSTFLAGS:-} -C target-feature=+crt-static"
export RUSTFLAGS
# ITS OWN TARGET DIRECTORY, because the flag above is not the one `make build`
# and `make check` use. Sharing one directory has cargo rebuild the whole
# crate every time anybody alternates between them -- and, worse, leaves
# target/release holding a binary built with flags the check did not ask for.
# Both happened while this script was being written.
TDIR=${CARGO_TARGET_DIR:-$ROOT/target/dist-build}
CARGO_TARGET_DIR=$TDIR
export CARGO_TARGET_DIR
DIST=${DIST:-$ROOT/dist}
BINS="sstr ytq sstr-workspace"

RED=''; OFF=''
if [ -t 1 ]; then RED=$(printf '\033[31m'); OFF=$(printf '\033[0m'); fi

# Static or not, checked rather than claimed, and with nothing but grep: a
# dynamically linked ELF names its interpreter inside itself and a static one
# has no interpreter to name. `file` agrees, where there is a `file`.
linkage() {
    if grep -qa 'ld-musl' "$1" 2>/dev/null; then echo dynamic; else echo static; fi
}

host=$($CARGO --version >/dev/null 2>&1 && rustc -vV | sed -n 's/^host: //p')
[ -n "$host" ] || { printf '%serror:%s no rustc -- nothing can be built\n' "$RED" "$OFF"; exit 1; }

# V-E's targets. The Mac builds natively there, with Rust from Homebrew as
# ascitty's Makefile expects, so it is not in the cross set; Windows is
# optional and is listed so that its absence is a decision rather than an
# oversight.
CROSS="aarch64-unknown-linux-musl armv7-unknown-linux-musleabihf x86_64-unknown-linux-musl"

# Is this host the same machine as TARGET under another name? Alpine says
# aarch64-alpine-linux-musl where the report says aarch64-unknown-linux-musl.
same_machine() {
    _a=$(printf '%s' "$host" | sed 's/-alpine-/-unknown-/')
    [ "$_a" = "$1" ]
}

rm -rf "$DIST"
mkdir -p "$DIST"
built=""
skipped=""

# ---- this machine ----------------------------------------------------------
printf '  ..      %s (native)\n' "$host"
if $CARGO build --release --locked >/dev/null 2>&1; then
    mkdir -p "$DIST/$host"
    ok=yes
    for b in $BINS; do
        if [ -f "$TDIR/release/$b" ]; then
            cp "$TDIR/release/$b" "$DIST/$host/$b"
        else
            ok=no
        fi
    done
    if [ "$ok" = yes ]; then
        built="$built $host"
        printf '  ok      %s: %s (%s)\n' "$host" "$BINS" "$(linkage "$DIST/$host/sstr")"
        [ "$(linkage "$DIST/$host/sstr")" = static ] || \
            printf '  %swarning:%s %s came out dynamically linked -- it will want its loader wherever it goes\n' "$RED" "$OFF" "$host"
    else
        printf '%serror:%s %s built but a binary is missing\n' "$RED" "$OFF" "$host"
    fi
else
    printf '%serror:%s %s: the native build failed -- make build to see why\n' "$RED" "$OFF" "$host"
fi

# ---- the others ------------------------------------------------------------
# WHAT IS MISSING IS NAMED, WITH THE COMMAND THAT SUPPLIES IT. A backtrace
# from inside cargo tells someone that a thing went wrong; this tells them
# what to type.
# WHICH rustup, AND THEREFORE WHICH cargo. Alpine's rust and a rustup
# toolchain can both be installed, and only rustup's cargo can see the targets
# rustup added -- so the cross builds must use THAT one by path, not whichever
# `cargo` PATH happens to resolve to today. Copal's own ~/.profile puts
# ~/.cargo/bin ahead of the system directories once it exists, so PATH order
# here changes the moment rustup is installed, which is not a thing a release
# script should quietly depend on.
RUSTUP=""
if command -v rustup >/dev/null 2>&1; then
    RUSTUP=$(command -v rustup)
elif [ -x "$HOME/.cargo/bin/rustup" ]; then
    RUSTUP="$HOME/.cargo/bin/rustup"
fi
CROSS_CARGO=""
[ -n "$RUSTUP" ] && CROSS_CARGO="$(dirname "$RUSTUP")/cargo"
have_rustup=no; [ -n "$RUSTUP" ] && have_rustup=yes
have_zigbuild=no; command -v cargo-zigbuild >/dev/null 2>&1 && have_zigbuild=yes
have_zig=no; command -v zig >/dev/null 2>&1 && have_zig=yes

for t in $CROSS; do
    if same_machine "$t"; then
        printf '  --      %s: this machine, built above as %s\n' "$t" "$host"
        continue
    fi
    why=""
    if [ "$have_rustup" = no ]; then
        why="no rustup, so no standard library for it: apk add rustup, rustup-init -y --no-modify-path, then rustup target add $t"
    elif ! "$RUSTUP" target list --installed 2>/dev/null | grep -qx "$t"; then
        why="its standard library is not installed: rustup target add $t"
    elif [ "$have_zigbuild" = no ]; then
        if [ "$have_zig" = yes ]; then
            why="no cargo-zigbuild, though zig is here: cargo install cargo-zigbuild"
        else
            why="no linker for it: apk add zig && cargo install cargo-zigbuild"
        fi
    fi
    if [ -n "$why" ]; then
        skipped="$skipped$t|$why
"
        printf '  --      %s: %s\n' "$t" "$why"
        continue
    fi
    printf '  ..      %s (cargo zigbuild, %s)\n' "$t" "$CROSS_CARGO"
    if "$CROSS_CARGO" zigbuild --release --locked --target "$t" >/dev/null 2>&1; then
        mkdir -p "$DIST/$t"
        for b in $BINS; do cp "$TDIR/$t/release/$b" "$DIST/$t/$b"; done
        built="$built $t"
        printf '  ok      %s: %s (%s)\n' "$t" "$BINS" "$(linkage "$DIST/$t/sstr")"
    else
        skipped="$skipped$t|the build failed: cargo zigbuild --release --target $t
"
        printf '%serror:%s %s: the build failed\n' "$RED" "$OFF" "$t"
    fi
done

# ---- what travels with the binaries ----------------------------------------
# THE EVIDENCE GOES WITH THEM. The phase-4 row asks that the binaries RUN on a
# Pi 2B and an x86_64 VM, and a binary that prints its version has proved it
# can be loaded and little else. So the dist carries a capture written here, a
# damaged copy of it, the payload both should come back as, and the script that
# asks the far machine those questions -- which needs a shell and nothing else
# when it gets there. No cargo, no python3, no checkout, no network.
#
# The damage is done HERE, where damage.py and the prototype are, rather than
# on the far machine, where neither is. A pre-damaged file is a file; there is
# nothing left to go wrong at the other end.
PROTO=${PROTO:-$ROOT/tests/reference/copal-sstr.py}
ACC="$DIST/acceptance"
acceptance=no
if [ -n "$built" ] && command -v python3 >/dev/null 2>&1 && [ -f "$PROTO" ]; then
    first=$(printf '%s' "$built" | tr -s " " "\n" | grep -v "^$" | head -1)
    SS="$DIST/$first/sstr"
    mkdir -p "$ACC"
    # One full group at a 4 KiB chunk: 16 records, so "records 2 and 4" are
    # two records of one group with records either side of them.
    head -c 65536 /dev/urandom > "$ACC/payload.bin"
    if "$SS" record "$ACC/whole.sstr" --input "$ACC/payload.bin" \
            --type application/octet-stream --chunk 4096 >/dev/null 2>&1 &&
       python3 "$ROOT/tests/damage.py" "$PROTO" wipe "$ACC/whole.sstr" "$ACC/two-lost.sstr" 2 4 >/dev/null 2>&1; then
        # PROVED HERE BEFORE IT IS SHIPPED. A fixture that does not actually
        # lose two records would have the far machine passing a test of
        # nothing, and saying so.
        if "$SS" play "$ACC/two-lost.sstr" -o "$DIST/.probe" >/dev/null 2>&1 &&
           cmp -s "$ACC/payload.bin" "$DIST/.probe"; then
            cp "$ROOT/tools/verify.sh" "$DIST/verify.sh"
            chmod 0755 "$DIST/verify.sh"
            acceptance=yes
            printf '  ok      acceptance: a capture, a damaged copy and verify.sh, for the far machine\n'
        else
            printf '  %swarning:%s the damaged fixture did not rebuild here -- not shipping it\n' "$RED" "$OFF"
        fi
        rm -f "$DIST/.probe"
    fi
    [ "$acceptance" = yes ] || rm -rf "$ACC"
else
    printf '  --      acceptance: needs python3 and %s to damage a capture; not built\n' "$PROTO"
fi

# ---- the manifest ----------------------------------------------------------
# NO TIMESTAMP. A manifest that changes when nothing changed cannot be
# compared with the last one, and the commit says when far better than a clock
# does.
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)
commit=$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || echo "not a git checkout")
dirty=""
git -C "$ROOT" diff --quiet 2>/dev/null || dirty="  (with uncommitted changes)"
{
    printf 'staticstream %s\n' "$version"
    printf 'commit %s%s\n' "$commit" "$dirty"
    printf 'format  version 0 written by default, version 1 with --format 1\n'
    printf 'linked  static (-C target-feature=+crt-static), so no loader is needed\n'
    if [ "$acceptance" = yes ]; then
        printf 'verify  sh verify.sh   on the machine these are carried to\n'
    fi
    printf '\n'
    for t in $built; do
        printf '%s\n' "$t"
        for b in $BINS; do
            f="$DIST/$t/$b"
            [ -f "$f" ] || continue
            printf '  %-16s %10s  %-8s %s\n' "$b" "$(wc -c < "$f")" "$(linkage "$f")" "$(sha256sum "$f" 2>/dev/null | cut -c1-64)"
        done
        printf '\n'
    done
    if [ -n "$skipped" ]; then
        printf 'not built here\n'
        printf '%s' "$skipped" | while IFS='|' read -r t why; do
            [ -n "$t" ] || continue
            printf '  %-32s %s\n' "$t" "$why"
        done
        printf '\n'
    fi
    printf 'aarch64-apple-darwin builds natively on the Mac: make, with Rust from Homebrew.\n'
    printf 'x86_64-pc-windows-gnu is optional and is not in this matrix.\n'
} > "$DIST/MANIFEST"

n=0
for t in $built; do n=$((n + 1)); done
if [ "$n" -eq 0 ]; then
    printf '%serror:%s nothing was built\n' "$RED" "$OFF"
    exit 1
fi
printf '  ok      dist: %s target(s) in %s, and %s\n' "$n" "$DIST" "$DIST/MANIFEST"
