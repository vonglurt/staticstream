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
| **3d** | **Services**: Play, Paced, Serve, Verify, Export MP4, Open as Text, Armor — each printed in the Transcript as a command line before it runs | the acceptance test above: every Service equals its own printed command line, run in a shell. **Done**: `make workspace-check`, 47 of 47 -- Verify, Export and Serve each driven by key, their printed line read off the screen and run in a shell with a fresh HOME, and what the two left compared. `make check` is 340. Queue moves to 3e, with the rest of the queue |
| **3e** | the **Shelf** and the **Queue**: items kept for later, and ytq's queue as one more object to browse, inspect and send Retry or Forget | a queue entry inspected and retried through the Workspace leaves `queue.json` exactly as `ytq` doing the same leaves it. **Done**: `make workspace-check`, 63 of 63 -- Retry and Forget sent from the Workspace leave the queue that `ytq retry` leaves in a shell, that `r` leaves in ytq's own window, and that `r` leaves in the **Python** ytq's window. `make check` is 356 |

**Phase 3 is done.** `sstr-workspace` draws the report's screen: the Browser,
the Shelf, the Inspector, the Transcript, the Services line and the Queue.
`make check` is 356 -- 293 from phases 1 and 2, and 63 checks of the
Workspace -- with 118 unit tests beside it.

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

### Step 3d

- **A Service is a string, built once, printed, then handed to `sh -c`.** Not
  an argv the Workspace assembles twice -- once to show and once to run --
  because two constructions drift and only one of them is the one a person
  reads. What runs *is* what was printed, and the acceptance test is
  therefore about whether that line does what the pane claims, not about
  whether the Workspace was honest in reporting it.
- **The line is said before it runs, and it is said whatever happens.** A
  Service that fails leaves behind the line to retype; a record of what was
  asked can be acted on, and a record of what happened cannot.
- **Every Service that prints takes the terminal.** There was a third kind at
  first -- run it captured, put the output in the Transcript -- and Verify and
  Export were both that kind, on the reasoning that a report belongs in a log.
  `sstr verify` writes eleven lines and `sstr play -o` writes eleven; the band
  is three; and what those eleven pushed out of it was the command line, which
  is the one line the band exists to show. The kind is gone rather than
  narrowed. The Transcript keeps the record -- the line, and how it went --
  and output goes where output goes.
- **The command line is echoed on the terminal too, above its own output**, as
  a shell shows what was typed above what it printed. Leaving the alternate
  screen puts back the ordinary one, which still holds the last Service's
  output; without the line between them there is nothing to say where one ends
  and the next begins. The check reads that mark.
- **A Service's standard input is closed, and a text capture is shown rather
  than paged.** The report says "shown in a pager". The window's key reader
  owns the real stdin for as long as the window lives and cannot be paused, so
  handing the same terminal to a pager would be two readers racing for every
  byte typed. What happens instead: the output goes to the terminal, and the
  Workspace waits for one key before drawing itself again.
- **Export never writes over a file.** `sstr play cap.sstr -o cap.txt` is what
  a text capture wants to be called, and `cap.txt` is exactly what ytq calls
  the transcript sitting beside `cap.sstr`. A person typing that line can see
  what they are about to destroy; someone pressing `x` cannot. So the line
  names a free one -- `cap-1.txt` -- and shows it before running.
- **Export says what it will write**: `x Export MP4` on a video, `x Export
  TXT` on a transcript, from the capture's own `content_type`.
- **PLAYER and SERVE come from the report's settings table** (V-D), and Play
  on a video is `sstr play cap.sstr | mpv -` -- a pipeline, which is still
  something a person could have typed. `sstr play` writes the payload to
  standard output, and an MP4 is not something to do to a terminal.
- **No Service may take a key the Browser moves with.** The keys that move are
  read first, so a clash would not misfire; it would make the Service
  unreachable, with nothing to say so. A unit test walks every Service of
  every kind of selection and asserts none of them takes `hjklq`.
- **The quoting is the boundary, because the line goes to `sh`.** ytq writes
  names with spaces and apostrophes, and a file called `; rm -rf x` is one a
  person can make. `services::quote` is checked by running its output through
  a real shell and asking what word arrived, and the harness records a capture
  called `Don't Look Up.sstr` and sends it Verify on the real screen.
- **The check runs at 200 columns**, and that is not cosmetic: a line is drawn
  cut to the pane, `sstr play LONG -o LONG` names the path twice, and at 110
  columns the check would be reading half a command line and then running it.
  The Workspace is asked what it printed at a size where the answer is whole.
- **`awk -v` is not the way to hand a mark to awk.** It runs escape processing
  over the value, so the `\'` in `'Don'\''t Look Up.sstr'` -- the whole point
  of that capture -- lost its backslash before the comparison and the mark
  never matched. `grep -F -x` compares the bytes.
- **One of these checks was vacuous, and breaking the code is what found it.**
  "the command line is said before the outcome" asked whether the first of two
  matches preceded the second, which is true however they are ordered; it
  passed against a Workspace altered to say the line *after* the outcome. Each
  row is now found by what it is. The same pass caught a Workspace with the
  quoting removed, in three other checks.

