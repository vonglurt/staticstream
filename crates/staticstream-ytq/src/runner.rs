// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Paul Richeson

//! The runner: the Python ytq's `check()`, `claim()`, `download()`,
//! `stop_current()`, `checker()`, `serve()`, `transcript()`, `notes()` and
//! `cmd_run()`, step 2b of docs/phase-2.md.
//!
//! yt-dlp stays a subprocess, driven with the Python ytq's arguments to the
//! letter -- NAME_OPTS, META_OPTS when ffmpeg is present, the progress
//! template -- and read line by line into the live record the window and
//! `ytq status` show, whichever ytq is looking.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, BufRead, BufReader, IsTerminal, Read};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use staticstream::json::{self, Value};
use staticstream::sha256::{hex, Sha256};
use staticstream::{reader, writer};

use crate::live::{compact, describe, fmt_secs, fmt_size, na, number, py_str, size_of, splitext, stage, stream_of, truthy, PROGRESS_KEYS};
use crate::log::{log, say, splitlines, Mode};
use crate::queue::{self, status, text, title_or_url, RunLock};
use crate::settings::{DEFAULT_FORMAT, DEFAULT_SUBS};
use crate::urls::{first_id, py_strip, short};
use crate::{sys, textwrap, Paths, DOWNLOADABLE, PENDING};

pub const COOKIE_WORDS: [&str; 13] =
    ["sign in", "log in", "login", "cookies", "age", "bot", "private video", "members", "premium", "confirm you", "403", "restricted", "subscriber"];

pub const NAME_OPTS: &[&str] = &[
    "--parse-metadata", "%(uploader,channel|)#S:(?s)(?P<safe_author>.+)",
    "--replace-in-metadata", "safe_author", "^[^A-Za-z0-9]+", "",
    "--replace-in-metadata", "safe_author", "(?s)[^A-Za-z0-9].*", "",
    "--parse-metadata", "%(title)#S:(?s)(?P<safe_title>.+)",
    "--replace-in-metadata", "safe_title", "[^A-Za-z0-9_-]+", "_",
    "--replace-in-metadata", "safe_title", "[-_]*_[-_]*", "_",
    "--replace-in-metadata", "safe_title", "(?<=^.{120}).+", "",
    "--replace-in-metadata", "safe_title", "^[-_]+|[-_]+$", "",
    "--parse-metadata", "id:(?s)(?P<safe_id>.+)",
    "--replace-in-metadata", "safe_id", "[^A-Za-z0-9_-]+", "_",
];
pub const NAME: &str = "%(safe_author&{}-|)s%(safe_title&{}_|)s%(safe_id)s.%(ext)s";
pub const META_OPTS: &[&str] = &[
    "--embed-metadata",
    "--parse-metadata",
    "Title = %(title)s\nAuthor = %(uploader,channel|unknown)s\nURL = %(webpage_url)s\nPublished = %(timestamp>%Y-%m-%d %H.%M.%S UTC,upload_date>%Y-%m-%d|unknown)s\nDownloaded = %(epoch>%Y-%m-%d %H.%M.%S UTC)s\nLicense = %(license|not stated)s:(?s)(?P<meta_comment>.+)",
    "--replace-in-metadata", "meta_comment", "(?m)^(Title|Author|URL|Published|Downloaded|License) = ", r"\1: ",
    "--replace-in-metadata", "meta_comment", r"(\d\d)\.(\d\d)\.(\d\d) UTC", r"\1:\2:\3 UTC",
];

pub fn progress_template() -> String {
    let fields = [
        "progress._percent_str", "progress.downloaded_bytes", "progress.total_bytes", "progress.total_bytes_estimate",
        "progress._speed_str", "progress._eta_str", "progress._elapsed_str", "progress.fragment_index", "progress.fragment_count",
        "info.format_id", "info.vcodec", "info.acodec", "info.height", "progress.status",
    ];
    format!("download:PROGRESS {}", fields.iter().map(|f| format!("%({f})s")).collect::<Vec<_>>().join("|"))
}

/// What running a command came to.
pub enum Ran {
    Done(i32, String, String),
    TimedOut,
    Failed(io::Error),
}

/// `subprocess.run(cmd, capture_output=True, timeout=secs)`.
pub fn run_timeout(cmd: &mut Command, secs: u64) -> Ran {
    let mut child = match cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn() {
        Ok(c) => c,
        Err(e) => return Ran::Failed(e),
    };
    let mut out = child.stdout.take().expect("piped");
    let mut err = child.stderr.take().expect("piped");
    let t_out = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = out.read_to_end(&mut b);
        b
    });
    let t_err = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = err.read_to_end(&mut b);
        b
    });
    let deadline = Instant::now() + Duration::from_secs(secs);
    let code = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st.code().unwrap_or_else(|| -st.signal().unwrap_or(0)),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Ran::TimedOut;
            }
            Err(e) => return Ran::Failed(e),
        }
    };
    let o = String::from_utf8_lossy(&t_out.join().unwrap_or_default()).into_owned();
    let e = String::from_utf8_lossy(&t_err.join().unwrap_or_default()).into_owned();
    Ran::Done(code, o, e)
}

/// `shlex.quote(s)`.
pub fn shlex_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".into();
    }
    if s.chars().all(|c| c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c)) {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}

/// `shutil.which(name)`.
pub fn which(name: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    std::env::var_os("PATH")?.to_str()?.split(':').map(|d| Path::new(if d.is_empty() { "." } else { d }).join(name)).find(|p| {
        fs::metadata(p).map_or(false, |m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    })
}

/// What a finished download leaves, from OUTPUT: `sstr` (the default) keeps a
/// Static Stream capture and removes the MP4 once the capture checks out;
/// `both` keeps both; `mp4` leaves what the Python ytq leaves.
pub fn output_of(s: &BTreeMap<String, String>) -> &'static str {
    match s.get("OUTPUT").map(|v| v.trim().to_lowercase()).as_deref() {
        Some("mp4") => "mp4",
        Some("both") => "both",
        _ => "sstr",
    }
}

/// Where captures go: ARCHIVE_DIR, else DIR.
pub fn archive_dir_of(s: &BTreeMap<String, String>) -> String {
    match s.get("ARCHIVE_DIR").filter(|a| !a.trim().is_empty()) {
        Some(a) => a.clone(),
        None => s.get("DIR").cloned().unwrap_or_default(),
    }
}

