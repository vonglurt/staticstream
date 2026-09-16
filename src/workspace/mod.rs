// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Paul Richeson

//! sstr-workspace -- the terminal Workspace, in NeXTSTEP's and Smalltalk's
//! words: a Browser of folders, a Shelf, an Inspector, a Transcript, and
//! Services sent to the selection.
//!
//! Step 3a drew the frame and the **Browser**; step 3b adds the
//! **Inspector**. The Transcript (3c), Services (3d) and the Shelf and Queue
//! (3e) have their places on the screen and say which step fills them, so the
//! window is honest about what it does not do yet.
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

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use crate::ytq::term::{self, cut, ljust, Frame, Key, Keys, Screen, Seg, Style, Term};
use crate::ytq::{runner, settings};

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

const USAGE: &str = "\
sstr-workspace -- the Workspace: a Browser over folders of streams

  sstr-workspace [DIR]     browse DIR, or ARCHIVE_DIR, or the home

  up down / k j   move          right / l / Enter   open the folder
  left / h        back out      q / Esc             leave

The Inspector shows the selection: for a capture, what `sstr verify` says
about it, in the same words.

ARCHIVE_DIR comes from ~/.config/copal/media.conf, then ~/.config/ytq/config,
as ytq reads them. Every Service will also be a command line, shown in the
Transcript before it runs.
";

/// One entry in a folder, as `ls` would list it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
}

/// One folder's worth of entries: a Miller column.
pub struct Column {
    pub dir: PathBuf,
    pub entries: Vec<Entry>,
    /// Which entry is selected. Meaningless, and 0, when the folder is empty.
    pub sel: usize,
    /// The first entry drawn, so a long folder scrolls.
    pub top: usize,
}

impl Column {
    pub fn open(dir: &Path) -> Column {
        Column { dir: dir.to_path_buf(), entries: read_dir_sorted(dir), sel: 0, top: 0 }
    }

    pub fn selected(&self) -> Option<&Entry> {
        self.entries.get(self.sel)
    }

    /// The selection's full path, if there is a selection.
    pub fn path(&self) -> Option<PathBuf> {
        self.selected().map(|e| self.dir.join(&e.name))
    }
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
            Some(Entry { name, is_dir })
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

