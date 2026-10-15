// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Paul Richeson

//! Starting Brave without the tabs it had last time.
//!
//! Brave's own default is "Continue where you left off". So a `brave URL`
//! from ytq -- a sign-in page, or a video to warm -- brought back every tab of
//! the last session as well, and the window ytq meant to show or close was
//! one among many. The Flatpak has no policy directory to say otherwise in,
//! and there is no command-line flag for it, so the setting is changed in the
//! profile: `session.restore_on_startup` 1 (or unset, which is Brave's 1)
//! becomes 5, "Open the New Tab page". Brave is not told anything else: a
//! choice of 4, a list of pages, is a person's and is left alone.
//!
//! **ONLY WHILE BRAVE IS NOT RUNNING.** A running Brave writes its Preferences
//! back over any edit, and it reads them only at start; the edit is for the
//! start that ytq is about to cause.
//!
//! **THE FILE IS EDITED AS TEXT, NOT REWRITTEN.** Its numbers include doubles
//! such as `1.0`, which a parse and print through `json::Value` would hand
//! back as `1`, an integer, to a browser that types its prefs. So the one
//! value is spliced in, and the result is parsed to prove it says what was
//! meant and nothing else changed. A file this cannot place the edit in
//! safely is left as it was, and the log says so.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant, UNIX_EPOCH};

use crate::format::json::{self, Value};
use crate::ytq::runner::{brave_cmd, run_timeout, Ran};
use crate::ytq::sys::strftime_local;
use crate::ytq::{warm, Paths};

/// Brave's "Continue where you left off".
const LAST_SESSION: f64 = 1.0;
/// Brave's "Open the New Tab page".
const NEW_TAB: u32 = 5;

/// The two ways Brave is started: Copal's wrapper, then the Flatpak itself.
pub const LAUNCHERS: [&[&str]; 2] = [&["brave"], &["flatpak", "run", "com.brave.Browser"]];

/// Start Brave on `args`, from the wrapper or the Flatpak, after making sure
/// it will not bring the last session back with it. The launcher that
/// started, if one did.
pub fn launch(s: &BTreeMap<String, String>, args: &[&str], log: &dyn Fn(&str)) -> Option<&'static [&'static str]> {
    fresh_start(s, log);
    LAUNCHERS.into_iter().find(|pre| {
        Command::new(pre[0]).args(&pre[1..]).args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).process_group(0).spawn().is_ok()
    })
}

/// Is Brave running? The browser itself, native or inside the Flatpak's
/// sandbox, is `brave`; the Flatpak's launcher is flatpak or bwrap with the
/// app id as one of its arguments. An argument, not a substring: a shell, an
/// editor or a grep whose command line merely mentions the id is not Brave.
pub fn running() -> bool {
    let Ok(dir) = fs::read_dir("/proc") else { return false };
    dir.flatten().filter(|e| e.file_name().to_str().is_some_and(|n| n.bytes().all(|b| b.is_ascii_digit()))).any(|e| {
        let p = e.path();
        fs::read_to_string(p.join("comm")).is_ok_and(|c| c.trim_end() == "brave")
            || fs::read(p.join("cmdline")).is_ok_and(|c| launcher(&c))
    })
}

/// A NUL-separated command line that is the Flatpak starting Brave.
fn launcher(cmdline: &[u8]) -> bool {
    let mut args = cmdline.split(|b| *b == 0);
    let first = args.next().unwrap_or_default();
    let name = first.rsplit(|b| *b == b'/').next().unwrap_or_default();
    matches!(name, b"flatpak" | b"bwrap") && args.any(|a| a == b"com.brave.Browser")
}

