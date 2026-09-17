<!-- SPDX-License-Identifier: MIT — Copyright (c) 2026 Paul Richeson -->

# The architecture of staticstream

One crate, four modules, three binaries, one queue file and one file format.
This describes what is there today, the agreements that hold it together, and
the one seam the comments work is meant to go through.

It is written outside in: what a person touches first, then what is under
that, and so on down to the bytes. Each layer gets its own section, and the
last two sections are the two things that cross every layer — the data flow,
and the contract with yt-dlp.

---

## 0. The shape, in one view

```
              THE PERSON
   keybinding   shell    ytq window    Workspace
       │          │          │             │
       └──────────┴────┬─────┴─────────────┘
                       │
                  ┌────┴─────┐
                  │   ytq    │  the queue, the settings, the runner
                  └────┬─────┘
                       │  drives                    reads/writes
              ┌────────┴────────┐            ┌──────────────────┐
              │     yt-dlp      │            │   queue.json     │
              │  (not ours)     │            │  + two locks     │
              └────────┬────────┘            └──────────────────┘
                       │  leaves an MP4
                  ┌────┴─────┐
                  │  format  │  record, RS(255,223), P+Q, sign, verify
                  └────┬─────┘
                       │
                  ┌────┴─────┐
                  │   disk   │  Author-Title_ID.sstr  + .txt
                  └──────────┘
```

`tty` (the armor) sits beside `format` and is reached only by `sstr armor`,
`unarmor` and `recv`. `workspace` sits beside `ytq` and reaches down into both
`format` and the queue, and is the only module that is purely a way of
looking.

**The stacking rule, from `lib.rs`: `format` knows nothing above it and holds
no I/O policy.** It does not know what a download is, where `ARCHIVE_DIR` is,
or that yt-dlp exists. Everything about *why* a file is being recorded lives
in `ytq`. This is the single most load-bearing boundary in the crate, because
it is what lets the format be checked against a frozen Python prototype
without any of the queue being involved.

---

## 1. Outside in: the four faces

The same program is reached four ways. They are not four programs, and they
are not a core with three wrappers — they are four ways of sending the same
messages.

### The keyboard

| Chord | What happens |
|---|---|
| Super+Shift+Y | `ytq clip` — whatever is on the clipboard is read for video links, each queued once |
| Super+Shift+A | `sstr-workspace` — the Browser over the archive, queue included |

Nothing else. The keyboard is the thinnest face and deliberately so: one key
to capture, one key to look. Both are guarded — until something on PATH
answers to the name, the key does nothing.

### The shell

```
ytq clip | add URL… | run | status | cookies | list | clear
ytq transcript URL | ytq --help
sstr record | play | verify | armor | unarmor | recv | paths
sstr ytq …                     the same program under another name
sstr-workspace [DIR]
```

**Everything the other three faces do, this face can do.** That is not a
courtesy; it is the check. The Workspace's Services are command lines, and the
test that they are correct is that they can be pasted into this face and mean
the same thing.

### The window (`ytq` with no arguments)

A full-screen list of the queue with a live line above it. While it has focus
every URL copied is queued. Keys: `h` twice to forget history, `c` to retry
through Brave's cookies, `d` to send yt-dlp at the selection now.

It is a **view of `queue.json`**, not an owner of it. Two windows, a `ytq run`
and a background runner can all be open at once and none of them is the
authority — the file is.

### The Workspace (`sstr-workspace`)

The NeXTSTEP face: a **Browser** in columns over a folder of streams, a
**Shelf** to put a selection down on, an **Inspector** that shows what
`sstr verify` says about the selection *in the same words*, a **Transcript**
that follows the log, and **Services** sent to the selection with one key.
`Q` brings ytq's queue in as one more column to browse.

It owns nothing and keeps no database. The Browser reads the disk; the
Inspector reads a file's own header. There is nothing to keep in step,
which is why there is no synchronisation anywhere in this module.

---

## 2. The layers, and what each is allowed to know

