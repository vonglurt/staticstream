// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Paul Richeson

//! `sstr ytq …`: the Python ytq's commands, as far as step 2a of
//! docs/phase-2.md goes -- add, clip, list, status, clear, cookies and the
//! help -- plus two probes the crosscheck uses to hold this to the Python ytq.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

use staticstream::json::{self, Value};

use crate::live::{live_view, now};
use crate::log::{log, say, Mode};
use crate::queue::{self, status, text, title_or_url, RunLock};
use crate::runner::{brave_ready, run_timeout, Ran, Runner, NAME};
use crate::urls::{as_url, first_id, py_strip, short, youtube_urls};
use crate::{settings, Paths, HISTORY, ORDER, PENDING};

/// The Python ytq's own header, which its help prints.
const HEADER: &str = "SPDX-License-Identifier: MIT
Copyright (c) 2026 Paul Richeson -- part of Copal Linux.

ytq -- a yt-dlp download queue that watches the clipboard.

  ytq                the queue window. The URL on the clipboard right now is
                     queued at once; while the window has focus, every URL
                     copied afterwards is checked and queued too.
  ytq clip           queue the URL on the clipboard. This is Super+Shift+Y.
                     A clipboard holding more than a URL -- a bookmarks
                     export, a page of notes -- queues every YouTube video
                     in it, each once. A YouTube link may lack its https://
                     (youtube.com/shorts/ID), here and everywhere else.
  ytq add URL...     the same, for URLs typed in a shell
  ytq run            download what is queued, in this terminal
  ytq status         what is downloading, the step it is on, what is left
  ytq cookies        retry what was waiting on a Brave sign-in
  ytq transcript URL...  just the captions, as text, for YouTube videos
  ytq list           every entry: queued, done, waiting, failed
  ytq clear          forget finished, rejected and failed entries (h twice in
                     the window); the files and the log stay

