// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Paul Richeson

//! sstr-workspace -- the terminal Workspace, in NeXTSTEP's and Smalltalk's
//! words: a Browser of folders, a Shelf, an Inspector, a Transcript, and
//! Services sent to the selection.
//!
//! Step 3a drew the frame and the **Browser**, step 3b the **Inspector**,
//! step 3c the **Transcript**, step 3d **Services**, and step 3e the
//! **Shelf** and the **Queue**. That is the whole of the report's screen.
//!
//! **A verb is always something a person could have typed.** A Service is a
//! command line, written into the Transcript and then handed to `sh -c`
//! exactly as written -- see [`services`].
//!
//! **The Inspector does not read a capture itself.** It calls
//! [`crate::format::reader::inspect`] and draws the lines `sstr verify` would
//! have printed, so the pane and the command say the same thing in the same
//! words.
//!
//! **The Workspace is a way of looking.** The Browser reads the disk and
//! nothing else: there is no database, no index and nothing to keep in step
//! with the folders. That is what stops this being a media library, and it is
//! why a column is exactly what `ls` would have shown -- which is how the
//! check tests it, phase 3 having no Python prototype to compare against.
//!
//! The terminal layer is ytq's ([`crate::ytq::term`]), as step 2d wrote it:
//! `stty` for raw mode, escape sequences for the screen, `TIOCGWINSZ` for its
//! size, and keys read on a thread. No curses, and no second terminal layer.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};
use std::time::Duration;

use crate::ytq::term::{self, cut, ljust, Frame, Key, Keys, Screen, Seg, Style, Term};
use crate::ytq::{live, queue, runner, settings, sys, Paths};

pub mod services;
pub mod transcript;
pub use services::{How, Service};
pub use transcript::Transcript;

/// The nouns and the one plural of verbs, and where each word comes from.
pub const VOCABULARY: [(&str, &str, &str); 7] = [
    ("Workspace", "the whole window", "NeXTSTEP's Workspace Manager"),
    ("Browser", "Miller columns over any folder, from the archive folder", "the File Viewer's Browser view"),
    ("Shelf", "items picked for later: files to stream, URLs to queue", "the File Viewer's Shelf"),
    ("Inspector", "the selection's header, notes, license and verify result", "Tools > Inspector"),
    ("Transcript", "the running log: ytq.log and sstr's events", "Smalltalk-80's Transcript"),
    ("Services", "verbs sent to the selection: Play, Serve, Verify, Export MP4, Open as Text", "NeXTSTEP Services"),
    ("Queue", "ytq's queue as one more object to browse and inspect", "ytq"),
];

/// How many Miller columns are on screen at once. The third pane of the
/// window is the Inspector, as the report's screen has it.
const MILLER: usize = 2;
/// The panes across the window: the Miller columns, then the Inspector.
const PANES: i64 = 3;
/// The narrowest a column may be before the Browser stops drawing them.
const MIN_COL: i64 = 8;

/// How many rows the Transcript's band gets in a window `h` rows tall.
///
/// **The report draws the Transcript as one row.** It is drawn that way
/// because nothing is happening on that screen: a single `done:` line is the
/// whole of what there was to say. A download says a good deal more than
/// that, and Smalltalk's Transcript is a pane that scrolls, so the band is
/// three rows where there is room for them -- enough to see a line, the one
/// before it and the one after, which is what makes "in the order the log has
/// them" something a person can read rather than infer. A short window keeps
/// the report's single row: the Browser is what a 12-row terminal is for.
pub fn transcript_rows(h: i64) -> i64 {
    if h >= 20 {
        3
    } else {
        1
    }
}

const USAGE: &str = "\
sstr-workspace -- the Workspace: a Browser over folders of streams

  sstr-workspace [DIR]     browse DIR, or ARCHIVE_DIR, or the home

  up down / k j   move          right / l / Enter   open the folder
  left / h        back out      q / Esc             leave
  Space           pick the selection up onto the Shelf, or put it down
  Q               ytq's queue, as one more column to browse

The Inspector shows the selection: for a capture, what `sstr verify` says
about it, in the same words.

Services are sent to the selection with one key, and the foot of the window
says which the selection takes:

  p Play      P Paced     s Serve     v Verify
  x Export    t Text      a Armor
  r Retry     f Forget                        (on a queue entry)

With anything on the Shelf, a Service goes to everything on it, one command
line each, and the foot of the window says so.

EVERY SERVICE IS A COMMAND LINE, written into the Transcript before it runs
and then run exactly as written -- so it can be read, copied, and typed
again. A Service that prints takes the terminal until a key is pressed.

ARCHIVE_DIR comes from ~/.config/copal/media.conf, then ~/.config/ytq/config,
as ytq reads them; PLAYER (mpv) and SERVE (127.0.0.1:8080) come from the same
files.
";

/// One row of a column: a file, a folder, or an entry in ytq's queue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
    /// The URL of the queue entry this row stands for, when the column is the
    /// Queue; empty otherwise.
    ///
    /// **A row stands for an entry rather than holding one.** `queue.json` is
    /// a file several ytq processes write, and a copy taken when the column
    /// was built would be out of date before anyone looked at it. The URL is
    /// the name of a thing; the thing is read again when it is wanted.
    pub url: String,
}

/// What a column is over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Over {
    /// A folder. A column over one is exactly what `ls` would have shown --
    /// the bar that replaces phase 3's crosscheck.
    Folder,
    /// ytq's queue, which is one file rather than a folder.
    Queue,
}

/// What is selected, which is not always a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Selection {
    File(PathBuf),
    /// An entry in ytq's queue, by URL.
    Queued(String),
    Nothing,
}

impl Selection {
    pub fn file(&self) -> Option<&Path> {
        match self {
            Selection::File(p) => Some(p),
            _ => None,
        }
    }
}

