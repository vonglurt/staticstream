// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Paul Richeson

//! `ytq` with no command: the queue window, the Python ytq's `tui()` without
//! curses. The same header, live panel, list, keys line and message line,
//! drawn from the same queue file; the same clipboard watcher, which acts
//! only while the window has focus; the same worker, which downloads whenever
//! nobody else is; and the same keys, doing the same things to `queue.json`.
//!
//! One difference, on purpose: SIGTERM, SIGHUP (the terminal closed) or
//! SIGINT end the window as q does, stopping its download and putting the
//! entry back. The Python window dies of them, leaving yt-dlp running and the
//! entry `downloading` until the next runner finds it.

use std::collections::HashSet;
use std::io::IsTerminal;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::format::json::{self, Value};

use crate::ytq::cli::read_clipboard;
use crate::ytq::live::{live_view, number};
use crate::ytq::log::{log, Mode};
use crate::ytq::queue::{self, status, text, title_or_url, RunLock};
use crate::ytq::runner::{self, brave_ready, run_timeout, Ran, Runner};
use crate::ytq::term::{self, cut, ljust, Frame, Key, Keys, Screen, Style, Term};
use crate::ytq::urls::{as_url, py_strip, short};
use crate::ytq::{sys, HISTORY, ORDER};

const KEYS: &str = " a add  d delete  r retry  c continue with Brave's cookies  o open in Brave  p pause  h h clear history  q quit";
const EMPTY: &str = "Nothing queued. Copy a video URL while this window is focused, or press a.";
const RED: u8 = 1;
const GREEN: u8 = 2;
const YELLOW: u8 = 3;
const CYAN: u8 = 6;

fn mark(st: &str) -> &'static str {
    match st {
        "downloading" => ">",
        "queued" => ".",
        "retry" => "r",
        "retry-cookies" => "c",
        "cookies" => "C",
        "checking" => "?",
        "done" => "+",
        "failed" => "x",
        "rejected" => "-",
        _ => " ",
    }
}

/// The list's order: by state as ORDER has them, then by when added.
fn order_key(i: &Value) -> (usize, f64) {
    (ORDER.iter().position(|s| *s == status(i)).unwrap_or(3), number(i.get("added")))
}

/// The message line when there is no message: what the selected entry is waiting on, or where it is.
fn info(it: &Value) -> String {
    let st = status(it);
    if st == "cookies" {
        return format!("Brave is open on it: sign in / pass the check, then press c.  {}", text(it, "error"));
    }
    if matches!(st, "failed" | "rejected" | "retry") {
        return text(it, "error").to_string();
    }
    let file = text(it, "file");
    if file.is_empty() { text(it, "url") } else { file }.to_string()
}

/// This process and its parents: the terminal window it runs in is one of them.
fn ancestors() -> HashSet<u64> {
    let mut pids = HashSet::new();
    let mut p = std::process::id() as u64;
    while p > 1 {
        pids.insert(p);
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{p}/stat")) else { break };
        match stat.rsplit_once(')').and_then(|(_, rest)| rest.split_whitespace().nth(1)).and_then(|s| s.parse().ok()) {
            Some(parent) => p = parent,
            None => break,
        }
    }
    pids
}

/// `focused()`: is the terminal this runs in the focused window? Unknown counts as yes.
fn focused(ancestors: &HashSet<u64>) -> bool {
    let set = |k: &str| std::env::var_os(k).map_or(false, |v| !v.is_empty());
    if set("HYPRLAND_INSTANCE_SIGNATURE") {
        return match run_timeout(Command::new("hyprctl").args(["activewindow", "-j"]), 2) {
            Ran::Done(_, out, _) => match json::parse(&out) {
                Ok(v @ Value::Obj(_)) => match v.get("pid") {
                    Some(Value::Num(n)) if n.fract() == 0.0 && *n >= 0.0 => ancestors.contains(&(*n as u64)),
                    _ => false,
                },
                _ => true,
            },
            _ => true,
        };
    }
    if set("DISPLAY") {
        if let Ran::Done(0, out, _) = run_timeout(Command::new("xdotool").args(["getactivewindow", "getwindowpid"]), 2) {
            return match py_strip(&out).parse::<u64>() {
                Ok(pid) => ancestors.contains(&pid),
                Err(_) => true,
            };
        }
    }
    true
}

