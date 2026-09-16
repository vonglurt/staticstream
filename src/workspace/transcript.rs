// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Paul Richeson

//! The Transcript: ytq's `ytq.log` and sstr's own events in one pane,
//! following as they are written.
//!
//! **Following a log is not reading a file.** `ytq.log` is appended to by
//! every ytq process at once, and past `LOG_MAX` the next `log()` call moves
//! it to `ytq.log.1` with a plain `std::fs::rename` and starts again. A
//! follower that remembers only a byte offset loses exactly the lines either
//! side of that rename: the offset now points into a file nobody is writing
//! to, or past the end of a file that has just begun.
//!
//! So the handle is what is remembered, not the offset alone:
//!
//! 1. **Drain the handle we hold, first, every time.** A rename does not
//!    move an open file -- the descriptor still names the same inode -- so
//!    the lines written before the rename and not yet read are still there
//!    to be read. They are the ones that would otherwise go missing.
//! 2. **Then ask whether the path is still that file**, by device and inode.
//!    A different one means a rotation; a length below where we are reading
//!    means the file was truncated in place. Either way, reopen at nought and
//!    drain that too, in the same poll -- the seam is not a gap in time.
//!
//! **A line is split out of bytes, not out of a string.** Reading fixed
//! blocks and decoding each one would put a replacement character wherever an
//! 8 KiB boundary fell inside a UTF-8 title, and ytq really does write such
//! titles. The partial line is kept as bytes and only a complete line --
//! newline being ASCII, wherever it falls -- is decoded.
//!
//! **A line is kept whole and drawn short.** What is held in memory is the
//! line as ytq wrote it, date, pid and all, because that is what the log
//! says and what a comparison can be anchored to. What is drawn is
//! [`shown`]: the time out of the stamp, and the message. The report's own
//! screen writes `10:03:36 done: ...` and it is right to -- a full stamp is
//! 28 characters, which at 50 columns leaves ten for the message and makes
//! the pane a column of clocks.
//!
//! **sstr's own events are stamped like ytq's**, with the same
//! `%Y-%m-%d %H:%M:%S [pid]`, because the report asks for one pane rather
//! than two interleaved: a reader should not have to know which program
//! wrote a line to read it. What sstr says stays in memory; `ytq.log` is
//! ytq's file and the Workspace does not write to it.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::ytq::sys;

/// Lines kept in memory. The pane shows a handful; the rest is what a
/// Transcript is for -- what went past while you were looking elsewhere.
pub const KEEP: usize = 200;
/// Lines taken from the log at the start, so the pane is not blank on a
/// machine where ytq has been busy all morning.
const SEED: usize = KEEP;
/// How far back the seed reads. A line is short and this is generous; a log
/// at its 4 MiB limit is not read whole to find its last few lines.
const SEED_BYTES: u64 = 64 * 1024;

/// The file being followed, and where in it we have read to.
struct Open {
    file: File,
    dev: u64,
    ino: u64,
    pos: u64,
}

/// The pane's lines, and the log they come from.
pub struct Transcript {
    path: PathBuf,
    open: Option<Open>,
    /// Bytes after the last newline: half a line, waiting for its other half.
    partial: Vec<u8>,
    lines: VecDeque<String>,
}

/// A line's bytes as text. Lossy, because a log is bytes and a pane is text:
/// a truncated write is drawn with a replacement character rather than
/// dropped. The `\r` is for a line that came from a Windows-side tool.
fn decode(line: &[u8]) -> String {
    String::from_utf8_lossy(line).trim_end_matches('\r').to_string()
}

fn open_at(path: &Path, pos: u64) -> Option<Open> {
    let mut file = File::open(path).ok()?;
    let m = file.metadata().ok()?;
    let pos = pos.min(m.len());
    file.seek(SeekFrom::Start(pos)).ok()?;
    Some(Open { file, dev: m.dev(), ino: m.ino(), pos })
}

impl Transcript {
    /// Follow `path`, seeded with the lines already at the end of it.
    pub fn follow(path: &Path) -> Transcript {
        let mut t = Transcript {
            path: path.to_path_buf(),
            open: None,
            partial: Vec::new(),
            lines: VecDeque::new(),
        };
        t.seed();
        t
    }

