// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Paul Richeson

//! Settings: the Python ytq's `settings()` and `videos_dir()`, and the shared
//! `~/.config/copal/media.conf` the project report adds in front of them.
//!
//! `KEY=VALUE` lines, `#` for comments, keys case-insensitive, values with
//! their surrounding quotes stripped and a leading `~` expanded. media.conf is
//! read first and ytq's own config after, so a key set in ytq's file wins.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::ytq::urls::py_strip;
use crate::ytq::Paths;

pub const DEFAULT_FORMAT: &str = "bv*[ext=mp4][vcodec^=avc1]+ba[ext=m4a]/bv*[ext=mp4]+ba[ext=m4a]/b[ext=mp4]/bv*+ba/b";
// Exact names, not en.*: that also takes YouTube's machine translations into
// English (en-de is English from German), each one more caption request.
pub const DEFAULT_SUBS: &str = "en,en-orig,en-US,en-GB";
// How many comments a download may take, at most. Comments cost requests, and
// a video with a million of them must not be able to turn one download into an
// afternoon. COMMENTS= (empty) asks for none at all, as SUBS= does for captions.
//
// NOT a default in `load()` below, and deliberately: that map is compared to the
// Python ytq's `settings()` byte for byte, and a key the specification has never
// heard of does not belong in it. `runner::comments_of` resolves it instead, as
// `output_of`, `archive_dir_of` and `sstr_key_of` resolve the other three
// settings that decide what a download becomes.
pub const DEFAULT_COMMENTS: &str = "200";

/// Where downloads go when nothing says otherwise: the folder shared with the
/// Mac when that share is really mounted -- a link to an unmounted share would
/// fill the empty mount point instead -- then XDG_VIDEOS_DIR, then ~/Videos.
pub fn videos_dir(home: &Path, env: &dyn Fn(&str) -> Option<String>) -> String {
    let shared = home.join("Downloads").join("SharedVM");
    if std::fs::canonicalize(&shared).map_or(false, |real| is_mount(&real)) {
        return shared.to_string_lossy().into_owned();
    }
    if let Ok(text) = std::fs::read_to_string(home.join(".config").join("user-dirs.dirs")) {
        for line in text.lines() {
            if let Some(v) = line.strip_prefix("XDG_VIDEOS_DIR=") {
                return expandvars(py_strip(v).trim_matches('"'), env);
            }
        }
    }
    home.join("Videos").to_string_lossy().into_owned()
}

/// `os.path.ismount`: a different device from its parent, or its own parent.
pub fn is_mount(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Ok(me) = std::fs::symlink_metadata(path) else { return false };
    if me.file_type().is_symlink() {
        return false;
    }
    let Ok(parent) = std::fs::symlink_metadata(path.join("..")) else { return false };
    me.dev() != parent.dev() || me.ino() == parent.ino()
}