struct Window {
    runner: Arc<Runner>,
    /// `WATCH["on"]`: w turns the watcher off and on.
    watching: AtomicBool,
    /// `WATCH["focused"]`, as the watcher last found it.
    focused: AtomicBool,
    /// run.lock is this window's: the Python ytq's `RUN.fd`.
    held: AtomicBool,
    ancestors: HashSet<u64>,
}

/// `sstr ytq`, and later `ytq`: the window, until q.
pub fn main(runner: Arc<Runner>) -> i32 {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        eprintln!("ytq: the window needs a terminal -- 'ytq run' downloads without one, and 'ytq status' shows what is happening");
        return 2;
    }
    runner.set_mode(Mode::Quiet);
    runner::catch_signals();
    let term = match Term::enter() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("ytq: cannot set up the terminal: {e}");
            return 1;
        }
    };
    let win = Arc::new(Window {
        runner,
        watching: AtomicBool::new(true),
        focused: AtomicBool::new(true),
        held: AtomicBool::new(false),
        ancestors: ancestors(),
    });
    for job in [Window::watcher as fn(&Window), Window::worker] {
        let w = Arc::clone(&win);
        std::thread::spawn(move || job(&w));
    }
    let keys = Keys::start();
    let code = win.run(&keys);
    drop(term);
    code
}

impl Window {
    fn stopping(&self) -> bool {
        self.runner.stop.load(Ordering::SeqCst) || runner::signalled()
    }

    fn sleep(&self, d: Duration) {
        let until = Instant::now() + d;
        while !self.stopping() {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            std::thread::sleep(left.min(Duration::from_millis(50)));
        }
    }

    /// `watcher()`: what is on the clipboard at start counts -- launching ytq
    /// with a link already copied is the common case -- and then every new
    /// copy while the window has focus. It starts no runner: the window's own
    /// worker takes run.lock as soon as nobody else has it.
    fn watcher(&self) {
        let paths = &self.runner.paths;
        let mut last = read_clipboard();
        if let Some(url) = as_url(&last) {
            let _ = queue::enqueue(paths, &RunLock::default(), &[url], false);
        }
        let poll = self.runner.s.get("POLL").and_then(|p| py_strip(p).parse::<f64>().ok()).filter(|p| p.is_finite() && *p >= 0.0).unwrap_or(1.0);
        while !self.stopping() {
            self.sleep(Duration::from_secs_f64(poll));
            if self.stopping() {
                return;
            }
            self.focused.store(focused(&self.ancestors), Ordering::SeqCst);
            if !self.watching.load(Ordering::SeqCst) || !self.focused.load(Ordering::SeqCst) {
                continue;
            }
            let text = read_clipboard();
            if text == last {
                continue;
            }
            last = text;
            if let Some(url) = as_url(&last) {
                let _ = queue::enqueue(paths, &RunLock::default(), &[url], false);
            }
        }
    }

    /// `window_worker()`: download from the window whenever nobody else is --
    /// take run.lock and keep it.
    fn worker(&self) {
        let r = &self.runner;
        while !self.stopping() {
            let taken = r
                .run
                .lock()
                .unwrap()
                .take(&r.paths, 1, || {
                    self.held.store(true, Ordering::SeqCst);
                    r.log_start("");
                })
                .unwrap_or(false);
            if taken {
                let _ = r.serve(true, false);
                return;
            }
            self.sleep(Duration::from_secs(1));
        }
    }

