// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Paul Richeson

//! ytq, the download queue that watches the clipboard -- moving into Rust
//! beside the format it now archives into.
//!
//! Today ytq is one Python file that copal-prep.sh installs as
//! /usr/local/bin/ytq: a curses window, `ytq clip` on Super+Shift+Y, a runner
//! that drives yt-dlp, and one `queue.json` that every ytq process shares
//! under `queue.lock`. This crate takes it over in pieces. The Rust ytq reads
//! and writes the same files under the same locks, so the two can run side by
//! side, and the Workspace's Queue is that same queue rather than a second
//! one. The binary called `ytq` arrives only when it does everything the
//! Python one does; until then the Python ytq is the one on PATH.
//!
//! Phase 0 is the first piece: where ytq keeps things, found exactly as ytq
//! finds them, and the states an entry moves through.

use std::path::{Path, PathBuf};

pub mod cli;
pub mod live;
pub mod log;
pub mod queue;
pub mod runner;
pub mod settings;
pub mod sys;
pub mod term;
pub mod textwrap;
pub mod urls;
pub mod window;

/// What a runner may take next, in the order it looks for them.
pub const DOWNLOADABLE: [&str; 3] = ["retry-cookies", "queued", "retry"];

/// Every state an entry can be in, in the order ytq's window lists them.
pub const ORDER: [&str; 9] = [
    "downloading",
    "cookies",
    "retry-cookies",
    "checking",
    "queued",
    "retry",
    "done",
    "failed",
    "rejected",
];
/// Entries that are over: what `ytq clear` forgets.
pub const HISTORY: [&str; 3] = ["done", "failed", "rejected"];
/// Work a runner still has to do. `cookies` is in neither list: it waits on a person.
pub const PENDING: [&str; 5] = ["checking", "queued", "retry", "retry-cookies", "downloading"];

/// The fields of a queue entry, as the Python ytq writes them.
pub const ENTRY_FIELDS: [&str; 11] = [
    "url",
    "title",
    "status",
    "quality",
    "progress",
    "file",
    "attempts",
    "error",
    "added",
    "live",
    "cookie_tried",
];

/// Where ytq's files are, and the shared media configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// `~/.config/copal/media.conf`: the shared settings, read first.
    pub media_conf: PathBuf,
    /// `~/.config/ytq/config`: ytq's own `KEY=VALUE` settings, read after.
    pub ytq_config: PathBuf,
    /// `~/.config/ytq/auto`: present means autostart is on.
    pub autostart: PathBuf,
    pub queue: PathBuf,
    pub queue_lock: PathBuf,
    pub run_lock: PathBuf,
    pub log: PathBuf,
}

impl Paths {
    /// From this process's environment. None without a HOME.
    pub fn from_env() -> Option<Paths> {
        Paths::from_lookup(|k| std::env::var_os(k).map(PathBuf::from))
    }

    /// From any environment. As ytq does it: `XDG_CONFIG_HOME` or `~/.config`,
    /// `XDG_DATA_HOME` or `~/.local/share`, an empty variable counting as unset
    /// (ytq's `os.environ.get(...) or ...`).
    pub fn from_lookup<F: Fn(&str) -> Option<PathBuf>>(get: F) -> Option<Paths> {
        let home = get("HOME").filter(|p| !p.as_os_str().is_empty())?;
        let set = |k: &str| get(k).filter(|p| !p.as_os_str().is_empty());
        let config = set("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config"));
        let data = set("XDG_DATA_HOME").unwrap_or_else(|| home.join(".local").join("share")).join("ytq");
        Some(Paths {
            media_conf: config.join("copal").join("media.conf"),
            ytq_config: config.join("ytq").join("config"),
            autostart: config.join("ytq").join("auto"),
            queue: data.join("queue.json"),
            queue_lock: data.join("queue.lock"),
            run_lock: data.join("run.lock"),
            log: data.join("ytq.log"),
        })
    }

    /// Each path with a label, in the order a person reads them.
    pub fn labelled(&self) -> [(&'static str, &Path); 7] {
        [
            ("media.conf", &self.media_conf),
            ("ytq config", &self.ytq_config),
            ("autostart", &self.autostart),
            ("queue", &self.queue),
            ("queue lock", &self.queue_lock),
            ("run lock", &self.run_lock),
            ("log", &self.log),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<PathBuf> {
        let map: HashMap<String, PathBuf> = pairs.iter().map(|(k, v)| (k.to_string(), PathBuf::from(v))).collect();
        move |k| map.get(k).cloned()
    }

    #[test]
    fn defaults_under_home() {
        let p = Paths::from_lookup(env(&[("HOME", "/home/u")])).unwrap();
        assert_eq!(p.ytq_config, PathBuf::from("/home/u/.config/ytq/config"));
        assert_eq!(p.media_conf, PathBuf::from("/home/u/.config/copal/media.conf"));
        assert_eq!(p.queue, PathBuf::from("/home/u/.local/share/ytq/queue.json"));
        assert_eq!(p.log, PathBuf::from("/home/u/.local/share/ytq/ytq.log"));
    }

    #[test]
    fn xdg_wins_and_empty_means_unset() {
        let p = Paths::from_lookup(env(&[("HOME", "/h"), ("XDG_CONFIG_HOME", "/c"), ("XDG_DATA_HOME", "")])).unwrap();
        assert_eq!(p.autostart, PathBuf::from("/c/ytq/auto"));
        assert_eq!(p.run_lock, PathBuf::from("/h/.local/share/ytq/run.lock"));
    }

    #[test]
    fn no_home_no_paths() {
        assert_eq!(Paths::from_lookup(env(&[])), None);
    }

    #[test]
    fn states_partition() {
        for s in HISTORY.iter().chain(PENDING.iter()) {
            assert!(ORDER.contains(s));
        }
        assert!(!HISTORY.contains(&"cookies") && !PENDING.contains(&"cookies"));
    }

    /// The Python ytq is the definition until this crate replaces it, so its
    /// source is checked for the paths and states copied here. The copal
    /// checkout is found beside this one in ~/code, or at $STATICSTREAM_YTQ
    /// (the installed /usr/local/bin/ytq will do); without either the test
    /// says so and passes.
    #[test]
    fn matches_the_python_ytq() {
        let candidates: Vec<PathBuf> = std::env::var_os("STATICSTREAM_YTQ")
            .map(|p| vec![PathBuf::from(p)])
            .unwrap_or_else(|| {
                vec![
                    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../copal/copal-prep.sh"),
                    PathBuf::from("/usr/local/bin/ytq"),
                ]
            });
        let Some((path, src)) = candidates.iter().find_map(|p| std::fs::read_to_string(p).ok().map(|s| (p, s))) else {
            eprintln!("skipped: no ytq source found");
            return;
        };
        for line in [
            r#""ytq", "config")"#,
            r#"QUEUE = os.path.join(DATA, "queue.json")"#,
            r#"QLOCK = os.path.join(DATA, "queue.lock")"#,
            r#"RUNLOCK = os.path.join(DATA, "run.lock")"#,
            r#"LOG = os.path.join(DATA, "ytq.log")"#,
            r#"ORDER = ["downloading", "cookies", "retry-cookies", "checking", "queued", "retry", "done", "failed", "rejected"]"#,
            r#"HISTORY = ("done", "failed", "rejected")"#,
            r#"PENDING = ("checking", "queued", "retry", "retry-cookies", "downloading")"#,
        ] {
            assert!(src.contains(line), "{} no longer has: {line}", path.display());
        }
    }
}
