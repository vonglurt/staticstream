<!-- SPDX-License-Identifier: MIT — Copyright (c) 2026 Paul Richeson -->

# Phase 2: ytq in Rust

The plan for phase 2 of `docs/staticstream-project-lab-report.md` (in
copal). It was written after reading that report, the Static Stream lab
report, the ytq lab report (`docs/ytq-clipboard-lab-report.md`), the ytq
source in `copal-prep.sh`, `yt-brave` and `/etc/yt-dlp.conf`.

## Done when

This is the report's phase-2 row, unchanged:
- **One queue:** Rust and Python ytq run side by side on one `queue.json`.
- **Archiving:** a queued video leaves a `.sstr` that `sstr verify` passes
  and that plays back identical to the MP4 it replaced.
- **The old way still works:** `OUTPUT=mp4` leaves today's files.
- **Only then:** the binary `ytq` is built, and the Python one is retired
  from `copal-prep.sh`.

## What the Python ytq is

1,436 lines, one file. Everything below is behaviour that the Python ytq has
earned through a week of fixes, recorded in the ytq report. The Rust ytq keeps
all of it.

| Part | Python | Rust module |
|---|---|---|
| where things are | `CONF`, `AUTO`, `DATA`, `QUEUE`, `QLOCK`, `RUNLOCK`, `LOG` from `XDG_*` or `$HOME` | `Paths` (phase 0) |
| settings | `settings()`: `DIR` (`videos_dir()`: the Mac's share only when really mounted, then `XDG_VIDEOS_DIR`, then `~/Videos`), `FORMAT`, `PROFILE`, `KEYRING`, `POLL`, `SUBS`; `KEY=VALUE`, quotes stripped, `~` expanded | `settings` |
| which text is a video | `YT_RE` (optional guarded scheme, `watch?` with `v` anywhere, Shorts, live, embed, youtu.be, 11-character id), `URL_RE`, `youtube_urls()` after `html.unescape`, `as_url()`, `short()` | `urls` |
| the queue file | `Queue.edit()`: `flock(queue.lock)`, read afresh, change, write through a rename only if changed; `snapshot()` without a lock; entries with `url title status quality attempts cookie_tried error added file progress live` | `queue` |
| the one downloader | `RunLock`: `flock(run.lock)` non-blocking with ten tries, pid inside; `holder()` by a shared probe; a new holder puts back entries a dead runner left `downloading`; `start_runner()` inside `Q.edit()`, so a URL cannot land between "nothing left" and "gone" | `queue` |
| the log | `ytq.log`, pid on every line, rotated to `.1` past 4 MiB; `log_start()` names ytq, yt-dlp, Python and the settings | `log` |
| checking | `check()`: `yt-dlp --simulate -f FORMAT --print title/height/ext`, 180 s, cookie complaints queued anyway | `runner` |
| downloading | `download()`: `NAME_OPTS`, `META_OPTS` when ffmpeg is present, `--progress --no-quiet --progress-template`, `FILE` after move, a live record in the entry once a second and at every new step (`stage()`), stop by process group, retry, cookies, failed | `runner`, `live` |
| the live record | `stage()`, `stream_of()`, `size_of()`, `describe()`, `compact()`, `bar()`, `live_view()` | `live` |
| transcripts | a second yt-dlp run; `vtt_text()`; `notes()` with local time and offset, license, the captions' source | `transcript` |
| cookies | `COOKIE_WORDS`, `yt-brave --profile … [--keyring …]`, `brave_ready()`, `open_browser()`, one cookie retry per entry | `runner` |
| telling people | `say()`: print on a terminal, `notify-send` otherwise, only log in the window; `report()` counts above five URLs | `log` |
| the clipboard and focus | `wl-paste`, then `xclip`; focus from `hyprctl activewindow -j` or `xdotool`, against the process's ancestors | `desk` |
| the window | curses: header, live panel for up to two downloads, the list, keys `a d r c o p w h/x q`, a watcher thread and a worker that takes `run.lock` | `window` |
| the commands | `clip add run status cookies transcript list clear`, `--run --no-run --quiet`, help with the settings in effect | `sstr ytq …`, then `ytq` |

## What is new

These come from the project report, V-C and V-D:
- **Settings file:** `~/.config/copal/media.conf` is read before
  `~/.config/ytq/config`, so a key in ytq's own file still wins.
- **`OUTPUT`:** `sstr` by default, or `mp4` or `both`.
  - The download happens as today, to an MP4.
  - With `sstr` or `both`, the MP4 is recorded in-process into
    `Author-Title_ID.sstr`, carrying the source URL, the license the site
    states, and the title and author as the note, and signed with `SSTR_KEY`
    when there is one.
  - The capture is read back. With `sstr`, the MP4 is removed only when the
    read-back verifies and its payload SHA-256 is the MP4's.
  - The entry's `file` names what was kept.
- **`ARCHIVE_DIR`:** defaults to `DIR`. **`SSTR_KEY`:** defaults to
  `~/.ssh/id_ed25519` when it exists.

## Side by side, safely

- **No binary called `ytq`** exists until the last step. The Rust ytq is
  `sstr ytq …`: `sstr ytq add`, `sstr ytq list` and the rest.
- **Same files, same locks, same formats.**
  - It reads and writes `queue.json` exactly as the Python one does:
    `json.dump(indent=1)`, ASCII-escaped, through a rename.
  - It takes `queue.lock` and `run.lock` with `flock`, the lock Python's
    `fcntl.flock` takes.
  - It logs in the same format to the same `ytq.log`.
- **A runner started before step 2b is the Python one:** `sstr ytq add` with
  autostart starts `ytq run --quiet`. From step 2b the Rust runner starts
  itself.
- **The Python ytq is the specification.** Each step has a crosscheck that
  loads the Python ytq's own functions, or runs its commands, and compares.

## The steps

| Step | Delivers | Done when |
|---|---|---|
| **2a** | paths, settings (with `media.conf`), URL recognition, the queue file and both locks, the log, the live record's text; `sstr ytq add`, `clip`, `list`, `status`, `clear`, `help` | `make ytq-crosscheck` agrees with the Python ytq: `youtube_urls`, `as_url` and `short` on a corpus of links including every case in the ytq report; `settings()` under a set of homes; `list` and `status` output on the same queue, the live panel included; entries added by either, and by both at once, all present once; `clip` from a stub clipboard queues the same URLs with the same message. **Done**: 32 of 32 comparisons agree, over 6,466 link texts and five homes |
| 2b | `check`, `claim`, `download` with the live record, stops, retries and the cookie flow; `transcript` and `notes`; `run`, `cookies`, `transcript`; the runner starts itself | against the ytq report's stand-in yt-dlp, the same queue states, log steps and `status` panels as the Python ytq; SIGTERM puts the entry back; a real download of `jNQXAC9IVRw` gives the same file name, tags and `.txt` Notes. **Done**: `make ytq-runner-crosscheck`, 50 of 50 comparisons agree, the real download among them |
| 2c | `OUTPUT`, `ARCHIVE_DIR`, `SSTR_KEY`: archiving to `.sstr` in-process | a queued video with `OUTPUT=sstr` leaves a `.sstr` that verifies and plays back identical to the MP4 (kept for the test with `OUTPUT=both`); `OUTPUT=mp4` leaves exactly what the Python ytq leaves |
| 2d | the window: raw terminal without curses, the watcher and focus, the worker, the keys | a tmux run beside the Python window shows the same header, list and live panel for the same queue, and each key does the same thing to `queue.json` |
| 2e | the binary `ytq`; Super+Shift+Y reaches it; the Python ytq retired from `copal-prep.sh` | the whole crosscheck passes with `ytq` in place of `sstr ytq`, on the guest, with the Python ytq gone |

## Decisions already made

- **yt-dlp stays a subprocess.** It is a Python program and moves too
  often to embed.
- **No curses, no crates.** Step 2d writes to the terminal itself, as
  ascitty's `ascitty-tty` does.
- **Archiving happens after the download, from the file.** The report's
  path (c) came back byte-identical, while a pipe changed the format yt-dlp
  chose.
- **The C library's own calls:** `flock`, `kill`, `localtime_r` and
  `strftime`, through `extern "C"`. std has none of them, and each is one
  line of the C library Rust already links.