/// Before ytq starts Brave: if it is not running, have it open on a new tab
/// rather than on the last session's tabs.
fn fresh_start(s: &BTreeMap<String, String>, log: &dyn Fn(&str)) {
    if running() {
        return;
    }
    let Some(dir) = profile_dir(s) else { return };
    let prefs = dir.join("Preferences");
    let Ok(text) = fs::read_to_string(&prefs) else { return };
    match no_restore(&text) {
        Ok(None) => {}
        Ok(Some(new)) => {
            let tmp = prefs.with_file_name("Preferences.ytq");
            match fs::write(&tmp, new).and_then(|_| fs::rename(&tmp, &prefs)) {
                Ok(()) => log("Brave set to start on a new tab, not on the last session's tabs"),
                Err(e) => {
                    let _ = fs::remove_file(&tmp);
                    log(&format!("Brave's Preferences could not be written ({e}); it may reopen the last session's tabs"));
                }
            }
        }
        Err(why) => log(&format!("Brave's Preferences left as they are ({why}); it may reopen the last session's tabs")),
    }
}

/// The Brave profile yt-brave reads, as `yt-brave --path` names it.
fn profile_dir(s: &BTreeMap<String, String>) -> Option<PathBuf> {
    let cmd = brave_cmd(s);
    match run_timeout(Command::new(&cmd[0]).args(&cmd[1..]).arg("--path"), 10) {
        Ran::Done(0, out, _) if !out.trim().is_empty() => Some(PathBuf::from(out.trim())),
        _ => None,
    }
}

/// When Brave last wrote its cookie database, in seconds since the epoch.
fn cookies_written(file: &Path) -> Option<f64> {
    let t = fs::metadata(file).ok()?.modified().ok()?;
    Some(t.duration_since(UNIX_EPOCH).ok()?.as_secs_f64())
}

/// The sign-ins ytq has opened Brave for and not yet retried: one line each,
/// `URL TAB opened TAB window`, beside the queue. A file of its own and not
/// fields of the entry, because the queue is the Python ytq's format byte for
/// byte (A3), and the Python ytq never watched Brave's cookies.
fn signins(paths: &Paths) -> PathBuf {
    paths.queue.with_file_name("brave-signin")
}

fn read_signins(paths: &Paths) -> Vec<(String, f64, String)> {
    fs::read_to_string(signins(paths))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let mut f = l.split('\t');
            Some((f.next()?.to_string(), f.next()?.parse().ok()?, f.next().unwrap_or("").to_string()))
        })
        .collect()
}

fn write_signins(paths: &Paths, rows: &[(String, f64, String)]) {
    let path = signins(paths);
    if rows.is_empty() {
        let _ = fs::remove_file(path);
        return;
    }
    let text: String = rows.iter().map(|(u, t, w)| format!("{u}\t{t}\t{w}\n")).collect();
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let tmp = path.with_extension("tmp");
    if fs::write(&tmp, text).is_ok() {
        let _ = fs::rename(&tmp, &path);
    }
}

/// Note that Brave was opened on `url` for a sign-in at `opened`, and, once
/// Hyprland shows it, which window that was.
pub fn record_signin(paths: &Paths, url: &str, opened: f64, window: Option<&str>) {
    let mut rows: Vec<_> = read_signins(paths).into_iter().filter(|(u, _, _)| u != url).collect();
    rows.push((url.to_string(), opened, window.unwrap_or("").to_string()));
    write_signins(paths, &rows);
}

/// The sign-in noted for `url`, taken off the list: when, and in which window.
pub fn take_signin(paths: &Paths, url: &str) -> Option<(f64, Option<String>)> {
    let rows = read_signins(paths);
    let found = rows.iter().find(|(u, _, _)| u == url).map(|(_, t, w)| (*t, (!w.is_empty()).then(|| w.clone())));
    if found.is_some() {
        write_signins(paths, &rows.into_iter().filter(|(u, _, _)| u != url).collect::<Vec<_>>());
    }
    found
}

/// How long Brave may take to write its cookies once its window is closed.
const SETTLE: u64 = 30;