/// The key captures are signed with: SSTR_KEY, or ~/.ssh/id_ed25519 when it
/// exists and SSTR_KEY is not set at all. `SSTR_KEY=` with nothing after it
/// means unsigned.
pub fn sstr_key_of(s: &BTreeMap<String, String>, home: &Path) -> Option<PathBuf> {
    match s.get("SSTR_KEY") {
        Some(k) if k.trim().is_empty() => None,
        Some(k) => Some(PathBuf::from(k)),
        None => Some(home.join(".ssh").join("id_ed25519")).filter(|k| k.exists()),
    }
}

/// A capture's content type, from the downloaded file's extension.
pub fn content_type(path: &str) -> &'static str {
    match splitext(path).1.to_lowercase().as_str() {
        ".mp4" | ".m4v" => "video/mp4",
        ".webm" => "video/webm",
        ".mkv" => "video/x-matroska",
        ".mov" => "video/quicktime",
        ".m4a" => "audio/mp4",
        ".mp3" => "audio/mpeg",
        ".opus" | ".ogg" => "audio/ogg",
        _ => "application/octet-stream",
    }
}

/// What yt-dlp prints after the move when a capture will be made: the fields
/// its header carries.
pub const NOTES_PRINT: &str = "after_move:NOTES %(.{webpage_url,license,title,uploader,channel})j";

fn file_sha256(path: &str) -> io::Result<String> {
    let mut f = fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finish()))
}

pub fn cookie_problem(text: &str) -> bool {
    let t = text.to_lowercase();
    COOKIE_WORDS.iter().any(|w| t.contains(w))
}

fn basename(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

fn first_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// yt-dlp by way of yt-brave, which owns the Brave profile path and the keyring.
pub fn brave_cmd(s: &BTreeMap<String, String>) -> Vec<String> {
    let mut cmd = vec!["yt-brave".to_string(), "--profile".into(), s.get("PROFILE").cloned().unwrap_or_default()];
    if let Some(k) = s.get("KEYRING").filter(|k| !k.is_empty()) {
        cmd.push("--keyring".into());
        cmd.push(k.clone());
    }
    cmd
}

/// `brave_ready()`: None if yt-brave can find the profile, else the reason it cannot.
pub fn brave_ready(s: &BTreeMap<String, String>) -> Option<String> {
    let cmd = brave_cmd(s);
    match run_timeout(Command::new(&cmd[0]).args(&cmd[1..]).arg("--path"), 10) {
        Ran::Done(0, _, _) => None,
        Ran::Done(_, _, err) => Some(splitlines(py_strip(&err)).first().copied().unwrap_or("yt-brave found no Brave profile").to_string()),
        Ran::TimedOut => Some("cannot run yt-brave: it did not answer in 10 s".into()),
        Ran::Failed(e) => Some(format!("cannot run yt-brave: {e}")),
    }
}

static SIGNALLED: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: i32) {
    SIGNALLED.store(true, Ordering::SeqCst);
}

fn interrupted() -> bool {
    SIGNALLED.load(Ordering::SeqCst)
}

/// Has SIGTERM, SIGHUP or SIGINT arrived since `catch_signals()`?
pub fn signalled() -> bool {
    interrupted()
}

/// Catch SIGTERM, SIGHUP and SIGINT, so a runner or the window ends as on
/// Ctrl+C -- the download stopped and put back -- rather than just dying.
pub fn catch_signals() {
    sys::on_signals(&[sys::SIGTERM, sys::SIGHUP, sys::SIGINT], on_signal);
}

/// A download under way: what stop_current needs to stop it.
#[derive(Default)]
struct Current {
    pid: Option<u32>,
    url: String,
    cookies: bool,
    attempts: f64,
    phase: String,
    started: f64,
}

enum End {
    Finished,
    Interrupted,
}

pub struct Runner {
    pub paths: Paths,
    pub home: PathBuf,
    pub s: BTreeMap<String, String>,
    mode: Mutex<Mode>,
    pub stop: AtomicBool,
    pub paused: AtomicBool,
    current: Mutex<Current>,
    pub run: Mutex<RunLock>,
}

impl Runner {
    pub fn new(paths: Paths, home: PathBuf, s: BTreeMap<String, String>, mode: Mode) -> Runner {
        Runner {
            paths,
            home,
            s,
            mode: Mutex::new(mode),
            stop: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            current: Mutex::new(Current::default()),
            run: Mutex::new(RunLock::default()),
        }
    }

    fn setting(&self, k: &str) -> String {
        self.s.get(k).cloned().unwrap_or_default()
    }

    pub fn set_mode(&self, m: Mode) {
        *self.mode.lock().unwrap() = m;
    }

    fn mode(&self) -> Mode {
        *self.mode.lock().unwrap()
    }

    fn log(&self, msg: &str) {
        log(&self.paths, msg);
    }

    fn say(&self, msg: &str, urgent: bool) {
        say(&self.paths, self.mode(), msg, urgent);
    }

    fn update(&self, url: &str, fields: Vec<(&str, Value)>) -> Option<Value> {
        queue::update(&self.paths, url, fields).ok().flatten()
    }