FILENAMES are the first word of the uploader's name, the title and the video
id, in the URL-safe base64 alphabet (A-Z a-z 0-9 - _) and nothing else, then
the extension: 'Café Tour: Part 2/3 [4K]' by Rick Astley is
Rick-Cafe_Tour_Part_2_3_4K_dQw4w9WgXcQ.mp4. A YouTube video also gets
Rick-Cafe_Tour_Part_2_3_4K_dQw4w9WgXcQ.txt beside it: Notes (full title,
author, URL, published and downloaded times, any license the site states,
the video's filename), the description, then the captions as plain text,
fetched once the video is done. The .mp4 carries the same notes in its
metadata. See NAME_OPTS, META_OPTS and transcript().

CITATIONS. The notes are the parts of a reference someone else can check --
author, full title, when it was published, the watch URL, when you fetched
it -- taken at download time, because a title or description can be edited
and a video made private or deleted after you saw it. The id, in the name
and the URL, is what finds it again. Automatic captions are speech
recognition, not a verbatim record, and the Captions line says whether a
transcript came from them or from the uploader: check a quote against the
audio. The Author line is the uploader, not necessarily the speaker, and
Published is the upload, not the event recorded. The merge and the tagging
copy the streams without re-encoding, and the transcript is the lossy part
(no timings, a repeated caption line dropped). A faithful copy with good
notes is still a copy: the guide's \"What the copy is\" says what that does
and does not settle, and the License line shows the Creative Commons cases.
The yt-dlp guide ('guide') covers dates, archiving, quoting and
accessibility.

AUTOSTART is off until you ask for it:  touch ~/.config/ytq/auto
With that file there, clip, add and cookies also start downloading in the
background whenever nothing is downloading already. Without it they only
queue, and 'ytq run' or the window does the downloading. --run starts a
runner this once whatever the file says; --no-run never does.

HOW IT WORKS. One file, ~/.local/share/ytq/queue.json, is the queue, and any
number of ytq processes use it at once. Every change takes queue.lock, reads
the file afresh, changes what it came to change, writes it back and lets go.
(Each process used to keep its own copy, and a 'ytq add' made while 'ytq
run' was downloading vanished at the runner's next save.)

Exactly one process downloads: whichever holds run.lock, with its pid
written inside. That is 'ytq run', or the window, or the background runner
that autostart starts when nobody holds the lock. The runner checks
each new URL (can yt-dlp get it as an MP4, and at what height?) and
downloads one at a time, best video plus best audio merged by ffmpeg. A
background runner leaves when the queue is empty, and the next 'ytq clip'
starts another. It lets go of run.lock while holding queue.lock, and 'ytq
clip' adds its URL and looks for a runner under that same lock -- so a URL
cannot land in the gap between \"nothing left\" and \"gone\" and sit there.

WHEN IT FAILS. A failed download goes to the back of the queue for one more
try. If yt-dlp's complaint is about a login, an age gate, a \"confirm you are
not a bot\" page or cookies, the retry is the cookie retry, and there is only
ever one of those per entry: Brave is opened on the URL so you can sign in
or pass the check, and the entry waits, marked 'cookies'. 'ytq cookies' (or
'c' in the window) then runs it again through yt-brave, which finds Brave's
profile (Flatpak or native) and hands yt-dlp its cookies. A second failure
after that is final. A runner with a terminal asks for Enter instead, once
the rest of the queue is done; one in the background sends a notification.

WHAT IT IS DOING. The process that downloads keeps a live record in the
entry -- the step (looking it up, downloading part 1 of 2, merging, the
transcript), bytes, speed, ETA, yt-dlp's last line -- and the window shows
it above the list for whichever process is downloading; 'ytq status' too.
Quitting the window stops its download, merge and all.

THE LOG, ~/.local/share/ytq/ytq.log, is for finding out what happened. Every
line carries the pid of the ytq that wrote it, and a download's lines its
video id. It has every line yt-dlp printed, progress every 30 seconds, each
part's size and speed, how long each step took, and why a download stopped;
a runner begins by naming its ytq, its yt-dlp and its settings. Past 4 MiB
it moves to ytq.log.1 and starts again.

WHY \"WHILE IT HAS FOCUS\". The clipboard is a shared thing, and a queue that
grabbed every link you copied for any reason would be a nuisance. So the
window's watcher only acts while the window is the focused one: copy a link,
click here, and it is queued; copy a link for a note and nothing happens.
Focus is asked of the compositor (hyprctl on Hyprland, xdotool on X11);
where neither answers, the watcher stays on. 'ytq clip' is the other way in:
it takes the clipboard once, when you ask.

Settings, if you want them, in ~/.config/ytq/config as KEY=VALUE:
  DIR       where files go            (default ~/Downloads/SharedVM if the
                                       share is mounted, else XDG_VIDEOS_DIR
                                       or ~/Videos)
  FORMAT    yt-dlp -f selector        (default: best MP4 video + M4A audio)
  PROFILE   Brave profile, passed to yt-brave --profile   (default: Default)
  KEYRING   passed to yt-brave --keyring, e.g. basictext, for a desktop with no keyring
  POLL      clipboard poll, seconds   (default 1)
  SUBS      caption languages for the transcript, a yt-dlp --sub-langs list
            (default en,en-orig,en-US,en-GB: exact names, since en.* also
            takes YouTube's translations into English); SUBS= turns it off
and next to it the empty file ~/.config/ytq/auto, which switches autostart on.";

struct Ctx {
    paths: Paths,
    home: PathBuf,
    s: BTreeMap<String, String>,
    mode: Mode,
    run: RunLock,
}

impl Ctx {
    fn tilde(&self, p: &str) -> String {
        p.replace(&*self.home.to_string_lossy(), "~")
    }

    fn say(&self, msg: &str, urgent: bool) {
        say(&self.paths, self.mode, msg, urgent);
    }

    fn setting(&self, k: &str) -> &str {
        self.s.get(k).map(String::as_str).unwrap_or("")
    }
}

pub(crate) fn read_clipboard() -> String {
    for cmd in [vec!["wl-paste", "-n", "--type", "text/plain"], vec!["xclip", "-o", "-selection", "clipboard"]] {
        if let Ran::Done(0, out, _) = run_timeout(Command::new(cmd[0]).args(&cmd[1..]), 3) {
            return py_strip(&out).to_string();
        }
    }
    String::new()
}

/// `clipboard_urls()`: the YouTube videos in the clipboard, however much text
/// they are buried in; failing those, the clipboard itself if it is one URL.
fn clipboard_urls() -> Vec<String> {
    let text = read_clipboard();
    let urls = youtube_urls(&text);
    if !urls.is_empty() {
        return urls;
    }
    as_url(&text).into_iter().collect()
}

/// `report(urls, added, runner)`: one message, so a keypress makes one notification.
fn report(ctx: &Ctx, urls: &[String], added: &[String], runner: Option<(String, bool)>) {
    let mut lines: Vec<String> = if urls.len() > 5 {
        vec![format!("queued {} of {} links; the other {} were already in the queue", added.len(), urls.len(), urls.len() - added.len())]
    } else {
        urls.iter().map(|u| format!("{}{}", if added.contains(u) { "queued: " } else { "already queued: " }, u)).collect()
    };
    if let Some((pid, started)) = runner {
        lines.push(format!("downloading: {} pid {}", if started { "started," } else { "already, in" }, pid));
    } else if queue::snapshot(&ctx.paths).iter().any(|i| PENDING.contains(&status(i))) {
        lines.push(format!(
            "nothing is downloading -- 'ytq run' starts it, or touch {} to have it start by itself",
            ctx.tilde(&ctx.paths.autostart.to_string_lossy())
        ));
    }
    if !lines.is_empty() {
        ctx.say(&lines.join("\n"), false);
    }
}

fn cmd_status(ctx: &Ctx) {
    let items = queue::snapshot(&ctx.paths);
    let pid = ctx.run.holder(&ctx.paths);
    println!("runner: {}", match &pid {
        Some(p) => format!("pid {p}"),
        None => "none -- 'ytq run' or the next 'ytq clip' starts one".into(),
    });
    let t = now();
    for i in items.iter().filter(|i| status(i) == "downloading") {
        println!("  downloading  {}   {}", title_or_url(i), text(i, "quality"));
        for row in live_view(i, 110, t) {
            println!("    {row}");
        }
    }
    for i in items.iter().filter(|i| status(i) == "cookies") {
        println!("  sign-in    {}  -- then 'ytq cookies'", title_or_url(i));
    }
    let counts: Vec<String> = ORDER
        .iter()
        .map(|st| (st, items.iter().filter(|i| status(i) == *st).count()))
        .filter(|(_, n)| *n > 0)
        .map(|(st, n)| format!("{n} {st}"))
        .collect();
    println!("  {}", if counts.is_empty() { "the queue is empty".into() } else { counts.join(", ") });
}

fn cmd_list(ctx: &Ctx) {
    let items = queue::snapshot(&ctx.paths);
    for it in &items {
        println!("{:<13} {:<10} {}", status(it), text(it, "quality"), title_or_url(it));
        let err = text(it, "error");
        if !err.is_empty() {
            println!("              {}", err.chars().take(100).collect::<String>());
        }
    }
    if items.is_empty() {
        println!("the queue is empty");
    }
}

fn cmd_cookies(ctx: &Ctx, run: bool) -> i32 {
    if !queue::snapshot(&ctx.paths).iter().any(|i| status(i) == "cookies") {
        println!("nothing is waiting on a Brave sign-in");
        return 0;
    }
    if let Some(why) = brave_ready(&ctx.s) {
        ctx.say(&format!("{why} -- start Brave once, or set PROFILE in ~/.config/ytq/config"), true);
        return 1;
    }
    let result = queue::edit(&ctx.paths, |items| {
        let mut n = 0;
        for i in items.iter_mut() {
            if status(i) == "cookies" {
                i.set("status", Value::str("retry-cookies"));
                n += 1;
            }
        }
        let runner = if run { Some(queue::start_runner(&ctx.paths, &ctx.run)) } else { ctx.run.holder(&ctx.paths).map(|p| (p, false)) };
        (n, runner)
    });
    match result {
        Ok((n, runner)) => {
            log(&ctx.paths, &format!("retrying {n} with Brave's cookies"));
            report(ctx, &[], &[], runner);
            0
        }
        Err(e) => {
            eprintln!("ytq: {e}");
            1
        }
    }
}

fn help(ctx: &Ctx, cmd: &str) -> i32 {
    println!("{HEADER}");
    println!("usage: ytq | ytq clip | ytq add URL... | ytq run | ytq status | ytq cookies | ytq transcript URL... | ytq list | ytq clear");
    println!();
    let conf = &ctx.paths.ytq_config;
    println!("config: {} ({})", ctx.tilde(&conf.to_string_lossy()), if conf.is_file() { "found" } else { "not there -- the defaults apply" });
    println!("  KEY=VALUE lines, # for comments: DIR, FORMAT, PROFILE, KEYRING, POLL, SUBS");
    let keyring = ctx.setting("KEYRING");
    println!(
        "  in effect: DIR={}  PROFILE={}{}",
        ctx.tilde(ctx.setting("DIR")),
        ctx.setting("PROFILE"),
        if keyring.is_empty() { String::new() } else { format!("  KEYRING={keyring}") }
    );
    let auto = &ctx.paths.autostart;
    println!(
        "auto:   {} ({})",
        ctx.tilde(&auto.to_string_lossy()),
        if auto.exists() { "found -- clip, add and cookies start downloading by themselves" } else { "not there -- clip and add only queue; touch it to have them start downloading" }
    );
    println!("  --run or --no-run on clip, add or cookies overrides it for that one command");
    let media = &ctx.paths.media_conf;
    println!("media:  {} ({})", ctx.tilde(&media.to_string_lossy()), if media.is_file() { "found, read before the config" } else { "not there" });
    println!(
        "archive: OUTPUT={}  ARCHIVE_DIR={}  SSTR_KEY={}",
        crate::runner::output_of(&ctx.s),
        ctx.tilde(&crate::runner::archive_dir_of(&ctx.s)),
        crate::runner::sstr_key_of(&ctx.s, &ctx.home).map(|k| ctx.tilde(&k.to_string_lossy())).unwrap_or_else(|| "(none: captures unsigned)".into())
    );
    println!("  OUTPUT=sstr keeps a Static Stream capture and removes the MP4 once the capture verifies; both keeps both; mp4 keeps the MP4 alone");
    if matches!(cmd, "-h" | "--help" | "help") {
        0
    } else {
        2
    }
}

/// `sstr ytq probe urls FILE` and `sstr ytq probe settings`: what the Python
/// ytq's `youtube_urls`, `as_url`, `short` and `settings()` give, as JSON, for
/// the crosscheck to compare line for line.
fn probe(ctx: &Ctx, args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        Some("urls") => {
            let Some(path) = args.get(1) else { return 2 };
            let Ok(Value::Arr(texts)) = std::fs::read(path).map_err(|e| e.to_string()).and_then(|b| json::parse_bytes(&b).map_err(|e| e.0)) else {
                eprintln!("probe urls: FILE must be a JSON list of strings");
                return 2;
            };
            for t in texts {
                let t = t.as_str().unwrap_or("");
                let row = Value::obj(vec![
                    ("youtube_urls", Value::Arr(youtube_urls(t).into_iter().map(Value::Str).collect())),
                    ("as_url", as_url(t).map(Value::Str).unwrap_or(Value::Null)),
                    ("short", Value::str(short(t))),
                ]);
                println!("{}", row.to_python_json(None));
            }
            0
        }
        Some("settings") => {
            let obj = Value::Obj(ctx.s.iter().map(|(k, v)| (k.clone(), Value::str(v))).collect());
            println!("{}", obj.to_python_json(None));
            0
        }
        Some("vtt") => {
            let Some(path) = args.get(1) else { return 2 };
            match crate::textwrap::vtt_text(std::path::Path::new(path)) {
                Ok(t) => {
                    println!("{}", Value::str(t).to_python_json(None));
                    0
                }
                Err(e) => {
                    eprintln!("probe vtt: {e}");
                    1
                }
            }
        }
        Some("fill") => {
            let Some(path) = args.get(1) else { return 2 };
            let Ok(Value::Arr(texts)) = std::fs::read(path).map_err(|e| e.to_string()).and_then(|b| json::parse_bytes(&b).map_err(|e| e.0)) else {
                eprintln!("probe fill: FILE must be a JSON list of strings");
                return 2;
            };
            for t in texts {
                println!("{}", Value::str(crate::textwrap::fill(t.as_str().unwrap_or(""), 78)).to_python_json(None));
            }
            0
        }
        _ => 2,
    }
}

/// `sstr ytq ARGS`: the Python ytq's `main(argv)`, for the commands step 2a has.
pub fn main(argv: &[String]) -> i32 {
    let Some((paths, home, s)) = settings::from_env() else {
        eprintln!("ytq: HOME is not set");
        return 2;
    };
    let mut ctx = Ctx { paths, home, s, mode: Mode::for_stdout(), run: RunLock::default() };
    let flags: Vec<&str> = argv.iter().map(String::as_str).filter(|a| matches!(*a, "--run" | "--no-run" | "--quiet")).collect();
    let args: Vec<String> = argv.iter().filter(|a| !flags.contains(&a.as_str())).cloned().collect();
    let cmd = args.first().map(String::as_str).unwrap_or("tui");
    let run = !flags.contains(&"--no-run") && (flags.contains(&"--run") || ctx.paths.autostart.exists());
    match cmd {
        "add" if args.len() > 1 => {
            let mut urls = Vec::new();
            for u in &args[1..] {
                match as_url(u) {
                    Some(url) => urls.push(url),
                    None => ctx.say(&format!("not a URL: {u}"), false),
                }
            }
            if !urls.is_empty() {
                match queue::enqueue(&ctx.paths, &ctx.run, &urls, run) {
                    Ok((added, runner)) => report(&ctx, &urls, &added, runner),
                    Err(e) => {
                        eprintln!("ytq: {e}");
                        return 1;
                    }
                }
            }
            0
        }
        "clip" => {
            let urls = clipboard_urls();
            if urls.is_empty() {
                ctx.say("the clipboard holds no URL and no YouTube link", false);
                return 1;
            }
            match queue::enqueue(&ctx.paths, &ctx.run, &urls, run) {
                Ok((added, runner)) => {
                    report(&ctx, &urls, &added, runner);
                    0
                }
                Err(e) => {
                    eprintln!("ytq: {e}");
                    1
                }
            }
        }
        "status" => {
            cmd_status(&ctx);
            0
        }
        "list" => {
            cmd_list(&ctx);
            0
        }
        "clear" => match queue::edit(&ctx.paths, |items| {
            let n = items.len();
            items.retain(|i| !HISTORY.contains(&status(i)));
            n - items.len()
        }) {
            Ok(n) => {
                println!("removed {n}");
                0
            }
            Err(e) => {
                eprintln!("ytq: {e}");
                1
            }
        },
        "cookies" => cmd_cookies(&ctx, run),
        "probe" => {
            ctx.mode = Mode::Quiet;
            probe(&ctx, &args[1..])
        }
        "run" => {
            let runner = Arc::new(Runner::new(ctx.paths.clone(), ctx.home.clone(), ctx.s.clone(), ctx.mode));
            runner.cmd_run(flags.contains(&"--quiet"), &argv.join(" "))
        }
        "transcript" if args.len() > 1 => {
            let runner = Runner::new(ctx.paths.clone(), ctx.home.clone(), ctx.s.clone(), ctx.mode);
            let dir = ctx.setting("DIR").to_string();
            let mut failed = false;
            for u in &args[1..] {
                let u = as_url(u).unwrap_or_else(|| u.clone());
                if first_id(&u).is_none() {
                    println!("not a YouTube video: {u}");
                    failed = true;
                    continue;
                }
                let _ = std::fs::create_dir_all(&dir);
                let tmpl = format!("{}/{}", dir.replace('%', "%%").trim_end_matches('/'), NAME);
                match runner.transcript(&u, &tmpl, &["yt-dlp".to_string()], None) {
                    Ok(txt) => println!("{}", ctx.tilde(&txt)),
                    Err(why) => {
                        println!("no transcript for {u} -- {why}");
                        failed = true;
                    }
                }
            }
            i32::from(failed)
        }
        "tui" => {
            let runner = Arc::new(Runner::new(ctx.paths.clone(), ctx.home.clone(), ctx.s.clone(), Mode::Quiet));
            crate::window::main(runner)
        }
        other => help(&ctx, other),
    }
}