    /// The last lines of the log as they stand, and the reading position left
    /// at the end of the file so that `poll` carries on from there.
    fn seed(&mut self) {
        let Ok(m) = std::fs::metadata(&self.path) else { return };
        let len = m.len();
        let from = len.saturating_sub(SEED_BYTES);
        let Some(mut o) = open_at(&self.path, from) else { return };
        let mut buf = Vec::new();
        if o.file.read_to_end(&mut buf).is_err() {
            return;
        }
        o.pos = len;
        let mut lines: Vec<&[u8]> = buf.split(|&c| c == b'\n').collect();
        // The last piece is what follows the final newline: nothing, in a log
        // that ends properly. It is not a line, and dropping it here is what
        // leaves `partial` empty and the position at the end.
        lines.pop();
        // The first is half a line when the read did not start at the
        // beginning of the file, and a whole one when it did.
        if from > 0 && !lines.is_empty() {
            lines.remove(0);
        }
        for l in lines.iter().rev().take(SEED).rev() {
            let s = String::from_utf8_lossy(l).trim_end_matches('\r').to_string();
            self.push(s);
        }
        self.open = Some(o);
    }

    /// Read whatever has been written since the last call, across a rotation
    /// if one happened. Cheap when there is nothing: one read returning zero.
    pub fn poll(&mut self) {
        // The handle we hold, first and always: a renamed file is still open.
        self.drain();

        let meta = std::fs::metadata(&self.path).ok();
        let rotated = match (&self.open, &meta) {
            // A different file wearing the same name, or the same file cut
            // back to a length we have already read past.
            (Some(o), Some(m)) => m.dev() != o.dev || m.ino() != o.ino || m.len() < o.pos,
            // No log when we started, and one now: the first ytq to run.
            (None, Some(_)) => true,
            // No log at all, or it has been removed and not replaced.
            (_, None) => false,
        };
        if !rotated {
            return;
        }
        // Half a line left on the old file is all of that line there will
        // ever be -- nothing appends to it now -- so it is a line.
        if !self.partial.is_empty() {
            let s = String::from_utf8_lossy(&self.partial).trim_end_matches('\r').to_string();
            self.partial.clear();
            self.push(s);
        }
        self.open = open_at(&self.path, 0);
        self.drain();
    }

    /// Everything readable on the handle we hold, turned into lines.
    ///
    /// **A block at a time, not the whole of what is waiting.** This runs on
    /// the thread that draws, so the Workspace is frozen for as long as it
    /// takes; and what is waiting can be a great deal. A log at its 4 MiB
    /// limit, a runner that has been downloading all night into a Workspace
    /// only just opened -- the burst is bounded by the log, not by the pane.
    /// Each block is turned into lines and dropped, so this costs 8 KiB
    /// however much there is, and time in proportion to the bytes rather than
    /// to their square.
    fn drain(&mut self) {
        let mut buf = [0u8; 8192];
        loop {
            // The borrow of the handle ends with this match, so the block can
            // then be handed to a method that needs the whole of self.
            let n = match self.open.as_mut() {
                Some(o) => match o.file.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        o.pos += n as u64;
                        n
                    }
                    Err(_) => break,
                },
                None => break,
            };
            self.take(&buf[..n]);
        }
    }

    /// Bytes in, whole lines out; the remainder waits for its other half.
    ///
    /// THE LINES ARE CUT OUT OF THE BLOCK, not out of a buffer the block was
    /// appended to. Appending first and then taking lines off the front means
    /// every line shifts everything behind it: 70,000 lines of a rotated log
    /// cost about 200 GB of copying, which is not slow, it is a Workspace
    /// that has stopped. Only the FIRST line of a block can have a head
    /// waiting in `partial`, and only the last can be left unfinished.
    fn take(&mut self, bytes: &[u8]) {
        let mut rest = bytes;
        while let Some(i) = rest.iter().position(|&c| c == b'\n') {
            let (line, after) = rest.split_at(i);
            let s = if self.partial.is_empty() {
                decode(line)
            } else {
                self.partial.extend_from_slice(line);
                let s = decode(&self.partial);
                self.partial.clear();
                s
            };
            self.push(s);
            rest = &after[1..];
        }
        self.partial.extend_from_slice(rest);
    }

    /// An event of sstr's own, stamped as ytq stamps its lines.
    pub fn say(&mut self, msg: &str) {
        let stamp = format!(
            "{} [{}] ",
            sys::strftime_local("%Y-%m-%d %H:%M:%S", sys::now()),
            std::process::id()
        );
        for l in msg.lines() {
            self.push(format!("{stamp}{l}"));
        }
    }

    fn push(&mut self, line: String) {
        if self.lines.len() == KEEP {
            self.lines.pop_front();
        }
        self.lines.push_back(line);
    }

    /// The last `n` lines, oldest first, which is the order the log has them.
    pub fn tail(&self, n: usize) -> Vec<&str> {
        let from = self.lines.len().saturating_sub(n);
        self.lines.iter().skip(from).map(String::as_str).collect()
    }

    /// How many lines are being kept, for the tests.
    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }
}

