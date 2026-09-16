// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Paul Richeson

//! The terminal, by hand, for ytq's window: no curses and no crate.
//!
//! As ascitty's `ascitty-tty` does it: raw mode is `stty`, the alternate
//! screen and the cursor are escape sequences, and keys are read on a thread
//! that posts them to a channel. The size is the `TIOCGWINSZ` ioctl. A frame
//! is one styled run of text per row, as the window draws it, and only the
//! rows that changed are written -- what curses' refresh does.

use std::io::{self, Read, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::Mutex;
use std::time::Duration;

use crate::ytq::sys;

/// `stty -g` from before raw mode: what [`restore`] puts back.
static SAVED: Mutex<Option<String>> = Mutex::new(None);

/// Raw mode, the alternate screen and no cursor, until dropped -- or until a
/// panic, which must not leave a shell nobody can type into.
pub struct Term;

impl Term {
    pub fn enter() -> io::Result<Term> {
        let saved = Command::new("stty").arg("-g").stdin(Stdio::inherit()).stderr(Stdio::null()).output()?;
        if !saved.status.success() {
            return Err(io::Error::new(io::ErrorKind::Other, "stty -g failed: stdin is not a terminal"));
        }
        stty(&["raw", "-echo"])?;
        *SAVED.lock().unwrap_or_else(|e| e.into_inner()) = Some(String::from_utf8_lossy(&saved.stdout).trim().to_string());
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore();
            prev(info);
        }));
        write("\x1b[?1049h\x1b[?25l\x1b[2J")?;
        Ok(Term)
    }
}

impl Drop for Term {
    fn drop(&mut self) {
        restore();
    }
}

/// The terminal as it was, without forgetting how to get back.
///
/// [`restore`] is the end of the window and takes the saved settings with it;
/// this is an interruption in the middle of one, for a Service that writes
/// output a person reads. The saved settings stay saved, so the `Term` guard
/// still puts everything right when the window really does end.
pub fn suspend() -> io::Result<()> {
    let saved = SAVED.lock().unwrap_or_else(|e| e.into_inner()).clone();
    write("\x1b[0m\x1b[?25h\x1b[?1049l")?;
    match saved {
        Some(s) => stty(&[&s]).or_else(|_| stty(&["sane"])),
        None => stty(&["sane"]),
    }
}

/// Raw mode again, with whatever a Service wrote still on the screen -- so
/// one key can be read without waiting for a line and an Enter.
pub fn raw_again() -> io::Result<()> {
    stty(&["raw", "-echo"])
}

/// The window again: raw mode and the alternate screen, cleared.
pub fn resume() -> io::Result<()> {
    raw_again()?;
    write("\x1b[?1049h\x1b[?25l\x1b[2J")
}

/// Undo [`Term::enter`]. Safe to call more than once.
pub fn restore() {
    let saved = SAVED.lock().unwrap_or_else(|e| e.into_inner()).take();
    let Some(saved) = saved else { return };
    let _ = write("\x1b[0m\x1b[?25h\x1b[?1049l");
    if stty(&[&saved]).is_err() {
        let _ = stty(&["sane"]);
    }
}

fn stty(args: &[&str]) -> io::Result<()> {
    let st = Command::new("stty").args(args).stdin(Stdio::inherit()).stdout(Stdio::null()).stderr(Stdio::null()).status()?;
    if st.success() {
        Ok(())
    } else {
        Err(io::Error::new(io::ErrorKind::Other, format!("stty {} failed", args.join(" "))))
    }
}

/// Write to the terminal at once.
pub fn write(s: &str) -> io::Result<()> {
    let mut out = io::stdout();
    out.write_all(s.as_bytes())?;
    out.flush()
}

/// (rows, columns).
pub fn size() -> (usize, usize) {
    sys::window_size(1).or_else(|| sys::window_size(0)).unwrap_or((24, 80))
}