| Layer | Module | Knows about | Must not know about |
|---|---|---|---|
| Interface | `ytq::window`, `ytq::term`, `workspace` | the queue file, the archive folder, the terminal | how a record is laid out, how yt-dlp is invoked |
| Policy | `ytq::cli`, `ytq::settings` | what the person asked, what the config files say | terminals, escape codes |
| Work | `ytq::runner`, `ytq::queue` | yt-dlp, locks, the queue file, `format`'s writer and reader | which key was pressed |
| Format | `format::*` | records, parity, signatures, bytes | files' meanings, folders, settings |
| Serial | `tty` | base64url lines, CRC, erasures | everything else |

`ytq::sys` is the thin floor under all of it — clocks, signals, `kill`,
`strftime` — so that the layers above can be tested with time and signals
supplied rather than real.

**Why `urls.rs` is its own module and contains no regex.** Which text counts
as a video is *specification*, not convenience: it decides what a keypress
captures. It is written as a hand-matched transliteration of the Python
ytq's expressions, step for step in the order Python's engine tries them,
because the crate takes no dependencies and because an equivalent-looking
regex is not the same thing as the same regex.

---

## 3. The agreements

These are the promises each layer makes to the others. Everything else in the
design is negotiable; these are not, because other parts are correct only
while they hold.

**A1 — One file is the queue.** `~/.local/share/ytq/queue.json`. Every change
takes `queue.lock` (`flock`), **reads the file afresh**, changes only what it
came to change, and writes it back. No process keeps its own copy. This was
learned the hard way: a private copy meant a `ytq add` made during a download
vanished at the runner's next save.

**A2 — Exactly one process downloads.** Whichever holds `run.lock`, with its
pid written inside. A background runner releases it *while still holding*
`queue.lock`, so a `ytq clip` that adds a URL and then looks for a runner
under that same lock can never look in the gap between the two.

**A3 — The queue file is byte-compatible with Python's `json.dump(indent=1)`.**
This is why the Rust and Python ytq can share one queue, and it is the chain
back to the specification. It is checked, not assumed.

**A4 — Nine states, and `cookies` is not work.** An entry is one of
`downloading, cookies, retry-cookies, checking, queued, retry, done, failed,
rejected`. Three are history (`ytq clear` forgets those); five are pending.
`cookies` is in neither list, because it is waiting on a *person*, and a
runner that treated it as work would loop against a sign-in page forever.

**A5 — The original is deleted only after the copy has been proved.** The MP4
goes only when the capture has been written, read back, verified, and its
played-back payload's SHA-256 equals the original file's. Anything short of
that keeps the MP4 and says why in the log. This is the agreement that makes
`OUTPUT=sstr` safe to have as the default.

**A6 — A capture is self-describing.** Source URL, licence, title and author,
creation time, content type, and the FEC parameters are in the capture's own
header. A capture separated from this program, this queue and this machine
still says what it is.

**A7 — Every Service is a command line, printed before it runs.** Not an argv
built twice; the string that was printed is the string that runs. Absolute
paths, shell-quoted, no reliance on the Workspace's directory — so it means
the same thing pasted into a terminal tomorrow. *A record of what was asked is
worth more than a record of what happened, because only one of them can be
acted on.*

**A8 — The crate takes no dependency outside this repository.** `make deps`
proves it from `Cargo.lock`. zlib, SHA-256, CRC-32 and JSON are written here.
This is what lets one `cargo build` produce a static binary for a Pi 2B.

**A9 — A step that can fail alone must fail alone.** See §6.

---

## 4. Data flow: one URL, end to end

