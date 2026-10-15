// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Paul Richeson

//! Warming: what a person does by hand when YouTube answers 429 to a caption
//! request -- open the video in Brave, let it play, try again -- done by ytq.
//!
//! A video that has played in Brave a moment ago is one YouTube will hand the
//! captions of to Brave's cookies; the same request with no history behind it
//! is refused. So: `brave --new-window URL`, the new window found by the
//! addresses Hyprland lists before and after, sent out of sight to a special
//! workspace, its sound muted the moment pactl sees the stream, and -- if
//! nothing plays within PLAY_WAIT -- a `k`, YouTube's play key, sent to that
//! window and no other. After PLAY_FOR of playback the caller tries again with
//! Brave's cookies, then closes the window it was given here.
//!
//! **THE WINDOW IS CLOSED BY ADDRESS, NEVER BY TITLE.** A title is what the
//! page says it is; a person may have the same video open in a window of their
//! own, and closing by title would close that. The address is the one window
//! that appeared after `--new-window`, and nothing else.
//!
//! Without Hyprland the window is opened and left open, and the log says so:
//! there is no address to send a key to or to close.

use std::collections::BTreeMap;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use crate::format::json::{self, Value};
use crate::ytq::runner::{run_timeout, Ran};

/// How long playback may take to start before `k` is sent, in seconds.
const PLAY_WAIT: u64 = 10;
/// How long the video plays before the retry, in seconds.
const PLAY_FOR: u64 = 10;
/// How long the new window may take to appear, in seconds.
const WINDOW_WAIT: u64 = 15;
/// Where the window goes while it plays: a special workspace, out of sight.
const HIDE: &str = "special:ytq";

/// Is WARM on? It is unless it says otherwise.
pub fn warm_of(s: &BTreeMap<String, String>) -> bool {
    !matches!(s.get("WARM").map(|v| v.trim().to_lowercase()).as_deref(), Some("off" | "no" | "0" | "false"))
}

/// Does yt-dlp's complaint read like YouTube's rate limit?
pub fn rate_limited(err: &str) -> bool {
    err.contains("429") || err.to_lowercase().contains("too many requests")
}

/// The window opened for one video.
pub struct Warm {
    address: Option<String>,
}

/// Open url in a new Brave window, out of sight, muted, and playing.
pub fn open(url: &str, log: &dyn Fn(&str)) -> Warm {
    let windows = brave_windows_now();
    let streams: Vec<u64> = brave_streams_now().into_iter().map(|(i, _)| i).collect();
    let spawned = [vec!["brave", "--new-window", url], vec!["flatpak", "run", "com.brave.Browser", "--new-window", url]]
        .iter()
        .any(|cmd| Command::new(cmd[0]).args(&cmd[1..]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).process_group(0).spawn().is_ok());
    if !spawned {
        log("cannot run brave; trying Brave's cookies without it");
        return Warm { address: None };
    }
    let address = match &windows {
        Some(before) => {
            let t0 = Instant::now();
            loop {
                let found = brave_windows_now().and_then(|now| now.into_iter().find(|a| !before.contains(a)));
                if found.is_some() || t0.elapsed() > Duration::from_secs(WINDOW_WAIT) {
                    break found;
                }
                sleep(Duration::from_millis(250));
            }
        }
        None => None,
    };
    match &address {
        Some(a) => {
            hyprctl(&["dispatch", "movetoworkspacesilent", &format!("{HIDE},address:{a}")]);
            log(&format!("opened Brave on it, window {a}, on {HIDE}"));
        }
        None if windows.is_some() => log(&format!("opened Brave on it, but no new window appeared in {WINDOW_WAIT} s")),
        None => log("opened Brave on it; no Hyprland, so it is not hidden and will not be closed"),
    }
    // Playing is a Brave stream that was not there before and is not corked.
    let mut muted: Vec<u64> = Vec::new();
    let mut playing = |muted: &mut Vec<u64>| {
        let mut any = false;
        for (i, corked) in brave_streams_now().into_iter().filter(|(i, _)| !streams.contains(i)) {
            if !muted.contains(&i) {
                let _ = run_timeout(Command::new("pactl").args(["set-sink-input-mute", &i.to_string(), "1"]), 5);
                muted.push(i);
            }
            any |= !corked;
        }
        any
    };
    let wait = |secs: u64, muted: &mut Vec<u64>, playing: &mut dyn FnMut(&mut Vec<u64>) -> bool| {
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(secs) {
            if playing(muted) {
                return true;
            }
            sleep(Duration::from_millis(250));
        }
        false
    };
    let mut started = wait(PLAY_WAIT, &mut muted, &mut playing);
    if !started {
        if let Some(a) = &address {
            log(&format!("nothing played in {PLAY_WAIT} s; sending k"));
            hyprctl(&["dispatch", "sendshortcut", &format!(",k,address:{a}")]);
            started = wait(PLAY_WAIT, &mut muted, &mut playing);
        }
    }
    if started {
        log(&format!("playing, muted; trying again in {PLAY_FOR} s"));
        // Keep muting: YouTube may open a second stream for the video after an ad.
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(PLAY_FOR) {
            playing(&mut muted);
            sleep(Duration::from_millis(250));
        }
    } else {
        log("it never played; trying Brave's cookies anyway");
    }
    Warm { address }
}