/// A line as the pane draws it: the time, and the message after it.
///
/// `2026-09-16 08:02:19 [13613] added https://...` is drawn
/// `08:02:19 added https://...`. The date is the same all day, and the pid
/// belongs to a log several ytq processes share rather than to a pane showing
/// the last three lines -- and between them they are 20 of the 28 characters
/// the stamp costs. At 50 columns that is the difference between ten
/// characters of message and thirty.
///
/// The two pieces are not next to each other in the line, so a pair is
/// returned rather than a slice, and the caller puts them together. Nothing
/// is allocated: this runs on every line of every frame.
///
/// **A line not in that shape is left exactly as it is**, with an empty time.
/// The pane would rather draw something unexpected whole than cut a line that
/// merely looks like it has a stamp.
pub fn shown(line: &str) -> (&str, &str) {
    let b = line.as_bytes();
    // `YYYY-MM-DD HH:MM:SS ` -- checked by shape rather than parsed.
    let stamped = b.len() > 20
        && b[4] == b'-'
        && b[7] == b'-'
        && b[10] == b' '
        && b[13] == b':'
        && b[16] == b':'
        && b[19] == b' '
        && b[..19].iter().enumerate().all(|(i, c)| matches!(i, 4 | 7 | 10 | 13 | 16) || c.is_ascii_digit());
    if !stamped {
        return ("", line);
    }
    // ... then `[pid] `, and the message after it.
    match line[20..].find("] ") {
        Some(i) => (&line[11..19], &line[20 + i + 2..]),
        None => ("", line),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("sstr-transcript-{}-{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn append(p: &Path, text: &str) {
        append_bytes(p, text.as_bytes());
    }

    fn append_bytes(p: &Path, bytes: &[u8]) {
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(p).unwrap();
        f.write_all(bytes).unwrap();
    }

    #[test]
    fn lines_come_out_in_the_order_they_went_in() {
        let d = scratch("order");
        let log = d.join("ytq.log");
        append(&log, "one\ntwo\n");
        let mut t = Transcript::follow(&log);
        assert_eq!(t.tail(9), ["one", "two"], "the log already there is the seed");
        append(&log, "three\n");
        t.poll();
        append(&log, "four\nfive\n");
        t.poll();
        assert_eq!(t.tail(9), ["one", "two", "three", "four", "five"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn half_a_line_waits_for_its_other_half() {
        let d = scratch("partial");
        let log = d.join("ytq.log");
        let mut t = Transcript::follow(&log);
        append(&log, "the beginning of a ");
        t.poll();
        assert!(t.is_empty(), "half a line is not a line yet");
        append(&log, "line\n");
        t.poll();
        assert_eq!(t.tail(9), ["the beginning of a line"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_rotation_loses_nothing_at_the_seam() {
        let d = scratch("seam");
        let log = d.join("ytq.log");
        append(&log, "before\n");
        let mut t = Transcript::follow(&log);

        // THE LINE THAT GETS LOST. It is written to the old file after the
        // last poll and before the rename, so a follower that notices the
        // rotation and simply reopens has already stepped over it.
        append(&log, "the seam\n");
        std::fs::rename(&log, d.join("ytq.log.1")).unwrap();
        append(&log, "after\n");

        t.poll();
        assert_eq!(t.tail(9), ["before", "the seam", "after"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_log_that_appears_later_is_picked_up() {
        let d = scratch("late");
        let log = d.join("ytq.log");
        let mut t = Transcript::follow(&log);
        assert!(t.is_empty(), "there is no log to follow yet");
        append(&log, "the first ytq to run\n");
        t.poll();
        assert_eq!(t.tail(9), ["the first ytq to run"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_log_truncated_in_place_is_read_from_the_start() {
        let d = scratch("truncated");
        let log = d.join("ytq.log");
        append(&log, "a long line that was here before\n");
        let mut t = Transcript::follow(&log);
        std::fs::write(&log, b"short\n").unwrap();
        t.poll();
        assert_eq!(t.tail(9), ["a long line that was here before", "short"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn sstr_says_its_own_events_in_ytqs_shape() {
        let d = scratch("say");
        let mut t = Transcript::follow(&d.join("ytq.log"));
        t.say("sstr-workspace ~/Videos/Archive");
        let line = t.tail(1)[0].to_string();
        // `2026-09-16 07:57:11 [11083] ` -- the stamp ytq's log() writes.
        let stamp = format!(" [{}] ", std::process::id());
        assert!(line.contains(&stamp), "stamped with the pid, got {line:?}");
        assert!(line.ends_with("sstr-workspace ~/Videos/Archive"), "got {line:?}");
        assert_eq!(line.chars().nth(4), Some('-'), "a year first, got {line:?}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_rotated_logs_worth_of_lines_is_read_in_proportion_to_its_bytes() {
        let d = scratch("burst");
        let log = d.join("ytq.log");
        let mut t = Transcript::follow(&log);
        // A log at its 4 MiB limit, arriving between two polls -- which is
        // what a Workspace opened on a busy machine actually reads.
        let mut burst = String::new();
        for i in 0..70_000 {
            burst.push_str(&format!("2026-09-16 08:00:00 [1] line {i}\n"));
        }
        append(&log, &burst);
        let started = std::time::Instant::now();
        t.poll();
        let took = started.elapsed();
        assert_eq!(t.tail(1), ["2026-09-16 08:00:00 [1] line 69999"]);
        assert_eq!(t.len(), KEEP);
        // THE BOUND IS DELIBERATELY LOOSE. It is not measuring how fast this
        // machine is; it is the difference between reading the bytes once and
        // shifting the whole buffer down for every line, which for this many
        // lines is about 200 GB of copying and minutes of it. Ten seconds
        // will not fail on a loaded machine and cannot pass on that.
        assert!(took.as_secs() < 10, "reading a rotated log's worth took {took:?}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_line_is_drawn_as_a_time_and_a_message() {
        let (t, m) = shown("2026-09-16 08:02:19 [13613] added https://example.invalid/one");
        assert_eq!(t, "08:02:19");
        assert_eq!(m, "added https://example.invalid/one");
        // A one-digit pid is as valid as a five-digit one.
        let (t, m) = shown("2026-09-16 08:02:19 [7] queued: x");
        assert_eq!((t, m), ("08:02:19", "queued: x"));
        // A message with `] ` of its own keeps all of it: the FIRST `] `
        // after the stamp ends the pid, and the rest is the message.
        let (t, m) = shown("2026-09-16 08:02:19 [13613] error: [youtube] bad");
        assert_eq!((t, m), ("08:02:19", "error: [youtube] bad"));
    }

    #[test]
    fn a_line_without_a_stamp_is_drawn_whole() {
        for line in [
            "a line from something else entirely",
            "",
            "2026-09-16 08:02:19 no pid here",
            "not-a-date 08:02:19 [1] x",
            "2026-09-16 08:02:19 [13613]",
        ] {
            let (t, m) = shown(line);
            assert_eq!((t, m), ("", line), "left alone: {line:?}");
        }
    }

    #[test]
    fn only_the_last_lines_are_kept() {
        let d = scratch("keep");
        let log = d.join("ytq.log");
        let mut t = Transcript::follow(&log);
        for i in 0..KEEP + 50 {
            append(&log, &format!("line {i}\n"));
        }
        t.poll();
        assert_eq!(t.len(), KEEP, "a Transcript does not grow without bound");
        assert_eq!(t.tail(1), [format!("line {}", KEEP + 49)], "the newest is kept");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_title_split_across_two_reads_is_not_mangled() {
        let d = scratch("utf8");
        let log = d.join("ytq.log");
        let mut t = Transcript::follow(&log);
        // A name ytq really writes, a byte at a time -- raw bytes, because
        // decoding one byte of an accent is exactly the mistake being tested
        // for and the test must not make it before the follower can. No read
        // boundary may put a replacement character inside the accent.
        let msg = "added Café-Über_ünïcode.mp4";
        for b in msg.as_bytes() {
            append_bytes(&log, &[*b]);
            t.poll();
        }
        append(&log, "\n");
        t.poll();
        assert_eq!(t.tail(1), [msg]);
        let _ = std::fs::remove_dir_all(&d);
    }
}
