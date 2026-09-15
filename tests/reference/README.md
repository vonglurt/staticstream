<!-- SPDX-License-Identifier: MIT — Copyright (c) 2026 Paul Richeson -->

# The specification

`ytq.py` is the Python ytq, frozen. It is the specification the Rust ytq was
written against, and the four ytq crosschecks run it beside `ytq` and compare
what both leave behind.

It is not edited here, and nothing in this workspace imports it. It is a
reference implementation kept so that the comparisons outlive the program
they compare against.

## Where it came from

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

## Why it is vendored

Step 2e of `docs/phase-2.md` retires the Python ytq from `copal-prep.sh`.
Until then each harness cut it out of that file on every run, which made the
comparisons depend on a second checkout and, after the retirement, on a file
that no longer exists. A frozen copy keeps them running anywhere, offline,
with nothing to drift.

The trade is that a Python file lives in a Rust repository. It earns its
place: 1,436 lines of behaviour that a week of fixes put there, and the only
thing that can say whether the Rust ytq still does what ytq did.
