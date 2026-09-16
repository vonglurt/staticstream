// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Paul Richeson

//! Services: verbs sent to the selection, in NeXTSTEP's sense.
//!
//! **A verb is always something a person could have typed.** That is the
//! report's rule and it is the whole design here. A Service is not a function
//! the Workspace calls with the selection as an argument; it is a **command
//! line**, built from the selection, written into the Transcript, and then
//! handed to `sh -c` exactly as written. Play is `sstr play cap.sstr`. The
//! Workspace is a way of sending that message with one key instead of forty
//! characters, and nothing else.
//!
//! Three things follow from it, and they are the point:
//!
//! - **The line is printed before it runs, not after.** A Service that fails
//!   still leaves the line that a person can retype to watch it fail for
//!   themselves. A record of what was asked is worth more than a record of
//!   what happened, because only one of them can be acted on.
//! - **What runs is the string that was printed**, not an argv built twice.
//!   There is no second construction to drift from the first.
//! - **The line stands on its own.** Absolute paths, quoted for a shell, no
//!   reliance on the Workspace's own directory -- so it means the same thing
//!   pasted into another terminal tomorrow, which is what the check does with
//!   it.
//!
//! What a Service needs from the terminal differs, and [`How`] says which:
//! text to read takes the screen, a file or a report goes to the Transcript,
//! and a server is left running.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::format::reader;

/// How a Service's output reaches the person who asked for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum How {
    /// It prints. The Workspace gives the terminal back, the command writes
    /// to it as it would have from a shell, and a key returns.
    ///
    /// **EVERY SERVICE THAT PRINTS IS ONE OF THESE**, including the ones
    /// whose real work is a file. Verify and Export were sent to the
    /// Transcript first, on the reasoning that a report belongs in a log;
    /// each of them writes eleven lines, the band is three, and what the
    /// eleven pushed out of it was the command line -- the one line the band
    /// exists to show. The Transcript keeps the record, which is the command
    /// line and how it went. The output goes where output goes.
    Terminal,
    /// It does not finish on its own -- a server waiting for players. Start
    /// it, say the pid, and leave it.
    Background,
}

/// A verb, and the command line it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Service {
    /// The key that sends it.
    pub key: char,
    /// What it is called on the Services line and in the Transcript. A
    /// String because Export says what it will write: `Export MP4` for a
    /// video, `Export txt` for a transcript.
    pub name: String,
    /// The command line, ready for `sh -c` and for a person to retype.
    pub line: String,
    pub how: How,
}

/// `mpv`, or PLAYER from media.conf. The report's table has this one read by
/// the Workspace alone.
pub fn player_of(s: &BTreeMap<String, String>) -> String {
    match s.get("PLAYER").map(|p| p.trim()).filter(|p| !p.is_empty()) {
        Some(p) => p.to_string(),
        None => "mpv".into(),
    }
}

/// `127.0.0.1:8080`, or SERVE from media.conf. Loopback by default on
/// purpose: a capture is served to a player on this machine, and anything
/// else is a decision someone has to write down.
pub fn serve_of(s: &BTreeMap<String, String>) -> String {
    match s.get("SERVE").map(|p| p.trim()).filter(|p| !p.is_empty()) {
        Some(p) => p.to_string(),
        None => "127.0.0.1:8080".into(),
    }
}