/// A key, decoded from what the terminal sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Up,
    Down,
    /// Left and right arrive from phase 3: the Browser walks columns with
    /// them. ytq's window ignores them, as it ignored them when they decoded
    /// as `Other`.
    Left,
    Right,
    Delete,
    Enter,
    Backspace,
    /// Ctrl+U: the line typed so far goes.
    KillLine,
    Escape,
    /// Ctrl+C, which raw mode delivers as a byte; or the terminal gone.
    Interrupt,
    Other,
}

/// Keys, read on their own thread so the window can redraw while waiting.
pub struct Keys {
    rx: Receiver<Key>,
}

impl Keys {
    pub fn start() -> Keys {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut stdin = io::stdin();
            let mut pending: Vec<u8> = Vec::new();
            let mut buf = [0u8; 256];
            loop {
                let n = match stdin.read(&mut buf) {
                    Ok(0) => {
                        let _ = tx.send(Key::Interrupt);
                        return;
                    }
                    Ok(n) => n,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => {
                        let _ = tx.send(Key::Interrupt);
                        return;
                    }
                };
                pending.extend_from_slice(&buf[..n]);
                let mut i = 0;
                while i < pending.len() {
                    let (key, used) = decode(&pending[i..]);
                    if used == 0 {
                        // An Escape with nothing after it in the same read is the
                        // Escape key; any other sequence cut short waits for the
                        // rest, unless it is too long to be one.
                        if pending.len() - i == 1 && pending[i] == 0x1b {
                            let _ = tx.send(Key::Escape);
                            i += 1;
                        } else if pending.len() - i > 16 {
                            i += 1;
                            continue;
                        }
                        break;
                    }
                    i += used;
                    if let Some(k) = key {
                        if tx.send(k).is_err() {
                            return;
                        }
                    }
                }
                pending.drain(..i);
            }
        });
        Keys { rx }
    }

    /// The next key, or None when `timeout` passes first.
    pub fn next(&self, timeout: Duration) -> Option<Key> {
        match self.rx.recv_timeout(timeout) {
            Ok(k) => Some(k),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => Some(Key::Interrupt),
        }
    }

    /// The next key, however long it takes.
    pub fn wait(&self) -> Key {
        self.rx.recv().unwrap_or(Key::Interrupt)
    }
}

/// One key from the front of `b`, and the bytes it took; 0 bytes when `b`
/// holds only the start of one.
pub fn decode(b: &[u8]) -> (Option<Key>, usize) {
    match b {
        [] | [0x1b] => (None, 0),
        [0x1b, b'[', ..] | [0x1b, b'O', ..] => {
            // ESC [ parameters final, or ESC O final: the whole sequence goes,
            // so none of it is left to arrive as letters -- which are the keys.
            let mut end = 2;
            while end < b.len() && !(0x40..=0x7e).contains(&b[end]) {
                end += 1;
            }
            if end >= b.len() {
                return (None, 0);
            }
            let key = match (b[1], b[end], &b[2..end]) {
                (_, b'A', _) => Key::Up,
                (_, b'B', _) => Key::Down,
                (_, b'C', _) => Key::Right,
                (_, b'D', _) => Key::Left,
                (b'[', b'~', b"3") => Key::Delete,
                _ => Key::Other,
            };
            (Some(key), end + 1)
        }
        [0x1b, ..] => (Some(Key::Escape), 1),
        [b'\r', ..] | [b'\n', ..] => (Some(Key::Enter), 1),
        [0x7f, ..] | [0x08, ..] => (Some(Key::Backspace), 1),
        [0x15, ..] => (Some(Key::KillLine), 1),
        [0x03, ..] => (Some(Key::Interrupt), 1),
        [c, ..] if *c < 0x20 => (Some(Key::Other), 1),
        [c, ..] => {
            let len = match *c {
                0x00..=0x7f => 1,
                0xc0..=0xdf => 2,
                0xe0..=0xef => 3,
                0xf0..=0xf7 => 4,
                _ => return (Some(Key::Other), 1),
            };
            if b.len() < len {
                return (None, 0);
            }
            match std::str::from_utf8(&b[..len]) {
                Ok(s) => (s.chars().next().map(Key::Char), len),
                Err(_) => (Some(Key::Other), 1),
            }
        }
    }
}

