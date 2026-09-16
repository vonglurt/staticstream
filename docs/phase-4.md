<!-- SPDX-License-Identifier: MIT — Copyright (c) 2026 Paul Richeson -->

# Phase 4: the release matrix, and version 1's outer code

The plan for phase 4 of `docs/staticstream-project-lab-report.md` (in copal).
Written after rereading that report — V-E for the architectures, V-F for what
version 1 changes and why, V-G for the Makefile — and the Static Stream lab
report, whose Section D is the outer code as it stands and Section F is the
survey the replacement comes from.

## Done when

This is the report's phase-4 row, unchanged:

> `make dist` for the targets of V-E, and version 1's stronger outer code —
> binaries run on the Pi 2B and the x86_64 VM; version 1 rebuilds two lost
> records per group.

## What is different about this phase

**Half of it cannot be finished on this machine, and that is worth saying
first.** Alpine's rust package ships only its own target's standard library
and there is no rustup, no cargo-make and no cargo-zigbuild here; `zig` is
installed. The report predicted exactly this in IV-A. So the cross builds and
the two done-conditions that name hardware — *binaries run on the Pi 2B and
the x86_64 VM* — need machines this session does not have. What can be built
and checked here is the machinery, the native target, and the behaviour when
the cross tooling is absent, which is the state this machine is in.

**The other half has no Python to compare against, for a new reason.** Phases
1 and 2 were checked against `tools/copal-sstr.py`. Version 1 changes the
format, and V-F says the prototype stays at version 0 — so the Python cannot
read a version 1 capture, by design. The bar that replaces the crosscheck
here is not a second implementation but an **experiment**:

> Damage a capture in a way that loses two records of one group. Version 0
> loses them. Version 1 plays back byte-identical.

That is falsifiable, it is the done-condition in the report's own words, and
it is run both ways round — the version 0 failure is as much a part of the
result as the version 1 success, because a battery that only ever passes is
measuring the battery.

**And the chain back to the specification must not be cut.** The 44
comparisons of phase 1 are the reason anyone believes this format is what the
prototype writes. They are comparisons *at version 0*, and they stay, which
means version 0 stays written, readable and exercised for as long as the
Python does.

## The outer code, as it is and as it becomes

Section D of the Static Stream report: after every G = 16 data records a
parity record lists their sequence numbers, times, flags, lengths and CRCs,
and holds the **XOR** of their bodies padded to the longest. Any *one* record
of the group lost outright can be rebuilt. Section 53 of that report's own
findings names the limit plainly: *the XOR outer code rebuilds only one record
per group.*

Version 1 keeps the record layout and the entry table and replaces the single
XOR row with **two rows, P and Q**, over the group's bodies read as columns:

- **P**[j] = the XOR of every body's byte j — *which is exactly version 0's
  row*, unchanged.
- **Q**[j] = the XOR of `g^i · body_i[j]` in GF(256), i being the record's
  place in the group.

Two rows, two erasures. With one record missing it is the same arithmetic
version 0 already does; with two missing at known places the pair solves:

      d_a ⊕ d_b = P'         g^a·d_a ⊕ g^b·d_b = Q'
      d_a = (Q' ⊕ g^b·P') / (g^a ⊕ g^b)        d_b = P' ⊕ d_a

**Why P+Q and not a general RS(k+2, k).** The field, the tables and the
arithmetic are already in `format::rs` for the inner RS(255,223), so no new
mathematics enters the crate. P+Q is the smallest change that reaches two —
version 0's row survives untouched as the first of the two, which means a
version 1 parity record with its second row dropped still rebuilds one, and
the code that does it is the code that already did.

**The row count is not a new field.** Every entry carries its `plain_len`, so
the padded width `n` is `max(plain_len)` and the number of rows is
`blob.len() / n`. A parity record therefore says how many rows it has without
being asked and without being told the stream's version — which matters,
because a parity record can be the first thing a reader sees after a resync.

## The steps

| Step | Delivers | Done when |
|---|---|---|
| **4a** | **version 1's outer code**: P+Q written and read, version 0 still written and read | a group with two records missing rebuilds both; the same capture at version 0 rebuilds neither; every phase-1 comparison still agrees. **Done**: `make outer-check`, 7 of 7 -- on one 200,000-byte capture with records 2 and 4 wiped, version 0 exits 1 with *2 data records lost* and version 1 exits 0 with *2 records rebuilt from parity* and a byte-identical payload. `make check` is 363 |
| 4b | **the damage battery at version 1**: the Static Stream report's thirteen kinds of damage, re-run against version 1, and the redundancy figure remeasured | the battery passes at version 1 and the overhead is the measured number, not the designed one |
| 4c | **`make dist`**: what this machine can build, and an exact account of what it cannot | `dist/` holds the native target's three binaries and a manifest; a missing target or linker is named, with the command that supplies it, rather than a backtrace from inside cargo |
| 4d | **the report, revised for phase 4** | V-E, V-F and the phase table say what was built and what was measured |

## Deviations, written down as they are made

### Step 4a

- **`--format 0|1` on `sstr record`, and 0 is still the default.** The 44
  comparisons are comparisons at version 0 and they are the chain back to the
  prototype; they pass unchanged because nothing about the default moved. When
  version 1 becomes the default it will be because a line in the report says
  so, not because a step needed somewhere to put a byte.
- **The check requires version 0 to FAIL.** That is the half of the experiment
  that is easy to leave out and expensive to lose. If the damage ever stops
  landing inside one group -- a smaller capture, a different chunk size, a
  bigger group -- version 1 would go on passing and nothing would say the test
  had stopped testing. So `outer-check` fails when version 0 succeeds.
- **The payload is chunked at 4 KiB in the check, and that is not incidental.**
  At the default 64 KiB a 200,000-byte capture is four data records, so
  "records 2 and 4 of one group" is most of the capture rather than two holes
  in the middle of a full group. The first run of this experiment did exactly
  that and `damage.py` fell over on an index it did not have, which was the
  useful answer: there was no group to speak of.
- **`Outer::of_version` takes the version, not the other way round.** A
  version is a thing a file has. The reader never asks it, though: the row
  count is `blob.len() / max(plain_len)`, so a parity record answers for
  itself even when it is the first record seen after a resync, before any
  header.
- **The CRC is still what says a rebuild was right.** Two holes and a third
  record quietly corrupt would solve to two wrong bodies; each rebuilt body is
  truncated to its own `plain_len` and checked against its own CRC before it
  is allowed into the stream, exactly as one rebuilt body always was.
- **The second row costs what it costs**: 17,888 bytes on a 200,000-byte
  payload chunked at 4 KiB, 268,275 against 286,163. The designed figure is
  (1 + 32/223) x (1 + 2/16) = 1.286 against version 0's 1.215; the measured
  one is step 4b's business, along with the rest of the battery.

## Decisions already made

- **Version 0 stays the default until the end of the phase.** `sstr record`
  writes version 0 unless asked for version 1, and the crosschecks pin it
  explicitly so that they go on meaning what they meant. Making version 1 the
  default is its own decision, with its own line in the report, and it is not
  one to take in the same step that first writes a version 1 byte.
- **The reader takes both, always.** A version is a thing files have, not a
  thing programs have; a reader that dropped version 0 would strand every
  capture made before today, which is the opposite of what an archival format
  is for.
- **The Python prototype is not changed.** It is the specification for version
  0 and the oracle the crosschecks run against, and V-F says it stays there.