/// One column: a folder's entries, or the queue's.
pub struct Column {
    pub dir: PathBuf,
    pub over: Over,
    pub entries: Vec<Entry>,
    /// Which entry is selected. Meaningless, and 0, when the column is empty.
    pub sel: usize,
    /// The first entry drawn, so a long column scrolls.
    pub top: usize,
}

impl Column {
    pub fn open(dir: &Path) -> Column {
        Column { dir: dir.to_path_buf(), over: Over::Folder, entries: read_dir_sorted(dir), sel: 0, top: 0 }
    }

    /// The Queue as a column: ytq's queue.json, in the order it has them.
    pub fn queue(paths: &Paths) -> Column {
        Column { dir: paths.queue.clone(), over: Over::Queue, entries: queue_rows(paths), sel: 0, top: 0 }
    }

    pub fn selected(&self) -> Option<&Entry> {
        self.entries.get(self.sel)
    }

    /// What the selected row stands for.
    pub fn selection(&self) -> Selection {
        match self.selected() {
            None => Selection::Nothing,
            Some(e) if self.over == Over::Queue => Selection::Queued(e.url.clone()),
            Some(e) => Selection::File(self.dir.join(&e.name)),
        }
    }

    /// Read the queue again, keeping the selection on the same entry.
    ///
    /// The queue is written by whichever ytq is downloading, so a column over
    /// it is redrawn from the file rather than remembered -- and the entry
    /// the person is looking at is found again by its URL, not by where it
    /// was in the list, because entries come and go around it.
    pub fn refresh_queue(&mut self, paths: &Paths) {
        if self.over != Over::Queue {
            return;
        }
        let was = self.selected().map(|e| e.url.clone());
        self.entries = queue_rows(paths);
        self.sel = was
            .and_then(|u| self.entries.iter().position(|e| e.url == u))
            .unwrap_or_else(|| self.sel.min(self.entries.len().saturating_sub(1)));
    }
}

/// The queue as rows, in the order `queue.json` has them -- which is the
/// order ytq added them, and the order `ytq list` prints them.
///
/// A row is cut where it does not fit, not elided: `elide` keeps a name's
/// extension because that is what tells two captures apart, and a title has
/// no extension to keep.
pub fn queue_rows(paths: &Paths) -> Vec<Entry> {
    queue::snapshot(paths)
        .iter()
        .map(|it| Entry {
            name: format!("{:<11} {}", queue::status(it), queue::title_or_url(it)),
            is_dir: false,
            url: queue::text(it, "url").to_string(),
        })
        .collect()
}

/// A folder's entries, in the order `ls` gives them: by name, bytewise, with
/// the dot files left out. `ls` sorts by the locale's collation; the Browser
/// and the check that compares it to `ls` both take the C locale's, which is
/// the byte order, so the two cannot disagree about an accent.
pub fn read_dir_sorted(dir: &Path) -> Vec<Entry> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<Entry> = rd
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                return None;
            }
            // A symlink to a folder is a folder here, as `ls -p` marks it.
            let is_dir = std::fs::metadata(e.path()).map(|m| m.is_dir()).unwrap_or(false);
            Some(Entry { name, is_dir, url: String::new() })
        })
        .collect();
    out.sort_by(|a, b| a.name.as_bytes().cmp(b.name.as_bytes()));
    out
}

/// The Browser: the columns from the root folder down to the selection.
pub struct Browser {
    pub root: PathBuf,
    pub cols: Vec<Column>,
}

impl Browser {
    pub fn open(root: &Path) -> Browser {
        Browser { root: root.to_path_buf(), cols: vec![Column::open(root)] }
    }

    /// The column the keys act on: the deepest one.
    fn last(&self) -> &Column {
        self.cols.last().expect("a Browser always has its root column")
    }

    fn last_mut(&mut self) -> &mut Column {
        self.cols.last_mut().expect("a Browser always has its root column")
    }

    /// The folder being shown, for the title bar.
    pub fn cwd(&self) -> &Path {
        &self.last().dir
    }

    /// What is selected right now.
    pub fn selection(&self) -> Selection {
        self.last().selection()
    }

    /// Is the deepest column the Queue?
    pub fn over_queue(&self) -> bool {
        self.last().over == Over::Queue
    }

    /// Open ytq's queue as a column to the right, unless it is already the
    /// one being looked at. The report has the Queue as one more object to
    /// browse; here it is opened with a key rather than drawn as a row among
    /// a folder's entries, so that a column over a folder stays exactly what
    /// `ls` would have shown.
    pub fn open_queue(&mut self, paths: &Paths) {
        if self.over_queue() {
            return;
        }
        self.cols.push(Column::queue(paths));
    }

    /// Read the queue again, if that is what is being looked at.
    pub fn refresh_queue(&mut self, paths: &Paths) {
        self.last_mut().refresh_queue(paths);
    }

    pub fn move_by(&mut self, delta: i64) {
        let col = self.last_mut();
        if col.entries.is_empty() {
            return;
        }
        let n = col.entries.len() as i64;
        col.sel = (col.sel as i64 + delta).clamp(0, n - 1) as usize;
    }

    /// Open the selected folder in a column to the right. A file is not opened
    /// here: Services (3d) is what a file is sent to, and neither is a queue
    /// entry, which is not a folder of anything.
    pub fn descend(&mut self) {
        if self.over_queue() {
            return;
        }
        if let Some(e) = self.last().selected() {
            if e.is_dir {
                let path = self.last().dir.join(&e.name);
                self.cols.push(Column::open(&path));
            }
        }
    }

    /// Back out to the column on the left. The root column stays.
    pub fn ascend(&mut self) {
        if self.cols.len() > 1 {
            self.cols.pop();
        }
    }