/// How a row is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub dim: bool,
    pub reverse: bool,
    /// An ANSI colour on the default background: 1 red, 2 green, 3 yellow, 6 cyan.
    pub color: Option<u8>,
}

impl Style {
    pub fn sgr(&self) -> String {
        let mut p: Vec<String> = Vec::new();
        if self.bold {
            p.push("1".into());
        }
        if self.dim {
            p.push("2".into());
        }
        if self.reverse {
            p.push("7".into());
        }
        if let Some(c) = self.color {
            p.push(format!("3{c}"));
        }
        if p.is_empty() {
            String::new()
        } else {
            format!("\x1b[{}m", p.join(";"))
        }
    }
}

/// What curses shows for a character it cannot show as itself: ^X for a
/// control character, ^? for DEL. A title can hold anything, and an escape
/// sequence in one must not reach the terminal.
pub fn printable(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\u{0}'..='\u{1f}' => {
                out.push('^');
                out.push((c as u8 + 64) as char);
            }
            '\u{7f}' => out.push_str("^?"),
            '\u{80}'..='\u{9f}' => out.push('?'),
            _ => out.push(c),
        }
    }
    out
}

/// A row: its column, its text and its style. None is a blank row.
pub type Seg = (usize, String, Style);

/// One row: the pieces written on it, left to right. Empty is a blank row.
///
/// This was one piece per row until phase 3. ytq's window never needed two --
/// every line it draws is one full-width string in one style -- but the
/// Workspace's Browser puts three Miller columns on a row and reverses the
/// selection inside one of them, which one style per row cannot say. A row of
/// exactly one piece is written exactly as it was before, so ytq's screens are
/// unchanged, cell for cell.
pub type Row = Vec<Seg>;

/// One screen's worth of rows.
pub struct Frame {
    pub rows: Vec<Row>,
}

impl Frame {
    pub fn new(height: usize) -> Frame {
        Frame { rows: vec![Vec::new(); height] }
    }

    /// curses' addstr, for the one string a row gets: off the screen, nothing.
    pub fn put(&mut self, y: i64, x: usize, text: &str, style: Style) {
        self.puts(y, vec![(x, text.to_string(), style)]);
    }

    /// Several pieces on one row, left to right. The caller owns the order and
    /// the gaps: a piece that overlaps an earlier one simply writes over it.
    pub fn puts(&mut self, y: i64, segs: Vec<Seg>) {
        if y >= 0 && (y as usize) < self.rows.len() {
            self.rows[y as usize] = segs;
        }
    }
}

/// Python's `s[:n]`, for the n >= 0 a window asks for.
pub fn cut(s: &str, n: i64) -> String {
    s.chars().take(n.max(0) as usize).collect()
}

/// Python's `s.ljust(n)`.
pub fn ljust(s: &str, n: i64) -> String {
    let len = s.chars().count() as i64;
    if len >= n {
        s.to_string()
    } else {
        format!("{s}{}", " ".repeat((n - len) as usize))
    }
}

/// What is on the terminal, so a frame writes only the rows that changed.
#[derive(Default)]
pub struct Screen {
    last: Vec<Row>,
    size: (usize, usize),
}

impl Screen {
    /// Draw everything again next time.
    pub fn invalidate(&mut self) {
        self.size = (0, 0);
    }

