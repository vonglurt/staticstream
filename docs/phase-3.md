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
| **3a** | the frame and the **Browser**: Miller columns over folders, starting at `ARCHIVE_DIR`; the terminal, the layout, the keys that move | at three window sizes the columns show what `ls` shows, in the same order, with the same selection after the same keys; nothing is drawn outside the frame. **Done**: `make workspace-check`, 16 of 16; commit `d6826e1` |
| **3b** | the **Inspector**: a capture's header, notes and license, `verify`'s summary, a video's notes, a folder's count | for every file in a fixture folder, what the Inspector shows equals what `sstr verify` and the file's own header say, field for field. **Done**: `make workspace-check`, 23 of 23 -- the capture's pane compared line for line with `sstr verify` over a capture `sstr` itself recorded. `make check` is 316 |
| **3c** | the **Transcript**: `ytq.log` and sstr's own events in one pane, following as they are written | lines appear in the order the log has them, with the same text ytq wrote; a rotated `ytq.log.1` does not lose the line at the seam. **Done**: `make workspace-check`, 34 of 34 -- the band held against a log a real `ytq` wrote, and followed across the rename `ytq` itself makes past 4 MiB. `make check` is 327 |
| 3d | **Services**: Play, Paced, Serve, Verify, Export MP4, Open as Text, Armor, Queue — each printed in the Transcript as a command line before it runs | the acceptance test above: every Service equals its own printed command line, run in a shell |
| 3e | the **Shelf** and the **Queue**: items kept for later, and ytq's queue as one more object to browse, inspect and send Retry or Forget | a queue entry inspected and retried through the Workspace leaves `queue.json` exactly as `ytq` doing the same leaves it |

## Deviations, written down as they are made

### Steps 3a and 3b

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
- **A name too long for its column loses its middle, not its end** -- and `..`
  does the losing, where the report draws an ellipsis. The report is right
  about the shape: at 100 columns `jawed-Me_at_the_zoo_jNQXAC9IVRw.sstr` and
  the `.txt` beside it are the same 29 characters from the left, so cutting
  the end drew a capture and its transcript identically, on exactly the names
  ytq makes. What tells them apart is the end, so the end is kept:
  `jawed-Me_at_the_zoo_jNQ..sstr` and `jawed-Me_at_the_zoo_jNQX..txt`. It is
  `..` and not `…` for the reason the folder marker is `>`: U+2026 is
  ambiguous-width, and a column is compared with `ls` character by character,
  so an ambiguous glyph would have the check measuring the terminal's font
  instead of the Browser.
  The **Inspector's heading is the same name and loses its middle the same
  way** -- cut at its end, `jawed-Me_at_the_zoo_jNQXAC9IVRw.sstr` read
  `...IVRw.ss`, which is a different extension as far as a reader can tell.
  The lines under the heading are `sstr verify`'s prose, not names, and are
  cut where the pane ends.
- **The elide rule is written twice**, once as `workspace::elide` and once in
  shell in `tests/workspace-check.sh`, which builds what it expects from `ls`.
  That is duplication on purpose: the comparison IS the agreement between
  them, so a divergence fails the check loudly instead of passing quietly. It
  is written down here because it is the kind of thing that otherwise costs
  someone an hour.
- **Three checks say `no two entries draw alike`**, one per window size. That
  is the defect above asserted on the real screen rather than in the drawing
  code, and the fixture holds the `.sstr` and `.txt` pair that provoked it.
- **Look at the screen, not only at the check.** Both elision defects were
  found by capturing a pane with tmux and reading it, while every check was
  green. That is inherent to the shape of this test rather than bad luck: the
  harness re-implements `elide` in shell, so Browser-against-harness is a
  self-consistent comparison, and self-consistency cannot catch both sides
  being wrong in the same direction. Comparing with `ls` anchors what a column
  *contains* to something outside the program; nothing outside it anchors how
  a column is *drawn*. So a capture read by eye is part of the step, not a
  courtesy at the end of it.

### Step 3c

- **The band is three rows where there is room, not the report's one.** The
  report's screen draws the Transcript as a single row, and it is drawn that
  way because nothing is happening on it: one `done:` line is all there was to
  say. A download says more, and Smalltalk's Transcript is a pane that
  scrolls. Three rows is enough to see a line, the one before it and the one
  after -- which is what makes "in the order the log has them" something a
  person can read rather than infer. Below 20 rows the band keeps the
  report's single row; a 12-row terminal is for the Browser.
- **A line is drawn as `transcript::shown` draws it: the time, then the
  message.** Here the report is right and the first attempt was wrong. That
  attempt drew the line exactly as ytq wrote it, arguing that the
  done-condition says "the same text ytq wrote" and a pane that reformats
  cannot be held to it. At 50 columns the result was
  `2026-09-16 08:02:52 [13870] nothing is` -- twelve characters of label,
  twenty-eight of stamp, ten of message. The report's `10:03:36 done: ...` is
  the right shape: the date is the same all day and the pid belongs to a file
  several ytq processes share, not to a pane showing three lines. What is
  **kept** is still the line as ytq wrote it, so the comparison still has the
  file to be anchored to; only the drawing is short.
- **The follower remembers the handle, not the offset.** `ytq.log` is moved to
  `ytq.log.1` by a plain `std::fs::rename` past `LOG_MAX`, and an offset into
  a file that no longer wears that name is how the lines at the seam go
  missing. So every poll drains the descriptor it already holds -- a rename
  does not move an open file, and the lines written before it are still there
  to be read -- and only then asks, by device and inode, whether the path is
  still that file. A different one, or a length below where we are reading,
  means reopen at nought and drain that too, in the same poll. The seam is
  not a gap in time.
- **The seam is checked by a unit test and the rename by the harness**, and
  neither could do the other's job. The line at the seam is one written to the
  old file between the last poll and the rename; placing it in that window
  from a shell is not something a check can be made to do reliably, and
  `a_rotation_loses_nothing_at_the_seam` in `src/workspace/transcript.rs`
  places it exactly. What the harness does instead is drive a **real** `ytq`
  over a **real** 4 MiB log and watch the pane carry on across the rotation
  `ytq` itself performs -- which is the half that no unit test can claim.
- **A burst is read in proportion to its bytes, not to their square** -- and
  this was a defect, found by reading a pane. The first follower appended each
  block to a buffer and took lines off the front of it, so every line shifted
  everything behind it. A rotated log's worth is about 70,000 lines and some
  200 GB of copying: on screen the Workspace simply stopped, mid-poll, with
  the pane showing whatever it had. `make workspace-check` was green
  throughout, because nothing in it had yet written a log that big. It now
  does, and `a_rotated_logs_worth_of_lines_is_read_in_proportion_to_its_bytes`
  bounds it besides.
- **The Transcript's first line is the command line that opened the
  Workspace** -- `sstr-workspace ~/Videos/Archive`. Services (3d) has to hold
  to "a verb is always something a person could have typed"; starting as the
  pane means to go on costs nothing and gives the band something true to show
  on a machine where ytq has never run.
- **`transcript_rows` and `shown` are each written twice**, once in Rust and
  once in shell in `tests/workspace-check.sh`, for the reason `elide` is: the
  comparison IS the agreement between them, so a divergence fails loudly
  rather than passing quietly.
- **Each test got its own fixture folder.** Not 3c's work, but 3c's tests are
  what made it show: every test in `workspace::tests` used one directory named
  after the process, cargo runs them in parallel, and each ends by removing
  what it made -- so a test could have its fixture deleted under it by a
  neighbour that had just finished. It failed about one run in ten, which is
  the worst rate for a test to fail at.

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