    /// The columns on screen: the deepest `n`, so the Browser scrolls left as
    /// it goes deeper, which is what the File Viewer does.
    pub fn visible(&self, n: usize) -> &[Column] {
        let from = self.cols.len().saturating_sub(n);
        &self.cols[from..]
    }
}

/// A name cut to fit a column, the middle going and the extension staying.
///
/// The report's screen has `Trader-The_setup_I...sstr`, not a name cut at its
/// end, and the reason shows on ytq's own names: at 100 columns
/// `jawed-Me_at_the_zoo_jNQXAC9IVRw.sstr` and the `.txt` beside it are the
/// same 29 characters from the left, so a capture and its transcript draw
/// identically. What tells them apart is the end, so the end is what is kept.
///
/// `..` and not the ellipsis the report draws, for the reason the folder
/// marker is `>`: U+2026 is ambiguous-width, and a column is compared with
/// `ls` character by character. An ambiguous glyph would have the check
/// measuring the terminal's font instead of the Browser.
pub fn elide(name: &str, room: i64) -> String {
    if room <= 0 {
        return String::new();
    }
    let n = name.chars().count() as i64;
    if n <= room {
        return name.to_string();
    }
    // The extension is worth keeping when there is one and it is short; a
    // name that is one long run with a dot in it is not improved by it.
    if let Some((_, ext)) = name.rsplit_once('.') {
        let e = ext.chars().count() as i64;
        if e > 0 && e <= 8 && room > e + 3 {
            let head: String = name.chars().take((room - 2 - e) as usize).collect();
            return format!("{head}..{ext}");
        }
    }
    cut(name, room)
}

/// `~/x` for a path under the home, as ytq's window writes them.
fn tilde(p: &Path, home: &Path) -> String {
    let (s, h) = (p.to_string_lossy().into_owned(), home.to_string_lossy().into_owned());
    match (!h.is_empty() && s.starts_with(&h)).then(|| s[h.len()..].to_string()) {
        Some(rest) if rest.is_empty() => "~".into(),
        Some(rest) if rest.starts_with('/') => format!("~{rest}"),
        _ => s,
    }
}

/// Keep the selection on screen, given how many rows the columns have.
fn scroll(col: &mut Column, rows: i64) {
    let rows = rows.max(1) as usize;
    if col.sel < col.top {
        col.top = col.sel;
    } else if col.sel >= col.top + rows {
        col.top = col.sel + 1 - rows;
    }
}

/// Everything a frame is drawn from besides the Browser and the window's
/// size. A struct rather than eight arguments in a row, which is how many it
/// had got to by the Shelf.
pub struct View<'a> {
    pub home: &'a Path,
    pub transcript: &'a Transcript,
    /// The verbs the keys will send, which is not always the selection's --
    /// see [`targets`].
    pub services: &'a [Service],
    pub paths: &'a Paths,
    pub shelf: &'a [Selection],
}

/// What the Shelf row says.
pub fn shelf_line(shelf: &[Selection]) -> String {
    if shelf.is_empty() {
        return "Shelf: nothing picked yet -- Space picks the selection".into();
    }
    let mut out = String::from("Shelf:");
    for s in shelf {
        out.push_str(&format!(" [{}]", shelf_name(s)));
    }
    out
}

/// What one item on the Shelf is called there: a file by its name, a queue
/// entry by the short form of its URL that ytq's own log uses.
pub fn shelf_name(sel: &Selection) -> String {
    match sel {
        Selection::File(p) => p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        Selection::Queued(u) => crate::ytq::urls::short(u),
        Selection::Nothing => String::new(),
    }
}

/// What the keys send a verb to: everything on the Shelf, or the selection
/// when the Shelf is empty.
///
/// **That is what a Shelf is for.** The report has it hold "items picked for
/// later" -- later being when a verb is sent -- so picking four captures and
/// pressing `v` verifies four captures, each as its own command line in the
/// Transcript. Nothing is picked, so nothing has changed for anyone who does
/// not use it: the selection is the target.
pub fn targets(shelf: &[Selection], sel: &Selection) -> Vec<Selection> {
    if shelf.is_empty() {
        vec![sel.clone()]
    } else {
        shelf.to_vec()
    }
}

/// The verbs any of the targets takes, each key named once, in the order the
/// report's Services line lists them.
pub fn services_for_all(targets: &[Selection], s: &std::collections::BTreeMap<String, String>) -> Vec<Service> {
    let mut out: Vec<Service> = Vec::new();
    for t in targets {
        for sv in services::services_for(t, s) {
            if !out.iter().any(|o| o.key == sv.key) {
                out.push(sv);
            }
        }
    }
    out
}