    pub fn present(&mut self, frame: &Frame, size: (usize, usize)) -> io::Result<()> {
        let full = size != self.size;
        let mut out = String::new();
        if full {
            out.push_str("\x1b[0m\x1b[2J");
        }
        for (y, row) in frame.rows.iter().enumerate() {
            if !full && self.last.get(y) == Some(row) {
                continue;
            }
            out.push_str(&format!("\x1b[{};1H\x1b[0m\x1b[2K", y + 1));
            for (x, text, style) in row {
                let mut shown: String = printable(text).chars().take(size.1.saturating_sub(*x)).collect();
                if !style.reverse {
                    // Blanks that look blank go unwritten, as curses leaves them:
                    // bold, dim or a foreground colour do not show on a space,
                    // and the row was cleared already. Reverse does show.
                    let kept = shown.trim_end_matches(' ').len();
                    shown.truncate(kept);
                }
                out.push_str(&format!("\x1b[{};{}H{}{}\x1b[0m", y + 1, x + 1, style.sgr(), shown));
            }
        }
        if !out.is_empty() {
            write(&out)?;
        }
        self.last = frame.rows.clone();
        self.size = size;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_decode_as_curses_hands_them_to_the_window() {
        assert_eq!(decode(b"j"), (Some(Key::Char('j')), 1));
        assert_eq!(decode(b"J"), (Some(Key::Char('J')), 1));
        assert_eq!(decode(b"\x1b[B"), (Some(Key::Down), 3));
        assert_eq!(decode(b"\x1b[C"), (Some(Key::Right), 3));
        assert_eq!(decode(b"\x1b[D"), (Some(Key::Left), 3));
        assert_eq!(decode(b"\x1bOD"), (Some(Key::Left), 3));
        assert_eq!(decode(b"\x1bOA"), (Some(Key::Up), 3));
        assert_eq!(decode(b"\x1b[3~"), (Some(Key::Delete), 4));
        // Ctrl+Right is Right, because only the final byte is read and the
        // parameters are not -- which is how Ctrl+Up has always been Up here.
        // A sequence whose final byte is none of the four is still Other.
        assert_eq!(decode(b"\x1b[1;5C"), (Some(Key::Right), 6));
        assert_eq!(decode(b"\x1b[Z"), (Some(Key::Other), 3));
        assert_eq!(decode(b"\x1b[5~"), (Some(Key::Other), 4));
        assert_eq!(decode(b"\x1b["), (None, 0));
        assert_eq!(decode(b"\x1b"), (None, 0));
        assert_eq!(decode(b"\x1bq"), (Some(Key::Escape), 1));
        assert_eq!(decode(b"\r"), (Some(Key::Enter), 1));
        assert_eq!(decode(b"\x7f"), (Some(Key::Backspace), 1));
        assert_eq!(decode(b"\x03"), (Some(Key::Interrupt), 1));
        assert_eq!(decode("é".as_bytes()), (Some(Key::Char('é')), 2));
        assert_eq!(decode(&"é".as_bytes()[..1]), (None, 0));
    }

    #[test]
    fn nothing_unprintable_reaches_the_terminal() {
        assert_eq!(printable("a\tb\x1b[31m\x7f"), "a^Ib^[[31m^?");
        assert_eq!(printable("Fake Title — café"), "Fake Title — café");
    }

    #[test]
    fn cut_and_ljust_count_characters_as_python_does() {
        assert_eq!(cut("Fake Title — café", 12), "Fake Title —");
        assert_eq!(ljust("é", 3), "é  ");
        assert_eq!(ljust("long", 2), "long");
        assert_eq!(format!("{:<13}|", "done"), "done         |");
    }

    #[test]
    fn a_row_of_one_piece_is_written_as_it_always_was() {
        // The bar phase 3 must not move: one piece per row renders unchanged.
        let mut f = Frame::new(2);
        f.put(0, 3, "hello", Style { bold: true, ..Style::default() });
        assert_eq!(f.rows[0], vec![(3, "hello".to_string(), Style { bold: true, ..Style::default() })]);
        assert!(f.rows[1].is_empty());
    }

    #[test]
    fn styles_are_one_sgr() {
        assert_eq!(Style::default().sgr(), "");
        assert_eq!(Style { reverse: true, bold: true, ..Style::default() }.sgr(), "\x1b[1;7m");
        assert_eq!(Style { bold: true, color: Some(6), ..Style::default() }.sgr(), "\x1b[1;36m");
    }
}
