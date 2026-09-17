<!-- SPDX-License-Identifier: MIT — Copyright (c) 2026 Paul Richeson -->

# Phase 5: the counts, and the discussion

**This phase has no row in the report's table.** Every phase before it quoted
its done-condition from V-H of `docs/staticstream-project-lab-report.md` (in
copal); that table ends at phase 4. So the bar here is set from two other
places, and it is worth naming them rather than pretending the table grew a
row on its own:

- **What the owner asked for**: the replies to an x.com post as a discussion,
  in a `.txt`; the same for YouTube; and the counts a post carries, stamped
  with the day they were read.
- **The argument already in the ytq report**, Sections IV-N and V: a download
  should carry what a citation needs, and should be honest about what it does
  not carry. `notes()` exists because of that argument. This phase is the same
  argument applied to two things Notes does not yet hold.

## Done when

1. **An x.com download leaves a `.txt`.** Today it leaves none at all — the
   transcript step is gated on `first_id(&url).is_some()` (`runner.rs:655`),
   which is YouTube or nothing. An X post downloads, archives, and says
   nothing about itself in text.
2. **Both `.txt` files carry a `Stats` block whose counts are stamped with
   the moment they were read**, separately from `Downloaded:`.
3. **A YouTube `.txt` carries a threaded `Discussion` section** — parents and
   replies in the shape yt-dlp gives them, under a stated cap.
4. **The capture holds the same fields.** `archive()` already writes `source`,
   `license` and `note` into the `.sstr` metadata; stats belong there too. The
   `.txt` is a rendering, the capture is the record.
5. **`make check` grows, and the crosschecks against the Python ytq still
   agree on everything they covered before.**

## What is different about this phase

**The Python ytq cannot be the oracle, and this is the first ytq work where
that is true.** Phase 2 was believable because `tests/reference/ytq.py` ran
beside it on one queue and the two agreed 133 times. That file writes no
stats and no discussion, and it is frozen — V-F's rule for the format
prototype applies to it just as plainly. So the crosschecks stay and go on
holding the *unchanged* parts to the specification, and the new parts need a
different bar. Phase 4 already set the precedent when version 1 had no Python
to compare against: not a second implementation, but **fixtures and an
experiment** — a recorded `comments` payload rendered to an expected `.txt`,
byte for byte.

**Half of what was asked cannot be built, and that goes in writing.** X reply
*text* is not available:

| Evidence | Result |
|---|---|
| `yt_dlp/extractor/twitter.py` | no `extract_comments`, no `__post_extractor`, no `comments` key — `comment_count` is a number, nothing more |
| `yt-dlp --write-comments --print '%(comments)s'` on a real post | `COMMENTS=NA` |
| `cdn.syndication.twimg.com/tweet-result` | HTTP 200, returns `conversation_count` and the tweet; no replies array |

So phase 5 records **how many** replies an X post has and leaves the text a
hole. The hole is deliberate and named, not quietly dropped: reaching it means
a logged-in GraphQL call, which ytq could in principle make — it already knows
how to get Brave's cookies for `ytq cookies` — and which breaks whenever X
changes an endpoint. That is a phase of its own, if ever.

**A count is not a fact about the recording. It is a fact about a moment.**
Every row Notes holds today — title, author, published, license — is a
property of the thing downloaded, and re-reading it tomorrow gives the same
answer. Views and likes do not work that way; they are different by the time
the download finishes. That is the whole reason the owner asked for them
"taken on the date", and it decides the design: the counts are useless
unstamped, and the stamp is not decoration but the only thing that makes them
a record rather than a rumour.

## What the two sites actually give

Measured on this machine, 2026-09-17, yt-dlp 2026.08.19.

**X**, on `x.com/TheBTCTherapist/status/1935298536373170512` — one of the
owner's own downloads:

| Field | Value |
|---|---|
| `view_count` | 34776 |
| `like_count` | 412 |
| `repost_count` | 68 |
| `comment_count` | 38 |
| `timestamp` | 1750246101 |
| `description` | the tweet's own text |
| `comments` | `NA` |

