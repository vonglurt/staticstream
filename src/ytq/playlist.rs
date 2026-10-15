// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Paul Richeson

//! Playlists: a video queued from a link that named one, and the rest of
//! that playlist queued after it when a person asks.
//!
//! A link copied from a playlist reads `watch?v=ID&list=PL…&index=2`. ytq
//! queues the one video, as the Python ytq did -- every YouTube link is
//! queued as the plain `watch?v=ID`, which is what finds a link already
//! queued, and a download is `--no-playlist`. What is new is that the
//! playlist is not forgotten: it is noted, the window marks the entry, and
//! `n` (or `ytq next URL`) queues the videos that come after it, in the
//! playlist's order, each noted in its turn so that `n` on the last of them
//! goes on from there.
//!
//! **THE NOTE IS A FILE BESIDE THE QUEUE, NOT A FIELD OF THE ENTRY**, for the
//! reason brave.rs gives: the queue is the Python ytq's format byte for byte.
//! One line a video: `URL TAB LIST TAB INDEX TAB QUEUED`, QUEUED being 1 once
//! the rest of the playlist has been queued from it.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use crate::ytq::log::{log, splitlines};
use crate::ytq::queue::{self, RunLock};
use crate::ytq::runner::{run_timeout, Ran};
use crate::ytq::urls::{first_id, html_unescape, py_strip, short};
use crate::ytq::Paths;

/// A video's place in a playlist, as the link gave it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    /// The queued URL: `https://www.youtube.com/watch?v=ID`.
    pub url: String,
    pub list: String,
    /// `index=`, 1-based, when the link had one.
    pub index: Option<u32>,
    /// The rest of the playlist has been queued from this video.
    pub queued: bool,
}

/// The value of `key=` in a URL's query, as far as `&`, `#` or the end.
fn param<'a>(url: &'a str, key: &str) -> Option<&'a str> {
    let q = url.split_once('?')?.1.split('#').next()?;
    q.split('&').find_map(|kv| kv.strip_prefix(key)?.strip_prefix('='))
}

/// Every YouTube video in `text` whose link names a playlist. A list id is
/// the URL-safe alphabet and nothing else, or it is not taken.
pub fn places_in(text: &str) -> Vec<Place> {
    let mut out: Vec<Place> = Vec::new();
    for word in html_unescape(text).split_whitespace() {
        let Some(id) = first_id(word) else { continue };
        let Some(list) = param(word, "list").filter(|l| !l.is_empty() && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')) else { continue };
        let url = format!("https://www.youtube.com/watch?v={id}");
        if out.iter().all(|p| p.url != url) {
            out.push(Place { url, list: list.to_string(), index: param(word, "index").and_then(|i| i.parse().ok()), queued: false });
        }
    }
    out
}

fn file(paths: &Paths) -> PathBuf {
    paths.queue.with_file_name("playlists")
}

/// Every video noted as part of a playlist.
pub fn all(paths: &Paths) -> Vec<Place> {
    fs::read_to_string(file(paths))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let mut f = l.split('\t');
            let (url, list) = (f.next()?.to_string(), f.next()?.to_string());
            let index = f.next().and_then(|i| i.parse().ok());
            Some(Place { url, list, index, queued: f.next() == Some("1") })
        })
        .collect()
}

fn write(paths: &Paths, places: &[Place]) {
    let text: String = places
        .iter()
        .map(|p| format!("{}\t{}\t{}\t{}\n", p.url, p.list, p.index.map(|i| i.to_string()).unwrap_or_default(), if p.queued { "1" } else { "" }))
        .collect();
    let path = file(paths);
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let tmp = path.with_extension("tmp");
    if fs::write(&tmp, text).is_ok() {
        let _ = fs::rename(&tmp, &path);
    }
}

/// Note the playlists `text` names. A video already noted keeps its first
/// note: the playlist it was first queued from is the one it continues.
pub fn note(paths: &Paths, text: &str) {
    let new = places_in(text);
    if new.is_empty() {
        return;
    }
    let mut places = all(paths);
    let before = places.len();
    for p in new {
        if places.iter().all(|q| q.url != p.url) {
            places.push(p);
        }
    }
    if places.len() != before {
        write(paths, &places);
    }
}

/// The playlist `url` was queued from, if it was.
pub fn place(places: &[Place], url: &str) -> Option<Place> {
    places.iter().find(|p| p.url == url).cloned()
}

/// The ids after `id` in the playlist's order; where `id` is not in it (taken
/// down, or the list has changed), those after `index`.
pub fn after(ids: &[String], id: &str, index: Option<u32>) -> Vec<String> {
    let from = match ids.iter().position(|i| i == id) {
        Some(at) => at + 1,
        None => index.map_or(ids.len(), |i| (i as usize).min(ids.len())),
    };
    ids[from..].to_vec()
}

/// What `next` did.
#[derive(Debug)]
pub enum Next {
    /// Queued these many of those after it; the rest were in the queue already.
    Queued { queued: usize, after: usize },
    /// `url` was not queued from a playlist link.
    NotInOne,
    Failed(String),
}

