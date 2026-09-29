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
use std::io::{self, BufRead, BufReader, IsTerminal, Read, Write};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use crate::format::json::{self, Value};
use crate::format::sha256::{hex, Sha256};
use crate::format::{reader, writer};

use crate::ytq::clean::{clean_info, one_line};
use crate::ytq::live::{compact, describe, fmt_secs, fmt_size, na, number, py_str, size_of, splitext, stage, stream_of, truthy, PROGRESS_KEYS};
use crate::ytq::log::{log, say, splitlines, Mode};
use crate::ytq::queue::{self, status, text, title_or_url, RunLock};
use crate::ytq::settings::{DEFAULT_COMMENTS, DEFAULT_FORMAT, DEFAULT_SUBS};
use crate::ytq::term::printable;
use crate::ytq::urls::{first_id, py_strip, short};
use crate::ytq::{sys, textwrap, Paths, DOWNLOADABLE, PENDING};

pub const COOKIE_WORDS: [&str; 13] =
    ["sign in", "log in", "login", "cookies", "age", "bot", "private video", "members", "premium", "confirm you", "403", "restricted", "subscriber"];

pub const NAME_OPTS: &[&str] = &[
    "--parse-metadata", "%(uploader,channel|)#S:(?s)(?P<safe_author>.+)",
    "--replace-in-metadata", "safe_author", "^[^A-Za-z0-9]+", "",
    "--replace-in-metadata", "safe_author", r"(?s)^([A-Za-z0-9]+)(?:[^A-Za-z0-9]+([A-Za-z0-9]+))?(?:[^A-Za-z0-9]+([A-Za-z0-9]+))?.*", r"\1\2\3",
    "--parse-metadata", "%(title)#S:(?s)(?P<safe_title>.+)",
    "--replace-in-metadata", "safe_title", "[^A-Za-z0-9_-]+", "_",
    "--replace-in-metadata", "safe_title", "[-_]*_[-_]*", "_",
    "--parse-metadata", "%(safe_author&{}-|)s%(safe_title|)s:(?s)(?P<safe_head>.+)",
    "--replace-in-metadata", "safe_head", "(?<=^.{140}).+", "",
    "--replace-in-metadata", "safe_head", "^[-_]+|[-_]+$", "",
    "--parse-metadata", "id:(?s)(?P<safe_id>.+)",
    "--replace-in-metadata", "safe_id", "[^A-Za-z0-9_-]+", "_",
];
pub const NAME: &str = "%(safe_head&{}_|)s%(safe_id)s.%(ext)s";
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
/// How many comments a download may take: `COMMENTS` where it is set, the
/// default where it is not, and none at all where it is set to nothing --
/// `SUBS`'s shape, including empty-disables.
pub fn comments_of(s: &BTreeMap<String, String>) -> String {
    match s.get("COMMENTS") {
        Some(c) if c.trim().is_empty() => String::new(),
        Some(c) => c.trim().to_string(),
        None => DEFAULT_COMMENTS.to_string(),
    }
}

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

/// The fields of yt-dlp's info that ytq keeps: the notes, the capture's
/// header and the tags written into the video are all made from these. One
/// list for both runs, so a field added here reaches all three.
macro_rules! info_fields {
    () => {
        "webpage_url,webpage_url_domain,extractor_key,license,title,uploader,uploader_id,uploader_url,\
         channel,channel_id,channel_url,channel_follower_count,duration,timestamp,upload_date,description,\
         view_count,like_count,dislike_count,repost_count,comment_count,\
         tags,categories,language,location,chapters,heatmap,live_status,availability,age_limit,media_type,thumbnail,\
         resolution,vcodec,acodec,fps"
    };
}

/// What yt-dlp prints after the move when a capture will be made: the fields
/// its header carries.
pub const NOTES_PRINT: &str = concat!("after_move:NOTES %(.{", info_fields!(), "})j");

/// The transcript run's copy of the same fields, with the captions on offer.
const META_PRINT: &str = concat!("video:META %(.{", info_fields!(), ",subtitles})j");

/// The fields of yt-dlp's info that may run to more than one line. Every
/// other string is a name, a word or an address, and is kept to one.
const LONG_FIELDS: &[&str] = &["description"];

