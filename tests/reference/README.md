<!-- SPDX-License-Identifier: MIT — Copyright (c) 2026 Paul Richeson -->

# The specification

Two Python programs, frozen. They are what the Rust was written against, and
the harnesses run them beside it and compare what both leave behind.

| | |
|---|---|
| `copal-sstr.py` | the Static Stream prototype: **version 0 of the format**. `crosscheck.sh`'s 44 comparisons and `outer-check.sh`'s 26 run against it, and `damage.py` reads it for the record layout |
| `ytq.py` | the Python ytq. The four ytq crosschecks run it beside `ytq` |

Neither is edited here, and nothing in this workspace imports either. They are
reference implementations kept so that the comparisons outlive the programs
they compare against.

## copal-sstr.py: where it came from

`tools/copal-sstr.py` in `github.com/vonglurt/copal`, copied unchanged.

| | |
|---|---|
| From | `github.com/vonglurt/copal`, `tools/copal-sstr.py` |
| At commit | `29f64c1`; the last commit to change the file itself is `6fa0b0d` |
| Lines | 1,263 |
| MD5 | `b743b417714a8c5af78fb9403b5fc8fa` |

### Why it is vendored

**A clone of this repository alone could not run its own acceptance test.**
`crosscheck.sh` and `outer-check.sh` reached across to a second checkout for
it, and when that checkout was not there they said so and **skipped** -- and
`make check` exited 0 having run 312 of its 382 checks, with nothing to say
that the 44 comparisons which are the whole chain back to the specification
had not happened. A contributor with one clone got a green check that was not
the check.

A Rust unit test read it too, `constants_match_the_prototype` in
`src/format/mod.rs`, and had been skipping since the five packages became one:
its path was right when the manifest directory was `crates/staticstream/` and
resolved to `/home/copal/...` afterwards. It now reads the copy here and
**fails** rather than skipping, because a vendored file that is missing is a
broken checkout and not an absent extra.

That is the same argument step 2e made for `ytq.py`, arriving later for the
same reason it arrived at all: a comparison that depends on a second checkout
is a comparison that does not always run.

## ytq.py: where it came from

It is the file a clean Copal install wrote, cut out of `install_ytq` in
`copal-prep.sh` with the installer's own heredoc markers:

```sh
awk '/cat > \/usr\/local\/bin\/ytq <<.YTQ./{f=1;next} /^YTQ$/{f=0} f' \
    ../copal/copal-prep.sh > tests/reference/ytq.py
```

| | |
|---|---|
| From | `github.com/vonglurt/copal`, `copal-prep.sh` |
| At commit | `6d557cd`, whose last change to ytq was `95b0be1` |
| Lines | 1,436 |
| MD5 | `9904320ea6746e62655137e341d30ac7` |

### Why it is vendored

Step 2e of `docs/phase-2.md` retires the Python ytq from `copal-prep.sh`.
Until then each harness cut it out of that file on every run, which made the
comparisons depend on a second checkout and, after the retirement, on a file
that no longer exists. A frozen copy keeps them running anywhere, offline,
with nothing to drift.

The trade is that a Python file lives in a Rust repository. It earns its
place: 1,436 lines of behaviour that a week of fixes put there, and the only
thing that can say whether the Rust ytq still does what ytq did.
