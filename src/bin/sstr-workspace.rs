// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Paul Richeson

//! `sstr-workspace`: the terminal Workspace.
//!
//! A shim over [`staticstream::workspace::main`], as `ytq` is over
//! `staticstream::ytq::cli::main`. The Workspace is a way of looking at what
//! the other modules already do, so the program is a module beside them and
//! this file is only the name people type.

use std::process::ExitCode;

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    staticstream::workspace::main(&argv)
}