/// `ytq next URL`, and the window's `n`: queue the videos after `url` in the
/// playlist it was queued from.
pub fn next(paths: &Paths, url: &str) -> Next {
    let places = all(paths);
    let Some(me) = place(&places, url) else { return Next::NotInOne };
    let Some(id) = first_id(url) else { return Next::NotInOne };
    let list_url = format!("https://www.youtube.com/playlist?list={}", me.list);
    log(paths, &format!("{}: reading playlist {}", short(url), me.list));
    let ran = run_timeout(Command::new("yt-dlp").args(["--flat-playlist", "--no-warnings", "--print", "id", "--", &list_url]), 180);
    let ids: Vec<String> = match ran {
        Ran::Done(0, out, _) => splitlines(&out).into_iter().map(|l| py_strip(l).to_string()).filter(|l| first_id(&format!("https://youtu.be/{l}")).as_deref() == Some(l.as_str())).collect(),
        Ran::Done(code, _, err) => {
            let why = splitlines(py_strip(&err)).last().map_or(format!("yt-dlp exited {code}"), |l| l.to_string());
            log(paths, &format!("{}: playlist {}: {why}", short(url), me.list));
            return Next::Failed(why);
        }
        Ran::TimedOut => return Next::Failed("yt-dlp did not answer in 180 s".into()),
        Ran::Failed(e) => return Next::Failed(format!("cannot run yt-dlp: {e}")),
    };
    let rest = after(&ids, &id, me.index);
    let base = ids.iter().position(|i| *i == id).map_or(me.index.unwrap_or(0) as usize, |at| at + 1);
    let urls: Vec<String> = rest.iter().map(|i| format!("https://www.youtube.com/watch?v={i}")).collect();
    // Noted before they are queued, so a runner that picks one up at once
    // already finds it in the playlist.
    let mut places = all(paths);
    for (n, u) in urls.iter().enumerate() {
        if places.iter().all(|p| p.url != *u) {
            places.push(Place { url: u.clone(), list: me.list.clone(), index: Some((base + n + 1) as u32), queued: false });
        }
    }
    for p in places.iter_mut().filter(|p| p.url == url) {
        p.queued = true;
    }
    write(paths, &places);
    match queue::enqueue(paths, &RunLock::default(), &urls, false) {
        Ok((added, _)) => {
            log(paths, &format!("{}: playlist {}: {} of the {} after it queued", short(url), me.list, added.len(), urls.len()));
            Next::Queued { queued: added.len(), after: urls.len() }
        }
        Err(e) => Next::Failed(e.to_string()),
    }
}

/// What a person is told of [`next`].
pub fn say(n: &Next) -> String {
    match n {
        Next::Queued { after: 0, .. } => "it is the last in its playlist: nothing after it".into(),
        Next::Queued { queued, after } if queued == after => format!("queued the {after} after it in the playlist"),
        Next::Queued { queued: 0, after } => format!("the {after} after it in the playlist are all in the queue already"),
        Next::Queued { queued, after } => format!("queued {queued} of the {after} after it in the playlist; the others were in the queue already"),
        Next::NotInOne => "n is for entries queued from a playlist link".into(),
        Next::Failed(why) => format!("could not read the playlist: {why}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINK: &str = "https://www.youtube.com/watch?v=sS246TkX17w&list=PLB304-kfu9UE73Tlf05qnJ5jmsxsMNEre&index=2";

    #[test]
    fn a_playlist_link_is_its_video_and_its_place() {
        assert_eq!(
            places_in(&format!("look: {LINK} and https://youtu.be/jNQXAC9IVRw")),
            vec![Place { url: "https://www.youtube.com/watch?v=sS246TkX17w".into(), list: "PLB304-kfu9UE73Tlf05qnJ5jmsxsMNEre".into(), index: Some(2), queued: false }]
        );
        assert_eq!(places_in("https://www.youtube.com/watch?list=PLx&v=sS246TkX17w")[0].index, None);
        assert!(places_in("https://www.youtube.com/watch?v=sS246TkX17w&list=PL;rm").is_empty());
        assert!(places_in("https://www.youtube.com/watch?v=sS246TkX17w").is_empty());
    }

    #[test]
    fn the_rest_is_after_the_video_or_after_its_index() {
        let ids: Vec<String> = ["a", "b", "c", "d"].iter().map(|s| s.to_string()).collect();
        assert_eq!(after(&ids, "b", Some(9)), vec!["c", "d"]);
        assert_eq!(after(&ids, "gone", Some(2)), vec!["c", "d"]);
        assert_eq!(after(&ids, "d", None), Vec::<String>::new());
        assert_eq!(after(&ids, "gone", None), Vec::<String>::new());
        assert_eq!(after(&ids, "gone", Some(99)), Vec::<String>::new());
    }

    #[test]
    fn a_note_is_kept_once_and_read_back() {
        let home = std::env::temp_dir().join(format!("ytq-playlist-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        let paths = Paths::from_lookup(|k| (k == "HOME").then(|| home.clone())).unwrap();
        fs::create_dir_all(paths.queue.parent().unwrap()).unwrap();
        note(&paths, LINK);
        note(&paths, "https://www.youtube.com/watch?v=sS246TkX17w&list=PLother");
        let got = all(&paths);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].list, "PLB304-kfu9UE73Tlf05qnJ5jmsxsMNEre");
        assert_eq!(place(&got, "https://www.youtube.com/watch?v=sS246TkX17w").unwrap().index, Some(2));
        assert!(matches!(next(&paths, "https://www.youtube.com/watch?v=jNQXAC9IVRw"), Next::NotInOne));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn what_n_did_is_said_plainly() {
        assert_eq!(say(&Next::Queued { queued: 12, after: 12 }), "queued the 12 after it in the playlist");
        assert_eq!(say(&Next::Queued { queued: 0, after: 523 }), "the 523 after it in the playlist are all in the queue already");
        assert_eq!(say(&Next::Queued { queued: 0, after: 0 }), "it is the last in its playlist: nothing after it");
    }

}