/// One screen. The rows the Browser owns are drawn column by column, so a row
/// carries a piece from each -- which is why a row holds several pieces.
pub fn frame_of(b: &Browser, (h, w): (i64, i64), colors: bool, v: &View) -> Frame {
    let (home, t, svcs, paths) = (v.home, v.transcript, v.services, v.paths);
    let mut f = Frame::new(h.max(0) as usize);
    let color = |c: u8| colors.then_some(c);
    let dim = Style { dim: true, ..Style::default() };

    let head = if b.over_queue() {
        " Workspace -- the Queue".to_string()
    } else {
        format!(" Workspace -- {}", tilde(b.cwd(), home))
    };
    f.put(0, 0, &ljust(&cut(&head, w), w), Style { reverse: true, bold: true, ..Style::default() });
    f.put(1, 0, &cut(&format!(" {}", shelf_line(v.shelf)), w), dim);
    f.put(2, 0, &"-".repeat(w.max(0) as usize), dim);

    // The rows the columns get: everything between the two rules, less the
    // Transcript's band and the Services line below them.
    let tr = transcript_rows(h);
    let (top, bottom) = (3i64, h - 3 - tr);
    let rows = (bottom - top + 1).max(0);
    let cw = w / PANES;

    if rows > 0 && cw >= MIN_COL {
        let cols = b.visible(MILLER);
        let deepest = cols.len().saturating_sub(1);
        let ins_x = (PANES - 1) * cw;
        let ins_w = w - ins_x;
        // The Inspector reads the selection once, not once a row.
        let ins = inspect_lines(&b.selection(), paths, ins_w);
        for r in 0..rows {
            let mut segs: Vec<Seg> = Vec::new();
            for (i, col) in cols.iter().enumerate() {
                let x0 = i as i64 * cw;
                // Every column ends in a rule, the last one parting it from
                // the Inspector, so the panes are told apart without box art.
                let width = cw - 1;
                if let Some(e) = col.entries.get(col.top + r as usize) {
                    let marker = if e.is_dir { ">" } else { " " };
                    // A file's name loses its middle so the extension stays;
                    // a queue row is a status and a title, with no extension
                    // to keep, so it is simply cut.
                    let shown = if col.over == Over::Queue {
                        cut(&e.name, width - 3)
                    } else {
                        elide(&e.name, width - 3)
                    };
                    let text = format!("{} {}", ljust(&shown, width - 3), marker);
                    let selected = col.top + r as usize == col.sel;
                    let style = if selected && i == deepest {
                        Style { reverse: true, ..Style::default() }
                    } else if selected {
                        // A parent column's selection is the path you came by.
                        Style { reverse: true, dim: true, ..Style::default() }
                    } else if e.is_dir {
                        Style { bold: true, color: color(6), ..Style::default() }
                    } else {
                        Style::default()
                    };
                    segs.push((x0 as usize, ljust(&text, width), style));
                } else if r == 0 && col.entries.is_empty() {
                    segs.push((x0 as usize, ljust(&cut("(empty)", width), width), dim));
                }
                segs.push(((x0 + cw - 1) as usize, "|".into(), dim));
            }
            // The Inspector: the first line is the name, the rest what the
            // selection says about itself.
            if let Some(line) = ins.get(r as usize) {
                // The first line is the selection's name, so it loses its
                // middle like a column entry does -- cut at the end, a long
                // `.sstr` heading reads `.ss`, which is a different extension
                // as far as a reader can tell. The rest are the lines
                // `sstr verify` printed: prose, cut where the pane ends.
                let (style, shown) = if r == 0 {
                    (Style { bold: true, ..Style::default() }, elide(line, ins_w))
                } else {
                    (Style::default(), cut(line, ins_w))
                };
                segs.push((ins_x as usize, ljust(&shown, ins_w), style));
            }
            f.puts(top + r, segs);
        }
    } else if rows > 0 {
        f.put(top, 0, &cut("(the window is too narrow for the Browser)", w), dim);
    }

    f.put(h - 2 - tr, 0, &"-".repeat(w.max(0) as usize), dim);
    // The band: the log's last lines, oldest at the top and the newest on the
    // row above Services, which is the order the log has them. Fewer lines
    // than rows pads at the top, so the newest line does not move about as
    // the pane fills. The label sits on the first row, as the report's screen
    // labels its one row, and the rest are indented under it so a column of
    // timestamps lines up.
    //
    // A LINE IS DRAWN AS `transcript::shown` DRAWS IT: the time, then the
    // message, where the log has a date, a time and a pid. The report's
    // screen writes `10:03:36 done: ...` and it is right to. The full stamp
    // is 28 characters, and at 50 columns -- with the label -- that leaves
    // ten for the message and makes the pane a column of clocks. What is
    // KEPT is still the line as ytq wrote it, so the comparison has something
    // outside the program to be anchored to.
    let lines = t.tail(tr.max(0) as usize);
    let pad = (tr.max(0) as usize).saturating_sub(lines.len());
    for r in 0..tr {
        let i = r as usize;
        let text = if i < pad {
            String::new()
        } else {
            let (time, msg) = transcript::shown(lines[i - pad]);
            if time.is_empty() { msg.to_string() } else { format!("{time} {msg}") }
        };
        let label = if r == 0 { "Transcript " } else { "           " };
        f.put(h - 1 - tr + r, 0, &cut(&format!(" {label}{text}"), w), dim);
    }
    f.put(h - 1, 0, &cut(&format!(" {}", services::line_for(svcs, !v.shelf.is_empty())), w), dim);
    f
}