    /// `tui(win)`: draw, wait half a second for a key, do what it says.
    fn run(&self, keys: &Keys) -> i32 {
        let r = &self.runner;
        let home = r.home.to_string_lossy().into_owned();
        let colors = std::env::var("TERM").map_or(false, |t| !t.is_empty() && t != "dumb");
        let mut screen = Screen::default();
        // armed: when h (or x) was pressed once, so a second press clears the history.
        let (mut sel, mut message, mut armed) = (0i64, String::new(), 0.0f64);
        let mut last_size = None;
        loop {
            if runner::signalled() {
                break;
            }
            let mut items = queue::snapshot(&r.paths);
            items.sort_by(|a, b| order_key(a).partial_cmp(&order_key(b)).unwrap_or(std::cmp::Ordering::Equal));
            let held = self.held.load(Ordering::SeqCst);
            let holder = if held { Some(std::process::id().to_string()) } else { RunLock::default().holder(&r.paths) };
            let (h, w) = term::size();
            if last_size.map_or(false, |s| s != (h, w)) {
                // curses hands the window a resize as a key, which clears the message.
                message.clear();
                armed = 0.0;
            }
            last_size = Some((h, w));
            let frame = self.frame(&items, &mut sel, &message, holder.as_deref(), (h as i64, w as i64), colors, &home);
            let _ = screen.present(&frame, (h, w));
            let Some(k) = keys.next(Duration::from_millis(500)) else { continue };
            message.clear();
            if !matches!(k, Key::Char('h') | Key::Char('x')) {
                armed = 0.0;
            }
            let current = items.get(sel as usize);
            match k {
                Key::Char('q') | Key::Escape | Key::Interrupt => break,
                Key::Down | Key::Char('j') => sel += 1,
                Key::Up | Key::Char('k') => sel -= 1,
                Key::Char('a') => {
                    let url = prompt(keys, &mut screen, (h as i64, w as i64), "URL:");
                    if let Some(u) = as_url(&url) {
                        message = match queue::enqueue(&r.paths, &RunLock::default(), &[u], false) {
                            Ok((added, _)) if !added.is_empty() => "queued".into(),
                            Ok(_) => "already in the queue".into(),
                            Err(e) => format!("could not queue it: {e}"),
                        };
                    } else if !url.is_empty() {
                        message = "that is not a URL".into();
                    }
                }
                Key::Char('w') => {
                    let on = self.watching.load(Ordering::SeqCst);
                    self.watching.store(!on, Ordering::SeqCst);
                }
                Key::Char('p') => {
                    if self.held.load(Ordering::SeqCst) {
                        let paused = r.paused.load(Ordering::SeqCst);
                        r.paused.store(!paused, Ordering::SeqCst);
                    } else {
                        message = match &holder {
                            Some(pid) => format!("p pauses this window's downloads, and pid {pid} has the queue"),
                            None => "nothing to pause yet".into(),
                        };
                    }
                }
                Key::Char(c @ ('h' | 'x')) => {
                    // Twice in a row, within 5 s: forgetting a failed entry loses its
                    // error, so one stray key must not do it.
                    let now = sys::now();
                    if armed != 0.0 && now - armed < 5.0 {
                        let n = queue::edit(&r.paths, |live| {
                            let n = live.len();
                            live.retain(|i| !HISTORY.contains(&status(i)));
                            n - live.len()
                        })
                        .unwrap_or(0);
                        armed = 0.0;
                        log(&r.paths, &format!("history cleared from the window: {n} entries"));
                        message = format!("cleared {n} from the history; the files and the log stay");
                    } else {
                        let counts: Vec<String> = HISTORY
                            .iter()
                            .filter_map(|s| {
                                let n = items.iter().filter(|i| status(i) == *s).count();
                                (n > 0).then(|| format!("{n} {s}"))
                            })
                            .collect();
                        armed = if counts.is_empty() { 0.0 } else { now };
                        message = if counts.is_empty() {
                            "the history is empty".into()
                        } else {
                            format!("press {c} again to clear the history: {}", counts.join(", "))
                        };
                    }
                }
                Key::Char('d') | Key::Delete if current.is_some() => {
                    let it = current.unwrap();
                    let url = text(it, "url").to_string();
                    // Whoever is downloading it notices at its next progress line.
                    let _ = queue::forget(&r.paths, &url);
                    log(&r.paths, &format!("{}: deleted from the window (was {})", short(&url), status(it)));
                    r.kill_if_current(&url);
                }
                Key::Char('r') if current.is_some() => {
                    // queue::retry, so that `ytq retry URL` and this key
                    // cannot leave two different queues behind.
                    let _ = queue::retry(&r.paths, text(current.unwrap(), "url"));
                }
                Key::Char('c') if current.is_some() => {
                    message = if status(current.unwrap()) == "cookies" {
                        match brave_ready(&r.s) {
                            None => format!("retrying {} with Brave's cookies", r.mark_cookie_retries()),
                            Some(why) => format!("{why} -- start Brave once, or set PROFILE in ~/.config/ytq/config"),
                        }
                    } else {
                        "c is for entries marked 'cookies'".into()
                    };
                }
                Key::Char('o') if current.is_some() => {
                    r.open_browser(text(current.unwrap(), "url"));
                }
                _ => {}
            }
        }
        // Quitting stops this window's downloads; 'ytq run', or the next 'ytq
        // clip', carries on from where it was.
        r.stop.store(true, Ordering::SeqCst);
        log(&r.paths, "window closed");
        if self.held.load(Ordering::SeqCst) {
            r.stop_current(if runner::signalled() { "the window was told to stop (SIGTERM, SIGHUP or SIGINT)" } else { "the window was closed" });
            r.run.lock().unwrap().release();
        }
        0
    }

