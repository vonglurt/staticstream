// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Paul Richeson

//! queue.json, and the one process that downloads: the Python ytq's `Queue`,
//! `RunLock`, `enqueue()` and `start_runner()`.
//!
//! Every change takes queue.lock, reads the file afresh, changes what it came
//! to change, and writes it back through a rename only if something changed.
//! The lock is `flock`, the same lock Python's `fcntl.flock` takes, and the
//! file is written as Python's `json.dump(indent=1)` writes it, so the Python
//! and the Rust ytq share one queue without either noticing the other.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

use crate::format::json::{self, Value};

use crate::ytq::log::log;
use crate::ytq::{sys, Paths, PENDING};

/// A field of an entry as text, "" when absent or not a string.
pub fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(|x| x.as_str()).unwrap_or("")
}

pub fn status(v: &Value) -> &str {
    text(v, "status")
}

/// `i["title"] or i["url"]`.
pub fn title_or_url(v: &Value) -> &str {
    let t = text(v, "title");
    if t.is_empty() {
        text(v, "url")
    } else {
        t
    }
}

/// `new_entry(url)`: the fields in the Python ytq's order.
pub fn new_entry(url: &str) -> Value {
    Value::obj(vec![
        ("url", Value::str(url)),
        ("title", Value::str("")),
        ("status", Value::str("checking")),
        ("quality", Value::str("")),
        ("attempts", Value::num(0)),
        ("cookie_tried", Value::Bool(false)),
        ("error", Value::str("")),
        ("added", Value::num(sys::now() as u64)),
        ("file", Value::str("")),
        ("progress", Value::str("")),
    ])
}

pub fn find<'a>(items: &'a mut [Value], url: &str) -> Option<&'a mut Value> {
    items.iter_mut().find(|i| text(i, "url") == url)
}

/// The queue as it stands, for looking at. Writes are renames, so no lock.
pub fn snapshot(paths: &Paths) -> Vec<Value> {
    let Ok(bytes) = std::fs::read(&paths.queue) else { return Vec::new() };
    match json::parse_bytes(&bytes) {
        Ok(Value::Arr(items)) => items,
        _ => Vec::new(),
    }
}

/// Change the queue under queue.lock: `f` gets the live list, and whatever it
/// leaves is written back when it returns, if anything changed.
pub fn edit<R>(paths: &Paths, f: impl FnOnce(&mut Vec<Value>) -> R) -> io::Result<R> {
    if let Some(dir) = paths.queue.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let lock = OpenOptions::new().create(true).append(true).open(&paths.queue_lock)?;
    sys::file_lock(&lock, sys::LOCK_EX)?;
    let mut items = snapshot(paths);
    let before = Value::Arr(items.clone()).to_python_json(None);
    let r = f(&mut items);
    let list = Value::Arr(items);
    let result = if list.to_python_json(None) != before {
        let mut tmp = paths.queue.as_os_str().to_owned();
        tmp.push(".tmp");
        (|| -> io::Result<()> {
            let mut out = File::create(&tmp)?;
            out.write_all(list.to_python_json(Some(1)).as_bytes())?;
            out.flush()?;
            std::fs::rename(&tmp, &paths.queue)
        })()
    } else {
        Ok(())
    };
    let _ = sys::file_lock(&lock, sys::LOCK_UN);
    result.map(|_| r)
}

/// `update(url, **kw)`: set fields of one entry. Its new state, or None if it
/// was deleted meanwhile.
pub fn update(paths: &Paths, url: &str, fields: Vec<(&str, Value)>) -> io::Result<Option<Value>> {
    edit(paths, |items| {
        let it = find(items, url)?;
        for (k, v) in fields {
            it.set(k, v);
        }
        Some(it.clone())
    })
}

/// run.lock: held by the one process that downloads.
#[derive(Default)]
pub struct RunLock {
    file: Option<File>,
}

impl RunLock {
    pub fn held(&self) -> bool {
        self.file.is_some()
    }

