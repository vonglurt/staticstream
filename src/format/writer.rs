// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Paul Richeson

//! The writer: a stream in, records out.
//!
//! The file signature, the stream header twice, then for every chunk a data
//! record; after every `group` data records, or at a checkpoint, a parity
//! record; every `checkpoint_records` hashed records or `checkpoint_secs`,
//! a signed checkpoint, twice, always straight after a parity record so a
//! reader has rebuilt what it can before it checks the entries; at the end,
//! the last parity, the end record twice, and a last checkpoint.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use crate::format::json::Value;
use crate::format::record::{self, Header};
use crate::format::sha256::{hex, Sha256};
use crate::format::{crc32, zlib, RecordType, FLAG_DEFLATE, GROUP, MAGIC, SIGNING_NAMESPACE};

#[derive(Clone, Debug)]
pub struct Options {
    /// Data records per parity record.
    pub group: usize,
    /// Hashed records per checkpoint.
    pub checkpoint_records: usize,
    /// Seconds per checkpoint, whichever comes first.
    pub checkpoint_secs: f64,
    /// Compress a record when that saves 5 %.
    pub deflate: bool,
    /// The SSH private key to sign with, or a public key held by ssh-agent.
    pub key: Option<PathBuf>,
    /// The outer code, which is what the format version names.
    ///
    /// **Version 1 is the default.** A capture is worth more when two lost
    /// records of a group come back than when one does, and that is the whole
    /// of what the second parity row buys, at about 6 % of the file.
    ///
    /// What it costs: `tools/copal-sstr.py` reads version 0 and only version
    /// 0, so a capture written from here can no longer be read by the
    /// prototype. That is why `tests/crosscheck.sh` records with `--format 0`
    /// -- the 44 comparisons are comparisons at version 0 and go on being so,
    /// which keeps the chain back to the specification unbroken. Readers take
    /// both versions and always will: a version is a thing files have, and
    /// every capture written before today is still a capture.
    pub outer: record::Outer,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            group: GROUP,
            checkpoint_records: 64,
            checkpoint_secs: 10.0,
            deflate: false,
            key: None,
            outer: record::Outer::PQ,
        }
    }
}

pub struct Writer<W: Write> {
    out: W,
    opts: Options,
    t0: Instant,
    last_ckpt: Instant,
    seq: u64,
    stream_id: [u8; 16],
    prev: [u8; 32],
    group: Vec<(Header, Vec<u8>)>,
    window: Vec<(u64, [u8; 32])>,
    data_records: u64,
    total: u64,
    payload: Sha256,
    meta: Value,
}

/// What was written, for a caller that wants to know.
#[derive(Clone, Copy, Debug)]
pub struct Written {
    pub data_records: u64,
    pub bytes: u64,
}

impl<W: Write> Writer<W> {
    /// Start a capture: the signature and the stream header. `meta` is the
    /// header's JSON object; `format`, `version`, `stream_id` and, with a key,
    /// `key` are added to it.
    pub fn new(mut out: W, mut meta: Value, opts: Options) -> io::Result<Writer<W>> {
        let stream_id = random_id()?;
        meta.set("format", Value::str("sstr"));
        // THE VERSION IS THE OUTER CODE'S NAME. A reader that has the header
        // knows which arithmetic a parity record holds before it meets one --
        // though it does not need to, because the rows are countable from the
        // entries.
        meta.set("version", Value::num(if opts.outer == record::Outer::PQ { 1 } else { 0 }));
        meta.set("stream_id", Value::str(hex(&stream_id)));
        if let Some(k) = &opts.key {
            meta.set("key", Value::str(public_key(k)?));
        }
        out.write_all(&MAGIC)?;
        let now = Instant::now();
        let mut w = Writer {
            out,
            opts,
            t0: now,
            last_ckpt: now,
            seq: 0,
            stream_id,
            prev: record::genesis(&stream_id),
            group: Vec::new(),
            window: Vec::new(),
            data_records: 0,
            total: 0,
            payload: Sha256::new(),
            meta,
        };
        let body = format!("{}\n", w.meta.to_json_indented()).into_bytes();
        w.record(RecordType::Head, &body, 0, None, 2, true)?;
        Ok(w)
    }

    /// When the capture started: a record's time is measured from here.
    pub fn started(&self) -> Instant {
        self.t0
    }

    fn now_us(&self) -> u64 {
        self.t0.elapsed().as_micros() as u64
    }

    fn record(&mut self, typ: RecordType, body: &[u8], flags: u8, t_us: Option<u64>, repeat: usize, hashed: bool) -> io::Result<Header> {
        let h = Header {
            typ,
            flags,
            seq: self.seq,
            t_us: t_us.unwrap_or_else(|| self.now_us()),
            plain_len: body.len() as u32,
            body_crc: crc32::crc32(body),
        };
        let disk = record::encode(&h, body);
        for _ in 0..repeat {
            self.out.write_all(&disk)?;
        }
        // Flushed per record: a reader following the file sees each as it lands.
        self.out.flush()?;
        if hashed {
            self.window.push((self.seq, record::record_hash(&h, body)));
        }
        self.seq += 1;
        Ok(h)
    }

