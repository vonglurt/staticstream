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
