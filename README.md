<!-- SPDX-License-Identifier: MIT — Copyright (c) 2026 Paul Richeson -->

# staticstream

**Static Stream: a stream stopped in a file, and let go again.**

Light can be slowed until it seems to stand still, and released. What the
experiments keep is the pulse's state, written into matter and read back out
as light. A static stream does that with bytes. A stream is written into a
`.sstr` file with its order, its pace and its tolerance for damage intact,
and later let go again as a stream:
- into a new file;
- to a player at the pace it was recorded;
- to VLC or mpv as clients, while it is still being recorded.

Every record is protected by Reed–Solomon codes and parity, about 21 % more
bytes, and checkpoints are signed with your SSH key. Nothing is encrypted,
and nothing is decoded, so no codec is involved.

This repository is the Rust home of the format, of the `sstr` command, of
**ytq**, Copal's download queue that watches the clipboard, and of
`sstr-workspace`, the terminal Workspace over both. ytq archives what it
downloads as Static Stream by default, so the two belong together. ytq is a
Python program in [copal](https://github.com/vonglurt/copal) today, and moves
here piece by piece.

## Status: phase 1 of 4

The format is defined, built and measured by the Python prototype,
`tools/copal-sstr.py` in [copal](https://github.com/vonglurt/copal). Two
reports there describe it:
- [the Static Stream lab report](https://github.com/vonglurt/copal/blob/main/docs/static-stream-lab-report.md)
  sets out the format and what was measured;
- [the staticstream project report](https://github.com/vonglurt/copal/blob/main/docs/staticstream-project-lab-report.md)
  sets out this repository's plan.

| Phase | Delivers | Done when |
|---|---|---|
| **0** | this repository: the workspace, the Makefile, the constants | `make check` passes on the guest and on the Mac |
| 1 | the format library and `sstr` at parity with the prototype | Python writes and Rust reads, and the reverse, byte for byte, through the prototype's whole damage battery |
| 2 | ytq in Rust, sharing the Python ytq's `queue.json` and locks, archiving to `.sstr` by default through `~/.config/copal/media.conf` | a queued video leaves a `.sstr` that verifies and plays back identical to the MP4 it replaced; Rust and Python ytq run side by side on one queue; only then does the binary `ytq` exist |
| 3 | `sstr-workspace`: Browser, Inspector, Transcript, Services, Queue | every Service is a command line shown in the Transcript before it runs |
| 4 | `make dist` for every target, and version 1's stronger outer code | binaries run on a Pi 2B and an x86_64 VM |

Phases 0 and 1 are done on the guest. `sstr` does everything the prototype
does, with the same options and the same reports:
- **Capture and playback:** `record` and `play` (stdout, `-o`, `--paced`,
  `--speed`, `--start`, `--follow`, `--serve`).
- **Checking:** `verify`.
- **Serial lines:** `armor`, `unarmor` and `recv`.
- **ytq's files:** `paths`.

One difference is deliberate: `record` given no bytes at all exits 1 and
leaves no file. A failed download upstream looks exactly like that, and the
prototype recorded it as an empty capture.

`make crosscheck` is phase 1's acceptance test, and passes: 44 comparisons
against `tools/copal-sstr.py`.
- **Both directions:** each writes random, text and MPEG-TS captures, and
  both read all of them.
- **Damage:** 13 kinds, on a capture from each, with the same repairs, losses,
  exit codes and played bytes.
- **Armor:** identical lines, and the same payload and report through a noisy
  line.

Measured on the guest:
- Recording 20 MB takes 0.55 s against the prototype's 2.98 s.
- Repairing 20 MB with one bit in 10,000 flipped takes 1.7 s against 24.6 s.

No `ytq` binary is built yet. `~/.local/bin` comes before `/usr/local/bin`
on Copal's PATH, so an unfinished Rust `ytq` would hide the working Python
one. It is added when phase 2's test passes.

## Build

```sh
make            # the list
make check      # no external crates, the tests, an offline release build, the crosscheck
make crosscheck # sstr against tools/copal-sstr.py in ../copal, comparison by comparison
make run ARGS='play cap.sstr --paced'
make workspace
make install    # sstr and sstr-workspace into ~/.local/bin
make tools      # cargo-make and cargo-zigbuild, for:
make dist       # release binaries for aarch64, armv7 and x86_64 musl
```

Everything except `tools` and `dist` needs only `cargo`. `make dist` also
needs rustup's standard library for each target, because Alpine's `rust`
package ships only its own.

## No external crates

The crates here depend only on each other, as in
[orrery](https://github.com/vonglurt/orrery) and
[ascitty](https://github.com/vonglurt/ascitty). Copal's `copal-build`
compiles `~/code` on the machine itself, and a fleet node never reaches the
internet, so a crate fetch would fail on the machines this is for.
`make check` enforces it: `Cargo.lock` may name no package from a registry,
and the release build runs `--offline --locked`.

## The crates

| Crate | What |
|---|---|
| `staticstream` | the format: the record layout, RS(255,223), parity, checkpoints, the writer and the reader. It also holds what the prototype had from Python's library and this workspace writes itself: zlib, SHA-256 (from orrery), CRC-32 and JSON |
| `staticstream-tty` | the armor: a byte stream as 80-column CRC-checked lines for a serial console, and back, with what was lost as erasures |
| `staticstream-cli` | `sstr`: record, play, verify, armor, unarmor, recv, paths |
| `staticstream-ytq` | ytq: its queue, settings and downloads. Phase 0 has where ytq keeps things and the states an entry moves through, checked against the Python ytq's source |
| `staticstream-workspace` | `sstr-workspace`: the terminal Workspace |

## The Workspace's words

| Word | Is | From |
|---|---|---|
| Workspace | the whole window | NeXTSTEP's Workspace Manager |
| Browser | Miller columns over any folder | the File Viewer's Browser view |
| Shelf | items picked for later | the File Viewer's Shelf |
| Inspector | the selection's header, notes, license and verification | Tools > Inspector |
| Transcript | the running log | Smalltalk-80 |
| Services | verbs sent to the selection | NeXTSTEP Services |

A file is an object and a verb is a message sent to it. `sstr play cap.sstr`
and Play picked from Services are the same message.

## License

MIT — see `LICENSE`.