### Step 3e

- **Retry and Forget became `ytq` commands.** They were keys in ytq's window
  and nothing else, so the Workspace had no line to print for them -- and 3d's
  rule is that a verb is always something a person could have typed. Rather
  than invent a line no program answers to, the verbs ytq already performs
  were given the names they already had: `ytq retry URL...` and
  `ytq forget URL...`. Both, and the window's `r` and `d`, now go through
  `queue::retry` and `queue::forget`. **The surest way to make two things
  agree is for there to be one of them.**
  This is an addition to ytq beyond the Python one, and a deliberate one: it
  takes nothing away, the existing crosschecks are untouched by it, and the
  alternative was a Service that was not a command line.
  *(Later, and for the same reasons, `ytq -V | --version`: the Python has none,
  `sstr` and `sstr-workspace` both do, and a program should be able to say
  which program it is. It answers before the settings are read, so it works on
  a machine with no HOME.)*
- **THE FOURTH WAY IS THE ONE THAT COUNTS.** The check compares the queue the
  Workspace leaves with the queue `ytq retry` leaves in a shell and the queue
  `r` leaves in ytq's own window -- and all three go through the same
  function, so a comparison among them is self-consistent and would stay green
  if that function were wrong. So the fourth is the **Python** ytq, frozen at
  `tests/reference/ytq.py`, whose window has had `r` and `d` all along. It is
  the only one of the four that cannot change when this crate does.
  This is not a worry about a thing that might happen. `queue::retry` was
  altered to stop clearing the entry's error, and **the three Rust
  comparisons stayed green while only the Python one went red.** That is the
  whole argument for the method, run as an experiment.
- **A queue entry's Service names it by URL, and acts on this HOME's queue.**
  A Service on a file names the file absolutely and means the same thing from
  any home, which is why the check runs those in a fresh one. `ytq retry URL`
  has no path in it: the queue it is an entry of is the one under `$HOME`.
  The check learnt this the way these things are learnt -- it ran the shell
  side under a fresh home and spent a while comparing a retried queue with an
  untouched one.
- **The Queue is opened with a key, not drawn as a row among a folder's
  entries.** The report's screen has `Queue` in the first column beside
  `SharedVM` and `Archive`. A column over a folder being exactly what `ls`
  would have shown is the bar that replaces phase 3's crosscheck, and a
  synthetic row in a folder's column retires it. Shift-Q pushes the Queue as a
  column of its own, and Left backs out of it; `q` still leaves, because a
  queue is not worth losing a window over.
- **The Queue column is in `queue.json`'s order, which is `ytq list`'s** --
  not the window's, which sorts by state. Both orders are ytq's; the column is
  checked against `ytq list` because that is the one a person can print.
- **A queue row is cut, not elided.** `elide` keeps a name's extension because
  that is what tells two captures apart. A status and a title have no
  extension to keep.
- **The Inspector draws a queue entry with `live_view`**, ytq's own renderer
  for the live record, for the same reason it draws `sstr verify`'s lines for
  a capture: one renderer, so the pane and the command cannot drift.
  `attempts` is a number and `queue::text` is for strings -- it rendered as
  nothing until that was noticed, which is how a field goes missing quietly.
- **The Shelf is kept in memory, for as long as the window is open.** A file
  of it would be the one thing this Workspace has refused all the way
  through: something to keep in step with the folders, stale the moment a
  picked capture is renamed by something else. The Browser reads the disk; the
  Shelf holds what was picked while looking.
- **A Service goes to everything on the Shelf**, each as its own command line
  in the Transcript, and to the selection when the Shelf is empty. That is
  what "items picked for later" is for -- later being when a verb is sent. The
  Services line says `Services to the Shelf:` so the keys never lie about
  where they point, and it offers a key that **any** of the shelved items
  takes, named once.
- **The Transcript is polled before a Service's outcome is said.** It cannot
  follow `ytq.log` while a Service holds the screen, so `Retry ok` landed
  above ytq's own line about what it did -- with an earlier timestamp than the
  line above it, which reads as a fault in the clock rather than in the order.
- **The check holds `run.lock` while driving ytq's window.** The window takes
  that lock and downloads whatever is queued, which is what it is for and
  exactly what this comparison must not include: left alone it retried the
  entry and then downloaded it with the stand-in yt-dlp, and the check was
  comparing a retry against a retry and a finished download. Holding the lock
  is the ordinary state of another ytq already downloading. `flock(1)` and ytq
  both use `flock(2)`, so they contend; fcntl locks would not have.
- **The window sorts by state and the Workspace does not**, so the fixture's
  second entry is `rejected` rather than `done`: `ORDER` puts `done` above
  `failed` and `rejected` below it, and with a `done` entry the window's first
  row was the wrong one and `r` retried it.
- **Python writes `1700000000.0` where Rust writes `1700000000`**, and the
  Python comparison normalises whole floats as
  `tests/ytq-runner-crosscheck.sh` has since phase 2. The Rust-against-Rust
  comparisons are left byte for byte, which is a stronger statement and costs
  nothing.

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