    /// `log_start()`: a runner's first line -- which ytq and yt-dlp did what
    /// follows, with what settings.
    pub fn log_start(&self, argv: &str) {
        let ver = match run_timeout(Command::new("yt-dlp").arg("--version"), 20) {
            Ran::Done(_, out, _) => py_strip(&out).to_string(),
            Ran::TimedOut => "(cannot run: timed out)".into(),
            Ran::Failed(e) => format!("(cannot run: {e})"),
        };
        let me = std::env::current_exe().ok().and_then(|p| fs::canonicalize(p).ok()).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| "?".into());
        let mtime = fs::metadata(&me)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| sys::strftime_local("%Y-%m-%d %H:%M", d.as_secs_f64()))
            .unwrap_or_else(|| "?".into());
        let subs = self.setting("SUBS");
        self.log(&format!(
            "runner '{}' took run.lock: ytq {} of {}, yt-dlp {}, sstr {}; DIR={} FORMAT={} SUBS={} PROFILE={}; OUTPUT={} ARCHIVE_DIR={} SSTR_KEY={}; /etc/yt-dlp.conf {}",
            if argv.is_empty() { "window" } else { argv },
            me,
            mtime,
            if ver.is_empty() { "?".to_string() } else { ver },
            env!("CARGO_PKG_VERSION"),
            self.setting("DIR"),
            self.setting("FORMAT"),
            if subs.is_empty() { "(off)".to_string() } else { subs },
            self.setting("PROFILE"),
            output_of(&self.s),
            archive_dir_of(&self.s),
            sstr_key_of(&self.s, &self.home).map(|k| k.to_string_lossy().into_owned()).unwrap_or_else(|| "(unsigned)".into()),
            if Path::new("/etc/yt-dlp.conf").exists() { "present" } else { "absent" }
        ));
    }

    /// `check(url)`: can yt-dlp fetch this as an MP4, and at what height?
    pub fn check(&self, url: &str) {
        let tag = short(url);
        let t0 = sys::now();
        let format = self.s.get("FORMAT").cloned().unwrap_or_else(|| DEFAULT_FORMAT.into());
        let ran = run_timeout(
            Command::new("yt-dlp").args(["--simulate", "--no-playlist", "--no-warnings", "-f", &format, "--print", "%(title)s\t%(height)s\t%(ext)s", url]),
            180,
        );
        match ran {
            Ran::TimedOut => {
                self.log(&format!("{tag}: check: yt-dlp gave no answer in 180 s"));
                self.update(url, vec![("status", Value::str("rejected")), ("error", Value::str("timed out asking yt-dlp about it"))]);
                self.say(&format!("not downloadable (timed out): {url}"), false);
            }
            Ran::Failed(e) => {
                self.update(url, vec![("status", Value::str("rejected")), ("error", Value::str(format!("yt-dlp: {e}")))]);
                self.say(&format!("cannot run yt-dlp: {e}"), true);
            }
            Ran::Done(code, out, err) => {
                let took = sys::now() - t0;
                let out = py_strip(&out);
                if code == 0 && !out.is_empty() {
                    let first = splitlines(out).first().copied().unwrap_or("");
                    let mut f: Vec<&str> = first.split('\t').collect();
                    f.extend(["", ""]);
                    let (title, height, ext) = (f[0], f[1], f[2]);
                    let q = format!("{} {}", if matches!(height, "NA" | "" | "None") { "?".to_string() } else { format!("{height}p") }, ext);
                    self.update(url, vec![("status", Value::str("queued")), ("title", Value::str(first_chars(title, 200))), ("quality", Value::str(&q)), ("error", Value::str(""))]);
                    self.log(&format!("queued {title} [{q}] {url} (checked in {took:.1} s)"));
                } else {
                    self.log(&format!("{tag}: check: yt-dlp exited {code} after {took:.1} s"));
                    let lines = splitlines(py_strip(&err));
                    for line in &lines[lines.len().saturating_sub(4)..] {
                        self.log(&format!("{tag}: check: {line}"));
                    }
                    let e = first_chars(lines.last().copied().unwrap_or("no output"), 300);
                    if cookie_problem(&e) {
                        // Not rejected: queue it anyway. The first real attempt is
                        // what opens Brave if the complaint holds.
                        self.update(url, vec![("status", Value::str("queued")), ("title", Value::str(url)), ("quality", Value::str("?")), ("error", Value::str(&e))]);
                        self.log(&format!("queued despite a cookie complaint at check: {url}"));
                    } else {
                        self.update(url, vec![("status", Value::str("rejected")), ("error", Value::str(&e))]);
                        self.say(&format!("not downloadable: {url} -- {e}"), false);
                    }
                }
            }
        }
    }

    /// `claim()`: the next download, marked as downloading in one locked step.
    fn claim(&self) -> Option<(Value, bool)> {
        queue::edit(&self.paths, |items| {
            for st in DOWNLOADABLE {
                if let Some(it) = items.iter_mut().find(|i| status(i) == st) {
                    let cookies = st == "retry-cookies";
                    let attempts = number(it.get("attempts")) + 1.0;
                    let tried = truthy(it.get("cookie_tried")) || cookies;
                    it.set("status", Value::str("downloading"));
                    it.set("attempts", Value::Num(attempts));
                    it.set("progress", Value::str("starting"));
                    it.set("cookie_tried", Value::Bool(tried));
                    return Some((it.clone(), cookies));
                }
            }
            None
        })
        .ok()
        .flatten()
    }

    pub fn open_browser(&self, url: &str) -> bool {
        for cmd in [vec!["brave", url], vec!["flatpak", "run", "com.brave.Browser", url], vec!["xdg-open", url]] {
            let spawned = Command::new(cmd[0]).args(&cmd[1..]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).process_group(0).spawn();
            if spawned.is_ok() {
                self.log(&format!("opened browser: {}", cmd[..2].join(" ")));
                return true;
            }
        }
        false
    }

    /// `download(it, with_cookies)`.
    fn download(&self, it: &Value, with_cookies: bool) -> End {
        let url = text(it, "url").to_string();
        let title = title_or_url(it).to_string();
        let tag = short(&url);
        let dir = self.setting("DIR");
        let _ = fs::create_dir_all(&dir);
        let attempts = number(it.get("attempts"));
        let mut cmd: Vec<String> = if with_cookies { brave_cmd(&self.s) } else { vec!["yt-dlp".into()] };
        cmd.extend(NAME_OPTS.iter().map(|s| s.to_string()));
        if which("ffmpeg").is_some() {
            cmd.extend(META_OPTS.iter().map(|s| s.to_string()));
        }
        let format = self.s.get("FORMAT").cloned().unwrap_or_else(|| DEFAULT_FORMAT.into());
        let out_tmpl = format!("{}/{}", dir.replace('%', "%%").trim_end_matches('/'), NAME);
        for a in ["--no-playlist", "--newline", "--no-simulate", "-f", &format, "--merge-output-format", "mp4", "--progress", "--no-quiet", "--progress-template", &progress_template(), "--print", "after_move:FILE %(filepath)s"] {
            cmd.push(a.to_string());
        }
        // A capture's header wants the source, the license and who made it.
        // Only asked for when a capture will be made: with OUTPUT=mp4 the
        // command is the Python ytq's, argument for argument.
        if output_of(&self.s) != "mp4" {
            cmd.push("--print".into());
            cmd.push(NOTES_PRINT.into());
        }
        cmd.push("-o".into());
        cmd.push(out_tmpl.clone());
        cmd.push(url.clone());
        self.log(&format!("{tag}: attempt {} at {title}{}, into {dir}", py_str(&Value::Num(attempts)), if with_cookies { " with Brave's cookies" } else { "" }));
        self.log(&format!("run: {}", cmd.iter().map(|c| shlex_quote(c)).collect::<Vec<_>>().join(" ")));
        let started = sys::now();
        let mut live = Value::obj(vec![
            ("phase", Value::str("looking it up")),
            ("since", Value::Num(started)),
            ("started", Value::Num(started)),
            ("pid", Value::num(std::process::id() as u64)),
            ("cookies", Value::Bool(with_cookies)),
            ("done_b", Value::num(0)),
            ("parts_done", Value::num(0)),
        ]);
        self.update(&url, vec![("progress", Value::str("looking it up")), ("live", live.clone())]);
        let spawned = sys::pipe_pair().and_then(|(rd, wr)| {
            let err_end = wr.try_clone()?;
            let child = Command::new(&cmd[0])
                .args(&cmd[1..])
                .stdin(Stdio::null())
                .stdout(Stdio::from(wr))
                .stderr(Stdio::from(err_end))
                .process_group(0)
                .spawn()?;
            Ok((rd, child))
        });
        let (rd, mut child) = match spawned {
            Ok(x) => x,
            Err(e) => {
                self.update(&url, vec![("status", Value::str("failed")), ("error", Value::str(format!("cannot run {}: {}", cmd[0], e))), ("progress", Value::str("")), ("live", Value::Obj(Vec::new()))]);
                self.say(&format!("failed: cannot run {}: {}", cmd[0], e), true);
                return End::Finished;
            }
        };
        let pid = child.id();
        *self.current.lock().unwrap() = Current { pid: Some(pid), url: url.clone(), cookies: with_cookies, attempts: attempts - 1.0, phase: "looking it up".into(), started };
        let (tx, rx) = mpsc::channel::<String>();
        std::thread::spawn(move || {
            let mut r = BufReader::new(rd);
            let mut buf = Vec::new();
            loop {
                buf.clear();
                match r.read_until(b'\n', &mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        if tx.send(String::from_utf8_lossy(&buf).into_owned()).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        let mut tail: Vec<String> = Vec::new();
        let mut fname = String::new();
        let mut notes = Value::Obj(Vec::new());
        let mut wrote = 0.0;
        let mut sampled = started;
        loop {
            if interrupted() {
                return End::Interrupted;
            }
            let raw = match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(l) => l,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            };
            let line = raw.trim_end_matches(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)).to_string();
            if line.is_empty() {
                continue;
            }
            let now = sys::now();
            let mut changed = false;
            if let Some(rest) = line.strip_prefix("PROGRESS ") {
                for (k, v) in PROGRESS_KEYS.iter().zip(rest.split('|')) {
                    live.set(k, Value::str(py_strip(v)));
                }
                if text(&live, "state") == "finished" {
                    let total = number(live.get("total_b"));
                    let size = if total != 0.0 { total } else { number(live.get("got_b")) };
                    live.set("done_b", Value::Num(number(live.get("done_b")) + size));
                    live.set("parts_done", Value::Num(number(live.get("parts_done")) + 1.0));
                    let or_q = |s: String| if s.is_empty() { "?".to_string() } else { s };
                    self.log(&format!(
                        "{tag}: part {} done: {}, {} in {} at {}",
                        live.get("part").map(py_str).unwrap_or_else(|| "?".into()),
                        stream_of(&live),
                        fmt_size(size),
                        or_q(na(live.get("elapsed"))),
                        or_q(na(live.get("speed")))
                    ));
                    changed = true;
                } else if now - sampled >= 30.0 {
                    sampled = now;
                    self.log(&format!("{tag}: {}, {}, {}", compact(&live), stream_of(&live), size_of(&live)));
                }
            } else if let Some(n) = line.strip_prefix("NOTES ").filter(|_| output_of(&self.s) != "mp4") {
                if let Ok(v) = json::parse(n) {
                    notes = v;
                }
            } else if let Some(f) = line.strip_prefix("FILE ") {
                fname = f.to_string();
                self.log(&format!("{tag}: file {fname}"));
            } else if line.starts_with("[MetadataParser] ") {
                // A line per step of the filename rule, twice over with
                // /etc/yt-dlp.conf present; the name they make is logged as 'file'.
                continue;
            } else {
                tail.push(line.clone());
                if tail.len() > 6 {
                    tail.remove(0);
                }
                live.set("last", Value::str(&line));
                self.log(&format!("{tag}: yt-dlp: {line}"));
                let since = live.get("since").map(|v| number(Some(v))).unwrap_or(now);
                let took = fmt_secs(now - since);
                if stage(&line, &mut live, sys::now()) {
                    changed = true;
                    self.log(&format!("{tag}: now {} (the step before took {took})", describe(&live)));
                    if self.mode() == Mode::Print {
                        println!("  {}", describe(&live));
                    }
                }
            }
            // Once a second is plenty for the queue, and every write is a turn at
            // queue.lock that a 'ytq clip' might be waiting for. A new step goes in at once.
            if changed || now - wrote >= 1.0 {
                wrote = now;
                {
                    let mut cur = self.current.lock().unwrap();
                    cur.phase = live.get("phase").map(py_str).unwrap_or_default();
                }
                if self.update(&url, vec![("progress", Value::str(compact(&live))), ("live", live.clone())]).is_none() {
                    self.log(&format!("{tag}: deleted from the queue while downloading; stopping yt-dlp"));
                    let _ = sys::kill_group(pid, sys::SIGTERM);
                }
            }
        }
        let st = child.wait();
        let rc = st.map(|s| s.code().unwrap_or_else(|| -s.signal().unwrap_or(0))).unwrap_or(-1);
        let took = fmt_secs(sys::now() - started);
        {
            let mut cur = self.current.lock().unwrap();
            cur.pid = None;
            cur.url.clear();
            cur.cookies = false;
        }
        let phase = live.get("phase").map(py_str).unwrap_or_else(|| "None".into());
        if self.stop.load(Ordering::SeqCst) {
            self.log(&format!("{tag}: stopped while {phase}, {took} in; back in the queue"));
            self.update(&url, vec![
                ("status", Value::str(if with_cookies { "retry-cookies" } else { "queued" })),
                ("attempts", Value::Num(attempts - 1.0)),
                ("progress", Value::str("")),
                ("live", Value::Obj(Vec::new())),
            ]);
            return End::Finished;
        }
        self.current.lock().unwrap().attempts = 0.0;
        self.log(&format!("{tag}: yt-dlp exited {rc} after {took}, last step: {phase}"));
        if rc == 0 {
            if let Ok(m) = fs::metadata(&fname) {
                self.log(&format!("{tag}: {fname} is {}", fmt_size(m.len() as f64)));
            }
            // Static Stream. With OUTPUT=sstr (the default) or both, the download
            // is recorded into ARCHIVE_DIR and read back; with sstr the MP4 goes
            // only once the capture verifies and its payload is the MP4 byte for
            // byte. Anything short of that keeps the MP4, and says why.
            let output = output_of(&self.s);
            let mut kept = fname.clone();
            let mut video = basename(&fname).to_string();
            let mut stem = splitext(&fname).0.to_string();
            let mut not_archived = String::new();
            if !fname.is_empty() && output != "mp4" {
                let sstr = format!("{}/{}.sstr", archive_dir_of(&self.s).trim_end_matches('/'), basename(splitext(&fname).0));
                live.set("phase", Value::str("archiving to Static Stream"));
                live.set("since", Value::Num(sys::now()));
                live.set("last", Value::str(""));
                live.set("dest", Value::str(&sstr));
                self.update(&url, vec![("progress", Value::str("archiving")), ("live", live.clone())]);
                match self.archive(&tag, &url, &fname, &sstr, &notes) {
                    Ok(()) => {
                        stem = splitext(&sstr).0.to_string();
                        if output == "sstr" {
                            match fs::remove_file(&fname) {
                                Ok(()) => {
                                    self.log(&format!("{tag}: removed {fname}: the capture holds it"));
                                    kept = sstr.clone();
                                    video = basename(&sstr).to_string();
                                }
                                Err(e) => self.log(&format!("{tag}: could not remove {fname}: {e}")),
                            }
                        }
                    }
                    Err(why) => {
                        self.log(&format!("{tag}: not archived, so the MP4 stays: {why}"));
                        not_archived = format!(" (not archived: {why})");
                    }
                }
            }
            let mut also = "";
            let subs = self.setting("SUBS");
            if !fname.is_empty() && !subs.is_empty() && first_id(&url).is_some() {
                live.set("phase", Value::str("fetching the transcript"));
                live.set("since", Value::Num(sys::now()));
                live.set("last", Value::str(""));
                live.set("dest", Value::str(format!("{stem}.txt")));
                if self.update(&url, vec![("progress", Value::str("transcript")), ("live", live.clone())]).is_some() {
                    let tmpl = format!("{}.%(ext)s", stem.replace('%', "%%"));
                    let tcmd = if with_cookies { brave_cmd(&self.s) } else { vec!["yt-dlp".into()] };
                    match self.transcript(&url, &tmpl, &tcmd, Some(&video)) {
                        Ok(txt) => {
                            self.log(&format!("{tag}: transcript: {txt}"));
                            also = " + transcript";
                        }
                        Err(why) => {
                            self.log(&format!("{tag}: transcript: none -- {why}"));
                            also = " (no transcript)";
                        }
                    }
                }
            }
            if self.update(&url, vec![("status", Value::str("done")), ("file", Value::str(&kept)), ("progress", Value::str("")), ("error", Value::str("")), ("live", Value::Obj(Vec::new()))]).is_some() {
                self.say(&format!("done: {}{also}{not_archived}", if kept.is_empty() { title.as_str() } else { basename(&kept) }), false);
            }
            return End::Finished;
        }
        let err = first_chars(&tail.last().cloned().unwrap_or_else(|| format!("yt-dlp exited {rc}")), 300);
        let cookie_tried = truthy(it.get("cookie_tried"));
        let clear = |st: &str| vec![("status", Value::str(st)), ("error", Value::str(&err)), ("progress", Value::str("")), ("live", Value::Obj(Vec::new()))];
        if cookie_problem(&err) && !cookie_tried {
            self.log(&format!("{tag}: that reads like a login, age or bot check; waiting on a Brave sign-in"));
            if self.update(&url, clear("cookies")).is_some() {
                self.open_browser(&url);
                self.say(&format!("needs a Brave sign-in: {title} -- Brave is open on it; sign in, then 'ytq cookies'"), true);
            }
        } else if attempts < 2.0 && !cookie_tried {
            self.update(&url, clear("retry"));
            self.log(&format!("retry later: {url}: {err}"));
        } else if self.update(&url, clear("failed")).is_some() {
            self.say(&format!("failed: {title} -- {err}"), true);
        }
        End::Finished
    }

    /// Record a finished download into a Static Stream capture at `sstr`, read
    /// it back, and keep it only if it verifies and plays back as the file.
    fn archive(&self, tag: &str, url: &str, file: &str, sstr: &str, notes: &Value) -> Result<(), String> {
        let t0 = sys::now();
        if let Some(dir) = Path::new(sstr).parent() {
            fs::create_dir_all(dir).map_err(|e| format!("cannot make {}: {e}", dir.display()))?;
        }
        let part = format!("{sstr}.part");
        let pick = |k: &str| notes.get(k).filter(|v| truthy(Some(v))).map(py_str);
        let mut meta = Value::obj(vec![
            ("created", Value::str(sys::strftime_local("%Y-%m-%dT%H:%M:%S%z", sys::now()))),
            ("content_type", Value::str(content_type(file))),
            ("chunk", Value::num(65536)),
            ("align", Value::num(1)),
            ("fec", Value::obj(vec![("inner", Value::str("RS(255,223) GF(2^8)/0x11d, interleaved per record")), ("outer", Value::str("XOR, one per 16 data records"))])),
            ("checkpoint", Value::obj(vec![("records", Value::num(64)), ("seconds", Value::Num(10.0))])),
            ("source", Value::str(pick("webpage_url").unwrap_or_else(|| url.to_string()))),
        ]);
        if let Some(license) = pick("license") {
            meta.set("license", Value::str(license));
        }
        if let Some(t) = pick("title") {
            meta.set("note", Value::str(match pick("uploader").or_else(|| pick("channel")) {
                Some(by) => format!("{t} by {by}"),
                None => t,
            }));
        }
        let key = sstr_key_of(&self.s, &self.home);
        self.log(&format!(
            "{tag}: archiving {file} into {sstr}, {}",
            key.as_ref().map(|k| format!("signed with {}", k.display())).unwrap_or_else(|| "unsigned".into())
        ));
        let opts = writer::Options { key, deflate: self.setting("SSTR_DEFLATE").trim().eq_ignore_ascii_case("yes"), ..Default::default() };
        let fail = |why: String| {
            let _ = fs::remove_file(&part);
            why
        };
        let written = writer::record_file(Path::new(file), Path::new(&part), meta, opts).map_err(|e| fail(format!("recording failed: {e}")))?;
        let want = file_sha256(file).map_err(|e| fail(format!("cannot read {file}: {e}")))?;
        match reader::check_file(Path::new(&part)) {
            Ok((0, payload, _)) if payload == want => {}
            Ok((0, _, _)) => return Err(fail("the capture plays back as something other than the file".into())),
            Ok((code, _, st)) => {
                return Err(fail(format!("the capture does not verify (exit {code}: {} data records lost, {} mismatched, {} bad signatures)", st.data_lost, st.mismatched, st.sig_bad)))
            }
            Err(e) => return Err(fail(format!("cannot read the capture back: {e}"))),
        }
        fs::rename(&part, sstr).map_err(|e| fail(format!("cannot rename {part}: {e}")))?;
        let size = fs::metadata(sstr).map(|m| m.len() as f64).unwrap_or(0.0);
        let orig = fs::metadata(file).map(|m| m.len() as f64).unwrap_or(0.0);
        self.log(&format!(
            "{tag}: archived {sstr}: {} data records, {} for {} ({:.1} %), read back and verified in {}; its payload's SHA-256 is the file's, {}",
            written.data_records,
            fmt_size(size),
            fmt_size(orig),
            if orig > 0.0 { size * 100.0 / orig } else { 0.0 },
            fmt_secs(sys::now() - t0),
            &want[..16]
        ));
        Ok(())
    }

    /// The window's d on the entry being downloaded: yt-dlp goes at once. The
    /// entry is already gone from the queue, so there is nothing to put back.
    pub fn kill_if_current(&self, url: &str) {
        let cur = self.current.lock().unwrap();
        if cur.url == url {
            if let Some(pid) = cur.pid {
                let _ = sys::kill_group(pid, sys::SIGTERM);
            }
        }
    }

    /// `stop_current(why)`: kill the download under way and put its entry back as it was.
    pub fn stop_current(&self, why: &str) {
        let cur = self.current.lock().unwrap();
        let Some(pid) = cur.pid else { return };
        let url = cur.url.clone();
        let now = sys::now();
        self.log(&format!(
            "{}: stopping yt-dlp (pid {}) while {}, {} in, because {}. Back in the queue; finished parts and .part files stay, and the next attempt picks them up.",
            short(&url),
            pid,
            if cur.phase.is_empty() { "?" } else { &cur.phase },
            fmt_secs(now - cur.started),
            why
        ));
        let _ = sys::kill_group(pid, sys::SIGTERM);
        let status_back = if cur.cookies { "retry-cookies" } else { "queued" };
        let attempts = cur.attempts;
        drop(cur);
        self.update(&url, vec![("status", Value::str(status_back)), ("attempts", Value::Num(attempts)), ("progress", Value::str("")), ("live", Value::Obj(Vec::new()))]);
    }

    pub fn mark_cookie_retries(&self) -> usize {
        queue::edit(&self.paths, |items| {
            let mut n = 0;
            for i in items.iter_mut() {
                if status(i) == "cookies" {
                    i.set("status", Value::str("retry-cookies"));
                    n += 1;
                }
            }
            n
        })
        .unwrap_or(0)
    }

    fn sleep_watching(&self, d: Duration) {
        let until = Instant::now() + d;
        while Instant::now() < until && !interrupted() && !self.stop.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// `serve(stay, interactive)`: work the queue. Call holding run.lock; returns
    /// having let go of it once nothing is left -- or never, when told to stay.
    /// Err when a signal interrupted it.
    pub fn serve(self: &Arc<Self>, stay: bool, mut interactive: bool) -> Result<(), ()> {
        let me = Arc::clone(self);
        std::thread::spawn(move || {
            while !me.stop.load(Ordering::SeqCst) && !interrupted() {
                // Looking needs no lock; only this thread moves an entry out of 'checking'.
                match queue::snapshot(&me.paths).into_iter().find(|i| status(i) == "checking") {
                    Some(it) => me.check(text(&it, "url")),
                    None => std::thread::sleep(Duration::from_millis(500)),
                }
            }
        });
        while !self.stop.load(Ordering::SeqCst) {
            if interrupted() {
                return Err(());
            }
            let items = queue::snapshot(&self.paths);
            if !self.paused.load(Ordering::SeqCst) && items.iter().any(|i| DOWNLOADABLE.contains(&status(i))) {
                if let Some((it, cookies)) = self.claim() {
                    if interactive {
                        println!("{}  {}", if cookies { "with Brave's cookies" } else { "fetching" }, title_or_url(&it));
                    }
                    if let End::Interrupted = self.download(&it, cookies) {
                        return Err(());
                    }
                    continue;
                }
            }
            if stay || self.paused.load(Ordering::SeqCst) || items.iter().any(|i| PENDING.contains(&status(i))) {
                self.sleep_watching(Duration::from_secs(1));
                continue;
            }
            if interactive && items.iter().any(|i| status(i) == "cookies") {
                for i in items.iter().filter(|i| status(i) == "cookies") {
                    println!("needs a Brave sign-in: {}\n  {}", text(i, "url"), text(i, "error"));
                }
                println!("  Brave has been opened on it. Sign in or pass the check there, then press Enter here.");
                let mut line = String::new();
                if io::stdin().read_line(&mut line).map_or(true, |n| n == 0) {
                    interactive = false;
                    continue;
                }
                match brave_ready(&self.s) {
                    Some(why) => {
                        println!("{why} -- start Brave once, or set PROFILE in ~/.config/ytq/config");
                        interactive = false;
                    }
                    None => {
                        self.mark_cookie_retries();
                    }
                }
                continue;
            }
            // Nothing left. Let go of run.lock under queue.lock, so that a 'ytq
            // clip' either got its URL in before this look or finds the lock free.
            let left = queue::edit(&self.paths, |items| {
                if items.iter().any(|i| PENDING.contains(&status(i))) {
                    return false;
                }
                self.run.lock().unwrap().release();
                log(&self.paths, &format!("runner {}: the queue is empty, leaving", std::process::id()));
                true
            });
            if left.unwrap_or(false) {
                return Ok(());
            }
        }
        Ok(())
    }

    /// `cmd_run(quiet)`: `ytq run`.
    pub fn cmd_run(self: &Arc<Self>, quiet: bool, argv: &str) -> i32 {
        if quiet {
            self.set_mode(Mode::Notify);
        }
        let taken = {
            let mut run = self.run.lock().unwrap();
            run.take(&self.paths, 10, || self.log_start(argv)).unwrap_or(false)
        };
        if !taken {
            if !quiet {
                let holder = self.run.lock().unwrap().holder(&self.paths).unwrap_or_else(|| "?".into());
                println!("already downloading in pid {holder} -- 'ytq status' to follow it");
            }
            return 0;
        }
        catch_signals();
        let interactive = !quiet && io::stdin().is_terminal() && io::stdout().is_terminal();
        if self.serve(false, interactive).is_err() {
            self.stop.store(true, Ordering::SeqCst);
            self.stop_current("'ytq run' was interrupted (Ctrl+C, SIGTERM or SIGHUP)");
            if !quiet {
                println!();
            }
        }
        self.run.lock().unwrap().release();
        0
    }

    /// `transcript(url, outtmpl, cmd, video)`: the path of a .txt of url's
    /// captions, named by outtmpl as the video is, or why there is none.
    ///
    /// Its own yt-dlp run, after the video's: a caption fetch that fails -- and
    /// YouTube answers 429 to captions far sooner than to video -- fails the whole
    /// run it is part of, and would put a finished video back in the queue.
    pub fn transcript(&self, url: &str, outtmpl: &str, cmd: &[String], video: Option<&str>) -> Result<String, String> {
        let subs = Some(self.setting("SUBS")).filter(|s| !s.is_empty()).unwrap_or_else(|| DEFAULT_SUBS.into());
        let mut full: Vec<String> = cmd.to_vec();
        full.extend(NAME_OPTS.iter().map(|s| s.to_string()));
        for a in [
            "--skip-download", "--no-simulate", "--no-playlist", "--no-warnings", "--write-subs", "--write-auto-subs", "--sub-langs", &subs,
            "--sub-format", "vtt", "--print", "video:STEM %(filename)s",
            "--print", "video:META %(.{title,uploader,channel,webpage_url,timestamp,upload_date,license,description,subtitles})j",
            "-o", outtmpl, url,
        ] {
            full.push(a.to_string());
        }
        let tag = short(url);
        let t0 = sys::now();
        self.log(&format!("{tag}: fetching the transcript, captions {subs}"));
        self.log(&format!("run: {}", full.iter().map(|c| shlex_quote(c)).collect::<Vec<_>>().join(" ")));
        let (code, out, err) = match run_timeout(Command::new(&full[0]).args(&full[1..]), 300) {
            Ran::Done(c, o, e) => (c, o, e),
            Ran::TimedOut => {
                let e = format!("Command '{}' timed out after 300 seconds", full[0]);
                self.log(&format!("{tag}: transcript: {e}"));
                return Err(format!("cannot run {}: {}", full[0], e));
            }
            Ran::Failed(e) => {
                self.log(&format!("{tag}: transcript: {e}"));
                return Err(format!("cannot run {}: {}", full[0], e));
            }
        };
        self.log(&format!("{tag}: transcript run exited {code} after {}", fmt_secs(sys::now() - t0)));
        let errs = splitlines(py_strip(&err));
        for line in &errs[errs.len().saturating_sub(8)..] {
            self.log(&format!("{tag}: transcript: yt-dlp: {line}"));
        }
        let mut stem = String::new();
        let mut meta = Value::Obj(Vec::new());
        for line in splitlines(&out) {
            if let Some(f) = line.strip_prefix("STEM ") {
                // The extension is a guess made without choosing a format; the stem is right.
                stem = splitext(f).0.to_string();
            } else if let Some(m) = line.strip_prefix("META ") {
                if let Ok(v) = json::parse(m) {
                    meta = v;
                }
            }
        }
        // One file per language that matched: stem.en.vtt, stem.en-orig.vtt. The
        // shortest name, the plain language, is taken.
        let mut vtts = if stem.is_empty() { Vec::new() } else { siblings(&stem, |name, base| name.len() >= base.len() + 5 && name.ends_with(".vtt")) };
        vtts.sort_by(|a, b| (a.chars().count(), a).cmp(&(b.chars().count(), b)));
        self.log(&format!(
            "{tag}: captions written: {}",
            if vtts.is_empty() { "none".to_string() } else { vtts.iter().map(|v| basename(v)).collect::<Vec<_>>().join(", ") }
        ));
        if vtts.is_empty() {
            let last = errs.last().map(|s| s.to_string()).unwrap_or_else(|| format!("no captions matching {subs}"));
            return Err(first_chars(&last, 300));
        }
        let lang = vtts[0][stem.len() + 1..vtts[0].len() - ".vtt".len()].to_string();
        let result = (|| -> Result<Option<String>, String> {
            let text = textwrap::vtt_text(Path::new(&vtts[0])).map_err(|e| format!("cannot write the transcript: {e}"))?;
            if text.is_empty() {
                return Ok(None);
            }
            let video = match video {
                Some(v) => Some(v.to_string()),
                None => {
                    let mut all = siblings(&stem, |_, _| true);
                    all.sort();
                    all.into_iter()
                        .find(|p| matches!(splitext(p).1.to_lowercase().as_str(), ".mp4" | ".mkv" | ".webm" | ".m4v" | ".mov"))
                        .map(|p| basename(&p).to_string())
                }
            };
            let body = format!("{}{}\n", notes(&meta, url, &lang, video.as_deref()), text);
            fs::write(format!("{stem}.txt"), body).map_err(|e| format!("cannot write the transcript: {e}"))?;
            let words = text.split_whitespace().count();
            self.log(&format!("{tag}: wrote {stem}.txt: {words} words from the {lang} captions"));
            Ok(Some(text))
        })();
        for v in &vtts {
            let _ = fs::remove_file(v);
        }
        match result {
            Ok(Some(_)) => Ok(format!("{stem}.txt")),
            Ok(None) => Err(format!("the {lang} captions are empty")),
            Err(e) => Err(e),
        }
    }
}

/// `glob.glob(glob.escape(stem) + ".*")`, with a test on each name: the files
/// beside `stem` whose names start with its base name and a dot.
fn siblings(stem: &str, keep: impl Fn(&str, &str) -> bool) -> Vec<String> {
    let (dir, base) = match stem.rfind('/') {
        Some(i) => (&stem[..=i], &stem[i + 1..]),
        None => ("", stem),
    };
    let prefix = format!("{base}.");
    let Ok(rd) = fs::read_dir(if dir.is_empty() { "." } else { dir }) else { return Vec::new() };
    rd.filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| name.starts_with(&prefix) && (!name.starts_with('.') || base.starts_with('.')) && keep(name, base))
        .map(|name| format!("{dir}{name}"))
        .collect()
}

/// `notes(meta, url, lang, video)`: the top of a transcript -- the title; Notes,
/// with the times in local time and its offset; the description; the heading.
pub fn notes(meta: &Value, url: &str, lang: &str, video: Option<&str>) -> String {
    notes_at(meta, url, lang, video, sys::now())
}

pub fn notes_at(meta: &Value, url: &str, lang: &str, video: Option<&str>, now: f64) -> String {
    let when = |t: f64| sys::strftime_local("%Y-%m-%d %H:%M:%S %z", t);
    // yt-dlp takes the uploader's captions over automatic ones in the same
    // language, so a language among 'subtitles' is the uploader's.
    let theirs = matches!(meta.get("subtitles"), Some(Value::Obj(m)) if m.iter().any(|(k, _)| k == lang));
    let source = if theirs { "written by the uploader" } else { "automatic: YouTube's speech recognition, not verbatim" };
    let pick = |k: &str| meta.get(k).filter(|v| truthy(Some(v))).map(py_str);
    let title = pick("title").unwrap_or_else(|| url.to_string());
    let published = match meta.get("timestamp") {
        Some(Value::Num(t)) => when(*t),
        Some(Value::Bool(b)) => when(f64::from(u8::from(*b))),
        _ => match meta.get("upload_date").and_then(|v| v.as_str()) {
            Some(d) if d.len() == 8 && d.bytes().all(|b| b.is_ascii_digit()) => format!("{}-{}-{}", &d[..4], &d[4..6], &d[6..]),
            _ => "unknown".into(),
        },
    };
    let rows = [
        ("Title", title.clone()),
        ("Author", pick("uploader").or_else(|| pick("channel")).unwrap_or_else(|| "unknown".into())),
        ("URL", pick("webpage_url").unwrap_or_else(|| url.to_string())),
        ("Published", published),
        ("Downloaded", when(now)),
        // yt-dlp's license is what the site states: on YouTube, a Creative
        // Commons license's row on the watch page, and nothing otherwise.
        ("License", pick("license").unwrap_or_else(|| "not stated".into())),
        ("Video", video.filter(|v| !v.is_empty()).unwrap_or("none downloaded").to_string()),
        ("Captions", format!("{lang}, {source}")),
    ];
    let mut out = format!("{title}\n\nNotes\n");
    for (k, v) in rows {
        out.push_str(&format!("  {:<11} {}\n", format!("{k}:"), v));
    }
    out.push('\n');
    let desc = meta.get("description").map(py_str).unwrap_or_default();
    let desc = py_strip(if truthy(meta.get("description")) { &desc } else { "" });
    if !desc.is_empty() {
        out.push_str(&format!("Description\n{desc}\n\n"));
    }
    out.push_str("Transcript\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_as_shlex() {
        assert_eq!(shlex_quote("yt-dlp"), "yt-dlp");
        assert_eq!(shlex_quote(""), "''");
        assert_eq!(shlex_quote("%(title)#S:(?s)(?P<safe_title>.+)"), "'%(title)#S:(?s)(?P<safe_title>.+)'");
        assert_eq!(shlex_quote("it's"), "'it'\"'\"'s'");
        assert_eq!(shlex_quote("a/b.c,d=e+f@g%h:i-j_k"), "a/b.c,d=e+f@g%h:i-j_k");
    }

    #[test]
    fn the_progress_template() {
        assert!(progress_template().starts_with("download:PROGRESS %(progress._percent_str)s|%(progress.downloaded_bytes)s|"));
        assert!(progress_template().ends_with("|%(info.height)s|%(progress.status)s"));
    }

    #[test]
    fn notes_read_as_the_python_ytqs() {
        let meta = json::parse(r#"{"title": "Me at the zoo", "uploader": "jawed", "webpage_url": "https://www.youtube.com/watch?v=jNQXAC9IVRw",
            "timestamp": 1114313512, "license": null, "description": "  desc  ", "subtitles": {"en": []}}"#).unwrap();
        let n = notes_at(&meta, "u", "en", Some("jawed-Me_at_the_zoo_jNQXAC9IVRw.mp4"), 1114313512.0);
        assert!(n.starts_with("Me at the zoo\n\nNotes\n  Title:      Me at the zoo\n  Author:     jawed\n"), "{n}");
        assert!(n.contains("  License:    not stated\n  Video:      jawed-Me_at_the_zoo_jNQXAC9IVRw.mp4\n  Captions:   en, written by the uploader\n\nDescription\ndesc\n\nTranscript\n"), "{n}");
        assert!(cookie_problem("ERROR: Sign in to confirm you're not a bot") && !cookie_problem("HTTP Error 500"));
    }
}