/// Thousands separated by commas, as the report's screen writes a size.
fn thousands(n: u64) -> String {
    let d = n.to_string();
    let mut out = String::new();
    for (i, c) in d.chars().enumerate() {
        if i > 0 && (d.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// What the Inspector shows for the selection.
///
/// A capture's lines are `sstr verify`'s own, through
/// [`crate::format::reader::inspect`], so the pane and the command cannot
/// drift. Everything else is what the file says about itself: a folder its
/// count, a text its first lines, and a download the notes ytq wrote beside
/// it.
pub fn inspect_lines(sel: &Selection, paths: &Paths, w: i64) -> Vec<String> {
    let p = match sel {
        Selection::Nothing => {
            return vec!["Inspector".into(), String::new(), "(nothing selected)".into()];
        }
        // A QUEUE ENTRY IS SHOWN AS ytq SHOWS IT. `live_view` is the renderer
        // ytq's own window and `ytq status` draw the live record with, so the
        // Inspector borrows it rather than reading the same fields again --
        // the same reason the Inspector draws `sstr verify`'s lines for a
        // capture instead of re-deriving them from the header.
        Selection::Queued(url) => {
            let items = queue::snapshot(paths);
            let Some(it) = items.iter().find(|i| queue::text(i, "url") == url) else {
                return vec![crate::ytq::urls::short(url), String::new(), "(no longer in the queue)".into()];
            };
            let mut v = vec![queue::title_or_url(it).to_string(), String::new()];
            v.push(format!("status       {}", queue::status(it)));
            for key in ["quality", "file"] {
                let val = queue::text(it, key);
                if !val.is_empty() {
                    v.push(format!("{key:<12} {val}"));
                }
            }
            // attempts is a number, and queue::text is for strings: it would
            // quietly render as nothing, which is how a field goes missing
            // without anyone noticing.
            if let Some(n) = it.get("attempts").filter(|a| live::truthy(Some(a))) {
                v.push(format!("{:<12} {}", "attempts", live::py_str(n)));
            }
            v.push(format!("url          {url}"));
            let err = queue::text(it, "error");
            if !err.is_empty() {
                v.push(String::new());
                v.push(format!("error        {err}"));
            }
            if queue::status(it) == "downloading" {
                v.push(String::new());
                v.extend(live::live_view(it, w.max(20), sys::now()));
            }
            return v;
        }
        Selection::File(p) => p.clone(),
    };
    let p = p.as_path();
    let name = p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let mut v = vec![name, String::new()];

    if p.is_dir() {
        let n = read_dir_sorted(p).len();
        v.push(format!("folder, {n} item{}", if n == 1 { "" } else { "s" }));
        return v;
    }

    let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    let ext = p.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();

    if ext == "sstr" {
        match crate::format::reader::inspect(p) {
            Ok((status, summary)) => {
                v.extend(summary.lines().map(String::from));
                v.push(String::new());
                v.push(match status {
                    0 => "verify: everything checked out".into(),
                    _ => "verify: SOMETHING IS WRONG -- sstr verify says what".to_string(),
                });
            }
            Err(e) => v.push(format!("cannot read it: {e}")),
        }
        return v;
    }

    v.push(format!("{} bytes", thousands(size)));
    if ext == "txt" {
        v.push(String::new());
        if let Ok(text) = std::fs::read_to_string(p) {
            v.extend(text.lines().take(20).map(String::from));
        }
        return v;
    }
    // ytq writes Author-Title_ID.txt beside Author-Title_ID.mp4 or .sstr.
    let notes = p.with_extension("txt");
    if notes != *p {
        if let Ok(text) = std::fs::read_to_string(&notes) {
            v.push(String::new());
            v.push(format!("notes: {}", notes.file_name().unwrap_or_default().to_string_lossy()));
            v.extend(text.lines().take(16).map(String::from));
        }
    }
    v
}

/// How a finished command is reported: `ok`, or what went wrong.
fn outcome(st: &std::io::Result<std::process::ExitStatus>) -> String {
    match st {
        Ok(s) if s.success() => "ok".into(),
        Ok(s) => match s.code() {
            Some(c) => format!("exit {c}"),
            None => "killed by a signal".into(),
        },
        Err(e) => format!("could not run: {e}"),
    }
}

/// Send a Service to the selection.
///
/// **THE LINE IS SAID BEFORE IT RUNS.** Not after, and not only when it
/// succeeds: the Transcript is a record of what was asked, so a Service that
/// fails still leaves behind the line a person can retype to watch it fail
/// themselves. And what runs is the string that was said -- one string, built
/// once, printed and then executed -- so there is no second construction for
/// the first to drift from.
fn send(sv: &Service, t: &mut Transcript, keys: &Keys, screen: &mut Screen, running: &mut Vec<(String, Child)>) {
    t.say(&sv.line);
    let sh = |line: &str| {
        let mut c = Command::new("sh");
        c.arg("-c").arg(line).stdin(Stdio::null());
        c
    };
    match sv.how {
        How::Terminal => {
            // THE SERVICE GETS THE TERMINAL, NOT A COPY OF IT. Paced text
            // arriving as it was recorded is the output of a program writing
            // to a terminal; caught and replayed into a pane it would be a
            // transcript of a teletype rather than one.
            //
            // Its stdin is closed. The window's key reader owns the real
            // stdin for as long as the window lives and cannot be paused, so
            // handing the same terminal to a second reader would be a race
            // over every byte typed. That is why a text capture is shown and
            // waited on here rather than handed to a pager -- see
            // docs/phase-3.md.
            let _ = term::suspend();
            // THE LINE IS SHOWN ON THE TERMINAL TOO, above its own output,
            // the way a shell shows what was typed above what it printed.
            // Without it the screen holds the last Service's output and this
            // one's with nothing to say where one ends and the other begins.
            println!("{}", sv.line);
            let _ = std::io::stdout().flush();
            let st = sh(&sv.line).status();
            let said = outcome(&st);
            println!("\r\n-- {} {} -- press a key to return to the Workspace --", sv.name, said);
            let _ = std::io::stdout().flush();
            let _ = term::raw_again();
            // Long, but not forever: a Workspace nobody is at comes back by
            // itself rather than sitting outside its own screen.
            let _ = keys.next(Duration::from_secs(600));
            let _ = term::resume();
            screen.invalidate();
            // WHAT THE SERVICE WROTE TO ytq.log COMES FIRST. The Transcript
            // cannot follow the log while a Service holds the screen, so
            // without this the Workspace's note that Retry finished lands
            // above ytq's own line saying what it did -- with an earlier
            // timestamp than the line above it, which reads as a fault in
            // the clock rather than in the order.
            t.poll();
            t.say(&format!("{} {}", sv.name, said));
        }
        How::Background => {
            match sh(&sv.line).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
                Ok(child) => {
                    t.say(&format!("{} started, pid {}", sv.name, child.id()));
                    running.push((sv.name.clone(), child));
                }
                Err(e) => t.say(&format!("{} could not start: {e}", sv.name)),
            }
        }
    }
}

/// Say so when something left running has finished, and stop holding it.
///
/// Without this a served capture that a player has finished with stays a
/// zombie for as long as the Workspace is open, and nothing on screen ever
/// says the server stopped.
fn reap(running: &mut Vec<(String, Child)>, t: &mut Transcript) {
    let mut i = 0;
    while i < running.len() {
        match running[i].1.try_wait() {
            Ok(Some(status)) => {
                let (name, _) = running.remove(i);
                t.say(&format!("{name} {}", outcome(&Ok(status))));
            }
            Ok(None) => i += 1,
            Err(_) => {
                running.remove(i);
            }
        }
    }
}

/// The Workspace itself.
pub fn main(argv: &[String]) -> ExitCode {
    match argv.first().map(String::as_str) {
        Some("-V" | "--version") => {
            println!("sstr-workspace {} (Static Stream format v{})", env!("CARGO_PKG_VERSION"), crate::format::FORMAT_VERSION);
            return ExitCode::SUCCESS;
        }
        Some("-h" | "--help") => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        _ => {}
    }

    let Some((paths, home, s)) = settings::from_env() else {
        eprintln!("sstr-workspace: no HOME");
        return ExitCode::FAILURE;
    };

    // Where to start: what was asked for, else ARCHIVE_DIR as ytq resolves it,
    // else the home. The Transcript says when it was not what was meant.
    let (root, note) = match argv.first().filter(|a| !a.starts_with('-')) {
        Some(a) => (PathBuf::from(a), String::new()),
        None => {
            let d = PathBuf::from(runner::archive_dir_of(&s));
            if d.is_dir() {
                (d, String::new())
            } else {
                (home.clone(), format!("no archive folder at {} -- showing the home", tilde(&d, &home)))
            }
        }
    };
    if !root.is_dir() {
        eprintln!("sstr-workspace: not a folder: {}", root.display());
        return ExitCode::FAILURE;
    }

    let colors = std::env::var("TERM").map_or(false, |t| !t.is_empty() && t != "dumb");
    let mut b = Browser::open(&root);

    // The Transcript follows ytq's log, and carries sstr's own events beside
    // it. Its first line is the command line that opened this Workspace --
    // which is the rule Services will keep in 3d, kept here from the start:
    // everything in this pane is something a person could have typed.
    let mut t = Transcript::follow(&paths.log);
    t.say(&format!("sstr-workspace {}", tilde(&root, &home)));
    if !note.is_empty() {
        t.say(&note);
    }

    let _term = match Term::enter() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("sstr-workspace: no terminal: {e}");
            return ExitCode::FAILURE;
        }
    };
    let keys = Keys::start();
    let mut screen = Screen::default();
    let mut last_size = None;
    // What Services left running, so their ends are said and reaped.
    let mut running: Vec<(String, Child)> = Vec::new();
    // THE SHELF IS KEPT IN MEMORY, for as long as the window is open. A file
    // of it would be the one thing this Workspace has refused all the way
    // through: something to keep in step with the folders, going stale the
    // moment a picked capture is moved or renamed elsewhere. The Browser
    // reads the disk; the Shelf holds what was picked while looking.
    let mut shelf: Vec<Selection> = Vec::new();

    loop {
        // Before the frame is built, so a line written while the last one was
        // on screen is on this one. Nothing to read costs a single read.
        t.poll();
        reap(&mut running, &mut t);
        // The queue is a file other ytq processes write, so a column over it
        // is read again rather than remembered.
        b.refresh_queue(&paths);
        let (h, w) = term::size();
        if last_size.map_or(false, |s| s != (h, w)) {
            screen.invalidate();
        }
        last_size = Some((h, w));
        let rows = (h as i64 - 3 - transcript_rows(h as i64)) - 3 + 1;
        scroll(b.last_mut(), rows);
        // The selection's Services: what the foot of the window offers, and
        // what a key sends. Built once a frame, so the two cannot disagree
        // about what `p` means.
        let sel = b.selection();
        // What the keys will send a verb to, and the verbs they send: the
        // Shelf when anything is on it, else the selection.
        let to = targets(&shelf, &sel);
        let svcs = services_for_all(&to, &s);
        let view = View { home: &home, transcript: &t, services: &svcs, paths: &paths, shelf: &shelf };
        let frame = frame_of(&b, (h as i64, w as i64), colors, &view);
        let _ = screen.present(&frame, (h, w));

        let Some(k) = keys.next(Duration::from_millis(500)) else { continue };
        match k {
            Key::Char('q') | Key::Escape | Key::Interrupt => break,
            Key::Down | Key::Char('j') => b.move_by(1),
            Key::Up | Key::Char('k') => b.move_by(-1),
            Key::Right | Key::Char('l') | Key::Enter => b.descend(),
            Key::Left | Key::Char('h') | Key::Backspace => b.ascend(),
            // The Queue, as one more object to browse. Shift-Q, because q
            // leaves and a queue is not worth losing a window over.
            Key::Char('Q') => b.open_queue(&paths),
            // Space picks the selection up, or puts it back down.
            Key::Char(' ') => {
                if !matches!(sel, Selection::Nothing) {
                    match shelf.iter().position(|x| *x == sel) {
                        Some(i) => {
                            shelf.remove(i);
                            t.say(&format!("{} off the Shelf", shelf_name(&sel)));
                        }
                        None => {
                            t.say(&format!("{} on the Shelf", shelf_name(&sel)));
                            shelf.push(sel.clone());
                        }
                    }
                }
            }
            // A Service, sent to everything the keys are aimed at -- each as
            // its own command line, because that is what a Service is. The
            // keys that move come first, so no Service can take one of them.
            Key::Char(c) => {
                for target in &to {
                    let one = services::services_for(target, &s);
                    if let Some(sv) = services::by_key(&one, c) {
                        let sv = sv.clone();
                        send(&sv, &mut t, &keys, &mut screen, &mut running);
                    }
                }
            }
            _ => {}
        }
    }
    term::restore();
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Transcript with no log to follow: the frame tests are about the
    /// frame, and an empty band is a band all the same.
    fn quiet() -> Transcript {
        Transcript::follow(Path::new("/nonexistent/ytq.log"))
    }

    /// A View over a throwaway home, with nothing shelved: the frame tests
    /// are about the frame.
    fn view_of<'a>(d: &'a Path, t: &'a Transcript, paths: &'a Paths) -> View<'a> {
        View { home: d, transcript: t, services: &[], paths, shelf: &[] }
    }

    /// ytq's paths under a throwaway home, so nothing here can see the real
    /// queue however hard it tries.
    fn paths_in(home: &Path) -> Paths {
        Paths::from_lookup(|k| (k == "HOME").then(|| home.to_path_buf())).expect("a home")
    }

    /// A folder of the test's own.
    ///
    /// ONE FOLDER FOR ALL OF THEM WAS A RACE. cargo runs these in parallel
    /// and each one ends by removing what it made, so a test could have its
    /// fixture deleted from under it by a test that had just finished --
    /// which showed up as `a_column_is_what_ls_would_have_shown` failing
    /// about one run in ten. The name makes each one distinct.
    fn fixture(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("sstr-ws-test-{}-{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("Archive").join("deep")).unwrap();
        std::fs::write(d.join("b.sstr"), b"x").unwrap();
        std::fs::write(d.join("a.txt"), b"x").unwrap();
        std::fs::write(d.join(".hidden"), b"x").unwrap();
        d
    }

    #[test]
    fn a_column_is_what_ls_would_have_shown() {
        let d = fixture("a_column_is_what_ls_would_have_shown");
        let entries = read_dir_sorted(&d);
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        // By name, bytewise; the dot file left out; folders not hoisted.
        assert_eq!(names, vec!["Archive", "a.txt", "b.sstr"]);
        assert!(entries[0].is_dir);
        assert!(!entries[1].is_dir);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn moving_stays_inside_the_folder() {
        let d = fixture("moving_stays_inside_the_folder");
        let mut b = Browser::open(&d);
        b.move_by(-1);
        assert_eq!(b.cols[0].sel, 0, "up from the top stays at the top");
        b.move_by(99);
        assert_eq!(b.cols[0].sel, 2, "down past the end stays at the end");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_folder_opens_a_column_and_a_file_does_not() {
        let d = fixture("a_folder_opens_a_column_and_a_file_does_not");
        let mut b = Browser::open(&d);
        b.descend();
        assert_eq!(b.cols.len(), 2, "Archive is a folder");
        assert_eq!(b.cwd().file_name().unwrap(), "Archive");
        b.descend();
        assert_eq!(b.cols.len(), 3, "deep is a folder too");
        b.ascend();
        b.ascend();
        assert_eq!(b.cols.len(), 1);
        b.ascend();
        assert_eq!(b.cols.len(), 1, "the root column stays");
        b.move_by(1);
        b.descend();
        assert_eq!(b.cols.len(), 1, "a.txt is a file: Services, not a column");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_inspector_says_what_a_folder_and_a_file_are() {
        let d = fixture("the_inspector_says_what_a_folder_and_a_file_are");
        let lines = inspect_lines(&Selection::File(d.join("Archive")), &paths_in(&d), 40);
        assert_eq!(lines[0], "Archive");
        assert_eq!(lines[2], "folder, 1 item");

        let lines = inspect_lines(&Selection::File(d.join("a.txt")), &paths_in(&d), 40);
        assert_eq!(lines[0], "a.txt");
        assert_eq!(lines[2], "1 bytes");
        assert!(lines.contains(&"x".to_string()), "a text shows its first lines");

        assert_eq!(inspect_lines(&Selection::Nothing, &paths_in(&d), 40)[2], "(nothing selected)");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_long_name_keeps_what_tells_it_apart() {
        // The case from the screen: the capture and its transcript, at the
        // room a 100-column window gives a column.
        let a = "jawed-Me_at_the_zoo_jNQXAC9IVRw.sstr";
        let b = "jawed-Me_at_the_zoo_jNQXAC9IVRw.txt";
        assert_eq!(elide(a, 29), "jawed-Me_at_the_zoo_jNQ..sstr");
        assert_eq!(elide(b, 29), "jawed-Me_at_the_zoo_jNQX..txt");
        assert_ne!(elide(a, 29), elide(b, 29), "a capture and its transcript must not draw alike");
        assert_eq!(elide(a, 29).chars().count(), 29);
        assert_eq!(elide(b, 29).chars().count(), 29);

        // Short enough is left alone.
        assert_eq!(elide("a.txt", 29), "a.txt");
        assert_eq!(elide("Zeta-report.sstr", 16), "Zeta-report.sstr");
        assert_eq!(elide("Zeta-report.sstr", 12), "Zeta-r..sstr");

        // No usable extension, or no room for one: cut the end, as before.
        assert_eq!(elide("a_name_with_no_extension_at_all", 10), "a_name_wit");
        assert_eq!(elide("x.averylongextension", 10), "x.averylon");
        assert_eq!(elide("name.sstr", 6), "name.s");
        assert_eq!(elide("anything", 0), "");

        // Characters, not bytes.
        assert_eq!(elide("café-and-more.txt", 12).chars().count(), 12);
    }

    #[test]
    fn the_inspectors_heading_keeps_its_extension() {
        let d = fixture("the_heading_keeps_its_extension");
        let long = d.join("jawed-Me_at_the_zoo_jNQXAC9IVRw.sstr");
        std::fs::write(&long, b"not really a capture").unwrap();
        let mut b = Browser::open(&d);
        // Put the selection on the long name.
        while b.selection().file().map(|p| p != long).unwrap_or(false) {
            let before = b.cols[0].sel;
            b.move_by(1);
            if b.cols[0].sel == before {
                break;
            }
        }
        let f = frame_of(&b, (24, 100), false, &view_of(&d, &quiet(), &paths_in(&d)));
        let ins_x = 2 * (100 / 3);
        let heading = f.rows[3]
            .iter()
            .find(|(x, _, _)| *x as i64 == ins_x)
            .map(|(_, s, _)| s.trim_end().to_string())
            .unwrap_or_default();
        assert!(heading.ends_with("sstr"), "the heading keeps its extension, got {heading:?}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_size_is_written_as_the_report_writes_it() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_000), "1,000");
        assert_eq!(thousands(692_231), "692,231");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }

    #[test]
    fn nothing_is_drawn_outside_the_frame() {
        let d = fixture("nothing_is_drawn_outside_the_frame");
        let b = Browser::open(&d);
        for (h, w) in [(24i64, 80i64), (12, 50), (30, 110), (8, 20)] {
            let f = frame_of(&b, (h, w), true, &view_of(&d, &quiet(), &paths_in(&d)));
            assert_eq!(f.rows.len(), h as usize, "a frame is exactly the window's rows");
            for row in &f.rows {
                for (x, text, _) in row {
                    let end = *x as i64 + text.chars().count() as i64;
                    assert!(end <= w, "a piece at {x} runs {end} past the {w} the window has");
                }
            }
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_shelf_says_what_is_on_it() {
        assert!(shelf_line(&[]).contains("nothing picked yet"));
        let shelf = vec![
            Selection::File(PathBuf::from("/x/Videos/Archive/capture.sstr")),
            Selection::Queued("https://www.youtube.com/watch?v=jNQXAC9IVRw".into()),
        ];
        // A file by its name, a queue entry by the short form ytq's own log
        // uses -- neither by a path nobody can read at a glance.
        assert_eq!(shelf_line(&shelf), "Shelf: [capture.sstr] [jNQXAC9IVRw]");
    }

    #[test]
    fn the_keys_point_at_the_shelf_when_there_is_one() {
        let sel = Selection::File(PathBuf::from("/x/a.sstr"));
        let shelved = Selection::File(PathBuf::from("/x/b.sstr"));
        assert_eq!(targets(&[], &sel), vec![sel.clone()], "an empty Shelf means the selection");
        assert_eq!(
            targets(std::slice::from_ref(&shelved), &sel),
            vec![shelved],
            "anything on the Shelf, and that is what the keys mean"
        );
    }

    /// A key is offered when ANY of the targets takes it, and named once.
    /// Otherwise a Shelf holding a capture and a transcript would offer
    /// either every verb twice or only the verbs they share, and neither is
    /// what pressing the key does.
    #[test]
    fn the_services_line_is_the_union_of_what_the_targets_take() {
        let d = fixture("the_services_line_is_the_union");
        let cap = d.join("b.sstr");
        let txt = d.join("a.txt");
        let s = std::collections::BTreeMap::new();
        let both = [Selection::File(cap.clone()), Selection::File(txt.clone())];
        let keys: Vec<char> = services_for_all(&both, &s).iter().map(|x| x.key).collect();
        assert!(keys.contains(&'v'), "the capture's Verify is offered");
        assert!(keys.contains(&'t'), "the transcript's Text is offered");
        let mut once = keys.clone();
        once.sort_unstable();
        once.dedup();
        assert_eq!(once.len(), keys.len(), "no key is named twice: {keys:?}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_queue_is_a_column_of_what_the_queue_holds() {
        let d = fixture("the_queue_is_a_column");
        let paths = paths_in(&d);
        std::fs::create_dir_all(paths.queue.parent().unwrap()).unwrap();
        std::fs::write(
            &paths.queue,
            r#"[{"url": "https://www.youtube.com/watch?v=jNQXAC9IVRw", "title": "Me at the zoo", "status": "failed", "added": 1.0},
                {"url": "https://www.youtube.com/watch?v=SWHZolxKdVU", "title": "", "status": "queued", "added": 2.0}]"#,
        )
        .unwrap();
        let rows = queue_rows(&paths);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "failed      Me at the zoo");
        // No title yet, so the row says the URL, as `ytq list` does.
        assert!(rows[1].name.starts_with("queued      https://"), "got {:?}", rows[1].name);
        // The row stands for the entry by URL, which is what a Service needs.
        assert_eq!(rows[0].url, "https://www.youtube.com/watch?v=jNQXAC9IVRw");

        let mut b = Browser::open(&d);
        b.open_queue(&paths);
        assert!(b.over_queue());
        assert_eq!(b.selection(), Selection::Queued("https://www.youtube.com/watch?v=jNQXAC9IVRw".into()));
        b.open_queue(&paths);
        assert_eq!(b.cols.len(), 2, "the Queue does not open on top of itself");
        b.descend();
        assert_eq!(b.cols.len(), 2, "a queue entry is not a folder");
        b.ascend();
        assert!(!b.over_queue(), "and backing out leaves it");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A queue entry's verbs are ytq's own commands, so that the Workspace,
    /// the window and a shell cannot leave three different queues.
    #[test]
    fn a_queue_entry_is_sent_ytqs_own_commands() {
        let s = std::collections::BTreeMap::new();
        let url = "https://www.youtube.com/watch?v=jNQXAC9IVRw";
        let v = services::services_for(&Selection::Queued(url.into()), &s);
        assert_eq!(v.iter().map(|x| x.key).collect::<Vec<_>>(), ['r', 'f']);
        assert_eq!(v[0].line, format!("ytq retry '{url}'"));
        assert_eq!(v[1].line, format!("ytq forget '{url}'"));
    }

    #[test]
    fn the_deepest_column_holds_the_selection() {
        let d = fixture("the_deepest_column_holds_the_selection");
        let mut b = Browser::open(&d);
        b.descend();
        assert_eq!(b.visible(MILLER).len(), 2);
        assert_eq!(b.selection().file().unwrap().file_name().unwrap(), "deep");
        let _ = std::fs::remove_dir_all(&d);
    }
}