```
  clipboard text
        │  urls: every video link, each once
        ▼
  ┌──────────────┐   A1
  │  queue.json  │◄──────── ytq clip / add / window / Workspace
  └──────┬───────┘
         │  a runner takes the lock            A2
         ▼
  ① CHECK      yt-dlp --simulate --print '%(title)s\t%(height)s\t%(ext)s'
         │     ← can it be got, and at what height?   → rejected, or queued
         ▼
  ② DOWNLOAD   yt-dlp  NAME_OPTS + META_OPTS + progress template
         │     → PROGRESS …   every line, into the live record
         │     → FILE  …      the path after the move
         │     → NOTES  {…}   metadata as JSON
         ▼
     Author-Title_ID.mp4
         │
         ▼
  ③ ARCHIVE    format::writer  →  .sstr   (RS inner, P+Q outer, signed)
         │     format::reader  →  read back, verify, compare SHA-256
         │                                                        A5
         ├─ identical ──→ delete the MP4   (OUTPUT=sstr, the default)
         └─ anything else → keep the MP4, log why
         ▼
  ④ TRANSCRIPT  a SECOND yt-dlp run          A9
         │     → STEM …   → META {…}   → .vtt per language
         │     vtt → plain text, notes on top
         ▼
     Author-Title_ID.txt
         │
         ▼
     status: done          the live record cleared, the log written
```

Two flows are not on this diagram because they do not touch the queue at all:
`sstr` on a capture directly, and the Workspace's Services, which are that
same `sstr` reached with one key.

**The live record** is the thread that makes this legible while it happens: a
small object updated as yt-dlp's lines arrive, written into the entry at most
once a second (because every write is a turn at `queue.lock` that a
`ytq clip` may be waiting for), and read back by the window, by `ytq status`,
and by the Workspace's queue column — whichever happens to be looking.

---

## 5. The contract with yt-dlp

yt-dlp is **not a library here and never becomes one.** It is a program driven
over an argv and read back over stdout. That is the whole interface, and it is
worth being precise about, because it is the only place this crate depends on
something it does not control.

### What we send

| Group | Purpose |
|---|---|
| `NAME_OPTS` | the filename rule: first word of the uploader, title, id, in the URL-safe base64 alphabet and nothing else |
| `META_OPTS` | the same notes written into the MP4's own metadata, when ffmpeg is there |
| progress template | one `PROGRESS` line per update, fields `\|`-separated in a fixed order |
| `--print` | `FILE`, `NOTES`, and on the transcript run `STEM` and `META` |

### What we read

**yt-dlp answers in tagged lines, and that is a protocol.** Every line of its
stdout is one of:

```
PROGRESS  pct|got|total|est|speed|eta|elapsed|frag|frags|fmt|vcodec|acodec|height|state
FILE      /abs/path/after/the/move.mp4
NOTES     {"webpage_url":…,"license":…,"title":…,"uploader":…,"channel":…}
STEM      /abs/path/without/extension        (transcript run)
META      {"title":…,"timestamp":…,"subtitles":…,…}   (transcript run)
[MetadataParser] …                            deliberately ignored
anything else                                 → the log, and the live "last" line
```

**Two hard-won points about this contract**, both of which are in the report
and both of which cost a day to find:

1. **`--print` puts yt-dlp in quiet mode**, and quiet mode hides the progress
   lines. Asking for a `FILE` line silently cost us every progress line, and
   the window sat blank through a three-minute merge. `--progress --no-quiet`
   asks for them back.
2. **An unknown line is never an error.** It goes to the log and to the live
   record's `last` field. A parser that insisted on knowing every line would
   break on the next yt-dlp release; one that keeps the last six lines as
   context for a failure message does not.

### Where it is allowed to change under us

Everything in that table is a *format string we send*, so a yt-dlp change
breaks it loudly and in one place. The one thing we read structurally is the
JSON of `NOTES` and `META`, and every field is picked with a
"present and truthy, else a stated default" helper — so a field that
disappears upstream degrades to `unknown` or `not stated` rather than to a
panic.

---

## 6. The seam: why transcripts work, and how comments ride the same rail

This is the agreement the comments work depends on, so it gets its own
section.

**A9 — a step that can fail alone must fail alone.**

The transcript is a **second yt-dlp run, after the video's**, and the reason
is exact: a caption download that fails makes the whole run exit non-zero,
`-i` or not, *even though the video was written*. Had captions been asked for
in the download run, a 429 on a subtitle — and YouTube answers 429 to captions
far sooner than to video — would have put a finished 200 MB download back in
the queue to be fetched again.