/// `os.path.expandvars`: $NAME and ${NAME} where NAME is set; the rest as it was.
pub fn expandvars(s: &str, env: &dyn Fn(&str) -> Option<String>) -> String {
    if !s.contains('$') {
        return s.to_string();
    }
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'$' {
            let (name, end) = if b.get(i + 1) == Some(&b'{') {
                match s[i + 2..].find('}') {
                    Some(close) => (&s[i + 2..i + 2 + close], i + 3 + close),
                    None => ("", i + 1),
                }
            } else {
                let n = s[i + 1..].bytes().take_while(|c| c.is_ascii_alphanumeric() || *c == b'_').count();
                (&s[i + 1..i + 1 + n], i + 1 + n)
            };
            if !name.is_empty() {
                if let Some(v) = env(name) {
                    out.push_str(&v);
                    i = end;
                    continue;
                }
            }
        }
        let ch = s[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// `os.path.expanduser`: ~ and ~/… to HOME, ~user and ~user/… to that user's home.
pub fn expanduser(s: &str, home: &Path) -> String {
    let Some(rest) = s.strip_prefix('~') else { return s.to_string() };
    let (user, tail) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    let base = if user.is_empty() {
        home.to_string_lossy().trim_end_matches('/').to_string()
    } else {
        match std::fs::read_to_string("/etc/passwd").ok().and_then(|p| {
            p.lines().find_map(|l| {
                let f: Vec<&str> = l.split(':').collect();
                (f.len() > 5 && f[0] == user).then(|| f[5].trim_end_matches('/').to_string())
            })
        }) {
            Some(h) => h,
            None => return s.to_string(),
        }
    };
    if base.is_empty() {
        format!("/{}", tail.trim_start_matches('/'))
    } else {
        format!("{base}{tail}")
    }
}

/// Every setting in effect, by upper-case key.
pub fn load(paths: &Paths, home: &Path, env: &dyn Fn(&str) -> Option<String>) -> BTreeMap<String, String> {
    let mut s = BTreeMap::new();
    let dir = videos_dir(home, env);
    for (k, v) in [
        ("DIR", dir.as_str()),
        ("FORMAT", DEFAULT_FORMAT),
        ("PROFILE", "Default"),
        ("KEYRING", ""),
        ("POLL", "1"),
        ("SUBS", DEFAULT_SUBS),
    ] {
        s.insert(k.to_string(), v.to_string());
    }
    for file in [&paths.media_conf, &paths.ytq_config] {
        read_into(file, home, &mut s);
    }
    s
}

fn read_into(file: &Path, home: &Path, s: &mut BTreeMap<String, String>) {
    let Ok(bytes) = std::fs::read(file) else { return };
    for line in String::from_utf8_lossy(&bytes).lines() {
        let line = py_strip(line);
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            s.insert(py_strip(k).to_uppercase(), expanduser(py_strip(v).trim_matches('"'), home));
        }
    }
}

// ------------------------------------------------------------- the table ---

/// Where a value in force came from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Source {
    /// Nothing said, so the built-in fallback.
    Fallback,
    /// `~/.config/copal/media.conf`, read first.
    Media,
    /// `~/.config/ytq/config`, read after, and so the winner.
    Ytq,
    /// Not a `KEY=VALUE` at all: the presence of `~/.config/ytq/auto`.
    File,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::Fallback => "default",
            Source::Media => "media.conf",
            Source::Ytq => "ytq/config",
            Source::File => "ytq/auto",
        }
    }
}

/// How a setting is changed, which is what the Workspace's Settings screen
/// needs to know to offer the right key for it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// One of a short list, in order: Space walks it.
    Cycle(&'static [&'static str]),
    /// Two states, the first meaning on: Space flips it.
    Toggle(&'static str, &'static str),
    /// A path. Typed, and `~` is kept as typed -- the readers expand it.
    Path,
    /// Anything else typed.
    Text,
}

/// A setting a person may reasonably change, what it means, and how.
pub struct Setting {
    pub key: &'static str,
    pub kind: Kind,
    /// One line, for the Settings screen's foot.
    pub help: &'static str,
}

/// EVERY SETTING THE THREE PROGRAMS READ, in the order a person meets them:
/// what a download becomes, then where things go, then how ytq behaves.
///
/// This is the one list. The Settings screen draws it, `sstr config` prints
/// it, and `sstr config set` refuses a key that is not in it -- so a setting
/// cannot be offered in one place and unknown in another, and a typo cannot
/// quietly become a line in a config file that nothing will ever read.
pub const SETTINGS: &[Setting] = &[
    Setting { key: "OUTPUT", kind: Kind::Cycle(&["sstr", "mp4", "both"]), help: "what a finished download leaves: a capture, the video file, or both" },
    Setting { key: "ARCHIVE_DIR", kind: Kind::Path, help: "where captures go; empty means beside the downloads, in DIR" },
    Setting { key: "SSTR_KEY", kind: Kind::Path, help: "the key captures are signed with; empty means unsigned" },
    Setting { key: "SSTR_DEFLATE", kind: Kind::Toggle("yes", "no"), help: "compress a capture's payload as it is recorded" },
    Setting { key: "DIR", kind: Kind::Path, help: "where downloads go" },
    Setting { key: "PLAYER", kind: Kind::Text, help: "what the Workspace's Play sends a video to" },
    Setting { key: "SERVE", kind: Kind::Text, help: "what the Workspace's Serve binds, as HOST:PORT" },
    Setting { key: "AUTOSTART", kind: Kind::Toggle("on", "off"), help: "queueing a URL also starts downloading it, by itself" },
    Setting { key: "SUBS", kind: Kind::Text, help: "caption languages for the transcript; empty asks for none" },
    Setting { key: "COMMENTS", kind: Kind::Text, help: "how many comments a download may fetch; empty asks for none" },
    Setting { key: "POLL", kind: Kind::Text, help: "how often the window looks at the clipboard, in seconds" },
    Setting { key: "FORMAT", kind: Kind::Text, help: "the yt-dlp -f selector" },
    Setting { key: "PROFILE", kind: Kind::Text, help: "the Brave profile yt-brave is given for the cookie retry" },
    Setting { key: "KEYRING", kind: Kind::Text, help: "passed to yt-brave --keyring, for a desktop with no keyring" },
];

