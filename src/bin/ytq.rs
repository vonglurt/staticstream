// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Paul Richeson

//! `ytq`: Copal's download queue, the one people type.
//!
//! The same program as `sstr ytq`, under the name the keybinding and the
//! habit already use. `cli::main` is the whole of it, so the two can never
//! drift: `ytq add` and `sstr ytq add` run the same code on the same queue.
//!
//! Until step 2e of `docs/phase-2.md` this binary did not exist on purpose.
//! `~/.local/bin` comes before `/usr/local/bin` on Copal's PATH, and
//! `copal-build` installs every executable at the top of `target/release`, so
//! an unfinished `ytq` here would have shadowed the working Python one on
//! every node.

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(staticstream::ytq::cli::main(&argv));
}