/// Before the cookie retry of a sign-in opened at `opened`: have Brave's
/// cookies been written since? Brave keeps them in memory and writes the file
/// about every 30 s, and when it closes, so a retry pressed straight after
/// signing in can read a file that does not have the sign-in in it yet --
/// which is the same failure again, looking like a sign-in that did not work.
///
/// **This watches a time, not a cookie.** A newer file says Brave wrote its
/// cookies after the page opened, which a sign-in makes it do; it does not
/// say the sign-in is among them. It is the one thing that can be known
/// without reading the cookies, and it tells the two failures apart.
pub fn settle(s: &BTreeMap<String, String>, opened: f64, window: Option<&str>, log: &dyn Fn(&str)) {
    // No cookie file is no Brave profile to wait on; yt-brave says so itself.
    let Some(file) = profile_dir(s).map(|d| d.join("Cookies")).filter(|f| f.exists()) else { return };
    let clock = |t: f64| strftime_local("%H:%M:%S", t);
    let fresh = || cookies_written(&file).filter(|t| *t > opened);
    if let Some(t) = fresh() {
        log(&format!("Brave's cookies were written at {}, after the sign-in page opened at {}", clock(t), clock(opened)));
        return;
    }
    if running() {
        match window {
            Some(w) => {
                log(&format!("Brave has not written its cookies since the sign-in page opened at {}; closing that window so it does", clock(opened)));
                warm::close_window(w, log);
            }
            None => log(&format!("Brave has not written its cookies since the sign-in page opened at {}; waiting up to {SETTLE} s for it to", clock(opened))),
        }
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(SETTLE) {
            if let Some(t) = fresh() {
                log(&format!("Brave's cookies were written at {}", clock(t)));
                return;
            }
            // A Brave that has exited has written all it had: the file is final.
            if !running() {
                break;
            }
            sleep(Duration::from_millis(500));
        }
    }
    let was = cookies_written(&file).map_or("?".to_string(), clock);
    log(&format!("Brave's cookies are unchanged since {was}, before the sign-in page opened at {}: no sign-in has reached them; trying anyway", clock(opened)));
}