So the shape is:

```
   download  ──→ succeeds or fails on its own terms
        │
        └─→ transcript ──→ succeeds or fails on its own terms
                            and says which, in the "done" message:
                            "done: … + transcript"  /  "… (no transcript)"
```

The download's outcome is already decided and already written when the
transcript is attempted. **That is the seam**, and it has three properties
worth naming, because they are exactly what was asked for:

- **Isolated.** Nothing after the archive step can turn a finished download
  into an unfinished one.
- **Editable.** It is one function, with its own command line, its own
  timeout, and its own entry in the log.
- **Removable.** `SUBS=` in the config turns it off entirely and nothing else
  changes.

### Comments go on this rail, not beside it

The rule that follows: **the comment fetch is its own run with its own
failure**, not an extra flag on a run that already matters.

It is tempting to add `--write-comments` to the transcript run, since one is
already happening and the metadata line is already being parsed. It is the
wrong call, for the same reason the transcript is not on the download run: a
comment fetch is the slowest and most rate-limited thing yt-dlp does here, and
attaching it to the transcript would mean a 429 on comments costs you the
captions. The ladder only works if each rung is allowed to fall on its own.

| Step | Fails how | Costs what |
|---|---|---|
| download | on its own | everything after it |
| archive | on its own | the MP4 stays, and says why |
| transcript | on its own | `(no transcript)` in the done line |
| **comments** | **on its own** | **`(no discussion)`, and nothing else** |

What we take, where it exists:

- **the text** of each comment
- **the author** — the display name and the stable id, because a display name
  can be changed afterwards and the id cannot
- **when** it was written
- **what it answers** — `parent`, `'root'` or another comment's id, which is
  the difference between a discussion and a list
- **whether it is pinned**, which is the uploader's own mark on it
- **its like count**, with the same stamping caveat as every other count

Capped, and off by one setting, exactly as `SUBS` is.

**What cannot be taken, stated plainly:** X reply *text* is not available from
yt-dlp at all — the extractor has no comment extraction, `--write-comments`
returns nothing, and the public syndication endpoint returns a conversation
*count* and no replies. On an X post the count is recorded and the discussion
is absent. That is a hole with a name, not a silence.

---

## 7. The reference card

Every download carries a card saying where it came from and when it was taken.
It is written in **three places, deliberately**, because each survives a
different accident:

| Where | Survives | Lost when |
|---|---|---|
| `Author-Title_ID.txt` | being read by anything at all, forever | the file is deleted or separated |
| the MP4's own metadata | being copied, renamed, sent | the container is transcoded away |
| the `.sstr` header | everything above; it is the record | nothing short of losing the capture |

The card answers what a citation needs — **who published it, its full title,
when it was published, the URL, and when you retrieved it** — because a video
can be retitled, edited, made private or deleted after you saw it, and the
retrieval date is the only part of that a reader cannot reconstruct.

To that, this phase adds the counts, and the counts change the card's
character in a way worth stating:

> Title, author, published date and licence are properties of the **thing**.
> Views, likes, reposts and replies are properties of a **moment**. They are
> different the second after they are read.

So they are stamped with their own reading time, separate from `Downloaded:`
— two rows that are usually equal being cheaper than one row that is
sometimes a lie. A stamped count is a record; an unstamped one is a rumour.

---

## 8. Integrity: from a byte to a signature

Four mechanisms, at four scopes. They are separate on purpose — each one
catches a thing the others cannot, and each can be absent without disabling
the rest.

| Scope | Mechanism | Catches |
|---|---|---|
| a record's header | CRC-32 over the 28 bare header bytes, stored beside them | a header corrupted into a plausible one |
| a record's body | `body_crc` in that header | a body that decoded to the wrong bytes |
| a record's bytes | RS(255,223) inner code, interleaved | bit errors, corrected without anyone being told |
| a group of 16 | the P+Q outer rows of version 1 | **two whole records lost outright**, rebuilt |
| the whole stream | a hash chain, checkpointed and signed | any edit anywhere, including a re-ordering |
| the payload | its SHA-256, written into the capture | the capture being a faithful record of the wrong file |

