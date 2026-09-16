// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Paul Richeson

//! sstr -- record a stream into a static stream, and play it back as one.
//!
//! Every verb here is a message sent to a file, and the same message the
//! Workspace sends when a person picks it from Services: what the window
//! does is always something a person could have typed.
//!
//! The verbs, options and reports are the prototype's (`tools/copal-sstr.py`
//! in copal), so a script written for one runs the other; the one deliberate
//! difference is that `record` given no bytes at all fails.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::ExitCode;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use staticstream::format::json::{self, Value};
use staticstream::format::reader::{self, Erasures, Reader, Source};
use staticstream::format::sha256::{hex, Sha256};
use staticstream::format::writer::{self, Writer};
use staticstream::tty::{self as tty, Unarmor};

const USAGE: &str = "\
sstr -- Static Stream: record a stream into a file, and play it back as one

  sstr record OUT [--input FILE] [--type MIME] [--key KEY] [--deflate]
                  [--format 0|1]
                  [--source URL] [--license TEXT] [--note TEXT]
                  [--chunk N] [--align N] [--flush-ms N] [--group N]
                  [--checkpoint-records N] [--checkpoint-secs S]
  sstr play IN [-o FILE] [--paced] [--speed N] [--start S] [--follow]
               [--gap skip|zero] [--erasures FILE] [--allowed-signers FILE] [-v]
  sstr play IN --serve [HOST]:PORT [--paced] [--follow] [--once]
  sstr verify IN [--erasures FILE] [--allowed-signers FILE] [-v]
  sstr armor [IN] [--baud N] [--flush-ms N]
  sstr unarmor [IN] [--erasures FILE]
  sstr recv [IN] [--verify] [--paced] [--speed N] [--gap skip|zero]
            [--allowed-signers FILE] [-v]
  sstr paths

IN and OUT may be - for stdin and stdout. `write` is another name for record.
";

// ------------------------------------------------------------- arguments ---

struct Args {
    pos: Vec<String>,
    opts: HashMap<String, String>,
    flags: Vec<String>,
}

impl Args {
    /// `valued` names the options that take a value; any other --name is a flag.
    fn parse(raw: &[String], valued: &[&str]) -> Result<Args, String> {
        let alias = |s: &str| match s {
            "-o" => "--output".to_string(),
            "-v" => "--verbose".to_string(),
            other => other.to_string(),
        };
        let mut a = Args { pos: Vec::new(), opts: HashMap::new(), flags: Vec::new() };
        let mut i = 0;
        while i < raw.len() {
            let r = &raw[i];
            if r.starts_with('-') && r != "-" {
                let (name, inline) = match r.split_once('=') {
                    Some((n, v)) if n.starts_with("--") => (alias(n), Some(v.to_string())),
                    _ => (alias(r), None),
                };
                if valued.contains(&name.as_str()) {
                    let v = match inline {
                        Some(v) => v,
                        None => {
                            i += 1;
                            raw.get(i).cloned().ok_or(format!("{name} needs a value"))?
                        }
                    };
                    a.opts.insert(name, v);
                } else {
                    a.flags.push(name);
                }
            } else {
                a.pos.push(r.clone());
            }
            i += 1;
        }
        Ok(a)
    }

    fn flag(&self, name: &str) -> bool {
        self.flags.iter().any(|f| f == name)
    }

    fn get(&self, name: &str) -> Option<&str> {
        self.opts.get(name).map(String::as_str)
    }

    fn num<T: std::str::FromStr>(&self, name: &str, default: T) -> Result<T, String> {
        match self.get(name) {
            Some(v) => v.parse().map_err(|_| format!("{name}: not a number: {v}")),
            None => Ok(default),
        }
    }

    fn check_flags(&self, known: &[&str]) -> Result<(), String> {
        match self.flags.iter().find(|f| !known.contains(&f.as_str())) {
            Some(f) => Err(format!("unknown option {f}")),
            None => Ok(()),
        }
    }
}

// ---------------------------------------------------------------- signals ---

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_stop(_: i32) {
    STOP.store(true, Ordering::SeqCst);
}