/// A word as a shell needs it written.
///
/// The bare characters are the ones no shell does anything with; everything
/// else is single-quoted, and a single quote inside is the usual
/// `'\''`. ytq's names hold spaces, brackets and apostrophes -- a video
/// called `Don't Look Up` is not hypothetical -- and the printed line has to
/// survive being pasted into a terminal, or it is not a command line.
pub fn quote(s: &str) -> String {
    let plain = !s.is_empty()
        && s.bytes().all(|c| c.is_ascii_alphanumeric() || b"_./-:@+,=".contains(&c));
    if plain {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// The content type a capture says it holds, if it says.
pub fn content_type(path: &Path) -> Option<String> {
    let m = reader::read_meta(path)?;
    let v = m.get("content_type")?;
    v.as_str().map(|s| s.to_string()).filter(|s| !s.is_empty())
}

/// What a payload of this type should be called once it is out of the
/// capture. Only the types the project actually makes are named; anything
/// else is `bin`, which is honest about not knowing.
pub fn extension_for(content_type: Option<&str>) -> &'static str {
    match content_type.unwrap_or("") {
        "video/mp4" => "mp4",
        "video/mp2t" => "ts",
        "audio/mpeg" => "mp3",
        "audio/mp4" => "m4a",
        t if t.starts_with("text/") => "txt",
        "application/json" => "json",
        _ => "bin",
    }
}

/// A name nothing is using yet: `cap.mp4`, else `cap-1.mp4`, `cap-2.mp4`.
///
/// **ONE KEY MUST NOT DESTROY A FILE.** Export is a keypress, and a text
/// capture's payload wants to be called `cap.txt` -- which is exactly what
/// ytq calls the transcript sitting beside `cap.sstr`. A person typing
/// `sstr play cap.sstr -o cap.txt` can see what they are about to write over;
/// someone pressing `x` cannot. So the line names a file that does not exist
/// yet, and the Transcript shows which one before it runs.
pub fn free_name(base: &Path) -> PathBuf {
    if !base.exists() {
        return base.to_path_buf();
    }
    let dir = base.parent().unwrap_or_else(|| Path::new("."));
    let stem = base.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = base.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
    for n in 1..1000 {
        let name = if ext.is_empty() { format!("{stem}-{n}") } else { format!("{stem}-{n}.{ext}") };
        let c = dir.join(name);
        if !c.exists() {
            return c;
        }
    }
    // A thousand of them is not a folder Export is going to improve.
    base.to_path_buf()
}

/// Is this a payload a person reads rather than watches?
fn is_text(content_type: Option<&str>) -> bool {
    match content_type {
        Some(t) => t.starts_with("text/") || t == "application/json",
        None => false,
    }
}

/// A file's extension, lowercased.
fn ext_of(path: &Path) -> String {
    path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

/// The Services the selection can be sent, in the order the report's Services
/// line lists them.
///
/// **A folder gets none.** The message a folder understands is Open, and the
/// Browser already sends it with Right.
pub fn services_for(sel: Option<&Path>, s: &BTreeMap<String, String>) -> Vec<Service> {
    let Some(p) = sel else { return Vec::new() };
    if p.is_dir() {
        return Vec::new();
    }
    let f = quote(&p.to_string_lossy());
    let mut v = Vec::new();

    if ext_of(p) == "sstr" {
        // The header is read once, here: what Play and Export mean depends on
        // what the capture says it holds, and a capture that will not say
        // gets the treatment for something that is not text.
        let ct = content_type(p);
        let text = is_text(ct.as_deref());
        let ext = extension_for(ct.as_deref());
        let base = free_name(&p.with_extension(ext));
        let out = quote(&base.to_string_lossy());

        v.push(Service {
            key: 'p',
            name: "Play".into(),
            // TEXT GOES TO THE SCREEN AND A VIDEO GOES TO A PLAYER, and both
            // are one line a person could have typed. `sstr play` writes the
            // payload to standard output, which for an MP4 is not something
            // to do to a terminal.
            line: if text {
                format!("sstr play {f}")
            } else {
                format!("sstr play {f} | {} -", player_of(s))
            },
            how: if text { How::Terminal } else { How::Background },
        });
        v.push(Service { key: 'P', name: "Paced".into(), line: format!("sstr play {f} --paced"), how: How::Terminal });
        v.push(Service {
            key: 's',
            name: "Serve".into(),
            line: format!("sstr play {f} --serve {}", quote(&serve_of(s))),
            how: How::Background,
        });
        v.push(Service { key: 'v', name: "Verify".into(), line: format!("sstr verify {f}"), how: How::Terminal });
        v.push(Service {
            key: 'x',
            name: format!("Export {}", ext.to_uppercase()),
            line: format!("sstr play {f} -o {out}"),
            how: How::Terminal,
        });
        if text {
            v.push(Service { key: 't', name: "Text".into(), line: format!("sstr play {f}"), how: How::Terminal });
        }
        v.push(Service { key: 'a', name: "Armor".into(), line: format!("sstr armor {f}"), how: How::Terminal });
        return v;
    }

    // Not a capture: a transcript beside one, or a video ytq left when
    // OUTPUT is mp4. The verbs are the same words for the same things.
    match ext_of(p).as_str() {
        "txt" | "md" | "json" | "log" | "csv" | "conf" => {
            v.push(Service { key: 't', name: "Text".into(), line: format!("cat {f}"), how: How::Terminal });
        }
        "mp4" | "mkv" | "webm" | "ts" | "m4a" | "mp3" | "opus" | "wav" => {
            v.push(Service { key: 'p', name: "Play".into(), line: format!("{} {f}", player_of(s)), how: How::Background });
        }
        _ => {}
    }
    v
}

/// The Services line at the foot of the window: the keys, and what they send.
pub fn line_for(services: &[Service]) -> String {
    if services.is_empty() {
        return "Services:  (nothing to send this selection)".into();
    }
    let mut out = String::from("Services:");
    for s in services {
        out.push_str(&format!("  {} {}", s.key, s.name));
    }
    out
}

/// The Service a key sends, if any.
pub fn by_key(services: &[Service], key: char) -> Option<&Service> {
    services.iter().find(|s| s.key == key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn settings(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("sstr-services-{}-{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_word_is_quoted_as_a_shell_needs_it() {
        assert_eq!(quote("plain.sstr"), "plain.sstr");
        assert_eq!(quote("/home/u/Videos/Archive/a-b_c.sstr"), "/home/u/Videos/Archive/a-b_c.sstr");
        assert_eq!(quote("127.0.0.1:8080"), "127.0.0.1:8080");
        // The names ytq really writes.
        assert_eq!(quote("Me at the zoo.sstr"), "'Me at the zoo.sstr'");
        assert_eq!(quote("Don't Look Up.mp4"), r#"'Don'\''t Look Up.mp4'"#);
        assert_eq!(quote("a;rm -rf b"), "'a;rm -rf b'");
        assert_eq!(quote("$HOME"), "'$HOME'");
        assert_eq!(quote(""), "''");
    }

    /// THE QUOTING IS THE SECURITY BOUNDARY, because the line goes to `sh`.
    /// A file called `; rm -rf ~` is a file a person can make, and the
    /// Workspace must build a line that names it rather than one that runs
    /// it. This runs the quoted word through a real shell and asks what it
    /// got.
    #[test]
    fn a_quoted_name_reaches_a_shell_as_one_word() {
        for name in ["plain.sstr", "Me at the zoo.sstr", "Don't Look Up.mp4", "; rm -rf x", "$HOME", "a\nb", "*"] {
            let out = std::process::Command::new("sh")
                .arg("-c")
                .arg(format!("printf %s {}", quote(name)))
                .output()
                .unwrap();
            assert_eq!(String::from_utf8_lossy(&out.stdout), name, "quoting {name:?}");
        }
    }

    #[test]
    fn a_payload_is_named_for_what_it_is() {
        assert_eq!(extension_for(Some("video/mp4")), "mp4");
        assert_eq!(extension_for(Some("video/mp2t")), "ts");
        assert_eq!(extension_for(Some("text/plain")), "txt");
        assert_eq!(extension_for(Some("text/vtt")), "txt");
        assert_eq!(extension_for(Some("application/octet-stream")), "bin");
        assert_eq!(extension_for(None), "bin");
    }

    #[test]
    fn export_never_names_a_file_that_is_already_there() {
        let d = scratch("free");
        let base = d.join("cap.txt");
        assert_eq!(free_name(&base), base, "nothing there: the plain name");
        std::fs::write(&base, b"a transcript ytq wrote").unwrap();
        assert_eq!(free_name(&base), d.join("cap-1.txt"));
        std::fs::write(d.join("cap-1.txt"), b"x").unwrap();
        assert_eq!(free_name(&base), d.join("cap-2.txt"));
        // And through services_for, which is where it matters. This capture
        // has no readable header, so its payload is `bin`; what the check is
        // about is that the name Export prints is one that is free.
        let cap = d.join("note.sstr");
        std::fs::write(&cap, b"not really a capture").unwrap();
        std::fs::write(d.join("note.txt"), b"the transcript ytq wrote").unwrap();
        let free = by_key(&services_for(Some(&cap), &settings(&[])), 'x').unwrap().line.clone();
        assert!(free.ends_with("note.bin"), "nothing is using note.bin: {free}");
        std::fs::write(d.join("note.bin"), b"an earlier export").unwrap();
        let taken = by_key(&services_for(Some(&cap), &settings(&[])), 'x').unwrap().line.clone();
        assert!(taken.ends_with("note-1.bin"), "note.bin is taken, so step aside: {taken}");
        assert_eq!(std::fs::read_to_string(d.join("note.bin")).unwrap(), "an earlier export");
        assert_eq!(std::fs::read_to_string(d.join("note.txt")).unwrap(), "the transcript ytq wrote");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_settings_the_report_gives_the_workspace() {
        assert_eq!(player_of(&settings(&[])), "mpv");
        assert_eq!(player_of(&settings(&[("PLAYER", "vlc")])), "vlc");
        assert_eq!(player_of(&settings(&[("PLAYER", "  ")])), "mpv", "blank is not a player");
        assert_eq!(serve_of(&settings(&[])), "127.0.0.1:8080");
        assert_eq!(serve_of(&settings(&[("SERVE", "0.0.0.0:9000")])), "0.0.0.0:9000");
    }

    #[test]
    fn a_folder_is_sent_nothing_and_neither_is_nothing() {
        let d = scratch("folder");
        assert!(services_for(Some(&d), &settings(&[])).is_empty(), "a folder is opened, not sent a verb");
        assert!(services_for(None, &settings(&[])).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_transcript_beside_a_capture_is_opened_as_text() {
        let d = scratch("txt");
        let f = d.join("jawed-Me_at_the_zoo_jNQXAC9IVRw.txt");
        std::fs::write(&f, b"notes\n").unwrap();
        let v = services_for(Some(&f), &settings(&[]));
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].key, 't');
        assert_eq!(v[0].line, format!("cat {}", quote(&f.to_string_lossy())));
        assert_eq!(v[0].how, How::Terminal);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_video_ytq_left_is_played_by_the_player() {
        let d = scratch("mp4");
        let f = d.join("clip.mp4");
        std::fs::write(&f, b"not really an mp4").unwrap();
        let v = services_for(Some(&f), &settings(&[("PLAYER", "vlc")]));
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].line, format!("vlc {}", quote(&f.to_string_lossy())));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The report's Services line, on a capture: `p Play  P Paced  s Serve
    /// v Verify  x Export MP4  t Text  a Armor`. A video has no Text.
    #[test]
    fn a_capture_is_sent_the_verbs_the_report_lists() {
        let d = scratch("sstr");
        // Not a real capture: services_for reads the header and settles for
        // what it can get, which is the `bin` treatment. The keys and their
        // order are what is being checked here.
        let f = d.join("clip.sstr");
        std::fs::write(&f, b"not really a capture").unwrap();
        let v = services_for(Some(&f), &settings(&[]));
        let keys: Vec<char> = v.iter().map(|s| s.key).collect();
        assert_eq!(keys, ['p', 'P', 's', 'v', 'x', 'a'], "a capture that is not text has no Text");
        let q = quote(&f.to_string_lossy());
        assert_eq!(by_key(&v, 'P').unwrap().line, format!("sstr play {q} --paced"));
        assert_eq!(by_key(&v, 'v').unwrap().line, format!("sstr verify {q}"));
        assert_eq!(by_key(&v, 'a').unwrap().line, format!("sstr armor {q}"));
        assert_eq!(by_key(&v, 's').unwrap().line, format!("sstr play {q} --serve 127.0.0.1:8080"));
        assert_eq!(by_key(&v, 'p').unwrap().line, format!("sstr play {q} | mpv -"));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// NO SERVICE MAY TAKE A KEY THE BROWSER MOVES WITH. The keys that move
    /// are read first, so a clash would not misfire -- it would make the
    /// Service unreachable, which is worse, because nothing would say so.
    #[test]
    fn no_service_takes_a_key_that_moves() {
        let d = scratch("keys");
        let f = d.join("clip.sstr");
        std::fs::write(&f, b"x").unwrap();
        let t = d.join("notes.txt");
        std::fs::write(&t, b"x").unwrap();
        let m = d.join("clip.mp4");
        std::fs::write(&m, b"x").unwrap();
        for sel in [&f, &t, &m] {
            for sv in services_for(Some(sel), &settings(&[])) {
                assert!(
                    !"hjklq".contains(sv.key),
                    "{} takes {:?}, which moves or leaves",
                    sv.name,
                    sv.key
                );
            }
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_services_line_says_the_keys_and_nothing_when_there_are_none() {
        let d = scratch("line");
        let f = d.join("notes.txt");
        std::fs::write(&f, b"x").unwrap();
        let v = services_for(Some(&f), &settings(&[]));
        assert_eq!(line_for(&v), "Services:  t Text");
        assert!(line_for(&[]).contains("nothing to send"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