    /// One chunk of the stream, which began to arrive `t_us` after the start.
    pub fn data(&mut self, chunk: &[u8], t_us: u64) -> io::Result<()> {
        let mut body = chunk.to_vec();
        let mut flags = 0;
        if self.opts.deflate {
            let z = zlib::compress(chunk);
            if (z.len() as f64) < chunk.len() as f64 * 0.95 {
                body = z;
                flags = FLAG_DEFLATE;
            }
        }
        let h = self.record(RecordType::Data, &body, flags, Some(t_us), 1, true)?;
        self.group.push((h, body));
        self.data_records += 1;
        self.total += chunk.len() as u64;
        self.payload.update(chunk);
        let due = self.window.len() >= self.opts.checkpoint_records || self.last_ckpt.elapsed().as_secs_f64() >= self.opts.checkpoint_secs;
        if self.group.len() >= self.opts.group || due {
            self.parity()?;
        }
        if due {
            self.checkpoint()?;
        }
        Ok(())
    }

    fn parity(&mut self) -> io::Result<()> {
        if self.group.is_empty() {
            return Ok(());
        }
        let body = record::encode_parity_with(&self.group, self.opts.outer);
        self.record(RecordType::Parity, &body, 0, None, 1, false)?;
        self.group.clear();
        Ok(())
    }

    fn checkpoint(&mut self) -> io::Result<()> {
        if self.window.is_empty() {
            return Ok(());
        }
        let entries = std::mem::take(&mut self.window);
        let d = record::checkpoint_digest(&self.prev, &entries);
        let sig = match &self.opts.key {
            Some(k) => Value::str(sign(k, &record::signed_message(&self.stream_id, &d))?),
            None => Value::Null,
        };
        // The stream header rides along, as MPEG-TS repeats its program tables:
        // a reader that lost the header record, or joined late, has the key and
        // the content type again at the next checkpoint.
        let body = Value::obj(vec![
            ("prev", Value::str(hex(&self.prev))),
            ("entries", Value::Arr(entries.iter().map(|(s, h)| Value::Arr(vec![Value::num(*s), Value::str(hex(h))])).collect())),
            ("digest", Value::str(hex(&d))),
            ("sig", sig),
            ("stream", self.meta.clone()),
        ]);
        self.prev = d;
        self.record(RecordType::Check, body.to_json().as_bytes(), 0, None, 2, false)?;
        self.last_ckpt = Instant::now();
        Ok(())
    }

    /// Close the capture: the last parity, the end record, the last checkpoint.
    pub fn finish(mut self, interrupted: bool) -> io::Result<(W, Written)> {
        self.parity()?;
        let end = Value::obj(vec![
            ("data_records", Value::num(self.data_records)),
            ("bytes", Value::num(self.total)),
            ("sha256", Value::str(hex(&self.payload.clone().finish()))),
            ("interrupted", Value::Bool(interrupted)),
        ]);
        self.record(RecordType::End, end.to_json().as_bytes(), 0, None, 2, true)?;
        self.checkpoint()?;
        let written = Written { data_records: self.data_records, bytes: self.total };
        Ok((self.out, written))
    }
}

/// Record a whole file into a capture, in 64 KiB records, and close it: what
/// `sstr record OUT --input FILE` does, for a caller that has a file rather
/// than a stream -- ytq archiving a download.
pub fn record_file(input: &Path, output: &Path, meta: Value, opts: Options) -> io::Result<Written> {
    let mut inp = std::fs::File::open(input)?;
    let out = io::BufWriter::new(std::fs::File::create(output)?);
    let mut w = Writer::new(out, meta, opts)?;
    let mut buf = vec![0u8; 65536];
    loop {
        let mut n = 0;
        while n < buf.len() {
            match inp.read(&mut buf[n..]) {
                Ok(0) => break,
                Ok(k) => n += k,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
        if n == 0 {
            break;
        }
        let t_us = w.started().elapsed().as_micros() as u64;
        w.data(&buf[..n], t_us)?;
        if n < buf.len() {
            break;
        }
    }
    let (out, written) = w.finish(false)?;
    let file = out.into_inner().map_err(|e| e.into_error())?;
    file.sync_all()?;
    Ok(written)
}

fn random_id() -> io::Result<[u8; 16]> {
    let mut id = [0u8; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut id)?;
    Ok(id)
}

/// The public half of `key`: KEY.pub beside it, or what `ssh-keygen -y` derives.
pub fn public_key(key: &Path) -> io::Result<String> {
    let mut pubpath = key.as_os_str().to_owned();
    pubpath.push(".pub");
    let pubpath = PathBuf::from(pubpath);
    if pubpath.exists() {
        return Ok(std::fs::read_to_string(pubpath)?.trim().to_string());
    }
    let out = Command::new("ssh-keygen").arg("-y").arg("-f").arg(key).output()?;
    if !out.status.success() {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("no public key for {}: {}", key.display(), String::from_utf8_lossy(&out.stderr).trim()),
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// An SSHSIG over `message`, by `ssh-keygen -Y sign`, under the format's namespace.
pub fn sign(key: &Path, message: &[u8]) -> io::Result<String> {
    let mut child = Command::new("ssh-keygen")
        .args(["-q", "-Y", "sign", "-f"])
        .arg(key)
        .args(["-n", SIGNING_NAMESPACE])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child.stdin.take().expect("piped").write_all(message)?;
    let out = child.wait_with_output()?;
    if !out.status.success() {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("ssh-keygen could not sign: {}", String::from_utf8_lossy(&out.stderr).trim()),
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}