/// Close the window open() opened, and only that one.
///
/// **BRAVE ACTS ON A CLOSE ONLY ONCE THE WINDOW HAS BEEN DRAWN.** A window
/// that has spent its life on a hidden workspace takes the request, Hyprland
/// says ok, and nothing happens until a person looks at it (seen on the guest,
/// Brave 1.x flatpak, Hyprland 0.54). So it is brought to the workspace on
/// screen first -- floating and small, so nothing else is laid out again --
/// and the close is believed only when the window is gone from the list.
pub fn close(w: &Warm, log: &dyn Fn(&str)) {
    let Some(a) = &w.address else { return };
    let target = format!("address:{a}");
    if let Some(ws) = hyprctl(&["activeworkspace", "-j"]).and_then(|o| json::parse(&o).ok()).and_then(|v| v.get("id")?.as_num()) {
        hyprctl(&["dispatch", "setfloating", &target]);
        hyprctl(&["dispatch", "resizewindowpixel", &format!("exact 320 180,{target}")]);
        hyprctl(&["dispatch", "movetoworkspacesilent", &format!("{ws},{target}")]);
        sleep(Duration::from_millis(500));
    }
    for _ in 0..2 {
        hyprctl(&["dispatch", "closewindow", &target]);
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(5) {
            if brave_windows_now().is_some_and(|now| !now.contains(a)) {
                log(&format!("closed window {a}"));
                return;
            }
            sleep(Duration::from_millis(250));
        }
    }
    log(&format!("asked window {a} to close twice and it is still open; close it by hand"));
}

/// hyprctl's answer, or None where there is no Hyprland to ask.
fn hyprctl(args: &[&str]) -> Option<String> {
    std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE")?;
    match run_timeout(Command::new("hyprctl").args(args), 5) {
        Ran::Done(0, out, _) => Some(out),
        _ => None,
    }
}

fn brave_windows_now() -> Option<Vec<String>> {
    hyprctl(&["clients", "-j"]).map(|out| brave_windows(&out))
}

fn brave_streams_now() -> Vec<(u64, bool)> {
    match run_timeout(Command::new("pactl").args(["-f", "json", "list", "sink-inputs"]), 5) {
        Ran::Done(0, out, _) => brave_streams(&out),
        _ => Vec::new(),
    }
}

/// The addresses of Brave's windows in `hyprctl clients -j`.
fn brave_windows(out: &str) -> Vec<String> {
    let all = json::parse(out).ok();
    let clients = all.as_ref().and_then(|v| v.as_array()).unwrap_or(&[]);
    clients
        .iter()
        .filter(|c| c.get("class").and_then(Value::as_str).is_some_and(|k| k.to_lowercase().contains("brave")))
        .filter_map(|c| c.get("address").and_then(Value::as_str).map(str::to_string))
        .collect()
}

/// Brave's streams in `pactl -f json list sink-inputs`: (index, corked).
fn brave_streams(out: &str) -> Vec<(u64, bool)> {
    let all = json::parse(out).ok();
    let inputs = all.as_ref().and_then(|v| v.as_array()).unwrap_or(&[]);
    inputs
        .iter()
        .filter(|i| {
            let p = |k: &str| i.get("properties").and_then(|p| p.get(k)).and_then(Value::as_str).unwrap_or("").to_lowercase();
            p("application.name").contains("brave") || p("application.process.binary").contains("brave")
        })
        .filter_map(|i| Some((i.get("index")?.as_u64()?, i.get("corked").and_then(Value::as_bool).unwrap_or(false))))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warm_is_on_unless_turned_off() {
        let s = |v: Option<&str>| v.map(|v| BTreeMap::from([("WARM".to_string(), v.to_string())])).unwrap_or_default();
        assert!(warm_of(&s(None)));
        assert!(warm_of(&s(Some("on"))));
        assert!(warm_of(&s(Some(""))));
        assert!(!warm_of(&s(Some("off"))));
        assert!(!warm_of(&s(Some(" Off "))));
        assert!(!warm_of(&s(Some("0"))));
    }

    #[test]
    fn a_429_is_a_rate_limit_and_a_404_is_not() {
        assert!(rate_limited("ERROR: Unable to download video subtitles for 'en': HTTP Error 429: Too Many Requests"));
        assert!(rate_limited("too many requests"));
        assert!(!rate_limited("ERROR: HTTP Error 404: Not Found"));
        assert!(!rate_limited("no captions matching en"));
    }

    #[test]
    fn only_brave_windows_are_listed() {
        let out = r#"[{"address": "0xa1", "class": "radbeeper-gui", "title": "radbeeper"},
                      {"address": "0xb2", "class": "brave-browser", "title": "Me at the zoo - YouTube - Brave"}]"#;
        assert_eq!(brave_windows(out), vec!["0xb2".to_string()]);
        assert!(brave_windows("not json").is_empty());
    }

    #[test]
    fn only_brave_streams_are_listed_with_their_cork() {
        // Trimmed from the guest's pactl 17.0, the Brave flatpak playing a video.
        let out = r#"[{"index":411,"corked":false,"mute":false,"properties":{"application.name":"Brave","application.process.binary":"brave"}},
                      {"index":412,"corked":true,"properties":{"application.name":"mpv"}},
                      {"index":413,"corked":true,"properties":{"application.process.binary":"brave"}}]"#;
        assert_eq!(brave_streams(out), vec![(411, false), (413, true)]);
        assert!(brave_streams("[]").is_empty());
    }
}