    /// One screen, as `tui()` draws it.
    fn frame(&self, items: &[Value], sel: &mut i64, message: &str, holder: Option<&str>, (h, w): (i64, i64), colors: bool, home: &str) -> Frame {
        let r = &self.runner;
        let mut f = Frame::new(h.max(0) as usize);
        let color = |c: u8| colors.then_some(c);
        let (on, focused) = (self.watching.load(Ordering::SeqCst), self.focused.load(Ordering::SeqCst));
        let state = if on && focused {
            "watching the clipboard"
        } else if on {
            "clipboard: paused (window not focused)"
        } else {
            "clipboard: off"
        };
        let worker = if self.held.load(Ordering::SeqCst) {
            if r.paused.load(Ordering::SeqCst) { "[paused]" } else { "" }.to_string()
        } else {
            match holder {
                Some(pid) => format!("[downloading in pid {pid}]"),
                None => "[no worker yet]".into(),
            }
        };
        let counts: Vec<String> = ORDER
            .iter()
            .filter_map(|st| {
                let n = items.iter().filter(|i| status(i) == *st).count();
                (n > 0).then(|| format!("{n} {st}"))
            })
            .collect();
        let dir = r.s.get("DIR").cloned().unwrap_or_default().replace(home, "~");
        let head = format!(" ytq  {state}   -> {dir}   {worker}   {}", counts.join("  "));
        f.put(0, 0, &ljust(&cut(&head, w - 1), w - 1), Style { reverse: true, bold: true, ..Style::default() });
        // Downloads under way go above the list, each with the step it is on,
        // read from the live record whichever process is downloading keeps.
        let now = sys::now();
        let mut top_y = 1i64;
        for it in items.iter().filter(|i| status(i) == "downloading").take(2) {
            let mut rows = live_view(it, w - 4, now);
            rows.truncate((h - top_y - 10).max(0) as usize);
            if rows.is_empty() {
                break;
            }
            let title = format!(" > {}   {}", title_or_url(it), text(it, "quality"));
            f.put(top_y, 0, &cut(&title, w - 1), Style { bold: true, color: color(CYAN), ..Style::default() });
            for (n, row) in rows.iter().enumerate() {
                f.put(top_y + 1 + n as i64, 0, &cut(&format!("   {row}"), w - 1), Style::default());
            }
            top_y += rows.len() as i64 + 2;
        }
        if top_y > 1 {
            f.put(top_y - 1, 0, &"-".repeat((w - 1).max(0) as usize), Style { dim: true, ..Style::default() });
        }
        let room = (h - 3 - top_y).max(1);
        *sel = (*sel).min(items.len() as i64 - 1).max(0);
        let top = (*sel - (room - 1)).max(0);
        for (row, it) in items.iter().skip(top as usize).take(room as usize).enumerate() {
            let st = status(it);
            let col = match st {
                "done" => Some(GREEN),
                "failed" | "rejected" => Some(RED),
                "cookies" | "retry" => Some(YELLOW),
                "downloading" => Some(CYAN),
                _ => None,
            };
            let line = if st == "downloading" {
                format!(" {} {:<13} {}  {}", mark(st), st, cut(text(it, "progress"), 40), title_or_url(it))
            } else {
                let extra = if st == "queued" || st == "done" { text(it, "quality").to_string() } else { cut(text(it, "error"), 40) };
                format!(" {} {:<13} {:<9} {}", mark(st), st, cut(&extra, 9), title_or_url(it))
            };
            let style = Style { color: col.and_then(color), reverse: top + row as i64 == *sel, ..Style::default() };
            f.put(top_y + row as i64, 0, &ljust(&cut(&line, w - 1), w - 1), style);
        }
        if items.is_empty() {
            f.put(2, 2, &cut(EMPTY, w - 3), Style::default());
        }
        f.put(h - 2, 0, &cut(KEYS, w - 1), Style { dim: true, ..Style::default() });
        let bottom = if message.is_empty() { items.get(*sel as usize).map(info).unwrap_or_default() } else { message.to_string() };
        f.put(h - 1, 0, &cut(&bottom, w - 1), Style::default());
        f
    }
}