pub fn setting(key: &str) -> Option<&'static Setting> {
    SETTINGS.iter().find(|s| s.key.eq_ignore_ascii_case(key))
}

/// AUTOSTART is a FILE, not a key, and it is in the table anyway.
///
/// `~/.config/ytq/auto` exists or it does not; that is the whole setting, and
/// it is the one ytq's own help singles out as the difference between a queue
/// and an appliance. Leaving it out of the table because its storage is
/// unusual would mean a Settings screen that cannot reach the setting people
/// most want to reach. So the storage is unusual and the setting is not: it
/// reads and writes like the others and only `apply` and `in_force` know.
pub fn is_autostart(key: &str) -> bool {
    key.eq_ignore_ascii_case("AUTOSTART")
}

/// Every setting in the table, with the value in force and where it came from.
///
/// NOT `load()`, and deliberately. That map is compared to the Python ytq's
/// `settings()` byte for byte and may hold only what the frozen specification
/// holds. This is the other question -- "what is actually in effect, and who
/// said so" -- which needs the four settings resolved by a function with a
/// fallback rather than by a default string, and needs to say `default` where
/// nothing was written down.
pub fn in_force(paths: &Paths, home: &Path, env: &dyn Fn(&str) -> Option<String>) -> Vec<(&'static str, String, Source)> {
    let s = load(paths, home, env);
    let media = read_keys(&paths.media_conf, home);
    let ytq = read_keys(&paths.ytq_config, home);
    SETTINGS
        .iter()
        .map(|d| {
            if is_autostart(d.key) {
                let on = paths.autostart.exists();
                return (d.key, if on { "on" } else { "off" }.to_string(), if on { Source::File } else { Source::Fallback });
            }
            let from = if ytq.contains_key(d.key) {
                Source::Ytq
            } else if media.contains_key(d.key) {
                Source::Media
            } else {
                Source::Fallback
            };
            let value = match s.get(d.key) {
                Some(v) => v.clone(),
                // The four the specification's map has never heard of: their
                // fallback lives in the function that resolves them, and this
                // asks that function rather than repeating its answer.
                None => match d.key {
                    "OUTPUT" => crate::ytq::runner::output_of(&s).to_string(),
                    "ARCHIVE_DIR" => crate::ytq::runner::archive_dir_of(&s),
                    "SSTR_KEY" => crate::ytq::runner::sstr_key_of(&s, home).map(|k| k.to_string_lossy().into_owned()).unwrap_or_default(),
                    "COMMENTS" => crate::ytq::runner::comments_of(&s),
                    "SSTR_DEFLATE" => "no".to_string(),
                    "PLAYER" => "mpv".to_string(),
                    "SERVE" => "127.0.0.1:8080".to_string(),
                    _ => String::new(),
                },
            };
            (d.key, value, from)
        })
        .collect()
}

/// The `KEY=VALUE` lines of one file, by upper-case key, with nothing else.
fn read_keys(file: &Path, home: &Path) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    read_into(file, home, &mut m);
    m
}

/// Write `KEY=VALUE` into `file`, or take the key out when `value` is None.
///
/// **THE COMMENTS SURVIVE, AND SO DOES EVERYTHING ELSE.** media.conf as Copal
/// installs it is three quarters explanation -- what each mode leaves, why mp4
/// is the one installed, the commented-out shape of the keys not set. A writer
/// that reads the settings and prints them back would throw all of that away
/// the first time anybody pressed a key, and the file would degrade into a
/// list of values with each edit. So this is a line edit: the line that sets
/// the key is replaced where there is one, a line is appended where there is
/// not, and every other byte of the file is the byte it already was.
///
/// A commented-out `#SSTR_KEY=` is not a line that sets the key and is not
/// touched. Appending under it is the honest thing: the comment stays as the
/// documentation it is, and the value below it is what the file now says.
pub fn write_key(file: &Path, key: &str, value: Option<&str>) -> std::io::Result<()> {
    let key = key.to_uppercase();
    let existing = std::fs::read(file).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    let ends_newline = existing.is_empty() || existing.ends_with('\n');
    let mut out: Vec<String> = Vec::new();
    let mut replaced = false;
    for line in existing.lines() {
        let trimmed = py_strip(line);
        let is_this_key = !trimmed.starts_with('#')
            && trimmed
                .split_once('=')
                .map_or(false, |(k, _)| py_strip(k).eq_ignore_ascii_case(&key));
        if !is_this_key {
            out.push(line.to_string());
            continue;
        }
        match value {
            // The first one is replaced in place, so a setting keeps the
            // company of whatever comment was written above it; a second line
            // setting the same key is a duplicate the reader already ignored,
            // and it goes.
            Some(v) if !replaced => {
                out.push(format!("{key}={v}"));
                replaced = true;
            }
            _ => {}
        }
    }
    if let (Some(v), false) = (value, replaced) {
        out.push(format!("{key}={v}"));
    }
    let mut text = out.join("\n");
    if !text.is_empty() && (ends_newline || value.is_some()) {
        text.push('\n');
    }
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Written whole, through a temporary in the same folder, so an
    // interrupted write cannot leave half a settings file behind.
    let tmp = file.with_extension("conf.new");
    std::fs::write(&tmp, text.as_bytes())?;
    std::fs::rename(&tmp, file)
}

