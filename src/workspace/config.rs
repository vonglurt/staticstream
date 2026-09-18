// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Paul Richeson

//! Settings: the config files as a screen, and every change a command line.
//!
//! **THIS PANEL WRITES NO FILE.** It builds `sstr config set KEY VALUE`, says
//! that line into the Transcript, runs it, and says back what it printed. The
//! rule Services follow -- a key press is a message you could have typed, and
//! the line is said before it runs -- is not relaxed because the object being
//! acted on happens to be a setting rather than a capture.
//!
//! Three things follow, and they are the reason it is built this way rather
//! than with a `write_key` call four lines shorter:
//!
//! - **`sstr config set OUTPUT mp4` is the whole feature**, and this is a way
//!   of sending it with one key. Somebody with no terminal gets a screen;
//!   somebody writing a setup script gets the same thing, already written.
//! - **What the panel can do is bounded by what the command can do.** Its
//!   refusals are the command's refusals -- a value outside a Cycle's list is
//!   rejected in one place, not two -- and so is its one piece of real advice,
//!   which no settings editor that wrote files itself would ever have thought
//!   to give: media.conf is read BEFORE ytq's own config, so a key set in both
//!   is a key the panel just changed with no effect at all. The command says
//!   so, and the panel shows it saying so.
//! - **The Transcript ends up holding the session's changes** as lines that can
//!   be read back, copied out, and run somewhere else.
//!
//! The one thing it does not borrow from Services is [`How::Terminal`]. Verify
//! and Export take the whole terminal because they write eleven lines; a set
//! writes one, or three when it has something to warn about, and suspending the
//! screen for that would make the panel unusable at exactly the moment it is
//! most useful -- walking down the list changing three things in a row.

use std::process::Command;

use crate::ytq::settings::{Kind, Setting, Source, SETTINGS};
use crate::ytq::term::{self, cut, ljust, Frame, Key, Keys, Screen, Style};
use crate::ytq::Paths;
use crate::workspace::transcript::Transcript;

/// Which file a change is written to. media.conf by default, because it is the
/// one the other two programs also read; `w` switches, and the screen says
/// which is in hand at all times rather than remembering it silently.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Into_ {
    Media,
    Ytq,
}

impl Into_ {
    fn arg(self) -> &'static str {
        match self {
            Into_::Media => "media",
            Into_::Ytq => "ytq",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Into_::Media => "~/.config/copal/media.conf   (sstr, ytq and the Workspace)",
            Into_::Ytq => "~/.config/ytq/config          (ytq alone -- read after, so it wins)",
        }
    }
}

/// What Space does to a setting: the next value, or None where Space means
/// nothing because the value is typed rather than chosen.
fn next_value(d: &Setting, now: &str) -> Option<String> {
    match d.kind {
        Kind::Cycle(list) => {
            let at = list.iter().position(|v| v.eq_ignore_ascii_case(now)).unwrap_or(list.len() - 1);
            Some(list[(at + 1) % list.len()].to_string())
        }
        Kind::Toggle(on, off) => Some(if now.eq_ignore_ascii_case(on) { off } else { on }.to_string()),
        Kind::Path | Kind::Text => None,
    }
}

/// A word as a shell needs it written. The same rule as `services::quote`, and
/// for the same reason: a PLAYER of `mpv --fs` or a folder with a space in it
/// has to survive being pasted into a terminal or the line is not a command.
fn quote(s: &str) -> String {
    crate::workspace::services::quote(s)
}

/// The command line a change is, ready for a shell and for a person to retype.
fn line_for(into: Into_, verb: &str, key: &str, value: Option<&str>) -> String {
    let file = if into == Into_::Media { String::new() } else { format!("--file {} ", into.arg()) };
    match value {
        Some(v) => format!("sstr config {file}{verb} {key} {}", quote(v)),
        None => format!("sstr config {file}{verb} {key}"),
    }
}

/// Say the line, run it, say what it said.
///
/// The order is the whole of it: a change that fails still leaves behind the
/// line somebody can retype to watch it fail for themselves, which is worth
/// more than a record of what happened.
fn send(line: &str, t: &mut Transcript) {
    t.say(line);
    let out = Command::new("sh").arg("-c").arg(line).output();
    match out {
        Ok(o) => {
            let text = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
            for said in text.lines().filter(|l| !l.trim().is_empty()) {
                t.say(said.trim_end());
            }
            if !o.status.success() && text.trim().is_empty() {
                t.say("it failed, and said nothing");
            }
        }
        Err(e) => t.say(&format!("could not run it: {e}")),
    }
}