/// The fields that describe the file downloaded rather than the video. A run
/// that downloads nothing still fills them, from the format it would have
/// chosen, so they are taken from the download's own run or not at all.
const FORMAT_FIELDS: [&str; 4] = ["resolution", "vcodec", "acodec", "fps"];

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
                    let (title, height, ext) = (&one_line(f[0]), &one_line(f[1]), &one_line(f[2]));
                    let (title, height, ext) = (title.as_str(), height.as_str(), ext.as_str());
                    let q = format!("{} {}", if matches!(height, "NA" | "" | "None") { "?".to_string() } else { format!("{height}p") }, ext);
                    self.update(url, vec![("status", Value::str("queued")), ("title", Value::str(first_chars(title, 200))), ("quality", Value::str(&q)), ("error", Value::str(""))]);
                    self.log(&format!("queued {title} [{q}] {url} (checked in {took:.1} s)"));
                } else {
                    self.log(&format!("{tag}: check: yt-dlp exited {code} after {took:.1} s"));
                    let lines = splitlines(py_strip(&err));
                    for line in &lines[lines.len().saturating_sub(4)..] {
                        self.log(&format!("{tag}: check: {line}"));
                    }
                    let e = first_chars(&one_line(lines.last().copied().unwrap_or("no output")), 300);
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
        // A capture's header wants the source, the license and who made it,
        // and the tags written into the video want all of it. Asked for every
        // time: with OUTPUT=mp4 there is no capture, but there are still tags,
        // and a site with no transcript run has no other source for them.
        cmd.push("--print".into());
        cmd.push(NOTES_PRINT.into());
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
            // A line of yt-dlp's can quote the site: an error it was given, a
            // name. JSON comes through as it was, its escapes being six
            // printable characters each until it is parsed.
            let line = one_line(raw.trim_end_matches(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)));
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
            } else if let Some(n) = line.strip_prefix("NOTES ") {
                if let Ok(v) = json::parse(n) {
                    notes = clean_info(v, LONG_FIELDS);
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
                        println!("  {}", printable(&describe(&live)));
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
            let subs = self.setting("SUBS");
            let ffmpeg = which("ffmpeg").is_some();
            let taggable = ffmpeg && matches!(splitext(&fname).1.to_lowercase().as_str(), ".mp4" | ".m4v" | ".mov" | ".m4a");
            // The transcript run comes before the capture, not after it: what
            // it brings back is written into the video, and a capture's payload
            // is the video byte for byte, so the video must be final first.
            // Nothing is written to the .txt until the capture is settled, so
            // the notes can name the file that was kept.
            let mut fetched: Option<Result<Fetched, String>> = None;
            if !fname.is_empty() && !subs.is_empty() && first_id(&url).is_some() {
                live.set("phase", Value::str("fetching the transcript"));
                live.set("since", Value::Num(sys::now()));
                live.set("last", Value::str(""));
                live.set("dest", Value::str(format!("{}.txt", splitext(&fname).0)));
                if self.update(&url, vec![("progress", Value::str("transcript")), ("live", live.clone())]).is_some() {
                    let tmpl = format!("{}.%(ext)s", splitext(&fname).0.replace('%', "%%"));
                    let tcmd = if with_cookies { brave_cmd(&self.s) } else { vec!["yt-dlp".into()] };
                    fetched = Some(self.fetch_transcript(&url, &tmpl, &tcmd, taggable));
                }
            }
            let info = match &fetched {
                Some(Ok(f)) => merged(&notes, &f.meta),
                _ => notes.clone(),
            };
            let cover = match &fetched {
                Some(Ok(f)) => f.cover.clone(),
                _ => None,
            };
            // The notes, the transcript, the chapters and the cover, in the
            // video itself: a file that is copied away from its .txt, or
            // outlives it, still says what it is and what was said in it.
            if taggable {
                live.set("phase", Value::str("writing the notes into the video"));
                live.set("since", Value::Num(sys::now()));
                live.set("last", Value::str(""));
                live.set("dest", Value::str(&fname));
                if self.update(&url, vec![("progress", Value::str("tagging")), ("live", live.clone())]).is_some() {
                    let captions = match &fetched {
                        Some(Ok(f)) => f.captions.as_ref().ok().map(|(l, t)| (l.as_str(), t.as_str())),
                        _ => None,
                    };
                    match self.tag_video(&url, &fname, &info, captions, cover.as_deref()) {
                        Ok(what) => self.log(&format!("{tag}: tagged {fname}: {what}")),
                        Err(why) => self.log(&format!("{tag}: not tagged, so the video is as yt-dlp left it: {why}")),
                    }
                }
            }
            if let Some(c) = &cover {
                let _ = fs::remove_file(c);
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
                match self.archive(&tag, &url, &fname, &sstr, &info) {
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
            // Two jobs, not one. The transcript needs a YouTube video and a
            // SUBS list; the notes need neither and are owed to every download,
            // a video whose captions could not be had included.
            let captions = match fetched {
                Some(Ok(f)) => Some(f.captions),
                Some(Err(why)) => Some(Err(why)),
                None => None,
            };
            if let Some(Ok((lang, text))) = &captions {
                match self.write_transcript(&url, &stem, Some(&video), &info, lang, text) {
                    Ok(txt) => {
                        self.log(&format!("{tag}: transcript: {txt}"));
                        also = " + transcript";
                    }
                    Err(why) => {
                        self.log(&format!("{tag}: transcript: none -- {why}"));
                        also = " (no transcript)";
                    }
                }
            } else if !fname.is_empty() {
                if let Some(Err(why)) = &captions {
                    self.log(&format!("{tag}: transcript: none -- {why}"));
                    also = " (no transcript)";
                }
                live.set("phase", Value::str("writing the notes"));
                live.set("since", Value::Num(sys::now()));
                live.set("last", Value::str(""));
                live.set("dest", Value::str(format!("{stem}.txt")));
                if self.update(&url, vec![("progress", Value::str("notes")), ("live", live.clone())]).is_some() {
                    match self.notes_file(&url, &stem, &video, &info) {
                        Ok(txt) => {
                            self.log(&format!("{tag}: notes: {txt}"));
                            if captions.is_none() {
                                also = " + notes";
                            }
                        }
                        Err(why) => {
                            self.log(&format!("{tag}: notes: none -- {why}"));
                            if captions.is_none() {
                                also = " (no notes)";
                            }
                        }
                    }
                }
            }
            // The discussion, on a rung of its own. A video that simply has no
            // comments is not a failure and says nothing in the done line; only a
            // fetch that broke earns "(no discussion)".
            let mut talk = String::new();
            let cap = comments_of(&self.s);
            if !fname.is_empty() && !cap.is_empty() && first_id(&url).is_some() {
                live.set("phase", Value::str("fetching the discussion"));
                live.set("since", Value::Num(sys::now()));
                live.set("last", Value::str(""));
                live.set("dest", Value::str(format!("{stem}.txt")));
                if self.update(&url, vec![("progress", Value::str("discussion")), ("live", live.clone())]).is_some() {
                    let ccmd = if with_cookies { brave_cmd(&self.s) } else { vec!["yt-dlp".into()] };
                    match self.discussion_file(&url, &stem, &video, &info, &ccmd, &cap) {
                        Ok(0) => self.log(&format!("{tag}: discussion: none")),
                        Ok(n) => {
                            self.log(&format!("{tag}: discussion: {n} comments in {stem}.txt"));
                            talk = " + discussion".to_string();
                        }
                        Err(why) => {
                            self.log(&format!("{tag}: discussion: none -- {why}"));
                            talk = " (no discussion)".to_string();
                        }
                    }
                }
            }
            if self.update(&url, vec![("status", Value::str("done")), ("file", Value::str(&kept)), ("progress", Value::str("")), ("error", Value::str("")), ("live", Value::Obj(Vec::new()))]).is_some() {
                self.say(&format!("done: {}{also}{talk}{not_archived}", if kept.is_empty() { title.as_str() } else { basename(&kept) }), false);
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
        let meta = capture_meta(notes, url, file, sys::now());
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
                        println!("{}  {}", if cookies { "with Brave's cookies" } else { "fetching" }, printable(title_or_url(&it)));
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
                    println!("needs a Brave sign-in: {}\n  {}", printable(text(i, "url")), printable(text(i, "error")));
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

    /// The comments of `url`, and how many the site says there are in all.
    ///
    /// **Its own yt-dlp run, deliberately not the transcript's.** A comment fetch
    /// is the slowest and most rate-limited thing yt-dlp does here; sharing a run
    /// would mean a 429 on comments cost us captions that were already in hand.
    /// Every step that can fail alone fails alone.
    fn fetch_comments(&self, url: &str, cmd: &[String], cap: &str) -> Result<(Vec<Value>, Option<f64>), String> {
        let mut full: Vec<String> = cmd.to_vec();
        let ea = format!("youtube:max_comments={cap}");
        for a in [
            "--skip-download", "--no-simulate", "--no-playlist", "--no-warnings", "--write-comments",
            // --write-comments puts the comments in the infojson, and under
            // --no-simulate yt-dlp then WRITES that infojson -- 82 KB of it,
            // into whatever directory the process happens to be in, because
            // this run needs no -o and so gives none. One per download, in a
            // place nobody chose. Asked for explicitly not to.
            "--no-write-info-json",
            "--extractor-args", &ea,
            "--print", "COUNT %(comment_count)s",
            "--print", "TALK %(comments)j",
            url,
        ] {
            full.push(a.to_string());
        }
        let tag = short(url);
        let t0 = sys::now();
        self.log(&format!("{tag}: fetching the discussion, at most {cap} comments"));
        self.log(&format!("run: {}", full.iter().map(|c| shlex_quote(c)).collect::<Vec<_>>().join(" ")));
        let (code, out, err) = match run_timeout(Command::new(&full[0]).args(&full[1..]), 600) {
            Ran::Done(c, o, e) => (c, o, e),
            Ran::TimedOut => return Err(format!("Command '{}' timed out after 600 seconds", full[0])),
            Ran::Failed(e) => return Err(format!("cannot run {}: {}", full[0], e)),
        };
        self.log(&format!("{tag}: discussion run exited {code} after {}", fmt_secs(sys::now() - t0)));
        let errs = splitlines(py_strip(&err));
        for line in &errs[errs.len().saturating_sub(4)..] {
            self.log(&format!("{tag}: discussion: yt-dlp: {line}"));
        }
        let mut list = Vec::new();
        let mut total = None;
        for line in splitlines(&out) {
            if let Some(n) = line.strip_prefix("COUNT ") {
                total = n.trim().parse::<f64>().ok();
            } else if let Some(t) = line.strip_prefix("TALK ") {
                if let Ok(Value::Arr(v)) = json::parse(t) {
                    list = v.into_iter().map(|c| clean_info(c, &["text"])).collect();
                }
            }
        }
        if list.is_empty() && code != 0 {
            let last = errs.last().map(|s| s.to_string()).unwrap_or_else(|| format!("yt-dlp exited {code}"));
            return Err(first_chars(&one_line(&last), 300));
        }
        Ok((list, total))
    }

    /// Append the Discussion to `{stem}.txt`, or write that file first if the
    /// captions never got far enough to. Returns how many comments were written.
    fn discussion_file(&self, url: &str, stem: &str, video: &str, meta: &Value, cmd: &[String], cap: &str) -> Result<usize, String> {
        let (list, total) = self.fetch_comments(url, cmd, cap)?;
        if list.is_empty() {
            return Ok(0);
        }
        let body = discussion(&list, total);
        let path = format!("{stem}.txt");
        if Path::new(&path).exists() {
            let mut f = fs::OpenOptions::new().append(true).open(&path).map_err(|e| format!("cannot open {path}: {e}"))?;
            f.write_all(format!("\n{body}").as_bytes()).map_err(|e| format!("cannot write the discussion: {e}"))?;
        } else {
            let head = notes(meta, url, None, Some(video));
            fs::write(&path, format!("{head}{body}")).map_err(|e| format!("cannot write the discussion: {e}"))?;
        }
        Ok(list.len())
    }

    /// `{stem}.txt` for a download with no captions to ask for: the notes of
    /// the run that has just finished, and nothing after them.
    ///
    /// The transcript's twin, and deliberately the cheap one -- it starts no
    /// process and makes no request, because `meta` is the NOTES line yt-dlp
    /// printed after moving the file into place.
    fn notes_file(&self, url: &str, stem: &str, video: &str, meta: &Value) -> Result<String, String> {
        let path = format!("{stem}.txt");
        fs::write(&path, notes(meta, url, None, Some(video))).map_err(|e| format!("cannot write the notes: {e}"))?;
        Ok(path)
    }

    /// `transcript(url, outtmpl, cmd, video)`: the path of a .txt of url's
    /// captions, named by outtmpl as the video is, or why there is none.
    ///
    /// Its own yt-dlp run, after the video's: a caption fetch that fails -- and
    /// YouTube answers 429 to captions far sooner than to video -- fails the whole
    /// run it is part of, and would put a finished video back in the queue.
    pub fn transcript(&self, url: &str, outtmpl: &str, cmd: &[String], video: Option<&str>) -> Result<String, String> {
        let got = self.fetch_transcript(url, outtmpl, cmd, false)?;
        let (lang, text) = got.captions?;
        let video = match video {
            Some(v) => Some(v.to_string()),
            None => {
                let mut all = siblings(&got.stem, |_, _| true);
                all.sort();
                all.into_iter()
                    .find(|p| matches!(splitext(p).1.to_lowercase().as_str(), ".mp4" | ".mkv" | ".webm" | ".m4v" | ".mov"))
                    .map(|p| basename(&p).to_string())
            }
        };
        self.write_transcript(url, &got.stem, video.as_deref(), &got.meta, &lang, &text)
    }

    /// `{stem}.txt`: the notes, then the captions as plain text.
    fn write_transcript(&self, url: &str, stem: &str, video: Option<&str>, meta: &Value, lang: &str, text: &str) -> Result<String, String> {
        let body = format!("{}{}\n", notes(meta, url, Some(lang), video), text);
        let path = format!("{stem}.txt");
        fs::write(&path, body).map_err(|e| format!("cannot write the transcript: {e}"))?;
        let words = text.split_whitespace().count();
        self.log(&format!("{}: wrote {path}: {words} words from the {lang} captions", short(url)));
        Ok(path)
    }

    /// The transcript run: the captions as plain text, the video's info as it
    /// stands now, and -- with `cover` -- its thumbnail as `{stem}.jpg` for the
    /// video's cover. Nothing is written but that picture; the captions' .vtt
    /// files are read and removed. `Err` only when yt-dlp could not be run at
    /// all: a run that found no captions still brings back the info.
    fn fetch_transcript(&self, url: &str, outtmpl: &str, cmd: &[String], cover: bool) -> Result<Fetched, String> {
        let subs = Some(self.setting("SUBS")).filter(|s| !s.is_empty()).unwrap_or_else(|| DEFAULT_SUBS.into());
        let mut full: Vec<String> = cmd.to_vec();
        full.extend(NAME_OPTS.iter().map(|s| s.to_string()));
        for a in [
            "--skip-download", "--no-simulate", "--no-playlist", "--no-warnings", "--write-subs", "--write-auto-subs", "--sub-langs", &subs,
            "--sub-format", "vtt", "--print", "video:STEM %(filename)s", "--print", META_PRINT,
        ] {
            full.push(a.to_string());
        }
        // The picture YouTube shows for the video, as a JPEG an MP4 can carry:
        // its own is usually WebP, which the MP4 muxer will not take.
        if cover {
            for a in ["--write-thumbnail", "--convert-thumbnails", "jpg"] {
                full.push(a.to_string());
            }
        }
        for a in ["-o", outtmpl, url] {
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
                    meta = clean_info(v, LONG_FIELDS);
                }
            }
        }
        // Nothing was downloaded, so these describe a format nobody has.
        if let Value::Obj(m) = &mut meta {
            m.retain(|(k, _)| !FORMAT_FIELDS.contains(&k.as_str()));
        }
        let cover = Some(format!("{stem}.jpg")).filter(|c| cover && !stem.is_empty() && Path::new(c).is_file());
        // One file per language that matched: stem.en.vtt, stem.en-orig.vtt. The
        // shortest name, the plain language, is taken.
        let mut vtts = if stem.is_empty() { Vec::new() } else { siblings(&stem, |name, base| name.len() >= base.len() + 5 && name.ends_with(".vtt")) };
        vtts.sort_by(|a, b| (a.chars().count(), a).cmp(&(b.chars().count(), b)));
        self.log(&format!(
            "{tag}: captions written: {}",
            if vtts.is_empty() { "none".to_string() } else { vtts.iter().map(|v| basename(v)).collect::<Vec<_>>().join(", ") }
        ));
        let captions = if vtts.is_empty() {
            let last = errs.last().map(|s| s.to_string()).unwrap_or_else(|| format!("no captions matching {subs}"));
            Err(first_chars(&one_line(&last), 300))
        } else {
            let lang = vtts[0][stem.len() + 1..vtts[0].len() - ".vtt".len()].to_string();
            match textwrap::vtt_text(Path::new(&vtts[0])) {
                Err(e) => Err(format!("cannot write the transcript: {e}")),
                Ok(text) if text.is_empty() => Err(format!("the {lang} captions are empty")),
                Ok(text) => Ok((lang, text)),
            }
        };
        for v in &vtts {
            let _ = fs::remove_file(v);
        }
        Ok(Fetched { stem, meta, captions, cover })
    }

    /// Write what ytq knows into the video file itself: the fields of `meta`
    /// as its tags, the transcript as its lyrics, its chapters, and `cover` as
    /// its picture. A copy is made and renamed over the original, so a failure
    /// at any point leaves the file as yt-dlp left it.
    ///
    /// Everything goes through an FFMETADATA file rather than `-metadata`: a
    /// long talk's transcript is past the 128 KiB Linux allows one argument.
    fn tag_video(&self, url: &str, file: &str, meta: &Value, captions: Option<(&str, &str)>, cover: Option<&str>) -> Result<String, String> {
        let (stem, ext) = splitext(file);
        let tmp = format!("{stem}.tagging{ext}");
        let ffmeta = format!("{stem}.ffmetadata");
        fs::write(&ffmeta, ffmetadata(meta, url, captions, basename(file), sys::now())).map_err(|e| format!("cannot write {ffmeta}: {e}"))?;
        // The cover goes after the video it belongs to, so the video stays the
        // first stream a player finds; its index is one past the real ones.
        let videos = match run_timeout(Command::new("ffprobe").args(["-v", "error", "-select_streams", "V", "-show_entries", "stream=index", "-of", "csv=p=0", file]), 60) {
            Ran::Done(0, o, _) => splitlines(&o).iter().filter(|l| !l.trim().is_empty()).count(),
            _ => 0,
        };
        let cover = cover.filter(|_| videos > 0);
        let mut cmd = Command::new("ffmpeg");
        cmd.args(["-nostdin", "-v", "error", "-y", "-i", file, "-i", &ffmeta]);
        if let Some(c) = cover {
            cmd.args(["-i", c]);
        }
        // V, not v: a cover from before is left out rather than doubled.
        cmd.args(["-map", "0:V?", "-map", "0:a?", "-map", "0:s?"]);
        if cover.is_some() {
            cmd.args(["-map", "2:v"]);
        }
        cmd.args(["-map_metadata", "1", "-map_chapters", "1", "-c", "copy"]);
        if cover.is_some() {
            cmd.args([format!("-disposition:v:{videos}"), "attached_pic".to_string()]);
        }
        cmd.arg(&tmp);
        let t0 = sys::now();
        let ran = run_timeout(&mut cmd, 1800);
        let _ = fs::remove_file(&ffmeta);
        let why = match ran {
            Ran::Done(0, _, _) => match fs::rename(&tmp, file) {
                Ok(()) => None,
                Err(e) => Some(format!("cannot replace {file}: {e}")),
            },
            Ran::Done(c, _, e) => Some(format!("ffmpeg exited {c}: {}", first_chars(splitlines(py_strip(&e)).last().copied().unwrap_or(""), 300))),
            Ran::TimedOut => Some("ffmpeg timed out after 1800 seconds".to_string()),
            Ran::Failed(e) => Some(format!("cannot run ffmpeg: {e}")),
        };
        if let Some(why) = why {
            let _ = fs::remove_file(&tmp);
            return Err(why);
        }
        let mut what = vec!["notes".to_string()];
        if let Some((lang, text)) = captions {
            what.push(format!("{} words of {lang} transcript", text.split_whitespace().count()));
        }
        let count = |k: &str| meta.get(k).and_then(|v| v.as_array()).map_or(0, |a| a.len());
        if count("tags") > 0 {
            what.push(format!("{} tags", count("tags")));
        }
        if count("chapters") > 0 {
            what.push(format!("{} chapters", count("chapters")));
        }
        if cover.is_some() {
            what.push("cover".to_string());
        }
        Ok(format!("{} in {}", what.join(", "), fmt_secs(sys::now() - t0)))
    }
}

/// What the transcript run brought back.
struct Fetched {
    stem: String,
    meta: Value,
    captions: Result<(String, String), String>,
    cover: Option<String>,
}

/// The download's info with the transcript run's laid over it: the later run
/// read the counts later, and has the captions on offer; the download alone
/// knows the format it got.
fn merged(download: &Value, later: &Value) -> Value {
    let mut out = download.clone();
    if let Value::Obj(m) = later {
        for (k, v) in m {
            if !matches!(v, Value::Null) {
                out.set(k, v.clone());
            }
        }
    }
    out
}

/// A value for an FFMETADATA file: `=`, `;`, `#`, `\`, newlines and carriage
/// returns escaped, and NUL left out.
///
/// ffmpeg ends a line at a carriage return as it does at a newline, so one
/// that is not escaped ends the value and starts a line of the file with
/// whatever followed it -- `[CHAPTER]`, say. It ends one at a NUL too, and
/// there is no escaping that: the value is a C string by the time the
/// backslash is taken off, so it would end there all the same.
fn ffescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c == '\0' {
            continue;
        }
        if matches!(c, '=' | ';' | '#' | '\\' | '\n' | '\r') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The FFMETADATA file for `tag_video()`: every tag the MP4 is to carry, and
/// its chapters. The file's tags replace the ones yt-dlp wrote, so the ones it
/// wrote are written again here, from the same fields: title, artist, date,
/// genre, description and synopsis.
///
/// The comment is the notes block of the .txt -- every row, the counts and
/// what they were read at, the tags and the chapters -- so the file on its own
/// says what the text beside it says, less the description (its own tag) and
/// the transcript (the lyrics).
pub fn ffmetadata(meta: &Value, url: &str, captions: Option<(&str, &str)>, video: &str, now: f64) -> String {
    let pick = |k: &str| meta.get(k).filter(|v| truthy(Some(v))).map(py_str);
    let list = |k: &str| -> Vec<String> { meta.get(k).and_then(|v| v.as_array()).map(|a| a.iter().map(py_str).filter(|s| !s.is_empty()).collect()).unwrap_or_default() };
    let mut tags: Vec<(&str, String)> = Vec::new();
    if let Some(t) = pick("title") {
        tags.push(("title", t));
    }
    if let Some(a) = pick("uploader").or_else(|| pick("channel")) {
        tags.push(("artist", a));
    }
    if let Some(d) = pick("upload_date") {
        tags.push(("date", d));
    }
    if !list("categories").is_empty() {
        tags.push(("genre", list("categories").join(", ")));
    }
    if let Some(d) = pick("description") {
        tags.push(("description", d.clone()));
        tags.push(("synopsis", d));
    }
    if !list("tags").is_empty() {
        tags.push(("keywords", list("tags").join(", ")));
    }
    if let Some(l) = pick("license") {
        tags.push(("copyright", l));
    }
    let head = notes_head(meta, url, captions.map(|c| c.0), Some(video), now);
    tags.push(("comment", py_strip(&head).to_string()));
    if let Some((_, text)) = captions {
        tags.push(("lyrics", text.to_string()));
    }
    let mut out = String::from(";FFMETADATA1\n");
    for (k, v) in tags {
        out.push_str(&format!("{k}={}\n", ffescape(&v)));
    }
    let duration = meta.get("duration").and_then(|v| v.as_num());
    for c in meta.get("chapters").and_then(|v| v.as_array()).unwrap_or(&[]) {
        let (Some(start), Some(title)) = (c.get("start_time").and_then(|v| v.as_num()), c.get("title").map(py_str)) else { continue };
        let end = c.get("end_time").and_then(|v| v.as_num()).or(duration).unwrap_or(start);
        out.push_str(&format!("\n[CHAPTER]\nTIMEBASE=1/1000\nSTART={}\nEND={}\ntitle={}\n", (start * 1000.0).round() as i64, (end * 1000.0).round() as i64, ffescape(&title)));
    }
    out
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

/// The metadata a capture carries about the download inside it.
///
/// Pure, and separated from `archive()` so it can be held to fixtures: a
/// capture outlives the queue, the program and very likely the site, so what
/// goes in its header is worth checking rather than assuming.
pub fn capture_meta(notes: &Value, url: &str, file: &str, now: f64) -> Value {
    let pick = |k: &str| notes.get(k).filter(|v| truthy(Some(v))).map(py_str);
    let stamp = |t: f64| sys::strftime_local("%Y-%m-%dT%H:%M:%S%z", t);
    let mut meta = Value::obj(vec![
        ("created", Value::str(stamp(now))),
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
    // Where it was posted and how long it runs, in the capture's own header --
    // the same two facts the .txt's Notes gained, by the same rules, because
    // the capture is the copy most likely to outlive both the page and the
    // .txt sitting beside it. A header that knows less than the text file next
    // to it is a header nobody will trust to stand alone.
    let author = pick("uploader").or_else(|| pick("channel")).unwrap_or_default();
    let where_ = match pick("extractor_key").unwrap_or_default().as_str() {
        "Reddit" => pick("channel_id").map(|sub| format!("r/{}", sub.trim_start_matches("r/"))),
        "Twitter" => pick("uploader_id").map(|h| format!("@{}", h.trim_start_matches('@'))),
        _ => pick("channel").filter(|c| !author.is_empty() && *c != author),
    };
    if let Some(w) = where_ {
        meta.set("where", Value::str(w));
    }
    if let Some(Value::Num(d)) = notes.get("duration").filter(|v| truthy(Some(v))) {
        meta.set("duration", Value::Num(*d));
    }
    // When the site said it was published. The .txt has carried this since the
    // notes did; there is no reason for the capture to be the poorer record.
    if let Some(Value::Num(t)) = notes.get("timestamp") {
        meta.set("published", Value::str(stamp(*t)));
    }
    // The rest of what the notes gained, by the same argument: the channel
    // by its lasting URL, the language, and the uploader's own words for it.
    if let Some(c) = pick("channel_url").or_else(|| pick("uploader_url")) {
        meta.set("channel_url", Value::str(c));
    }
    for k in ["language", "location"] {
        if let Some(v) = pick(k) {
            meta.set(k, Value::str(v));
        }
    }
    for (k, from) in [("categories", "categories"), ("tags", "tags"), ("chapters", "chapters")] {
        if let Some(Value::Arr(a)) = notes.get(from).filter(|v| truthy(Some(v))) {
            let a = if k == "chapters" {
                a.iter()
                    .filter_map(|c| Some(Value::obj(vec![("start", Value::Num(c.get("start_time")?.as_num()?)), ("title", Value::str(py_str(c.get("title")?)))])))
                    .collect()
            } else {
                a.clone()
            };
            meta.set(k, Value::Arr(a));
        }
    }
    // The counts, under the moment they were read -- the same stamp the Stats
    // block carries, for the same reason. A count is a fact about a moment, and
    // the capture is the copy most likely to outlive the page it came from, so
    // it is the copy that most needs to say when the numbers were true.
    let counts: Vec<(&str, f64)> = [
        ("views", "view_count"),
        ("likes", "like_count"),
        ("downvotes", "dislike_count"),
        ("reposts", "repost_count"),
        ("replies", "comment_count"),
        ("followers", "channel_follower_count"),
    ]
    .iter()
    .filter_map(|(name, key)| match notes.get(key) {
        Some(Value::Num(n)) => Some((*name, *n)),
        _ => None,
    })
    .collect();
    if !counts.is_empty() {
        let mut stats = Value::obj(vec![("read", Value::str(stamp(now)))]);
        for (k, n) in counts {
            stats.set(k, Value::Num(n));
        }
        meta.set("stats", stats);
    }
    meta
}

/// A comment's field as text, "" when absent or empty.
fn cfield(c: &Value, k: &str) -> String {
    c.get(k).filter(|v| truthy(Some(v))).map(py_str).unwrap_or_default()
}

/// One comment's line: who, when, whether the uploader pinned it, and its likes.
///
/// The author's display name AND nothing else would be a poor record -- a name
/// can be changed afterwards -- so `author_id`, which cannot, is kept beside it
/// where the two differ.
fn chead(c: &Value) -> String {
    let name = cfield(c, "author");
    let name = name.trim_start_matches('@');
    let mut bits = vec![format!("@{}", if name.is_empty() { "unknown" } else { name })];
    let id = cfield(c, "author_id");
    if !id.is_empty() && id.trim_start_matches('@') != name {
        bits.push(format!("({id})"));
    }
    if let Some(Value::Num(t)) = c.get("timestamp") {
        bits.push(sys::strftime_local("%Y-%m-%d", *t));
    }
    if truthy(c.get("is_pinned")) {
        bits.push("* pinned".to_string());
    }
    if let Some(Value::Num(n)) = c.get("like_count") {
        if *n > 0.0 {
            bits.push(format!("{} likes", commas(*n)));
        }
    }
    bits.join("  ")
}

/// The Discussion section: each comment that answers nobody, then what answers it.
///
/// `parent` is `'root'` or the id of the comment being answered, so the thread is
/// built from the flat list yt-dlp hands over rather than guessed from its order.
/// **A comment whose parent is not in the list is drawn as a root**, because a cap
/// can cut a thread in half and the half that was kept is still worth reading.
pub fn discussion(list: &[Value], total: Option<f64>) -> String {
    let ids: Vec<String> = list.iter().map(|c| cfield(c, "id")).collect();
    let shown = list.len() as f64;
    let count = match total {
        Some(t) if t > shown => format!("{} of {}", commas(shown), commas(t)),
        _ => commas(shown),
    };
    let mut out = format!("Discussion  ({count})\n");
    for (i, c) in list.iter().enumerate() {
        let parent = cfield(c, "parent");
        if !(parent.is_empty() || parent == "root" || !ids.iter().any(|x| *x == parent)) {
            continue;
        }
        out.push('\n');
        out.push_str(&format!("  {}\n", chead(c)));
        for line in textwrap::wrap(&cfield(c, "text"), 74) {
            out.push_str(&format!("    {line}\n"));
        }
        let me = &ids[i];
        if me.is_empty() {
            continue;
        }
        for r in list.iter().filter(|r| cfield(r, "parent") == *me) {
            out.push_str(&format!("    |  {}\n", chead(r)));
            for line in textwrap::wrap(&cfield(r, "text"), 70) {
                out.push_str(&format!("    |    {line}\n"));
            }
        }
    }
    out
}

/// `1234567` as `1,234,567`: a comma every three digits from the right.
///
/// No locale and no dependency, and deliberately not abbreviated. An archive
/// keeps whole numbers: `12k` cannot be un-rounded later, and the width it
/// saves was never scarce.
pub fn commas(n: f64) -> String {
    let whole = format!("{}", n.trunc() as i64);
    let (sign, digits) = match whole.strip_prefix('-') {
        Some(d) => ("-", d),
        None => ("", whole.as_str()),
    };
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    format!("{sign}{out}")
}

/// `notes(meta, url, lang, video)`: the top of a transcript -- the title; Notes,
/// with the times in local time and its offset; the description; the heading.
pub fn notes(meta: &Value, url: &str, lang: Option<&str>, video: Option<&str>) -> String {
    notes_at(meta, url, lang, video, sys::now())
}

/// `lang` is the caption language when captions were sought, and `None` when
/// there were none to ask for -- an x.com post, or SUBS turned off. The rows
/// above Captions are true of every download and are written either way; a
/// Captions row and a Transcript heading on a file with neither would be a
/// statement about a step that never ran.
pub fn notes_at(meta: &Value, url: &str, lang: Option<&str>, video: Option<&str>, now: f64) -> String {
    let mut out = notes_head(meta, url, lang, video, now);
    let desc = meta.get("description").map(py_str).unwrap_or_default();
    let desc = py_strip(if truthy(meta.get("description")) { &desc } else { "" });
    if !desc.is_empty() {
        out.push_str(&format!("Description\n{desc}\n\n"));
    }
    if lang.is_some() {
        out.push_str("Transcript\n");
    }
    out
}

/// The notes up to the description: the title, Notes, Stats, Tags and
/// Chapters. The part the video file's comment carries too.
pub fn notes_head(meta: &Value, url: &str, lang: Option<&str>, video: Option<&str>, now: f64) -> String {
    let when = |t: f64| sys::strftime_local("%Y-%m-%d %H:%M:%S %z", t);
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
    let author = pick("uploader").or_else(|| pick("channel")).unwrap_or_else(|| "unknown".into());
    let mut rows = vec![("Title", title.clone()), ("Author", author.clone())];
    let site = pick("extractor_key").unwrap_or_default();
    // The @handle finds the account again after a change of display name.
    // On X it is the Where row already.
    if site != "Twitter" {
        if let Some(h) = pick("uploader_id").filter(|h| h.starts_with('@')) {
            rows.push(("Handle", h));
        }
    }
    // WHO POSTED IT IS NOT ALWAYS WHERE IT WAS POSTED, and on three of the four
    // sites ytq is pointed at they are different facts. A Reddit video has an
    // uploader (a redditor) and a subreddit, and the subreddit is the half of
    // the citation that says what the thing was part of -- `u/someone` posting
    // to `r/videos` and to `r/conspiracy` is not the same context, and it is
    // exactly what is lost when the post is deleted. X has a display name and
    // an @handle, and the handle is the one that finds the account again.
    // YouTube has an uploader and a channel that are usually the same string.
    //
    // So: one row, filled from whichever field the site in hand actually means
    // by it, and absent when the site gave nothing -- `extractor_key` is
    // yt-dlp's own name for which extractor answered, which is the only
    // honest way to know whose field conventions are in play.
    let where_ = match site.as_str() {
        "Reddit" => pick("channel_id").map(|sub| format!("r/{}", sub.trim_start_matches("r/"))),
        "Twitter" => pick("uploader_id").map(|h| format!("@{}", h.trim_start_matches('@'))),
        _ => pick("channel").filter(|c| *c != author),
    };
    if let Some(w) = where_ {
        rows.push(("Where", w));
    }
    if let Some(d) = pick("webpage_url_domain") {
        rows.push(("Site", d));
    }
    rows.push(("URL", pick("webpage_url").unwrap_or_else(|| url.to_string())));
    // The channel by its id, which outlives a rename; the handle's URL if that is all there is.
    if let Some(c) = pick("channel_url").or_else(|| pick("uploader_url")) {
        rows.push(("Channel", c));
    }
    rows.push(("Published", published));
    rows.push(("Downloaded", when(now)));
    // How long the thing runs. Every one of the four sites gives it, and it is
    // the one number that says whether a citation points at a ten-second clip
    // or at a three-hour stream.
    if let Some(Value::Num(d)) = meta.get("duration").filter(|v| truthy(Some(v))) {
        rows.push(("Duration", fmt_secs(*d)));
    }
    if let Some(l) = pick("language") {
        rows.push(("Language", l));
    }
    let categories: Vec<String> = meta.get("categories").and_then(|v| v.as_array()).map(|a| a.iter().map(py_str).filter(|c| !c.is_empty()).collect()).unwrap_or_default();
    if !categories.is_empty() {
        rows.push(("Category", categories.join(", ")));
    }
    if let Some(l) = pick("location") {
        rows.push(("Location", l));
    }
    // Only what is out of the ordinary: a stream, a Short, a video not public,
    // one behind an age check. The ordinary case says nothing.
    match pick("live_status").as_deref() {
        Some("was_live") | Some("post_live") => rows.push(("Live", "recorded from a live stream".into())),
        Some("is_live") => rows.push(("Live", "live while it was downloaded".into())),
        _ => {}
    }
    if let Some(k) = pick("media_type").filter(|k| k != "video" && k != "livestream") {
        rows.push(("Kind", k));
    }
    if let Some(a) = pick("availability").filter(|a| a != "public") {
        rows.push(("Visibility", a));
    }
    if let Some(Value::Num(a)) = meta.get("age_limit").filter(|v| truthy(Some(v))) {
        rows.push(("Age limit", format!("{}+", py_str(&Value::Num(*a)))));
    }
    rows.extend([
        // yt-dlp's license is what the site states: on YouTube, a Creative
        // Commons license's row on the watch page, and nothing otherwise.
        ("License", pick("license").unwrap_or_else(|| "not stated".into())),
        ("Video", video.filter(|v| !v.is_empty()).unwrap_or("none downloaded").to_string()),
    ]);
    if let Some(f) = format_line(meta) {
        rows.push(("Format", f));
    }
    if let Some(t) = pick("thumbnail") {
        rows.push(("Thumbnail", t));
    }
    if let Some(lang) = lang {
        // yt-dlp takes the uploader's captions over automatic ones in the same
        // language, so a language among 'subtitles' is the uploader's.
        let theirs = matches!(meta.get("subtitles"), Some(Value::Obj(m)) if m.iter().any(|(k, _)| k == lang));
        let source = if theirs { "written by the uploader" } else { "automatic: YouTube's speech recognition, not verbatim" };
        rows.push(("Captions", format!("{lang}, {source}")));
        // Every language the uploader wrote captions in, the one taken or not:
        // a translation they paid for says who the video was for.
        if let Some(Value::Obj(m)) = meta.get("subtitles") {
            let langs: Vec<&str> = m.iter().map(|(k, _)| k.as_str()).filter(|k| *k != "live_chat").collect();
            if langs.len() > 1 || (langs.len() == 1 && langs[0] != lang) {
                rows.push(("Subtitles", format!("{}, by the uploader", langs.join(", "))));
            }
        }
    }
    let mut out = format!("{title}\n\nNotes\n");
    for (k, v) in rows {
        out.push_str(&format!("  {:<11} {}\n", format!("{k}:"), v));
    }
    out.push('\n');
    // Counts are facts about a MOMENT, not about the recording. Every row above
    // is a property of the thing and reads the same tomorrow; views and likes
    // are different the second after they are read. So the block carries its own
    // reading time rather than leaning on Downloaded -- two rows that are
    // usually equal cost less than one row that is sometimes a lie. A stamped
    // count is a record; an unstamped one is a rumour.
    //
    // A row appears only where the site gave a number, so a site with no reposts
    // says nothing about reposts rather than claiming nought.
    let counts: Vec<(&str, String)> = [
        ("Views", "view_count"),
        ("Likes", "like_count"),
        // Reddit's `downs`. Only Reddit fills it, so only Reddit shows it.
        ("Downvotes", "dislike_count"),
        ("Reposts", "repost_count"),
        ("Replies", "comment_count"),
        // The channel's, not the video's, but a count all the same.
        ("Followers", "channel_follower_count"),
    ]
    .iter()
    .filter_map(|(label, key)| match meta.get(key) {
        Some(Value::Num(n)) => Some((*label, commas(*n))),
        _ => None,
    })
    .collect();
    let mut counts = counts;
    // Where the audience went back to: the site's replay graph, read at the
    // same moment as the counts and just as much a fact about it.
    let peaks = most_replayed(meta.get("heatmap").and_then(|v| v.as_array()).unwrap_or(&[]), meta.get("duration").and_then(|v| v.as_num()).unwrap_or(0.0));
    if !peaks.is_empty() {
        counts.push(("Replayed", format!("{}  (most first)", peaks.iter().map(|t| fmt_secs(*t)).collect::<Vec<_>>().join(", "))));
    }
    if !counts.is_empty() {
        out.push_str(&format!("Stats  (read {})\n", when(now)));
        for (k, v) in counts {
            out.push_str(&format!("  {:<11} {}\n", format!("{k}:"), v));
        }
        out.push('\n');
    }
    // What the uploader filed it under: words chosen to be found by, which
    // is what they are for here too.
    let tags: Vec<String> = meta.get("tags").and_then(|v| v.as_array()).map(|a| a.iter().map(py_str).filter(|t| !t.is_empty()).collect()).unwrap_or_default();
    if !tags.is_empty() {
        out.push_str("Tags\n");
        for line in textwrap::wrap(&tags.join(", "), 74) {
            out.push_str(&format!("  {line}\n"));
        }
        out.push('\n');
    }
    let chapters: Vec<(f64, String)> = meta
        .get("chapters")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|c| Some((c.get("start_time")?.as_num()?, c.get("title").map(py_str)?))).collect())
        .unwrap_or_default();
    if !chapters.is_empty() {
        out.push_str("Chapters\n");
        let times: Vec<String> = chapters.iter().map(|(t, _)| fmt_secs(*t)).collect();
        let w = times.iter().map(|t| t.len()).max().unwrap_or(0);
        for (t, (_, title)) in times.iter().zip(&chapters) {
            out.push_str(&format!("  {t:>w$}  {title}\n"));
        }
        out.push('\n');
    }
    out
}

/// The most replayed moments, most first: the three highest peaks of the
/// site's replay graph, each at least a twentieth of the video from the one
/// before. The opening is left out -- everybody watches the start.
pub fn most_replayed(heatmap: &[Value], duration: f64) -> Vec<f64> {
    let pts: Vec<(f64, f64, f64)> = heatmap
        .iter()
        .filter_map(|p| Some((p.get("start_time")?.as_num()?, p.get("end_time")?.as_num()?, p.get("value")?.as_num()?)))
        .collect();
    let mut peaks: Vec<(f64, f64)> = (0..pts.len())
        .filter(|&i| pts[i].0 > 0.0)
        .filter(|&i| (i == 0 || pts[i - 1].2 <= pts[i].2) && (i + 1 == pts.len() || pts[i + 1].2 < pts[i].2))
        .map(|i| ((pts[i].0 + pts[i].1) / 2.0, pts[i].2))
        .collect();
    peaks.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let gap = duration / 20.0;
    let mut out: Vec<f64> = Vec::new();
    for (t, _) in peaks {
        if out.len() == 3 {
            break;
        }
        if out.iter().all(|o| (o - t).abs() >= gap) {
            out.push(t);
        }
    }
    out
}

/// What was downloaded, in a line: `3840x2160, avc1 + mp4a, 25 fps`. Only
/// from the download's own run; see FORMAT_FIELDS.
fn format_line(meta: &Value) -> Option<String> {
    let pick = |k: &str| meta.get(k).filter(|v| truthy(Some(v))).map(py_str).filter(|s| s != "none");
    let mut bits = Vec::new();
    if let Some(r) = pick("resolution").filter(|r| r != "audio only") {
        bits.push(r);
    }
    let codecs: Vec<String> = ["vcodec", "acodec"].iter().filter_map(|k| pick(k)).map(|c| c.split('.').next().unwrap_or("").to_string()).collect();
    if !codecs.is_empty() {
        bits.push(codecs.join(" + "));
    }
    if let Some(Value::Num(f)) = meta.get("fps").filter(|v| truthy(Some(v))) {
        bits.push(format!("{} fps", py_str(&Value::Num(*f))));
    }
    Some(bits.join(", ")).filter(|b| !b.is_empty())
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
        let n = notes_at(&meta, "u", Some("en"), Some("jawed-Me_at_the_zoo_jNQXAC9IVRw.mp4"), 1114313512.0);
        assert!(n.starts_with("Me at the zoo\n\nNotes\n  Title:      Me at the zoo\n  Author:     jawed\n"), "{n}");
        assert!(n.contains("  License:    not stated\n  Video:      jawed-Me_at_the_zoo_jNQXAC9IVRw.mp4\n  Captions:   en, written by the uploader\n\nDescription\ndesc\n\nTranscript\n"), "{n}");
        assert!(cookie_problem("ERROR: Sign in to confirm you're not a bot") && !cookie_problem("HTTP Error 500"));
    }

    #[test]
    fn a_capture_says_what_the_text_beside_it_says() {
        let notes = json::parse(r#"{"webpage_url":"https://x.com/a/status/1","title":"a post",
            "uploader":"someone","timestamp":1750246101,
            "view_count":34776,"like_count":412,"repost_count":68,"comment_count":38}"#).unwrap();
        let m = capture_meta(&notes, "u", "/tmp/a.mp4", 1758142323.0);
        assert_eq!(m.get("source").unwrap().as_str(), Some("https://x.com/a/status/1"));
        assert_eq!(m.get("note").unwrap().as_str(), Some("a post by someone"));
        assert!(m.get("published").unwrap().as_str().unwrap().starts_with("2025-06-18"), "{:?}", m.get("published"));
        let st = m.get("stats").expect("stats");
        assert!(matches!(st.get("views"), Some(Value::Num(n)) if *n == 34776.0));
        assert!(matches!(st.get("replies"), Some(Value::Num(n)) if *n == 38.0));
        // Stamped, like the block in the .txt: the numbers are a fact about a moment.
        assert!(st.get("read").unwrap().as_str().unwrap().starts_with("2025-09-17"), "{:?}", st.get("read"));
    }

    #[test]
    fn a_capture_with_nothing_new_is_the_header_it_always_was() {
        // What the stand-in gives: no counts, no timestamp. The header must be
        // untouched, or every capture written before today reads differently.
        let notes = json::parse(r#"{"webpage_url":"https://www.youtube.com/watch?v=x","title":"T",
            "uploader":"U","license":null}"#).unwrap();
        let m = capture_meta(&notes, "u", "/tmp/a.mp4", 1758142323.0);
        assert!(m.get("stats").is_none(), "no counts, no stats");
        assert!(m.get("published").is_none(), "no timestamp, no published");
        assert!(m.get("license").is_none(), "null license stays absent");
        assert_eq!(m.get("note").unwrap().as_str(), Some("T by U"));
    }

    #[test]
    fn a_discussion_is_a_tree_not_a_list() {
        let list = match json::parse(r#"[
            {"id":"c1","parent":"root","author":"@SanDiegoZoo","author_id":"@SanDiegoZoo",
             "timestamp":1682300000,"like_count":12004,"is_pinned":true,"text":"The elephants say hi."},
            {"id":"c2","parent":"c1","author":"@tacticals","author_id":"@tacticals",
             "timestamp":1682900000,"like_count":340,"text":"An answer."},
            {"id":"c3","parent":"root","author":"@frandovian","author_id":"@frandovian",
             "timestamp":1683000000,"like_count":0,"text":"A second root."},
            {"id":"c4","parent":"gone","author":"@orphan","author_id":"@orphan","text":"Half a thread."}
        ]"#).unwrap() { Value::Arr(v) => v, _ => panic!("not an array") };
        let d = discussion(&list, Some(99.0));
        // The cap is stated against the total, so the reader knows what is missing.
        assert!(d.starts_with("Discussion  (4 of 99)\n"), "{d}");
        // A reply is drawn under the comment it answers, not in the order it arrived.
        let root = d.find("@SanDiegoZoo").unwrap();
        let reply = d.find("@tacticals").unwrap();
        let second = d.find("@frandovian").unwrap();
        assert!(root < reply && reply < second, "{d}");
        assert!(d.contains("    |  @tacticals"), "{d}");
        // The uploader's mark and the likes, whole.
        assert!(d.contains("* pinned  12,004 likes"), "{d}");
        // No likes is no mention of likes, rather than "0 likes". Checked on the
        // whole line, because "340 likes" contains "0 likes" as a substring.
        assert!(d.contains("\n  @frandovian  2023-05-01\n"), "{d}");
        // A comment whose parent was cut by the cap is still shown, as a root.
        assert!(d.contains("\n  @orphan"), "{d}");
        // Without a total there is nothing to compare the count against.
        assert!(discussion(&list, None).starts_with("Discussion  (4)\n"));
    }

    #[test]
    fn counts_are_grouped_in_threes_and_never_rounded() {
        assert_eq!(commas(0.0), "0");
        assert_eq!(commas(38.0), "38");
        assert_eq!(commas(999.0), "999");
        assert_eq!(commas(1000.0), "1,000");
        assert_eq!(commas(34776.0), "34,776");
        assert_eq!(commas(378402118.0), "378,402,118");
        assert_eq!(commas(-1234.0), "-1,234");
    }

    /// The x.com post of the phase-5 measurements, with no captions sought.
    #[test]
    fn stats_are_stamped_and_only_what_the_site_gave() {
        let meta = json::parse(r#"{"title": "a post", "uploader": "someone",
            "webpage_url": "https://x.com/someone/status/1", "timestamp": 1750246101,
            "view_count": 34776, "like_count": 412, "repost_count": 68, "comment_count": 38}"#).unwrap();
        let n = notes_at(&meta, "u", None, Some("someone-a_post_1.mp4"), 1758142323.0);
        // No captions were sought, so neither row about them is written.
        assert!(!n.contains("Captions:"), "{n}");
        assert!(!n.contains("Transcript"), "{n}");
        // Stamped with its own time, and whole.
        assert!(n.contains("Stats  (read "), "{n}");
        assert!(n.contains("\n  Views:      34,776\n  Likes:      412\n  Reposts:    68\n  Replies:    38\n"), "{n}");
    }

    /// The four sites ytq is pointed at, and the one row that means a
    /// different field on each of them. Fixtures rather than downloads,
    /// because what is being held here is the rule, not the network.
    #[test]
    fn where_it_was_posted_is_read_from_whichever_field_the_site_means_by_it() {
        // Reddit: the uploader is a redditor and `channel_id` is the
        // subreddit, which is the half of the citation a deleted post takes
        // with it.
        let reddit = json::parse(r#"{"title": "a clip", "uploader": "u_someone", "channel_id": "aww",
            "extractor_key": "Reddit", "webpage_url_domain": "reddit.com", "duration": 74,
            "like_count": 8102, "dislike_count": 41, "comment_count": 219}"#).unwrap();
        let n = notes_at(&reddit, "u", None, Some("v.mp4"), 1758142323.0);
        assert!(n.contains("\n  Author:     u_someone\n  Where:      r/aww\n  Site:       reddit.com\n"), "{n}");
        assert!(n.contains("\n  Duration:   1:14\n"), "{n}");
        // Reddit is the one site that gives downvotes, so it is the one that shows them.
        assert!(n.contains("\n  Likes:      8,102\n  Downvotes:  41\n  Replies:    219\n"), "{n}");
        // And the capture's header carries the same two facts as the .txt.
        let m = capture_meta(&reddit, "u", "/tmp/v.mp4", 1758142323.0);
        assert_eq!(m.get("where").unwrap().as_str(), Some("r/aww"));
        assert!(matches!(m.get("duration"), Some(Value::Num(d)) if *d == 74.0));
        assert!(matches!(m.get("stats").unwrap().get("downvotes"), Some(Value::Num(n)) if *n == 41.0));
        // A subreddit yt-dlp already spelled `r/aww` is not spelled `r/r/aww`.
        let prefixed = json::parse(r#"{"title": "c", "uploader": "u", "channel_id": "r/aww", "extractor_key": "Reddit"}"#).unwrap();
        assert!(notes_at(&prefixed, "u", None, None, 0.0).contains("\n  Where:      r/aww\n"));

        // X: the display name is the Author and the @handle is what finds the
        // account again.
        let x = json::parse(r#"{"title": "a post", "uploader": "Some One", "uploader_id": "someone",
            "extractor_key": "Twitter", "webpage_url_domain": "x.com"}"#).unwrap();
        assert!(notes_at(&x, "u", None, None, 0.0).contains("\n  Author:     Some One\n  Where:      @someone\n  Site:       x.com\n"));

        // YouTube: a channel worth naming only when it is not the uploader's
        // own name repeated, which is what it usually is.
        let yt = json::parse(r#"{"title": "v", "uploader": "Rick Astley", "channel": "RickAstleyVEVO",
            "channel_id": "UCuAXFkgsw1L7xaCfnd5JJOw", "extractor_key": "Youtube", "duration": 3812}"#).unwrap();
        let n = notes_at(&yt, "u", None, None, 0.0);
        assert!(n.contains("\n  Where:      RickAstleyVEVO\n"), "{n}");
        // The opaque UC... id is never what the row shows.
        assert!(!n.contains("UCuAXF"), "{n}");
        assert!(n.contains("\n  Duration:   1:03:32\n"), "{n}");
        let same = json::parse(r#"{"title": "v", "uploader": "jawed", "channel": "jawed", "extractor_key": "Youtube"}"#).unwrap();
        assert!(!notes_at(&same, "u", None, None, 0.0).contains("Where:"), "the same name twice is not two facts");
    }

    /// Every row added since the frozen specification is absent when the site
    /// said nothing, so a capture written before today reads the same today.
    #[test]
    fn a_download_from_before_these_rows_reads_as_it_always_did() {
        let old = json::parse(r#"{"title": "Me at the zoo", "uploader": "jawed",
            "webpage_url": "https://www.youtube.com/watch?v=jNQXAC9IVRw", "timestamp": 1114313512}"#).unwrap();
        let n = notes_at(&old, "u", None, Some("v.mp4"), 1114313512.0);
        for row in ["Where:", "Site:", "Duration:"] {
            assert!(!n.contains(row), "{row} with nothing to put in it: {n}");
        }
        let m = capture_meta(&old, "u", "/tmp/v.mp4", 1758142323.0);
        assert!(m.get("where").is_none() && m.get("duration").is_none(), "{m:?}");
    }

    #[test]
    fn a_site_that_gave_no_number_says_nothing_about_it() {
        // YouTube has no reposts: that row is absent, not nought.
        let meta = json::parse(r#"{"title": "v", "uploader": "u", "view_count": 1234567, "comment_count": 0}"#).unwrap();
        let n = notes_at(&meta, "u", None, None, 1758142323.0);
        assert!(n.contains("\n  Views:      1,234,567\n"), "{n}");
        assert!(n.contains("\n  Replies:    0\n"), "{n}");
        assert!(!n.contains("Reposts"), "{n}");
        assert!(!n.contains("Likes"), "{n}");
        // And a download whose site gave nothing at all has no block at all.
        let bare = json::parse(r#"{"title": "v", "uploader": "u"}"#).unwrap();
        assert!(!notes_at(&bare, "u", None, None, 1758142323.0).contains("Stats"), "no counts, no block");
    }

    #[test]
    fn the_notes_carry_every_field_the_site_gave_and_nothing_it_did_not() {
        let meta = json::parse(r#"{"title": "Me at the zoo", "uploader": "jawed", "uploader_id": "@jawed", "extractor_key": "Youtube",
            "channel_url": "https://www.youtube.com/channel/UC4QobU6STFB0P71PMvOGN5A", "language": "en", "categories": ["Film & Animation"],
            "location": "SAN DIEGO ZOO", "live_status": "not_live", "availability": "public", "age_limit": 0, "media_type": "video",
            "resolution": "320x240", "vcodec": "avc1.42001E", "acodec": "mp4a.40.2", "fps": 15, "thumbnail": "https://i.ytimg.com/t.jpg",
            "channel_follower_count": 5140000, "duration": 19, "tags": ["me at the zoo", "jawed karim"],
            "chapters": [{"start_time": 0, "title": "Intro", "end_time": 5}, {"start_time": 5, "title": "The cool thing", "end_time": 19}],
            "subtitles": {"en": [], "de": []}}"#).unwrap();
        let n = notes_at(&meta, "u", Some("en"), Some("v.mp4"), 1114313512.0);
        for row in ["  Handle:     @jawed\n", "  Channel:    https://www.youtube.com/channel/UC4QobU6STFB0P71PMvOGN5A\n", "  Language:   en\n",
            "  Category:   Film & Animation\n", "  Location:   SAN DIEGO ZOO\n", "  Format:     320x240, avc1 + mp4a, 15 fps\n",
            "  Thumbnail:  https://i.ytimg.com/t.jpg\n", "  Subtitles:  en, de, by the uploader\n", "  Followers:  5,140,000\n",
            "Tags\n  me at the zoo, jawed karim\n\n", "Chapters\n  0:00  Intro\n  0:05  The cool thing\n\n"] {
            assert!(n.contains(row), "{row:?} in\n{n}");
        }
        // The ordinary case says nothing: public, not live, no age check, a plain video.
        for absent in ["Live:", "Visibility:", "Age limit:", "Kind:", "Replayed:"] {
            assert!(!n.contains(absent), "{absent} in\n{n}");
        }
        let odd = json::parse(r#"{"title": "t", "live_status": "was_live", "availability": "unlisted", "age_limit": 18}"#).unwrap();
        let n = notes_at(&odd, "u", None, None, 0.0);
        assert!(n.contains("  Live:       recorded from a live stream\n  Visibility: unlisted\n  Age limit:  18+\n"), "{n}");
    }

    #[test]
    fn the_most_replayed_moments_are_peaks_not_neighbours() {
        let pt = |s: f64, v: f64| Value::obj(vec![("start_time", Value::Num(s)), ("end_time", Value::Num(s + 10.0)), ("value", Value::Num(v))]);
        // The opening is highest and is left out; 40-50 and 50-60 are one hill,
        // and 30-40 is its slope, not a peak.
        let map = vec![pt(0.0, 1.0), pt(10.0, 0.2), pt(20.0, 0.1), pt(30.0, 0.3), pt(40.0, 0.9), pt(50.0, 0.8), pt(60.0, 0.1), pt(70.0, 0.5), pt(80.0, 0.2), pt(90.0, 0.25)];
        assert_eq!(most_replayed(&map, 100.0), vec![45.0, 75.0, 95.0]);
        assert!(most_replayed(&[], 100.0).is_empty());
    }

    #[test]
    fn the_video_tags_escape_what_ffmpeg_would_read_as_syntax() {
        let meta = json::parse(r#"{"title": "a=b; c#d\\e", "uploader": "U", "upload_date": "20050424", "categories": ["Film"],
            "tags": ["x", "y"], "description": "line one\nline two", "duration": 19,
            "chapters": [{"start_time": 0, "title": "Intro", "end_time": 5}, {"start_time": 5.5, "title": "Rest"}]}"#).unwrap();
        let f = ffmetadata(&meta, "u", Some(("en", "said\nthis")), "v.mp4", 0.0);
        assert!(f.starts_with(";FFMETADATA1\ntitle=a\\=b\\; c\\#d\\\\e\nartist=U\ndate=20050424\ngenre=Film\n"), "{f}");
        assert!(f.contains("description=line one\\\nline two\n") && f.contains("keywords=x, y\n") && f.contains("lyrics=said\\\nthis\n"), "{f}");
        assert!(f.contains("comment=a\\=b\\; c\\#d\\\\e\\\n\\\nNotes\\\n"), "{f}");
        assert!(f.contains("\n[CHAPTER]\nTIMEBASE=1/1000\nSTART=0\nEND=5000\ntitle=Intro\n\n[CHAPTER]\nTIMEBASE=1/1000\nSTART=5500\nEND=19000\ntitle=Rest\n"), "{f}");
        // A carriage return ends a line for ffmpeg, and a NUL does: neither
        // may reach the file as itself.
        assert_eq!(ffescape("a\rb\0c\r\nd"), "a\\\rbc\\\r\\\nd");
        let hostile = json::parse(r#"{"title":"t","chapters":[{"start_time":0,"end_time":5,"title":"one\r[CHAPTER]\rSTART=6000"}]}"#).unwrap();
        let f = ffmetadata(&hostile, "u", Some(("en", "said\0 more\rlyrics=x")), "v.mp4", 0.0);
        // One chapter was given, so one line of the file opens one.
        assert_eq!(f.split('\n').filter(|l| *l == "[CHAPTER]").count(), 1, "{f}");
        assert!(!f.contains('\0') && !f.replace("\\\r", "").contains('\r'), "{f}");
        // No transcript, no lyrics; no license, no copyright.
        let f = ffmetadata(&meta, "u", None, "v.mp4", 0.0);
        assert!(!f.contains("lyrics=") && !f.contains("copyright="), "{f}");
    }

    #[test]
    fn the_format_is_the_downloads_and_the_counts_the_later_reads() {
        let download = json::parse(r#"{"title": "T", "view_count": 1, "resolution": "1920x1080", "vcodec": "avc1"}"#).unwrap();
        let later = json::parse(r#"{"title": "T", "view_count": 5, "tags": ["t"], "license": null}"#).unwrap();
        let m = merged(&download, &later);
        assert!(matches!(m.get("view_count"), Some(Value::Num(n)) if *n == 5.0));
        assert_eq!(m.get("resolution").and_then(|v| v.as_str()), Some("1920x1080"));
        assert!(m.get("tags").is_some() && m.get("license").is_none());
    }

    #[test]
    fn a_capture_keeps_the_new_fields_too() {
        let notes = json::parse(r#"{"webpage_url":"https://www.youtube.com/watch?v=x","title":"T","uploader":"U",
            "channel_url":"https://www.youtube.com/channel/UCx","language":"en","tags":["a","b"],"categories":["Music"],
            "chapters":[{"start_time":0,"title":"Intro","end_time":5}],"channel_follower_count":10}"#).unwrap();
        let m = capture_meta(&notes, "u", "/tmp/a.mp4", 1758142323.0);
        assert_eq!(m.get("channel_url").unwrap().as_str(), Some("https://www.youtube.com/channel/UCx"));
        assert_eq!(m.get("tags").unwrap().as_array().unwrap().len(), 2);
        assert_eq!(m.get("chapters").unwrap().to_json(), r#"[{"start": 0, "title": "Intro"}]"#);
        assert!(matches!(m.get("stats").unwrap().get("followers"), Some(Value::Num(n)) if *n == 10.0));
    }
}