### The chain

Each record has a hash of its own header and body:

```
record_hash   = SHA-256( 28-byte header ‖ plain body )
```

Those are linked, so a checkpoint commits to everything before it:

```
genesis       = SHA-256( "sstr-v0 genesis"    ‖ stream_id )
checkpoint(n) = SHA-256( "sstr-v0 checkpoint" ‖ checkpoint(n-1)
                         ‖ for each entry: seq ‖ record_hash )
```

A checkpoint is written every 64 records or 10 seconds, whichever comes
first. **Changing any record changes every checkpoint after it**, which is
what makes the chain worth more than a per-record hash: it fixes the *order*
and the *set*, not only the contents.

### The signature

The last link is signed, and this is where the no-dependency rule shows its
teeth:

```
message  = "sstr-v0 " ‖ stream_id ‖ checkpoint_digest
signed   = ssh-keygen -Y sign,     namespace  sstr@copal
verified = ssh-keygen -Y verify -n sstr@copal -f <allowed_signers>
```

**We shell out to `ssh-keygen` rather than vendoring Ed25519.** That is the
honest consequence of A8: the crate will not take a crypto dependency, and
writing our own signature scheme would be far worse than using the one every
machine already has. It buys three things beyond the bytes:

- the key is **the one already on the machine** — `SSTR_KEY` defaults to
  `~/.ssh/id_ed25519` when there is one, so signing is on without anybody
  configuring it;
- the signature is an **SSHSIG**, a format other tools can verify without
  this program;
- the **namespace** `sstr@copal` means a signature made here cannot be
  replayed as a git commit signature, or the reverse.

Signing is optional and its absence is stated, not hidden: `SSTR_KEY=` turns
it off, a machine with no key records unsigned, and the log says which of the
two happened on every archive. Verification is likewise opt-in — without an
`allowed_signers` file, `sstr verify` checks every hash and says the signature
was *not* checked, rather than quietly calling an unverified capture good.

### And the one that guards the deletion

Separately from all of the above, `archive()` computes the SHA-256 of the
**original file** and of the **played-back payload**, and compares them. This
is the agreement A5 rests on, and the distinction matters: the chain proves
the capture has not been altered *since it was written*; this comparison
proves it was written *from the right bytes in the first place*. Only the
second one earns the right to delete the MP4.

---

## 9. The configuration, as Copal sets it, and what lands on disk

Nothing below was typed into this machine except one line. Everything else is
a default doing its job — which is the design working, but it also means the
resolved state is not visible in any file, so it is written out here.

### What is actually on the disk

```
~/.config/copal/media.conf     — does not exist
~/.config/ytq/config           — one line:  DIR=~/Downloads/SharedVM
~/.config/ytq/auto             — exists, empty
```

### How a setting is resolved

```
   built-in defaults
        ↓  overridden by
   ~/.config/copal/media.conf      shared by sstr, ytq and the Workspace
        ↓  overridden by
   ~/.config/ytq/config            ytq's own file always wins
```

`KEY=VALUE`, `#` for comments, keys case-insensitive, quotes stripped, a
leading `~` expanded. Three settings are not in that chain at all, because
they are resolved by a function with a fallback rather than by a default
string — `OUTPUT`, `ARCHIVE_DIR` and `SSTR_KEY` — and those three are exactly
the ones that decide what a download *becomes*.

### The resolved state

| Setting | Value here | Where from |
|---|---|---|
| `DIR` | `~/Downloads/SharedVM` | **the one line that was typed** |
| `ARCHIVE_DIR` | `~/Downloads/SharedVM` | unset → falls back to `DIR` |
| `OUTPUT` | `sstr` | unset → the built-in fallback |
| `SSTR_KEY` | `~/.ssh/id_ed25519` | unset → the key that exists |
| `SUBS` | `en,en-orig,en-US,en-GB` | default |
| `FORMAT` | best mp4/avc1 + m4a, falling back | default |
| `POLL` | `1` | default |
| autostart | **on** | `~/.config/ytq/auto` exists |

