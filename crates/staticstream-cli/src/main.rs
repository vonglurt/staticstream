// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Paul Richeson

//! sstr -- record a stream into a static stream, and play it back as one.
//!
//! Every verb here is a message sent to a file, and the same message the
//! Workspace sends when a person picks it from Services: what the window
//! does is always something a person could have typed.

use std::process::ExitCode;

/// The verbs, what each does, and the phase that writes it.
const VERBS: [(&str, &str, u8); 7] = [
    ("record", "freeze stdin, or --input FILE, into a .sstr", 1),
    ("play", "let a .sstr go: stdout, -o FILE, --paced, --follow, --serve", 1),
    ("verify", "repair in memory and report records, losses, checkpoints, signatures", 1),
    ("armor", "a .sstr as CRC-checked base64url lines for a tty", 1),
    ("unarmor", "armor lines back to bytes, with the ranges lost", 1),
    ("recv", "armor lines of a .sstr straight to its payload", 1),
    ("paths", "where ytq's config, queue and log, and media.conf, are here", 0),
];

fn usage() -> String {
    let mut s = format!(
        "sstr {} -- Static Stream, format v{}\n\n  sstr VERB [ARGS]\n\n",
        env!("CARGO_PKG_VERSION"),
        staticstream::FORMAT_VERSION
    );
    for (verb, what, phase) in VERBS {
        let note = if phase == 0 { String::new() } else { format!("   [phase {phase}]") };
        s.push_str(&format!("  {verb:<9} {what}{note}\n"));
    }
    s.push_str(
        "\nVerbs marked with a phase are not written yet. Until they are, the\n\
         prototype does each of them: tools/copal-sstr.py VERB in the copal\n\
         checkout, whose files this reads and writes the same.\n",
    );
    s
}

fn paths() -> ExitCode {
    let Some(p) = staticstream_ytq::Paths::from_env() else {
        eprintln!("sstr paths: HOME is not set");
        return ExitCode::from(1);
    };
    for (label, path) in p.labelled() {
        let mark = if path.exists() { "" } else { "   (not there)" };
        println!("{label:<11} {}{mark}", path.display());
    }
    ExitCode::SUCCESS
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("help" | "-h" | "--help") => {
            print!("{}", usage());
            ExitCode::SUCCESS
        }
        Some("-V" | "--version") => {
            println!("sstr {} (Static Stream format v{})", env!("CARGO_PKG_VERSION"), staticstream::FORMAT_VERSION);
            ExitCode::SUCCESS
        }
        Some("paths") => paths(),
        Some(verb) => match VERBS.iter().find(|(v, _, _)| *v == verb) {
            Some((_, _, phase)) => {
                eprintln!(
                    "sstr {verb}: not written yet -- phase {phase}.\n\
                     until then: tools/copal-sstr.py {verb} ... in the copal checkout"
                );
                ExitCode::from(2)
            }
            None => {
                eprint!("sstr: no verb '{verb}'\n\n{}", usage());
                ExitCode::from(2)
            }
        },
    }
}
