<!-- SPDX-License-Identifier: MIT — Copyright (c) 2026 Paul Richeson -->

# Phase 3: the Workspace

The plan for phase 3 of `docs/staticstream-project-lab-report.md` (in copal).
It was written after rereading that report — V-B for the vocabulary and the
screen, V-C for the link with ytq, V-D for the settings and VI for what the
Workspace is *not* — the Static Stream lab report, and `docs/phase-2.md`.

## Done when

This is the report's phase-3 row, unchanged:

> `sstr-workspace`: Browser, Inspector, Transcript, Services, Queue — **every
> Service is a command line shown in the Transcript before it runs.**

## What is different about this phase

Phases 1 and 2 had a specification to agree with: `tools/copal-sstr.py`, then
the Python ytq now frozen at `tests/reference/ytq.py`. Every step ended in a
crosscheck that ran the two side by side and compared them.

**The Workspace has no Python to compare against.** It is the one part of the
project with no prototype. So the rule that replaces it comes from the report's
own bar, and it is a stricter one than it looks:

> A verb is always something a person could have typed.

That is testable, and it is the acceptance test for every step here. For each
Service the Workspace offers, the harness:
1. drives the Workspace in tmux to the selection and presses the key;
2. reads the command line the Transcript printed *before* it ran;
3. runs that exact command line in a shell, in a fresh throwaway HOME;
4. compares what each left — bytes of output, files written, exit code.

If the two differ, the Workspace is doing something a person could not have
typed, and the step is not done. The Workspace is thereby checked against
`sstr` and `ytq` themselves, which are in turn checked against the Python. The
chain back to the specification is unbroken.

## What it is, in the report's words

| Word | Is | From |
|---|---|---|
| Workspace | the whole terminal window | NeXTSTEP's Workspace Manager |
| Browser | Miller columns over any folder, from the archive folder | the File Viewer's Browser view |
| Shelf | items picked for later: files to stream, URLs to queue | the File Viewer's Shelf |
| Inspector | the selection's header, notes, license and verify result | Tools > Inspector |
| Transcript | the running log: `ytq.log` and sstr's events | Smalltalk-80 |
| Services | verbs sent to the selection | NeXTSTEP Services |
| Queue | ytq's queue as one more object to browse and inspect | ytq |

```
┌ Workspace ─ ~/Downloads/SharedVM ──────────────────────────────────────────┐
│ Shelf: [cap.sstr] [notes.txt] [youtube.com/shorts/SWHZolxKdVU]             │
├──────────────┬──────────────────────────┬──────────────────────────────────┤
│ SharedVM   ▸ │ Trader-The_setup_I…sstr  │ Inspector                        │
│ Archive    ▸ │ jawed-Me_at_the_zoo…sstr │ Me at the zoo                    │
│ Queue      ▸ │ jawed-Me_at_the_zoo…txt  │ jawed · 2005-04-23 20:31 -0700   │
│              │ Berkman-William_Fish…sstr│ video/mp4 · 692,231 bytes        │
│              │                          │ License: not stated              │
│              │                          │ verify: 2 checkpoints good,      │
│              │                          │ signed by demo@copal (trusted)   │
├──────────────┴──────────────────────────┴──────────────────────────────────┤
│ Transcript  10:03:36 done: jawed-Me_at_the_zoo_jNQXAC9IVRw.sstr + transcript│
├────────────────────────────────────────────────────────────────────────────┤
│ Services: p Play  P Paced  s Serve  v Verify  x Export MP4  t Text  a Armor│
└────────────────────────────────────────────────────────────────────────────┘
```

## The steps

| Step | Delivers | Done when |
|---|---|---|
| **3a** | the frame and the **Browser**: Miller columns over folders, starting at `ARCHIVE_DIR`; the terminal, the layout, the keys that move | at three window sizes the columns show what `ls` shows, in the same order, with the same selection after the same keys; nothing is drawn outside the frame |
| 3b | the **Inspector**: a capture's header, notes and license, `verify`'s summary, a video's notes, a folder's count | for every file in a fixture folder, what the Inspector shows equals what `sstr verify` and the file's own header say, field for field |
| 3c | the **Transcript**: `ytq.log` and sstr's own events in one pane, following as they are written | lines appear in the order the log has them, with the same text ytq wrote; a rotated `ytq.log.1` does not lose the line at the seam |
| 3d | **Services**: Play, Paced, Serve, Verify, Export MP4, Open as Text, Armor, Queue — each printed in the Transcript as a command line before it runs | the acceptance test above: every Service equals its own printed command line, run in a shell |
| 3e | the **Shelf** and the **Queue**: items kept for later, and ytq's queue as one more object to browse, inspect and send Retry or Forget | a queue entry inspected and retried through the Workspace leaves `queue.json` exactly as `ytq` doing the same leaves it |

## Deviations, written down as they are made

- **ASCII markers, not box art.** The report's screen (V-B) is drawn with box
  characters, and a folder is marked `▸`. The Browser uses `>` for a folder and
  `|` for the rule between columns instead. A column is compared with `ls`
  character by character, and a glyph of ambiguous or double width makes that
  comparison a lie on some terminals -- the check would be measuring the font,
  not the Browser. The frame follows ytq's window instead: a reverse-video
  title bar and dim rules, which is the house style step 2d already set.
- **A row can hold several pieces now.** `term::Row` was one string in one
  style, which is all ytq's window ever needed. The Browser puts three columns
  on a row and reverses the selection inside one of them, so a row became a
  list of pieces. A row of exactly one piece is written exactly as before, and
  `make ytq-window-crosscheck` still agrees on all 133 comparisons.
- **`cut` and `ljust` moved from `ytq::window` to `ytq::term`.** Both windows
  need them, and they are the terminal's business. Their test moved with them.

## Decisions already made

- **The terminal layer is ytq's.** Step 2d wrote `ytq::term` — raw mode via
  `stty`, escape sequences, `TIOCGWINSZ`, keys on a thread — and it is the
  Workspace's too. No curses, no crates, as ascitty's `ascitty-tty`.
- **The Workspace is a way of looking.** The Browser reads the disk; the
  Inspector reads a file's own header and notes. There is no database and
  nothing to keep in step, which is what stops this being a media library.
- **The folders stay the structure.** A capture is a file, its transcript is
  the file beside it, and the archive folder is one the Mac also sees.
- **One queue.** The Queue is ytq's `queue.json` under the same `queue.lock`,
  not a second one. Sending Retry is what `ytq` itself does.
- **Every Service is echoed before it runs, not after.** The Transcript is a
  record of what was asked, so a Service that fails still leaves the command
  line that a person can rerun and see the failure for themselves.
