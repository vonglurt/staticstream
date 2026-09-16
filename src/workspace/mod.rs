// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Paul Richeson

//! sstr-workspace -- the terminal Workspace, in NeXTSTEP's and Smalltalk's
//! words. Phase 3 draws it. Phase 0 said what it will be, and shows the
//! files it will look at on this machine.
//!
//! The binary `sstr-workspace` is a shim over [`main`], as `ytq` is over
//! [`crate::ytq::cli::main`]: the Workspace is a way of LOOKING at what the
//! other modules already do, so it lives beside them rather than above them.

use std::process::ExitCode;

/// The nouns and the one plural of verbs, and where each word comes from.
pub const VOCABULARY: [(&str, &str, &str); 7] = [
    ("Workspace", "the whole window", "NeXTSTEP's Workspace Manager"),
    ("Browser", "Miller columns over any folder, from the archive folder", "the File Viewer's Browser view"),
    ("Shelf", "items picked for later: files to stream, URLs to queue", "the File Viewer's Shelf"),
    ("Inspector", "the selection's header, notes, license and verify result", "Tools > Inspector"),
    ("Transcript", "the running log: ytq.log and sstr's events", "Smalltalk-80's Transcript"),
    ("Services", "verbs sent to the selection: Play, Serve, Verify, Export MP4, Open as Text", "NeXTSTEP Services"),
    ("Queue", "ytq's queue as one more object to browse and inspect", "ytq"),
];

pub fn main(argv: &[String]) -> ExitCode {
    if matches!(argv.first().map(String::as_str), Some("-V" | "--version")) {
        println!("sstr-workspace {} (Static Stream format v{})", env!("CARGO_PKG_VERSION"), crate::format::FORMAT_VERSION);
        return ExitCode::SUCCESS;
    }
    println!("sstr-workspace {} -- not drawn yet: phase 3.\n", env!("CARGO_PKG_VERSION"));
    println!("What it will be:\n");
    for (word, meaning, from) in VOCABULARY {
        println!("  {word:<11} {meaning}\n  {:<11} ({from})", "");
    }
    println!("\nEvery Service is also a command line, shown in the Transcript before it runs.");
    if let Some(p) = crate::ytq::Paths::from_env() {
        println!("\nWhat it will read on this machine:\n");
        for (label, path) in p.labelled() {
            let mark = if path.exists() { "" } else { "   (not there)" };
            println!("  {label:<11} {}{mark}", path.display());
        }
    }
    ExitCode::SUCCESS
}
