// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Paul Richeson

//! Static Stream: a stream stopped in a file, and let go again.
//!
//! Light can be slowed until it seems to stand still, and released. What the
//! experiments keep is the pulse's state, written into matter and read back
//! out as light. A static stream does that with bytes: a stream is written
//! into a `.sstr` file with its order, its pace and its tolerance for damage
//! intact, and later let go again as a stream.
//!
//! Four modules, in the order they stack:
//!
//! - [`format`] — the `.sstr` file itself: the record layout, RS(255,223),
//!   parity, checkpoints, the writer and the reader, and what the prototype
//!   had from Python's library and this crate writes itself (zlib, SHA-256,
//!   CRC-32, JSON). Knows nothing above it, and holds no I/O policy.
//! - [`tty`] — the armor: any byte stream as 80-column CRC-checked base64url
//!   lines a serial console survives, and back, with what was lost as
//!   erasures.
//! - [`ytq`] — Copal's download queue that watches the clipboard: the queue
//!   file and its locks, the settings, the runner that drives yt-dlp, the
//!   live record, and the window. Archives what it downloads through
//!   [`format`].
//! - [`workspace`] — the terminal Workspace, in NeXTSTEP's and Smalltalk's
//!   words: a Browser, a Shelf, an Inspector, a Transcript and Services sent
//!   to the selection.
//!
//! It is a split into modules, not into packages. These were five crates
//! until the release, and they only ever depended on each other — one
//! version, one publish, one program. There is one crate here and it is named
//! after the repository.
//!
//! The three binaries are thin: [`ytq::cli::main`] is both `ytq` and
//! `sstr ytq`, and every verb the Workspace sends is a command line `sstr`
//! accepts, so the window and the shell teach each other.
//!
//! Version 0 of the format is defined by `tools/copal-sstr.py` in the copal
//! repository and set out in its `docs/static-stream-lab-report.md`. That
//! Python prototype, and the Python ytq frozen at `tests/reference/ytq.py`,
//! are the specification both halves of this crate are checked against.

pub mod format;
pub mod tty;
pub mod workspace;
pub mod ytq;