**YouTube**: a full comment extractor. `--write-comments` fills `comments`,
each one carrying `parent` (`'root'` or the id of the comment it answers),
`is_pinned`, `author`, `author_id`, `timestamp`, `like_count`, and
`--extractor-args youtube:max_comments=N,parents,replies,depth` caps the
work. A live run returned six comments and their authors in seconds. `parent`
is what makes a discussion possible rather than a list.

## The step that turns out to be free

The download run **already prints a JSON line of metadata** and the runner
already parses it (`runner.rs:549`):

```rust
pub const NOTES_PRINT: &str =
    "after_move:NOTES %(.{webpage_url,license,title,uploader,channel})j";
```

Five fields. Lengthening that list costs nothing — no second yt-dlp run, no
extra request, no extra second — because the line is printed after the move
whatever it contains. So **an X post's Notes and Stats are free**; the only
reason they are not written today is that nothing downstream writes a `.txt`
unless the URL is a YouTube video.

Two things are in the way and both are small:

- The `NOTES ` line is dropped when `OUTPUT=mp4` (`runner.rs:549`), because
  notes existed only to feed `archive()`. If the `.txt` is to be written
  whether or not the download is archived, that filter goes.
- The transcript step conflates two jobs: *fetch the captions* and *write the
  notes file*. They separate. Captions are asked for when there are captions
  to ask for; notes are written always.

## The steps

- **5a — Notes for everything.** Split writing the `.txt` from fetching the
  captions. Lengthen `NOTES_PRINT`. An x.com download leaves a `.txt` with
  Notes and no Transcript heading, on the metadata the download already
  fetched. `Captions:` is written only where captions were sought.
- **5b — The `Stats` block**, in `notes()`, stamped, rows only where the site
  gave a number. Both sites.
- **5c — The `Discussion` section**, YouTube only: `--write-comments` on the
  transcript run that is already happening, a cap, and the tree rendering.
- **5d — Into the capture.** `archive()`'s `meta` gains the counts and the
  moment; the `.sstr` stops being poorer than the `.txt` beside it.
- **5e — The checks, the report, and the docs**: fixtures for 5b and 5c, the
  crosschecks re-run, `docs/ytq-clipboard-lab-report.md` and Section IX of the
  project report brought up to date, and this file's deviations written down.

Each step is useful alone, as V-H asks of every phase: after 5a an X download
already explains itself, with no new interface at all.

## Decisions already made

- **One `.txt`, not two.** The discussion goes in the file that already
  exists, after the Transcript. A second `.comments.txt` was considered and
  refused: one download, one text file beside the recording, is what makes
  the folder readable without a program — the Browser shows a pair, not a
  litter.
- **The counts are stamped with their own time, not `Downloaded:`.** They
  coincide for a `ytq run`, and they do not for `ytq transcript` on a video
  downloaded last week. Two rows that are usually equal are cheaper than one
  row that is sometimes a lie.
- **Full numbers, everywhere.** The sketch this was approved from wrote
  `378,402,118` in Stats and `12k likes` in the discussion. That mixture is
  dropped in favour of commas throughout — **this is a deliberate departure
  from what was agreed**, on the grounds that an archival record should not
  round: `12k` cannot be un-rounded later, and the width it saves is width
  that was never scarce. Separators are inserted by hand, every three digits,
  no locale — the crate takes no dependency for this and will not start.
- **A cap, and a way to turn it off, following `SUBS`.** A `COMMENTS` setting
  in `media.conf` or ytq's own config, defaulting to 200, empty meaning none
  — exactly the shape `SUBS` already has, including empty-disables. Comments
  cost requests, and a video with a million of them must not be able to turn
  one download into an afternoon.
- **X replies are counted, not quoted.** See above. `Replies:` is a number
  in Stats, and there is no Discussion section on an X post.
- **The Python ytq is not changed.** It is the specification for the queue,
  the clipboard, the settings and the filenames, and phase 2's 133
  comparisons are the chain back to it. Adding stats to it to keep the
  crosscheck symmetrical would cut that chain to make a test look tidier.
