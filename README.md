<!-- SPDX-License-Identifier: MIT — Copyright (c) 2026 Paul Richeson -->

# staticstream

**Static Stream: a stream stopped in a file, and let go again.**

[![staticstream on crates.io](https://img.shields.io/crates/v/staticstream.svg?style=flat-square)](https://crates.io/crates/staticstream)
[![MIT licence](https://img.shields.io/crates/l/staticstream.svg?style=flat-square)](https://github.com/vonglurt/staticstream/blob/main/LICENSE)
![zero dependencies](https://img.shields.io/badge/dependencies-0-informational?style=flat-square)

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
downloads as Static Stream by default, so the two belong together. ytq was a Python
program in [copal](https://github.com/vonglurt/copal); phase 2 moved every
line of it here, and retired that one.

## Status: phases 0–3 done, phase 4 all but two runs

`make check` is **399 checks** and **141 unit tests**, and passes with nothing
installed but `cargo`: 44 comparisons against the Python prototype, 26 of
version 1's outer code, 32 + 50 + 34 + 133 across ytq, and 80 of the
Workspace. Clone it and run `make check` — it needs no second checkout, no
network and no crates.

The format is defined, built and measured by the Python prototype,
`copal-sstr.py`, frozen here at
[`tests/reference/copal-sstr.py`](tests/reference/README.md) and originally
`tools/copal-sstr.py` in [copal](https://github.com/vonglurt/copal). Two
reports there describe it:
- [the Static Stream lab report](https://github.com/vonglurt/copal/blob/main/docs/static-stream-lab-report.md)
  sets out the format and what was measured;
- [the staticstream project report](https://github.com/vonglurt/copal/blob/main/docs/staticstream-project-lab-report.md)
  sets out this repository's plan.

| Phase | Delivers | Done when |
|---|---|---|
| 0 | this repository: the workspace, the Makefile, the constants | `make check` passes on the guest and on the Mac |
| 1 | the format library and `sstr` at parity with the prototype | Python writes and Rust reads, and the reverse, byte for byte, through the prototype's whole damage battery |
| 2 | ytq in Rust, sharing the Python ytq's `queue.json` and locks, archiving to `.sstr` by default through `~/.config/copal/media.conf` | a queued video leaves a `.sstr` that verifies and plays back identical to the MP4 it replaced; Rust and Python ytq run side by side on one queue; only then does the binary `ytq` exist |
| 3 | `sstr-workspace`: Browser, Inspector, Transcript, Services, Queue | every Service is a command line shown in the Transcript before it runs — **done** |
| **4** | `make dist` for every target, and version 1's stronger outer code | binaries run on a Pi 2B and an x86_64 VM — **version 1 done, all three targets build; the two runs are outstanding** |

**Version 1 is what `sstr record` writes.** Its parity record carries two rows
where version 0 has one, so two lost records of a group come back instead of
one, at about 6 % more. `--format 0` writes the older one, and readers take
both — a version 0 reader can even read a version 1 capture and still rebuild
a single lost record from it, which was not designed but falls out of the
first row being version 0's own.

Phases 0, 1 and 2 are done on the guest. `sstr` does everything the prototype
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
against the frozen prototype.
- **Both directions:** each writes random, text and MPEG-TS captures, and
  both read all of them.
- **Damage:** 13 kinds, on a capture from each, with the same repairs, losses,
  exit codes and played bytes.
- **Armor:** identical lines, and the same payload and report through a noisy
  line.

Measured on the guest:
- Recording 20 MB takes 0.55 s against the prototype's 2.98 s.
- Repairing 20 MB with one bit in 10,000 flipped takes 1.7 s against 24.6 s.

`ytq` is a binary here since step 2e, and the Python ytq is retired.
`~/.local/bin` comes before `/usr/local/bin` on Copal's PATH, which is why no
`ytq` was built until phase 2's test passed: an unfinished one would have
hidden the working Python ytq on every node.

Phase 2, ytq in Rust, is done; `docs/phase-2.md` is the plan and the record.
Step 2a is done:
- **What is in it:** the Rust ytq is `sstr ytq`, with `add`, `clip`,
  `list`, `status`, `clear`, `cookies` and the help.
- **What it shares:** the Python ytq's `queue.json`, its locks and its log.
  A runner it starts is the Python one until step 2b.
- **The test:** `make ytq-crosscheck` agrees with the Python ytq on 32
  comparisons.
  - **Links:** 6,466 texts through its link matching.
  - **Settings:** five homes.
  - **Queue and panels:** both queue listings and the live status panels.
  - **Queue file:** a queue file rewritten byte for byte as Python's
    `json.dump` writes it.
  - **Concurrency:** thirty simultaneous adds from both.
  - **Clipboard:** the cases, messages and exit codes.

Step 2b is done: the Rust ytq runs its own downloads. `sstr ytq run` does
the whole job:
- **Checking and downloading:** checks, downloads with the live record,
  stops on SIGTERM, retries.
- **The cookie flow:** Brave is opened, and the retry goes through
  `yt-brave`.
- **Transcripts:** with their Notes, and `sstr ytq transcript` on its own.

A runner that `add` or `clip` starts is now the Rust one.

`make ytq-runner-crosscheck` agrees with the Python ytq on 50 comparisons.
- **A stand-in yt-dlp that pauses at gates:**
  - the queue, the log, the files, the notifications and the browser
    opened;
  - status mid-part and mid-merge;
  - SIGTERM mid-merge;
  - a rejected check, a bot check and a flaky download.
- **Transcript text:** identical to Python's `textwrap` on 807 texts and
  60 caption files.
- **A real download of `jNQXAC9IVRw`:** the same file name, MP4 tags,
  transcript Notes and queue entry.

Step 2c is done: what ytq downloads, it keeps as Static Stream.
- **`OUTPUT`:** `sstr` (the default) records the MP4 into a `.sstr`, reads
  it back, and removes the MP4 only once the capture verifies and its
  payload's SHA-256 is the MP4's. `both` keeps both; `mp4` keeps what the
  Python ytq keeps, and asks yt-dlp nothing more. `ytq --sstr`, `--mp4` and
  `--both` set it for one command, the way `--run` and `--no-run` already
  set autostart for one command.
- **`ARCHIVE_DIR`:** where captures and their transcripts go; `DIR` when unset.
- **`SSTR_KEY`:** the key captures are signed with; `~/.ssh/id_ed25519` when
  unset and present; `SSTR_KEY=` for unsigned.
- **The header:** the source page, the license the site states, and
  "title by author".
- **When archiving fails:** the MP4 stays, the entry is still done, and the
  notification says why.

`make ytq-archive-check` passes 34 checks. The captures play back byte for
byte as the MP4, they are signed and trusted through `--allowed-signers`, and
real downloads work in both `both` and `sstr` modes.
`ytq-runner-crosscheck` still agrees with the Python ytq on 50 comparisons,
with `OUTPUT=mp4`.

Step 2d is done: `sstr ytq` with no command is the queue window, without
curses.
- **What it draws:** the Python window's header, the live panel for up to
  two downloads, the list, the keys line and the message line, from the same
  queue file.
- **What it does:**
  - **The watcher:** queues what is on the clipboard at start, then each
    new copy while the window has focus. Focus is asked of Hyprland or X, as
    the Python window asks.
  - **The worker:** takes `run.lock` whenever nobody else has it.
  - **The keys:** `a d r c o p w h x q` and the arrows do what the Python
    window's keys do.
- **The terminal:** `stty` for raw mode, escape sequences for the screen,
  the `TIOCGWINSZ` ioctl for its size, and keys read on a thread.
- **One difference:** SIGTERM, SIGHUP or Ctrl+C end the window as q does.
  The Python window dies of them and leaves yt-dlp running.

`make ytq-window-crosscheck` runs both windows side by side in a private
tmux server and agrees with the Python window on 133 comparisons:
- **Screens, cell by cell with their attributes:** two downloads and a
  24-entry list, with the selection moved and scrolled, at three window
  sizes.
- **27 keys and key sequences**, each from the top of one queue: the screen
  after it, and what it left in `queue.json`, the log and the browser.
- **The window's own download:** the clipboard at start, a copy queued,
  mid-part, paused and mid-merge, then q stopping the merge and putting the
  entry back.
- **Focus:** a copy made while another window has focus is not queued; one
  made while this window has focus is.

Step 2e is done, and with it phase 2: `ytq` is the binary people type, and
the Python ytq is retired.
- **The binary:** `staticstream-ytq` builds `ytq` as well as the library,
  from the same `cli::main` that `sstr ytq` runs, so the two cannot drift.
  `make install` and `copal-build` put it in `~/.local/bin`, first on Copal's
  PATH. Super+Shift+Y needed no change at all: both keybindings already
  resolve `ytq` with `command -v`.
- **A runner it starts is itself:** run as `ytq` the subcommand is not
  repeated, so a background runner is `ytq run --quiet`, and
  `~/.config/ytq/auto` starts one as it always did.
- **Retired:** `copal-prep.sh` no longer writes a Python ytq -- 1,466 lines
  gone. `install_ytq` keeps what is still its business: `/etc/yt-dlp.conf`,
  `yt-brave`, and the wl-clipboard, xclip, xdotool and libnotify that ytq
  calls out to. It removes an old `/usr/local/bin/ytq` only when the header
  shows Copal wrote it.
- **The specification is kept:** the Python ytq is frozen at
  `tests/reference/ytq.py`, byte for byte the file a clean install wrote, so
  the comparisons outlive the program they compare against.

`make check` passes with `ytq` in the Python one's place: 76 unit tests, then
44 + 32 + 50 comparisons, 34 checks and 133 comparisons -- **293 in all**.
Run by hand, `make ytq-crosscheck` and the rest take their default and
exercise `sstr ytq` instead, so both spellings stay covered.

## Getting a folder of captures back out

`sstr export` is the bulk half of `sstr play IN -o FILE`: a folder in, the
files the captures hold out.

```
sstr export DIR                  every .sstr in DIR
sstr export DIR -r               and every folder under it
sstr export DIR -n               what it would do, writing nothing
sstr export DIR --into ELSEWHERE write the files somewhere else
sstr export DIR --remove         delete each capture once its file is
                                 written and verified
sstr export A.sstr B.sstr        named captures, as well as folders
```

The name comes from the capture's own header — its `content_type` decides
`.mp4`, `.webm`, `.mkv`, `.m4a`, `.txt` — which is the same answer the
Workspace's Export key gives, because it is the same function. A file already
there is skipped and counted, never overwritten and never quietly renamed to
a `-1` name. Nothing is renamed to the real name until the capture has
verified: a damaged capture's recovered bytes stay as `NAME.part` with the
damage counts printed under them, and the exit status is 1.

In the Workspace, **X** on a folder is that command line, printed into the
Transcript before it runs.

**[docs/clipboard-to-share.md](docs/clipboard-to-share.md)** is the short
guide to the workflow this exists for: a URL copied on the Mac, through the
SPICE clipboard, into ytq's queue, and out as a file on the folder the Mac
shares with the guest.

## Settings

`sstr config` is the settings of all three programs: what each one is, which
file said so, and how to change it.

```
sstr config                      every setting, its value, and where it came from
sstr config get OUTPUT
sstr config set OUTPUT mp4
sstr config unset SSTR_KEY
sstr config --file ytq set OUTPUT sstr     ~/.config/ytq/config instead
```

`~/.config/copal/media.conf` is written by default: it is read by `sstr`, `ytq`
and the Workspace alike. `~/.config/ytq/config` is read *after* it, so a key
set in both is a key changed in the first with no effect — and `sstr config`
says so, with the line that closes the gap, rather than leaving you to find out.

A change is a line edit: the line that sets the key is replaced where there is
one and appended where there is not, and every other byte of the file — which
for `media.conf` as Copal installs it is three quarters explanation — is the
byte it already was.

In the Workspace, **`,`** opens the same thing as a screen: the settings, the
value in force, and which file set it. Space walks a value with a short list of
choices (`OUTPUT`, `AUTOSTART`, `SSTR_DEFLATE`), `e` types one, `u` takes the
line out, `w` aims the next change at the other file.

**The screen writes no config file.** It builds `sstr config set KEY VALUE`,
says that line into the Transcript, runs it, and says back what it printed —
the rule every Service follows, so a change made with one key can be read out
of the Transcript and pasted into a setup script.

## Install

```sh
cargo install staticstream      # sstr, ytq and sstr-workspace
```

On a Copal machine it is already there: `copal-build` compiles
`~/code/staticstream` on the node itself and links what it builds into
`~/.local/bin`, first on Copal's PATH. `copal-build staticstream` rebuilds
this one alone.

### What it needs at run time

Nothing to build -- the crate has no dependencies, and `cargo install` fetches
this crate and nothing else. What the *programs* call out to, when asked:

| Wanted by | Program | For |
|---|---|---|
| `ytq` downloads | **yt-dlp** | the download itself. It is a Python program that moves too often to embed, so ytq drives it as a subprocess, as the Python ytq did |
| `ytq` downloads | ffmpeg | merging and tagging; without it ytq keeps what one stream gives |
| `ytq` on a desktop | wl-clipboard or xclip, xdotool, libnotify | the clipboard, the focus check, notifications |
| signed captures | ssh-keygen | `-Y sign` and `-Y verify`, as the prototype does |

`sstr`, the format and the Workspace need none of them.

## Build

```sh
make            # the list
make check      # no external crates, the tests, an offline release build, the crosscheck
make crosscheck # sstr against the frozen prototype, comparison by comparison
make outer-check # version 1's outer code: two lost records of a group, rebuilt
make ytq-crosscheck  # the Rust ytq against the Python ytq of tests/reference
make run ARGS='play cap.sstr --paced'
make workspace
make install    # sstr, ytq and sstr-workspace into ~/.local/bin
make package    # the crate tarball, and what is in it -- pushes nothing
make publish    # the one cargo publish call, gated on make check
make tools      # cargo-make and cargo-zigbuild, for:
make dist       # release binaries for aarch64, armv7 and x86_64 musl
```

Everything except `tools`, `dist` and `publish` needs only `cargo`. `make dist`
also needs rustup's standard library for each target, because Alpine's `rust`
package ships only its own.

## No external crates

This crate depends on nothing at all, as in
[orrery](https://github.com/vonglurt/orrery) and
[ascitty](https://github.com/vonglurt/ascitty). Copal's `copal-build`
compiles `~/code` on the machine itself, and a fleet node never reaches the
internet, so a crate fetch would fail on the machines this is for.
`make check` enforces it: `Cargo.lock` may name no package from a registry,
and the release build runs `--offline --locked`.

## One crate, four modules

This was five packages -- `staticstream`, `-tty`, `-cli`, `-ytq` and
`-workspace` -- until the release. They were never four dependencies and a
program: they depended only on each other, shared one version and would have
published together, which is a module boundary wearing a package's clothes.
So there is one crate, named after the repository, and one name on crates.io.

| Module | What |
|---|---|
| `format` | the `.sstr` file: the record layout, RS(255,223), parity, checkpoints, the writer and the reader. It also holds what the prototype had from Python's library and this crate writes itself: zlib, SHA-256 (from orrery), CRC-32 and JSON |
| `tty` | the armor: a byte stream as 80-column CRC-checked lines for a serial console, and back, with what was lost as erasures |
| `ytq` | ytq: its queue and locks, settings, the runner that drives yt-dlp, the live record and the window |
| `workspace` | the terminal Workspace. Phase 3 draws it |

| Binary | Is |
|---|---|
| `sstr` | record, play, verify, armor, unarmor, recv, paths -- and `sstr ytq`, which is `ytq` |
| `ytq` | the download queue people type, and what Super+Shift+Y reaches |
| `sstr-workspace` | the Workspace |

`ytq` and `sstr ytq` are one `ytq::cli::main`, so the two names cannot drift.

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