    /// What is selected right now, if anything.
    pub fn selection(&self) -> Option<PathBuf> {
        self.last().path()
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
    /// here: Services (3d) is what a file is sent to.
    pub fn descend(&mut self) {
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

/// One screen. The rows the Browser owns are drawn column by column, so a row
/// carries a piece from each -- which is why a row holds several pieces.
pub fn frame_of(b: &Browser, (h, w): (i64, i64), colors: bool, home: &Path, note: &str) -> Frame {
    let mut f = Frame::new(h.max(0) as usize);
    let color = |c: u8| colors.then_some(c);
    let dim = Style { dim: true, ..Style::default() };

    let head = format!(" Workspace -- {}", tilde(b.cwd(), home));
    f.put(0, 0, &ljust(&cut(&head, w), w), Style { reverse: true, bold: true, ..Style::default() });
    f.put(1, 0, &cut(" Shelf: nothing picked yet (3e)", w), dim);
    f.put(2, 0, &"-".repeat(w.max(0) as usize), dim);

    // The rows the columns get: everything between the two rules.
    let (top, bottom) = (3i64, h - 4);
    let rows = (bottom - top + 1).max(0);
    let cw = w / PANES;

    if rows > 0 && cw >= MIN_COL {
        let cols = b.visible(MILLER);
        let deepest = cols.len().saturating_sub(1);
        // The Inspector reads the selection once, not once a row.
        let ins = inspect_lines(b.selection().as_deref());
        let ins_x = (PANES - 1) * cw;
        let ins_w = w - ins_x;
        for r in 0..rows {
            let mut segs: Vec<Seg> = Vec::new();
            for (i, col) in cols.iter().enumerate() {
                let x0 = i as i64 * cw;
                // Every column ends in a rule, the last one parting it from
                // the Inspector, so the panes are told apart without box art.
                let width = cw - 1;
                if let Some(e) = col.entries.get(col.top + r as usize) {
                    let marker = if e.is_dir { ">" } else { " " };
                    let text = format!("{} {}", ljust(&elide(&e.name, width - 3), width - 3), marker);
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

    f.put(h - 3, 0, &"-".repeat(w.max(0) as usize), dim);
    let transcript = if note.is_empty() { "Transcript  (3c)".to_string() } else { format!("Transcript  {note}") };
    f.put(h - 2, 0, &cut(&format!(" {transcript}"), w), dim);
    f.put(h - 1, 0, &cut(" Services (3d):  up/down move   right open   left back   q leave", w), dim);
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
pub fn inspect_lines(sel: Option<&Path>) -> Vec<String> {
    let Some(p) = sel else {
        return vec!["Inspector".into(), String::new(), "(nothing selected)".into()];
    };
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

    let Some((_paths, home, s)) = settings::from_env() else {
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

    loop {
        let (h, w) = term::size();
        if last_size.map_or(false, |s| s != (h, w)) {
            screen.invalidate();
        }
        last_size = Some((h, w));
        let rows = (h as i64 - 4) - 3 + 1;
        scroll(b.last_mut(), rows);
        let frame = frame_of(&b, (h as i64, w as i64), colors, &home, &note);
        let _ = screen.present(&frame, (h, w));

        let Some(k) = keys.next(Duration::from_millis(500)) else { continue };
        match k {
            Key::Char('q') | Key::Escape | Key::Interrupt => break,
            Key::Down | Key::Char('j') => b.move_by(1),
            Key::Up | Key::Char('k') => b.move_by(-1),
            Key::Right | Key::Char('l') | Key::Enter => b.descend(),
            Key::Left | Key::Char('h') | Key::Backspace => b.ascend(),
            _ => {}
        }
    }
    term::restore();
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> PathBuf {
        let d = std::env::temp_dir().join(format!("sstr-ws-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("Archive").join("deep")).unwrap();
        std::fs::write(d.join("b.sstr"), b"x").unwrap();
        std::fs::write(d.join("a.txt"), b"x").unwrap();
        std::fs::write(d.join(".hidden"), b"x").unwrap();
        d
    }

    #[test]
    fn a_column_is_what_ls_would_have_shown() {
        let d = fixture();
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
        let d = fixture();
        let mut b = Browser::open(&d);
        b.move_by(-1);
        assert_eq!(b.cols[0].sel, 0, "up from the top stays at the top");
        b.move_by(99);
        assert_eq!(b.cols[0].sel, 2, "down past the end stays at the end");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_folder_opens_a_column_and_a_file_does_not() {
        let d = fixture();
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
        let d = fixture();
        let lines = inspect_lines(Some(&d.join("Archive")));
        assert_eq!(lines[0], "Archive");
        assert_eq!(lines[2], "folder, 1 item");

        let lines = inspect_lines(Some(&d.join("a.txt")));
        assert_eq!(lines[0], "a.txt");
        assert_eq!(lines[2], "1 bytes");
        assert!(lines.contains(&"x".to_string()), "a text shows its first lines");

        assert_eq!(inspect_lines(None)[2], "(nothing selected)");
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
        let d = fixture();
        let long = d.join("jawed-Me_at_the_zoo_jNQXAC9IVRw.sstr");
        std::fs::write(&long, b"not really a capture").unwrap();
        let mut b = Browser::open(&d);
        // Put the selection on the long name.
        while b.selection().map(|p| p != long).unwrap_or(false) {
            let before = b.cols[0].sel;
            b.move_by(1);
            if b.cols[0].sel == before {
                break;
            }
        }
        let f = frame_of(&b, (24, 100), false, &d, "");
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
        let d = fixture();
        let b = Browser::open(&d);
        for (h, w) in [(24i64, 80i64), (12, 50), (30, 110), (8, 20)] {
            let f = frame_of(&b, (h, w), true, &d, "");
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
    fn the_deepest_column_holds_the_selection() {
        let d = fixture();
        let mut b = Browser::open(&d);
        b.descend();
        assert_eq!(b.visible(MILLER).len(), 2);
        assert_eq!(b.selection().unwrap().file_name().unwrap(), "deep");
        let _ = std::fs::remove_dir_all(&d);
    }
}