fn frame_of(rows: &[(&'static str, String, Source)], sel: usize, into: Into_, (h, w): (i64, i64), colors: bool, t: &Transcript, home: &std::path::Path) -> Frame {
    let mut f = Frame::new(h.max(0) as usize);
    let color = |c: u8| colors.then_some(c);
    let dim = Style { dim: true, ..Style::default() };

    f.put(0, 0, &ljust(&cut(" Workspace -- Settings", w), w), Style { reverse: true, bold: true, ..Style::default() });
    f.put(1, 0, &cut(&format!("  changes go to {}", into.label()), w), dim);
    f.put(2, 0, &"-".repeat(w.max(0) as usize), dim);

    let listed = crate::workspace::transcript_rows(h);
    let first = 3i64;
    let last = h - 3 - listed - 1;
    let room = (last - first + 1).max(0) as usize;
    let top = sel.saturating_sub(room.saturating_sub(1));
    let wide = rows.iter().map(|(k, _, _)| k.len()).max().unwrap_or(8);

    for (i, (key, value, from)) in rows.iter().enumerate().skip(top).take(room) {
        let y = first + (i - top) as i64;
        let here = i == sel;
        // A value nothing has set reads as (none) rather than as an empty
        // column: "SSTR_KEY is empty" and "the line is missing" are the same
        // screen otherwise, and for SSTR_KEY they are opposite meanings --
        // empty is unsigned on purpose, missing is signed with the default key.
        let shown = if value.is_empty() { "(none)".to_string() } else { tilde(value, home) };
        let text = format!("  {key:<wide$}  {shown}");
        // ONE `puts` FOR THE ROW, because `puts` REPLACES a row rather than
        // drawing over it -- two calls leave only the second, which is a
        // screen of provenance labels with no settings beside them. The
        // Browser has always built a row as a list of pieces for this reason;
        // this is the same rule, not a special case.
        let label = from.label();
        let x = (w - label.len() as i64 - 1).max(0);
        // Where it came from, at the right edge, dim -- left out when the
        // value is long enough to reach it, since a FORMAT selector cut in
        // half to make room for the word "default" is the worse trade.
        let mut segs = vec![(0usize, ljust(&cut(&text, w), w), if here { Style { reverse: true, ..Style::default() } } else { Style::default() })];
        if !here && (text.chars().count() as i64) < x - 1 {
            segs.push((x.max(0) as usize, label.to_string(), Style { dim: true, color: color(6), ..Style::default() }));
        }
        f.puts(y, segs);
    }

    let foot = h - 2 - listed;
    f.put(foot, 0, &"-".repeat(w.max(0) as usize), dim);
    let help = rows.get(sel).and_then(|(k, _, _)| SETTINGS.iter().find(|d| d.key == *k)).map(|d| d.help).unwrap_or("");
    f.put(foot + 1, 0, &cut(&format!("  {help}"), w), dim);
    crate::workspace::put_transcript(&mut f, t, (h, w));
    let keys = match rows.get(sel).and_then(|(k, _, _)| SETTINGS.iter().find(|d| d.key == *k)).map(|d| d.kind) {
        Some(Kind::Cycle(_)) | Some(Kind::Toggle(_, _)) => "Space next  e type it  u unset  w which file  q back",
        _ => "e type it  u unset  w which file  q back",
    };
    f.put(h - 1, 0, &cut(&format!(" Settings:  {keys}"), w), dim);
    f
}

fn tilde(s: &str, home: &std::path::Path) -> String {
    match s.strip_prefix(&format!("{}/", home.display())) {
        Some(rest) => format!("~/{rest}"),
        None => s.to_string(),
    }
}

/// The Settings screen. Returns when the person leaves it.
pub fn screen(paths: &Paths, home: &std::path::Path, keys: &Keys, sc: &mut Screen, t: &mut Transcript, colors: bool) {
    let env = |k: &str| std::env::var(k).ok();
    let mut sel = 0usize;
    let mut into = Into_::Media;
    sc.invalidate();
    loop {
        // READ AGAIN EVERY FRAME, from the files, never from something this
        // screen remembers. A change is made by running a command, and the
        // only honest way to know what a command did is to look -- which also
        // means a config file edited in another terminal shows up here.
        let rows = crate::ytq::settings::in_force(paths, home, &env);
        if sel >= rows.len() {
            sel = rows.len().saturating_sub(1);
        }
        let (h, w) = term::size();
        t.poll();
        let _ = sc.present(&frame_of(&rows, sel, into, (h as i64, w as i64), colors, t, home), (h, w));

        let Some(k) = keys.next(std::time::Duration::from_millis(500)) else { continue };
        let Some((key, value, _)) = rows.get(sel).cloned().map(|(a, b, c)| (a, b, c)) else { break };
        let d = SETTINGS.iter().find(|d| d.key == key);
        match k {
            Key::Char('q') | Key::Escape | Key::Interrupt | Key::Left | Key::Char('h') => break,
            Key::Down | Key::Char('j') => sel = (sel + 1).min(rows.len().saturating_sub(1)),
            Key::Up | Key::Char('k') => sel = sel.saturating_sub(1),
            // Which file the next change is written to. Not which file the
            // value came FROM -- that is shown per row and is not a choice.
            Key::Char('w') => into = if into == Into_::Media { Into_::Ytq } else { Into_::Media },
            Key::Char(' ') | Key::Enter => match d.and_then(|d| next_value(d, &value)) {
                Some(next) => send(&line_for(into, "set", key, Some(&next)), t),
                // Space on a typed setting opens the same editor `e` does,
                // rather than doing nothing and leaving the person to guess
                // which settings Space is for.
                None => {
                    if let Some(v) = ask(keys, sc, key, &value) {
                        send(&line_for(into, "set", key, Some(&v)), t);
                    }
                }
            },
            Key::Char('e') => {
                if let Some(v) = ask(keys, sc, key, &value) {
                    send(&line_for(into, "set", key, Some(&v)), t);
                }
            }
            // Take the line out of the file, which is not the same as setting
            // it empty: SSTR_KEY= means unsigned and no SSTR_KEY at all means
            // the default key. Both are reachable, and by different keys.
            Key::Char('u') => send(&line_for(into, "unset", key, None), t),
            _ => {}
        }
    }
    sc.invalidate();
}

/// The value typed on the bottom row, or None when it was given up on.
///
/// Seeded with what the setting is now, because a path is nearly always an
/// edit of the path in force rather than a new one typed from nothing.
fn ask(keys: &Keys, sc: &mut Screen, key: &str, now: &str) -> Option<String> {
    let (h, w) = term::size();
    let typed = crate::ytq::window::prompt_with(keys, sc, (h as i64, w as i64), &format!("{key}:"), now);
    typed.map(|s| s.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(key: &str) -> &'static Setting {
        SETTINGS.iter().find(|s| s.key == key).unwrap()
    }

    #[test]
    fn space_walks_a_cycle_and_flips_a_toggle() {
        assert_eq!(next_value(d("OUTPUT"), "sstr").unwrap(), "mp4");
        assert_eq!(next_value(d("OUTPUT"), "mp4").unwrap(), "both");
        assert_eq!(next_value(d("OUTPUT"), "both").unwrap(), "sstr", "the last wraps to the first");
        // A value the list has never heard of -- a config file edited by hand
        // to something wrong -- still moves, to the first of the list, rather
        // than leaving the key dead under Space.
        assert_eq!(next_value(d("OUTPUT"), "wav").unwrap(), "sstr");
        assert_eq!(next_value(d("AUTOSTART"), "off").unwrap(), "on");
        assert_eq!(next_value(d("AUTOSTART"), "on").unwrap(), "off");
        assert_eq!(next_value(d("SSTR_DEFLATE"), "no").unwrap(), "yes");
        // A typed setting has no next value, which is what sends Space to the
        // editor instead.
        assert!(next_value(d("ARCHIVE_DIR"), "/x").is_none());
        assert!(next_value(d("PLAYER"), "mpv").is_none());
    }

    /// The line is the feature; the screen is a way of sending it. So the line
    /// has to be one a person could actually have typed -- including when the
    /// value has a space in it, which PLAYER and any real folder both do.
    #[test]
    fn the_line_a_key_sends_is_one_a_shell_takes_as_written() {
        assert_eq!(line_for(Into_::Media, "set", "OUTPUT", Some("mp4")), "sstr config set OUTPUT mp4");
        assert_eq!(line_for(Into_::Ytq, "set", "OUTPUT", Some("mp4")), "sstr config --file ytq set OUTPUT mp4");
        assert_eq!(line_for(Into_::Media, "unset", "SSTR_KEY", None), "sstr config unset SSTR_KEY");
        assert_eq!(line_for(Into_::Media, "set", "PLAYER", Some("mpv --fs")), "sstr config set PLAYER 'mpv --fs'");
        // An empty value is a real answer -- SSTR_KEY= is unsigned on purpose
        // -- and must reach the shell as an argument rather than as nothing.
        assert_eq!(line_for(Into_::Media, "set", "SSTR_KEY", Some("")), "sstr config set SSTR_KEY ''");
        // And it survives a shell, which is the only test that counts.
        for value in ["mpv --fs", "/home/a b/Don't", "$HOME", ""] {
            let line = line_for(Into_::Media, "set", "PLAYER", Some(value));
            // `sstr config set PLAYER <arg>`: the fifth field onward is the
            // value, and it is one field only because quoting made it one.
            let arg = line.splitn(5, ' ').nth(4).unwrap();
            let out = std::process::Command::new("sh").arg("-c").arg(format!("printf %s {arg}")).output().unwrap();
            assert_eq!(String::from_utf8_lossy(&out.stdout), value, "{line}");
        }
    }

    /// Every setting the screen draws is one `sstr config set` will accept.
    /// They read the same table, so this is really a check that nothing has
    /// grown a second one.
    #[test]
    fn every_row_the_screen_draws_is_a_key_the_command_knows() {
        for d in SETTINGS {
            assert!(crate::ytq::settings::setting(d.key).is_some(), "{}", d.key);
            assert!(!d.help.is_empty(), "{} has no line for the foot of the screen", d.key);
        }
    }
}