    /// Take run.lock, with a few tries, because `ytq status` looking at the
    /// lock holds it for a moment. Once taken, entries a dead runner left
    /// `downloading` go back in the queue.
    pub fn take(&mut self, paths: &Paths, tries: u32, on_taken: impl FnOnce()) -> io::Result<bool> {
        if let Some(dir) = paths.run_lock.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let file = OpenOptions::new().read(true).create(true).append(true).open(&paths.run_lock)?;
        let mut taken = false;
        for n in 0..tries.max(1) {
            if sys::file_lock(&file, sys::LOCK_EX | sys::LOCK_NB).is_ok() {
                taken = true;
                break;
            }
            if n + 1 < tries {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
        if !taken {
            return Ok(false);
        }
        file.set_len(0)?;
        (&file).write_all(format!("{}\n", std::process::id()).as_bytes())?;
        self.file = Some(file);
        on_taken();
        edit(paths, |items| {
            for it in items.iter_mut() {
                if status(it) == "downloading" {
                    let phase = it.get("live").and_then(|l| l.get("phase")).and_then(|p| p.as_str()).unwrap_or("no step recorded").to_string();
                    log(paths, &format!("{}: left 'downloading' ({}) by a runner that is gone; back in the queue", crate::ytq::urls::short(text(it, "url")), phase));
                    it.set("status", Value::str("queued"));
                    it.set("progress", Value::str(""));
                    it.set("live", Value::Obj(Vec::new()));
                }
            }
        })?;
        Ok(true)
    }

    pub fn release(&mut self) {
        if let Some(f) = self.file.take() {
            let _ = sys::file_lock(&f, sys::LOCK_UN);
        }
    }

    /// The pid holding run.lock, or None.
    pub fn holder(&self, paths: &Paths) -> Option<String> {
        if self.file.is_some() {
            return Some(std::process::id().to_string());
        }
        let file = OpenOptions::new().read(true).create(true).append(true).open(&paths.run_lock).ok()?;
        if sys::file_lock(&file, sys::LOCK_SH | sys::LOCK_NB).is_err() {
            let mut s = String::new();
            let _ = (&file).read_to_string(&mut s);
            let s = s.trim().to_string();
            return Some(if s.is_empty() { "?".into() } else { s });
        }
        let _ = sys::file_lock(&file, sys::LOCK_UN);
        None
    }
}

/// What starts a background runner: this program's own `run --quiet`.
/// $STATICSTREAM_YTQ_RUNNER overrides it, for tests.
///
/// Run as `ytq` the subcommand is not repeated, and run as `sstr` it is:
/// `ytq run --quiet` from the binary, `sstr ytq run --quiet` from the command.
/// Both reach the same `cli::main`, so a runner one started is the other's too.
fn runner_command() -> Vec<String> {
    match std::env::var("STATICSTREAM_YTQ_RUNNER") {
        Ok(cmd) if !cmd.trim().is_empty() => cmd.split_whitespace().map(str::to_string).collect(),
        _ => {
            let exe = std::env::current_exe().ok();
            let is_ytq = exe
                .as_deref()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy() == "ytq")
                .unwrap_or(false);
            let me = exe.map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| "sstr".into());
            let mut cmd = vec![me];
            if !is_ytq {
                cmd.push("ytq".into());
            }
            cmd.push("run".into());
            cmd.push("--quiet".into());
            cmd
        }
    }
}

/// `start_runner()`: (pid, started) -- a background runner, unless run.lock is
/// held. Call it inside `edit`, so that a URL cannot land between a runner's
/// "nothing left" and its letting go.
pub fn start_runner(paths: &Paths, run: &RunLock) -> (String, bool) {
    if let Some(pid) = run.holder(paths) {
        return (pid, false);
    }
    let cmd = runner_command();
    let logf = OpenOptions::new().create(true).append(true).open(&paths.log);
    let mut c = Command::new(&cmd[0]);
    c.args(&cmd[1..]).stdin(Stdio::null()).process_group(0);
    match logf.and_then(|f| Ok((f.try_clone()?, f))) {
        Ok((out, err)) => {
            c.stdout(out).stderr(err);
        }
        Err(_) => {
            c.stdout(Stdio::null()).stderr(Stdio::null());
        }
    }
    match c.spawn() {
        Ok(child) => {
            log(paths, &format!("started a runner, pid {}", child.id()));
            (child.id().to_string(), true)
        }
        Err(e) => {
            log(paths, &format!("could not start a runner ({}): {}", cmd.join(" "), e));
            ("?".into(), false)
        }
    }
}

/// `enqueue(urls, run)`: queue whatever is new. (added, runner): runner is
/// (pid, started) when one is downloading or was started for this.
pub fn enqueue(paths: &Paths, run_lock: &RunLock, urls: &[String], run: bool) -> io::Result<(Vec<String>, Option<(String, bool)>)> {
    edit(paths, |items| {
        let mut added = Vec::new();
        for u in urls {
            if find(items, u).is_none() {
                items.push(new_entry(u));
                added.push(u.clone());
                log(paths, &format!("added {u}"));
            }
        }
        let runner = if items.iter().any(|i| PENDING.contains(&status(i))) {
            if run {
                Some(start_runner(paths, run_lock))
            } else {
                run_lock.holder(paths).map(|pid| (pid, false))
            }
        } else {
            None
        };
        (added, runner)
    })
}