/// The settings of this process: its HOME, its environment, its files.
pub fn from_env() -> Option<(Paths, PathBuf, BTreeMap<String, String>)> {
    let paths = Paths::from_env()?;
    let home = PathBuf::from(std::env::var_os("HOME")?);
    let env = |k: &str| std::env::var(k).ok();
    let s = load(&paths, &home, &env);
    Some((paths, home, s))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("sstr-settings-{}-{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn defaults_then_media_conf_then_ytq_config() {
        let home = scratch("order");
        let paths = Paths::from_lookup(|k| (k == "HOME").then(|| home.clone())).unwrap();
        std::fs::create_dir_all(paths.media_conf.parent().unwrap()).unwrap();
        std::fs::create_dir_all(paths.ytq_config.parent().unwrap()).unwrap();
        std::fs::write(&paths.media_conf, "# shared\nOUTPUT=sstr\ndir = \"~/Archive\"\nPOLL=2\n").unwrap();
        std::fs::write(&paths.ytq_config, "  poll = 3  \nnot a setting\nSUBS=\n").unwrap();
        let env = |_: &str| None;
        let s = load(&paths, &home, &env);
        assert_eq!(s["DIR"], format!("{}/Archive", home.display()));
        assert_eq!(s["POLL"], "3");
        assert_eq!(s["OUTPUT"], "sstr");
        assert_eq!(s["SUBS"], "");
        assert_eq!(s["FORMAT"], DEFAULT_FORMAT);
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn videos_dir_falls_through_an_unmounted_share() {
        let home = scratch("share");
        let empty = home.join("mnt-share");
        std::fs::create_dir_all(&empty).unwrap();
        std::fs::create_dir_all(home.join("Downloads")).unwrap();
        std::os::unix::fs::symlink(&empty, home.join("Downloads/SharedVM")).unwrap();
        let env = |k: &str| (k == "HOME").then(|| home.to_string_lossy().into_owned());
        assert_eq!(videos_dir(&home, &env), format!("{}/Videos", home.display()));
        std::fs::create_dir_all(home.join(".config")).unwrap();
        std::fs::write(home.join(".config/user-dirs.dirs"), "XDG_VIDEOS_DIR=\"$HOME/Films\"\n").unwrap();
        assert_eq!(videos_dir(&home, &env), format!("{}/Films", home.display()));
        let _ = std::fs::remove_dir_all(home);
    }

    /// The file people actually have: Copal's media.conf is three quarters
    /// explanation, and an editor that keeps the values and loses the reasons
    /// is an editor that degrades the file with every keypress.
    #[test]
    fn a_written_key_costs_the_file_nothing_but_that_key() {
        let home = scratch("write");
        let f = home.join("media.conf");
        let before = "# SPDX-License-Identifier: MIT\n\
                      # WHAT A FINISHED DOWNLOAD LEAVES.\n\
                      #   mp4   the video file\n\
                      OUTPUT=mp4\n\
                      \n\
                      # The key captures are signed with.\n\
                      #SSTR_KEY=\n";
        std::fs::write(&f, before).unwrap();

        write_key(&f, "OUTPUT", Some("both")).unwrap();
        let after = std::fs::read_to_string(&f).unwrap();
        assert_eq!(after, before.replace("OUTPUT=mp4", "OUTPUT=both"), "only that one line moved:\n{after}");

        // A key the file does not set is appended. The commented-out
        // `#SSTR_KEY=` is documentation, not a setting, and is left as it is.
        write_key(&f, "SSTR_KEY", Some("~/.ssh/other")).unwrap();
        let after = std::fs::read_to_string(&f).unwrap();
        assert!(after.contains("#SSTR_KEY=\n"), "the comment stays: {after}");
        assert!(after.ends_with("SSTR_KEY=~/.ssh/other\n"), "{after}");

        // Unset takes the line out and leaves everything else.
        write_key(&f, "SSTR_KEY", None).unwrap();
        let after = std::fs::read_to_string(&f).unwrap();
        assert!(!after.contains("SSTR_KEY=~/.ssh/other"), "{after}");
        assert!(after.contains("# The key captures are signed with.\n#SSTR_KEY=\n"), "{after}");
        assert!(after.contains("# WHAT A FINISHED DOWNLOAD LEAVES."), "{after}");

        // A file that sets a key twice: the reader took the last one, so the
        // writer leaves one line, in the place the first one held -- with
        // whatever comment was written above it.
        std::fs::write(&f, "# why\nPOLL=1\nSUBS=en\nPOLL=9\n").unwrap();
        write_key(&f, "POLL", Some("3")).unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "# why\nPOLL=3\nSUBS=en\n");

        // And a file that is not there yet is a file with one line in it.
        let fresh = home.join("new").join("media.conf");
        write_key(&fresh, "OUTPUT", Some("sstr")).unwrap();
        assert_eq!(std::fs::read_to_string(&fresh).unwrap(), "OUTPUT=sstr\n");
        let _ = std::fs::remove_dir_all(home);
    }

    /// Which file said so, which is the question `load` cannot answer and the
    /// one that stops a settings screen being a trap.
    #[test]
    fn what_is_in_force_says_who_said_it() {
        let home = scratch("force");
        let paths = Paths::from_lookup(|k| (k == "HOME").then(|| home.clone())).unwrap();
        std::fs::create_dir_all(paths.media_conf.parent().unwrap()).unwrap();
        std::fs::create_dir_all(paths.ytq_config.parent().unwrap()).unwrap();
        std::fs::write(&paths.media_conf, "OUTPUT=mp4\nPLAYER=mpv --fs\n").unwrap();
        std::fs::write(&paths.ytq_config, "OUTPUT=sstr\n").unwrap();
        let env = |_: &str| None;
        let got = |k: &str| {
            in_force(&paths, &home, &env).into_iter().find(|(n, _, _)| *n == k).map(|(_, v, s)| (v, s)).unwrap()
        };
        // Set in both: ytq's file is read after, so ytq's file is the answer.
        assert_eq!(got("OUTPUT"), ("sstr".into(), Source::Ytq));
        assert_eq!(got("PLAYER"), ("mpv --fs".into(), Source::Media));
        // Set nowhere: the fallback, and said to be one -- including the four
        // the specification's map has never heard of.
        assert_eq!(got("SERVE"), ("127.0.0.1:8080".into(), Source::Fallback));
        assert_eq!(got("COMMENTS"), (DEFAULT_COMMENTS.to_string(), Source::Fallback));
        assert_eq!(got("SSTR_DEFLATE"), ("no".into(), Source::Fallback));
        // ARCHIVE_DIR falls back to DIR rather than to nothing.
        assert_eq!(got("ARCHIVE_DIR").0, got("DIR").0);
        // AUTOSTART is a file, and reads like every other setting anyway.
        assert_eq!(got("AUTOSTART"), ("off".into(), Source::Fallback));
        std::fs::write(&paths.autostart, "").unwrap();
        assert_eq!(got("AUTOSTART"), ("on".into(), Source::File));
        // Every setting offered is a setting that can be looked up by name.
        for d in SETTINGS {
            assert!(setting(d.key).is_some(), "{} is in the table and not findable", d.key);
        }
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn expansions() {
        let env = |k: &str| (k == "A").then(|| "x".to_string());
        assert_eq!(expandvars("$A/${A}/$B/${B}/$", &env), "x/x/$B/${B}/$");
        assert_eq!(expanduser("~/v", Path::new("/h")), "/h/v");
        assert_eq!(expanduser("~", Path::new("/h")), "/h");
        assert_eq!(expanduser("a~", Path::new("/h")), "a~");
        assert!(is_mount(Path::new("/")));
    }
}