/// `prompt(win, label)`: a line typed on the bottom row and echoed, as curses'
/// getstr reads one: at most w - len(label) - 3 characters, Backspace and
/// Ctrl+U to correct it, Enter to end it. Escape or Ctrl+C gives up on it.
fn prompt(keys: &Keys, screen: &mut Screen, (h, w): (i64, i64), label: &str) -> String {
    let row = h.max(1);
    let x = label.chars().count() as i64 + 1;
    let max = (w - x - 2).max(0) as usize;
    let shown = term::printable(&cut(&ljust(&format!("{label} "), w - 1), w - 1));
    let mut typed = String::new();
    loop {
        let _ = term::write(&format!("\x1b[{row};1H\x1b[0m\x1b[2K{shown}\x1b[{row};{}H{}\x1b[?25h", x + 1, term::printable(&typed)));
        match keys.wait() {
            Key::Enter => break,
            Key::Escape | Key::Interrupt => {
                typed.clear();
                break;
            }
            Key::Backspace => {
                typed.pop();
            }
            Key::KillLine => typed.clear(),
            Key::Char(c) if typed.chars().count() < max => typed.push(c),
            _ => {}
        }
    }
    let _ = term::write("\x1b[?25l");
    screen.invalidate();
    py_strip(&typed).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_goes_by_state_then_by_when_added() {
        let e = |st: &str, added: f64| Value::obj(vec![("status", Value::str(st)), ("added", Value::Num(added))]);
        let mut items = vec![e("done", 1.0), e("queued", 3.0), e("downloading", 9.0), e("queued", 2.0), e("strange", 0.0)];
        items.sort_by(|a, b| order_key(a).partial_cmp(&order_key(b)).unwrap());
        let got: Vec<(&str, f64)> = items.iter().map(|i| (status(i), number(i.get("added")))).collect();
        assert_eq!(got, vec![("downloading", 9.0), ("strange", 0.0), ("queued", 2.0), ("queued", 3.0), ("done", 1.0)]);
    }
}
