// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Paul Richeson

//! The log, and telling people: the Python ytq's `log()` and `say()`.
//!
//! One `ytq.log` for every ytq process, Python or Rust: timestamped lines,
//! each tagged with the pid that wrote it, moved to `ytq.log.1` past 4 MiB.

use std::fs::OpenOptions;
use std::io::{IsTerminal, Write};
use std::process::{Command, Stdio};

use crate::ytq::{sys, Paths};

pub const LOG_MAX: u64 = 4 * 1024 * 1024;

/// Python's `str.splitlines()`.
pub fn splitlines(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut chars = s.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let brk = matches!(c, '\n' | '\r' | '\u{b}' | '\u{c}' | '\u{1c}' | '\u{1d}' | '\u{1e}' | '\u{85}' | '\u{2028}' | '\u{2029}');
        if brk {
            out.push(&s[start..i]);
            let mut next = i + c.len_utf8();
            if c == '\r' {
                if let Some(&(j, '\n')) = chars.peek() {
                    chars.next();
                    next = j + 1;
                }
            }
            start = next;
        }
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

/// `log(msg)`: a line per line of msg, each stamped with the time and the pid.
pub fn log(paths: &Paths, msg: &str) {
    let Some(dir) = paths.log.parent() else { return };
    let _ = std::fs::create_dir_all(dir);
    if std::fs::metadata(&paths.log).map_or(false, |m| m.len() > LOG_MAX) {
        let mut old = paths.log.as_os_str().to_owned();
        old.push(".1");
        let _ = std::fs::rename(&paths.log, old);
    }
    let stamp = format!("{} [{}] ", sys::strftime_local("%Y-%m-%d %H:%M:%S", sys::now()), std::process::id());
    let mut lines = splitlines(msg);
    if lines.is_empty() {
        lines.push("");
    }
    let text: String = lines.iter().map(|l| format!("{stamp}{l}\n")).collect();
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&paths.log) {
        let _ = f.write_all(text.as_bytes());
    }
}

/// Who hears about things. A terminal gets them printed; Super+Shift+Y and a
/// background runner have no terminal, so they get a notification; the window
/// shows everything itself and only logs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Print,
    Notify,
    Quiet,
}

impl Mode {
    pub fn for_stdout() -> Mode {
        if std::io::stdout().is_terminal() {
            Mode::Print
        } else {
            Mode::Notify
        }
    }
}

/// `say(msg, urgent)`.
pub fn say(paths: &Paths, mode: Mode, msg: &str, urgent: bool) {
    log(paths, msg);
    match mode {
        Mode::Print => println!("{msg}"),
        Mode::Notify => {
            let _ = Command::new("notify-send")
                .args(["-a", "ytq", "-u", if urgent { "critical" } else { "normal" }, "ytq", msg])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
        }
        Mode::Quiet => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitlines_as_python() {
        assert_eq!(splitlines("a\nb\r\nc\rd"), ["a", "b", "c", "d"]);
        assert_eq!(splitlines("a\n"), ["a"]);
        assert!(splitlines("").is_empty());
        assert_eq!(splitlines("\n\nx"), ["", "", "x"]);
    }
}