/// `Preferences` with restore-on-startup turned from the last session to a
/// new tab: `Ok(None)` when there is nothing to change, `Err` when the edit
/// cannot be placed with certainty.
pub fn no_restore(text: &str) -> Result<Option<String>, &'static str> {
    let before = json::parse(text).map_err(|_| "not JSON")?;
    let Value::Obj(top) = &before else { return Err("not a JSON object") };
    let session = before.get("session");
    let current = session.and_then(|s| s.get("restore_on_startup"));
    match current {
        Some(Value::Num(n)) if *n == LAST_SESSION => {}
        None => {}
        _ => return Ok(None),
    }
    let new = if current.is_some() {
        let key = "\"restore_on_startup\":";
        let [at] = text.match_indices(key).map(|(i, _)| i + key.len()).collect::<Vec<_>>()[..] else { return Err("restore_on_startup is not in it exactly once") };
        let end = at + text[at..].find(|c: char| !c.is_ascii_digit()).ok_or("restore_on_startup has no end")?;
        format!("{}{NEW_TAB}{}", &text[..at], &text[end..])
    } else if session.is_some() {
        let key = "\"session\":{";
        let [at] = text.match_indices(key).map(|(i, _)| i + key.len()).collect::<Vec<_>>()[..] else { return Err("\"session\":{ is not in it exactly once") };
        let sep = if text[at..].trim_start().starts_with('}') { "" } else { "," };
        format!("{}\"restore_on_startup\":{NEW_TAB}{sep}{}", &text[..at], &text[at..])
    } else {
        let at = text.find('{').ok_or("no opening brace")? + 1;
        let sep = if top.is_empty() { "" } else { "," };
        format!("{}\"session\":{{\"restore_on_startup\":{NEW_TAB}}}{sep}{}", &text[..at], &text[at..])
    };
    // The proof: the new value is where it was meant to be, and every other
    // setting reads as it did.
    let after = json::parse(&new).map_err(|_| "the edit did not leave JSON")?;
    let strip = |v: &Value| -> String {
        let Value::Obj(m) = v else { return String::new() };
        let m: Vec<(String, Value)> = m
            .iter()
            .filter_map(|(k, v)| match (k.as_str(), v) {
                ("session", Value::Obj(s)) => {
                    let s: Vec<(String, Value)> = s.iter().filter(|(k, _)| k != "restore_on_startup").cloned().collect();
                    (!s.is_empty()).then(|| (k.clone(), Value::Obj(s)))
                }
                _ => Some((k.clone(), v.clone())),
            })
            .collect();
        Value::Obj(m).to_python_json(None)
    };
    let ok = after.get("session").and_then(|s| s.get("restore_on_startup")).and_then(Value::as_num) == Some(NEW_TAB.into()) && strip(&before) == strip(&after);
    if ok {
        Ok(Some(new))
    } else {
        Err("the edit changed more than the one setting")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_session_becomes_a_new_tab() {
        let t = r#"{"a":{"b":1.0},"session":{"restore_on_startup":1,"startup_urls":[]},"z":2}"#;
        assert_eq!(no_restore(t).unwrap().unwrap(), r#"{"a":{"b":1.0},"session":{"restore_on_startup":5,"startup_urls":[]},"z":2}"#);
    }

    #[test]
    fn unset_is_brave_s_default_and_is_set() {
        assert_eq!(no_restore(r#"{"session":{"startup_urls":[]}}"#).unwrap().unwrap(), r#"{"session":{"restore_on_startup":5,"startup_urls":[]}}"#);
        assert_eq!(no_restore(r#"{"session":{}}"#).unwrap().unwrap(), r#"{"session":{"restore_on_startup":5}}"#);
        assert_eq!(no_restore(r#"{"a":1.5}"#).unwrap().unwrap(), r#"{"session":{"restore_on_startup":5},"a":1.5}"#);
        assert_eq!(no_restore("{}").unwrap().unwrap(), r#"{"session":{"restore_on_startup":5}}"#);
    }

    #[test]
    fn a_person_s_choice_is_left_alone() {
        assert_eq!(no_restore(r#"{"session":{"restore_on_startup":5}}"#).unwrap(), None);
        assert_eq!(no_restore(r#"{"session":{"restore_on_startup":4,"startup_urls":["https://example.org/"]}}"#).unwrap(), None);
    }

    #[test]
    fn an_edit_it_cannot_place_is_refused() {
        // The key twice, the second in a setting of some other part of Brave.
        assert!(no_restore(r#"{"other":{"restore_on_startup":1},"session":{"restore_on_startup":1}}"#).is_err());
        // Spaced as Brave never writes it.
        assert!(no_restore(r#"{"session": {"startup_urls": []}}"#).is_err());
        assert!(no_restore("not json").is_err());
    }

    #[test]
    fn a_sign_in_is_noted_once_and_taken_once() {
        let home = std::env::temp_dir().join(format!("ytq-signin-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        let paths = Paths::from_lookup(|k| (k == "HOME").then(|| home.clone())).unwrap();
        fs::create_dir_all(paths.queue.parent().unwrap()).unwrap();
        record_signin(&paths, "https://a/1", 100.5, None);
        record_signin(&paths, "https://a/2", 200.0, None);
        record_signin(&paths, "https://a/1", 300.0, Some("0xabc"));
        assert_eq!(take_signin(&paths, "https://a/1"), Some((300.0, Some("0xabc".to_string()))));
        assert_eq!(take_signin(&paths, "https://a/1"), None);
        assert_eq!(take_signin(&paths, "https://a/2"), Some((200.0, None)));
        assert!(!signins(&paths).exists());
        let _ = fs::remove_dir_all(&home);
    }


    #[test]
    fn only_the_flatpak_starting_brave_is_brave() {
        assert!(launcher(b"/usr/bin/flatpak\0run\0com.brave.Browser\0https://x/\0"));
        assert!(launcher(b"bwrap\0--args\x0042\0com.brave.Browser\0"));
        assert!(!launcher(b"/bin/bash\0-c\0pgrep -f com.brave.Browser\0"));
        assert!(!launcher(b"grep\0com.brave.Browser\0"));
    }

}