### What happens when a link is copied

`~/.config/ytq/auto` exists, so Super+Shift+Y does not merely queue: a
background runner starts whenever nothing else is downloading, works the
queue, notifies as each file finishes, and leaves when the queue is empty.
Nothing has to be run by hand. **That one empty file is the difference
between a queue and an appliance.**

`OUTPUT` is unset and its fallback is `sstr`. That is why MP4s stop
appearing: every download is recorded into a capture, read back, verified
against the original's SHA-256, and only then is the MP4 removed.

### What lands on disk

`~/Downloads/SharedVM` is a symlink to `/mnt/share`, genuinely mounted — the
folder the Mac also sees. One download leaves **two files**:

```
Author-Title_ID.sstr     the capture: the video, its FEC, its chain, signed
Author-Title_ID.txt      the reference card, the description, the transcript
```

and, on an x.com post today, **one** — the `.sstr` alone, because the `.txt`
is still gated behind the YouTube-only transcript step. That is the gap
phase 5 closes.

Names carry the first word of the uploader, the title and the id, in the
URL-safe base64 alphabet and nothing else. Observed in that folder now: 26
captures, 41 text files, and 117 older `.mp4` and `.webm` from before
archiving was in effect, in 24 GB.

### Choosing what is kept

| `OUTPUT` | Leaves | For |
|---|---|---|
| `sstr` | the capture and the text | the archive, and the default |
| `both` | the capture, the MP4, and the text | keeping something every player opens |
| `mp4` | the MP4 and nothing else | turning all of this off |

**If what is wanted is "the file, plus its text, plus the archive", that is
`OUTPUT=both`** — one line in `~/.config/ytq/config`, and the MP4 stops being
deleted. Nothing else changes.

### One risk worth writing down

`videos_dir()` takes `~/Downloads/SharedVM` **only when it is really a mount**
— checking device and inode — and falls back to `~/Videos` otherwise,
precisely so that an unmounted share cannot be filled as if it were a folder
on the root disk.

Setting `DIR` explicitly, as is done here, **bypasses that guard.** The value
is the same one the default would choose today, so nothing is wrong now. But
if `/mnt/share` is ever not mounted when a download runs, the explicit setting
still points at the empty mount point, and the video goes onto the root
filesystem instead. Removing that one line restores the guard and changes
nothing else while the share is up.

---

## 10. What is checked, and against what

| Battery | Holds | Count |
|---|---|---|
| `crosscheck` | `sstr` against the frozen Python prototype, at version 0 | 44 |
| `outer-check` | version 1's P+Q: two lost records rebuilt where version 0 loses both | 26 |
| `ytq-crosscheck` | urls, settings, queue, clipboard against the Python ytq | 32 |
| `ytq-runner-crosscheck` | downloads, stops, cookies, retries, transcripts | 50 |
| `ytq-archive-check` | `OUTPUT`, `ARCHIVE_DIR`, `SSTR_KEY` | 34 |
| `ytq-window-crosscheck` | the window beside the Python one, in tmux | 133 |
| `workspace-check` | the Browser's columns against `ls`, the keys, the frame | 63 |
| | **total** | **382** |

**Two specifications, both frozen, both Python**: `tests/reference/copal-sstr.py`
for the format at version 0, and `tests/reference/ytq.py` for the queue, the
clipboard, the settings and the filenames. Neither is changed to make a test
tidier — they are the chain back to what was agreed, and a chain that is
adjusted to fit is not holding anything.

**Where there is no Python, there is an experiment instead.** Version 1 had
none, so its bar was falsifiable and run both ways: damage a capture so two
records of one group are lost; version 0 loses them, version 1 plays back
byte-identical. *A battery that only ever passes is measuring the battery.*
The comments work is in the same position and takes the same answer —
recorded payloads rendered to an expected `.txt`, byte for byte.