extern "C" {
    fn signal(signum: i32, handler: extern "C" fn(i32)) -> usize;
}

/// Ctrl+C and SIGTERM stop a capture cleanly -- the end record, marked
/// interrupted, and the last checkpoint -- instead of cutting it off. There is
/// no signal handling in std and no crates here, so this is the C library's
/// own `signal`, which std already links; the handler only sets a flag.
fn catch_stop() {
    const SIGINT: i32 = 2;
    const SIGTERM: i32 = 15;
    unsafe {
        signal(SIGINT, on_stop);
        signal(SIGTERM, on_stop);
    }
}

// ------------------------------------------------------------------- pump ---

/// Read `input` as a stream: whole chunks as they fill, and whatever whole
/// multiple of `align` has waited `flush_ms`, each with the moment its first
/// byte arrived. Returns true when a stop signal ended it.
fn pump(input: Box<dyn Read + Send>, chunk: usize, align: usize, flush_ms: u64, on_chunk: &mut dyn FnMut(&[u8], Instant) -> io::Result<()>) -> io::Result<bool> {
    let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(16);
    std::thread::spawn(move || {
        let mut input = input;
        let mut buf = vec![0u8; 1 << 16];
        loop {
            match input.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
    });
    let flush = Duration::from_millis(flush_ms);
    let tick = Duration::from_millis(200);
    let mut buf: Vec<u8> = Vec::new();
    let mut first = Instant::now();
    let mut stopped = false;
    loop {
        if STOP.load(Ordering::SeqCst) {
            stopped = true;
            break;
        }
        let timeout = if buf.is_empty() { tick } else { (first + flush).saturating_duration_since(Instant::now()).min(tick) };
        match rx.recv_timeout(timeout) {
            Ok(bytes) => {
                if buf.is_empty() {
                    first = Instant::now();
                }
                buf.extend_from_slice(&bytes);
                // Whole chunks by index, and the buffer trimmed once: draining
                // each chunk from the front shifts everything behind it, and at
                // armor's 45 bytes a chunk that is 1,400 shifts of a 64 KiB read.
                let mut start = 0;
                while buf.len() - start >= chunk {
                    on_chunk(&buf[start..start + chunk], first)?;
                    start += chunk;
                    first = Instant::now();
                }
                buf.drain(..start);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if !buf.is_empty() && Instant::now() >= first + flush {
            let n = buf.len() / align * align;
            if n > 0 {
                on_chunk(&buf[..n], first)?;
                buf.drain(..n);
            }
            first = Instant::now();
        }
    }
    if !buf.is_empty() {
        on_chunk(&buf, first)?;
    }
    Ok(stopped)
}

fn open_in(path: &str) -> io::Result<Box<dyn Read + Send>> {
    Ok(if path == "-" { Box::new(io::stdin()) } else { Box::new(File::open(path)?) })
}

fn load_erasures(path: Option<&str>) -> Result<Erasures, String> {
    let Some(path) = path else { return Ok(Erasures::new()) };
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    let v = json::parse(&text).map_err(|e| format!("{path}: {e}"))?;
    let ranges: Vec<(u64, u64)> = v
        .as_array()
        .unwrap_or(&[])
        .iter()
        .filter_map(|r| {
            let r = r.as_array()?;
            Some((r.first()?.as_u64()?, r.get(1)?.as_u64()?))
        })
        .collect();
    Ok(Erasures::from_ranges(&ranges))
}

fn local_time() -> String {
    // std has no time zones; `date` has, on every system this runs on.
    std::process::Command::new("date")
        .arg("+%Y-%m-%dT%H:%M:%S%z")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

// ------------------------------------------------------------------ verbs ---

fn cmd_record(raw: &[String]) -> Result<i32, String> {
    let valued = [
        "--input", "--type", "--chunk", "--align", "--flush-ms", "--group", "--checkpoint-records",
        "--checkpoint-secs", "--key", "--source", "--license", "--note", "--format",
    ];
    let a = Args::parse(raw, &valued)?;
    a.check_flags(&["--deflate"])?;
    let output = a.pos.first().ok_or("record needs OUT: a .sstr, or -")?.clone();
    let ctype = a.get("--type").unwrap_or("application/octet-stream").to_string();
    let align = match a.num::<usize>("--align", 0)? {
        0 if ctype == "video/mp2t" => 188,
        0 => 1,
        n => n,
    };
    let chunk = (a.num::<usize>("--chunk", 65536)? / align * align).max(align);
    let group = a.num::<usize>("--group", staticstream::format::GROUP)?.max(1);
    let ckpt_records = a.num::<usize>("--checkpoint-records", 64)?.max(1);
    let ckpt_secs = a.num::<f64>("--checkpoint-secs", 10.0)?;
    let mut meta = Value::obj(vec![
        ("created", Value::str(local_time())),
        ("content_type", Value::str(&ctype)),
        ("chunk", Value::num(chunk as u64)),
        ("align", Value::num(align as u64)),
        (
            "fec",
            Value::obj(vec![
                ("inner", Value::str("RS(255,223) GF(2^8)/0x11d, interleaved per record")),
                ("outer", Value::str(format!("XOR, one per {group} data records"))),
            ]),
        ),
        ("checkpoint", Value::obj(vec![("records", Value::num(ckpt_records as u64)), ("seconds", Value::Num(ckpt_secs))])),
    ]);
    for k in ["source", "license", "note"] {
        if let Some(v) = a.get(&format!("--{k}")) {
            meta.set(k, Value::str(v));
        }
    }
    let input = open_in(a.get("--input").unwrap_or("-")).map_err(|e| format!("--input: {e}"))?;
    let out: Box<dyn Write> = if output == "-" {
        Box::new(io::stdout())
    } else {
        Box::new(File::create(&output).map_err(|e| format!("{output}: {e}"))?)
    };
    // --format picks the outer code, and 0 is the default. Version 1 replaces
    // the parity record's single XOR row with P and Q, which rebuilds two lost
    // records of a group where version 0 rebuilds one; the prototype reads
    // version 0 and only version 0, so that is what is written unless asked.
    let version = a.num::<u64>("--format", 0)?;
    if version > 1 {
        return Err(format!("--format {version}: this build writes 0 or 1"));
    }
    let opts = writer::Options {
        group,
        checkpoint_records: ckpt_records,
        checkpoint_secs: ckpt_secs,
        deflate: a.flag("--deflate"),
        key: a.get("--key").map(PathBuf::from),
        outer: staticstream::format::record::Outer::of_version(version),
    };
    let mut w = Writer::new(out, meta, opts).map_err(|e| e.to_string())?;
    catch_stop();
    let started = w.started();
    let flush_ms = a.num::<u64>("--flush-ms", 200)?;
    let mut failed: Option<io::Error> = None;
    let stopped = pump(input, chunk, align, flush_ms, &mut |data, arrived| {
        let t_us = arrived.saturating_duration_since(started).as_micros() as u64;
        w.data(data, t_us).map_err(|e| {
            failed = Some(io::Error::new(e.kind(), e.to_string()));
            e
        })
    });
    if let Err(e) = stopped {
        return Err(format!("record: {}", failed.map(|f| f.to_string()).unwrap_or_else(|| e.to_string())));
    }
    let (_, written) = w.finish(stopped.unwrap_or(false)).map_err(|e| e.to_string())?;
    if written.bytes == 0 {
        // A capture of nothing is how a failed download upstream looks from
        // here -- yt-dlp exits 1 and the pipe closes empty -- and the
        // prototype accepted it. Say so, and leave no file that looks like one.
        if output != "-" {
            let _ = std::fs::remove_file(&output);
        }
        eprintln!("sstr record: no bytes arrived; nothing was recorded");
        return Ok(1);
    }
    Ok(0)
}

fn reader_opts(a: &Args) -> Result<reader::Options, String> {
    let gap = a.get("--gap").unwrap_or("skip");
    if gap != "skip" && gap != "zero" {
        return Err("--gap is skip or zero".into());
    }
    Ok(reader::Options {
        gap_zero: gap == "zero",
        paced: a.flag("--paced"),
        speed: a.num("--speed", 1.0)?,
        start: a.num("--start", 0.0)?,
        allowed_signers: a.get("--allowed-signers").map(PathBuf::from),
        verbose: a.flag("--verbose"),
    })
}

fn cmd_play(raw: &[String]) -> Result<i32, String> {
    let valued = ["--erasures", "--allowed-signers", "--speed", "--start", "--gap", "--output", "--serve"];
    let a = Args::parse(raw, &valued)?;
    a.check_flags(&["--verbose", "--paced", "--follow", "--once"])?;
    let input = a.pos.first().ok_or("play needs IN: a .sstr, or -")?.clone();
    if let Some(addr) = a.get("--serve") {
        return serve(&input, addr, &a);
    }
    let opts = reader_opts(&a)?;
    let out: Box<dyn Write> = match a.get("--output") {
        Some(p) => Box::new(File::create(p).map_err(|e| format!("{p}: {e}"))?),
        None => Box::new(io::stdout()),
    };
    let src = Source::new(open_in(&input).map_err(|e| format!("{input}: {e}"))?, a.flag("--follow"), Rc::new(RefCell::new(load_erasures(a.get("--erasures"))?)));
    let mut r = Reader::new(src, Some(out), opts);
    r.run().map_err(|e| e.to_string())?;
    if a.flag("--verbose") || a.get("--output").is_some() {
        eprintln!("{}", r.summary());
    }
    Ok(r.status())
}

fn cmd_verify(raw: &[String]) -> Result<i32, String> {
    let a = Args::parse(raw, &["--erasures", "--allowed-signers"])?;
    a.check_flags(&["--verbose"])?;
    let input = a.pos.first().ok_or("verify needs IN: a .sstr, or -")?.clone();
    let opts = reader::Options { allowed_signers: a.get("--allowed-signers").map(PathBuf::from), verbose: a.flag("--verbose"), ..Default::default() };
    let src = Source::new(open_in(&input).map_err(|e| format!("{input}: {e}"))?, false, Rc::new(RefCell::new(load_erasures(a.get("--erasures"))?)));
    let mut r = Reader::new(src, None, opts);
    r.run().map_err(|e| e.to_string())?;
    println!("{}", r.summary());
    Ok(r.status())
}

fn cmd_armor(raw: &[String]) -> Result<i32, String> {
    let a = Args::parse(raw, &["--baud", "--flush-ms"])?;
    a.check_flags(&[])?;
    let input = open_in(a.pos.first().map(String::as_str).unwrap_or("-")).map_err(|e| e.to_string())?;
    let rate = a.num::<f64>("--baud", 0.0)? / 10.0;
    let stdout = io::stdout();
    // Paced, every line leaves when its time comes; unpaced, lines are
    // gathered into large writes instead of one system call each.
    let mut out: Box<dyn Write> = if rate > 0.0 { Box::new(stdout.lock()) } else { Box::new(io::BufWriter::with_capacity(1 << 16, stdout.lock())) };
    let t0 = Instant::now();
    let mut sent = 0u64;
    let mut off = 0u64;
    let mut h = Sha256::new();
    let send = |out: &mut dyn Write, text: &str, sent: &mut u64| -> io::Result<()> {
        out.write_all(text.as_bytes())?;
        *sent += text.len() as u64;
        if rate > 0.0 {
            out.flush()?;
            let due = t0 + Duration::from_secs_f64(*sent as f64 / rate);
            let now = Instant::now();
            if due > now {
                std::thread::sleep(due - now);
            }
        }
        Ok(())
    };
    send(&mut out, &format!("{}\n", tty::BEGIN), &mut sent).map_err(|e| e.to_string())?;
    pump(input, tty::LINE_BYTES, tty::LINE_BYTES, a.num("--flush-ms", 200)?, &mut |data, _| {
        h.update(data);
        for piece in data.chunks(tty::LINE_BYTES) {
            send(&mut out, &tty::armor_line(off, piece), &mut sent)?;
            off += piece.len() as u64;
        }
        Ok(())
    })
    .map_err(|e| e.to_string())?;
    send(&mut out, &tty::end_line(off, &hex(&h.finish())), &mut sent).map_err(|e| e.to_string())?;
    out.flush().map_err(|e| e.to_string())?;
    Ok(0)
}

fn open_lines(path: Option<&str>) -> io::Result<Box<dyn io::BufRead>> {
    Ok(match path {
        None | Some("-") => Box::new(BufReader::new(io::stdin())),
        Some(p) => Box::new(BufReader::new(File::open(p)?)),
    })
}

fn cmd_unarmor(raw: &[String]) -> Result<i32, String> {
    let a = Args::parse(raw, &["--erasures"])?;
    a.check_flags(&[])?;
    let er = Rc::new(RefCell::new(Erasures::new()));
    let mut u = Unarmor::new(open_lines(a.pos.first().map(String::as_str)).map_err(|e| e.to_string())?, er.clone());
    let stdout = io::stdout();
    let mut out = stdout.lock();
    io::copy(&mut u, &mut out).map_err(|e| e.to_string())?;
    out.flush().map_err(|e| e.to_string())?;
    if let Some(path) = a.get("--erasures") {
        let ranges = Value::Arr(er.borrow().ranges().iter().map(|&(s, e)| Value::Arr(vec![Value::num(s), Value::num(e)])).collect());
        std::fs::write(path, ranges.to_json()).map_err(|e| format!("{path}: {e}"))?;
    }
    let st = u.stats;
    eprintln!(
        "unarmor: {} lines, {} failed CRC, {} not armor, {} bytes lost{}",
        st.lines,
        st.crc_failed,
        st.not_armor,
        st.lost_bytes,
        if u.total.is_some() { "" } else { ", no END line" }
    );
    Ok(0)
}

fn cmd_recv(raw: &[String]) -> Result<i32, String> {
    let a = Args::parse(raw, &["--allowed-signers", "--speed", "--gap"])?;
    a.check_flags(&["--verify", "--paced", "--verbose"])?;
    let er = Rc::new(RefCell::new(Erasures::new()));
    let u = Unarmor::new(open_lines(a.pos.first().map(String::as_str)).map_err(|e| e.to_string())?, er.clone());
    let out: Option<Box<dyn Write>> = if a.flag("--verify") { None } else { Some(Box::new(io::stdout())) };
    let mut r = Reader::new(Source::new(StatsReader::new(u), false, er), out, reader_opts(&a)?);
    r.run().map_err(|e| e.to_string())?;
    let st = r.src_stats();
    eprintln!("recv: {} lines, {} failed CRC, {} not armor, {} bytes lost on the line", st.lines, st.crc_failed, st.not_armor, st.lost_bytes);
    if a.flag("--verify") || a.flag("--verbose") {
        eprintln!("{}", r.summary());
    }
    Ok(r.status())
}

/// An `Unarmor` whose counts stay readable after the reader has taken it.
struct StatsReader<R: io::BufRead> {
    inner: Unarmor<R>,
    shared: Rc<RefCell<tty::UnarmorStats>>,
}

impl<R: io::BufRead> StatsReader<R> {
    fn new(inner: Unarmor<R>) -> Self {
        StatsReader { inner, shared: Rc::new(RefCell::new(tty::UnarmorStats::default())) }
    }
}

impl<R: io::BufRead> Read for StatsReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        *self.shared.borrow_mut() = self.inner.stats;
        Ok(n)
    }
}

trait SrcStats {
    fn src_stats(&self) -> tty::UnarmorStats;
}

impl<R: io::BufRead> SrcStats for Reader<StatsReader<R>> {
    fn src_stats(&self) -> tty::UnarmorStats {
        *self.src.inner().shared.borrow()
    }
}

/// The VideoLAN split: this is the server, and any player that opens
/// http://HOST:PORT/ is a client. Each client gets its own reader from the
/// start of the capture (or --start), so a capture still being written with
/// --follow is a timeshift buffer: join late and watch from the beginning.
fn serve(input: &str, addr: &str, a: &Args) -> Result<i32, String> {
    if input == "-" {
        return Err("--serve needs a file, not stdin".into());
    }
    let (host, port) = addr.rsplit_once(':').ok_or("--serve is [HOST]:PORT")?;
    let host = if host.is_empty() { "0.0.0.0" } else { host };
    let ctype = reader::read_meta(std::path::Path::new(input))
        .and_then(|m| m.get("content_type").and_then(|v| v.as_str()).map(str::to_string))
        .unwrap_or_else(|| "application/octet-stream".into());
    let listener = TcpListener::bind(format!("{host}:{port}")).map_err(|e| format!("{host}:{port}: {e}"))?;
    eprintln!("sstr: serving {input} ({ctype}) on http://{host}:{port}/");
    let opts = reader_opts(a)?;
    let follow = a.flag("--follow");
    for conn in listener.incoming() {
        let Ok(conn) = conn else { continue };
        let (input, ctype, opts) = (input.to_string(), ctype.clone(), opts.clone());
        let handle = move || client(conn, &input, &ctype, follow, opts);
        if a.flag("--once") {
            handle();
            return Ok(0);
        }
        std::thread::spawn(handle);
    }
    Ok(0)
}

fn client(mut conn: TcpStream, input: &str, ctype: &str, follow: bool, opts: reader::Options) {
    let peer = conn.peer_addr().map(|p| p.to_string()).unwrap_or_else(|_| "?".into());
    let result = (|| -> io::Result<(String, u64, f64)> {
        conn.set_read_timeout(Some(Duration::from_secs(10)))?;
        let mut req = [0u8; 4096];
        let n = conn.read(&mut req)?;
        let first = String::from_utf8_lossy(&req[..n]).split("\r\n").next().unwrap_or("").to_string();
        conn.set_read_timeout(None)?;
        conn.write_all(format!("HTTP/1.0 200 OK\r\nContent-Type: {ctype}\r\nCache-Control: no-cache\r\n\r\n").as_bytes())?;
        let src = Source::new(File::open(input)?, follow, Rc::new(RefCell::new(Erasures::new())));
        let mut r = Reader::new(src, Some(Box::new(conn.try_clone()?)), opts);
        let t0 = Instant::now();
        r.run()?;
        Ok((first, r.s.bytes, t0.elapsed().as_secs_f64()))
    })();
    match result {
        Ok((req, bytes, secs)) => eprintln!("sstr: {peer} '{req}': {bytes} bytes in {secs:.1} s"),
        Err(e) => eprintln!("sstr: {peer} left: {e}"),
    }
}

fn paths() -> i32 {
    let Some(p) = staticstream::ytq::Paths::from_env() else {
        eprintln!("sstr paths: HOME is not set");
        return 1;
    };
    for (label, path) in p.labelled() {
        let mark = if path.exists() { "" } else { "   (not there)" };
        println!("{label:<11} {}{mark}", path.display());
    }
    0
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = args.get(1..).unwrap_or(&[]);
    let result = match args.first().map(String::as_str) {
        None | Some("help" | "-h" | "--help") => {
            print!("{USAGE}");
            Ok(0)
        }
        Some("-V" | "--version") => {
            println!("sstr {} (Static Stream format v{})", env!("CARGO_PKG_VERSION"), staticstream::format::FORMAT_VERSION);
            Ok(0)
        }
        Some("record" | "write") => cmd_record(rest),
        Some("play") => cmd_play(rest),
        Some("verify") => cmd_verify(rest),
        Some("armor") => cmd_armor(rest),
        Some("unarmor") => cmd_unarmor(rest),
        Some("recv") => cmd_recv(rest),
        Some("paths") => Ok(paths()),
        Some("ytq") => Ok(staticstream::ytq::cli::main(rest)),
        Some(verb) => Err(format!("no verb '{verb}'\n\n{USAGE}")),
    };
    match result {
        Ok(code) => ExitCode::from(code as u8),
        // A player that closed the pipe has had what it wanted.
        Err(e) if e.contains("Broken pipe") => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("sstr: {e}");
            ExitCode::from(2)
        }
    }
}
