// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Paul Richeson

//! Playlists: a video queued from a link that named one, and the videos after
//! it queued when a person asks -- one at a time.
//!
//! A link copied from a playlist reads `watch?v=ID&list=PL…&index=2`. ytq
//! queues the one video, as the Python ytq did -- every YouTube link is
//! queued as the plain `watch?v=ID`, which is what finds a link already
//! queued, and a download is `--no-playlist`. What is new is that the
//! playlist is not forgotten: it is noted, and the window marks the entry.
//!
//! `n` (`ytq next URL`) queues the next video, and only that one: a person
//! keeps pressing it to keep going. `N` (`ytq next --auto URL`) makes the
//! playlist go on by itself, still one at a time -- each video of it that is
//! done queues the one after it -- so a playlist of 500 never becomes 500
//! entries in the queue. Over CONFIRM_ABOVE left, N asks again first. N on a
//! playlist going on stops it, and n or N on any of its videos resumes it
//! from there: every video is noted in its turn, with its number.
//!
//! **THE NOTE IS A FILE BESIDE THE QUEUE, NOT A FIELD OF THE ENTRY**, for the
//! reason brave.rs gives: the queue is the Python ytq's format byte for byte.
//! One line a video: `URL TAB LIST TAB INDEX TAB QUEUED`, QUEUED being 1 once
//! the next video has been queued from it. Beside it: `playlists-auto`, the
//! lists going on, and `playlist-LIST`, a list's videos as last read.

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

/// How many videos `N` goes on through without being asked twice.
pub const CONFIRM_ABOVE: usize = 10;

/// What `next` is asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Want {
    /// `n`: the next video, and no more.
    One,
    /// `N`: the next, and from then on, each one of the playlist that is
    /// done queues the one after it.
    Auto,
}

/// What `next` did.
#[derive(Debug, PartialEq)]
pub enum Next {
    /// Queued #`index` of the playlist; `left` come after that one.
    Queued { index: usize, left: usize, auto: bool },
    /// Nothing after it that is not already in the queue (`left` after it in all).
    Nothing { left: usize },
    /// `N` on a playlist of more than CONFIRM_ABOVE left: asked again first.
    Confirm(usize),
    /// `N` on a playlist that was going on: it stops.
    Stopped,
    /// `url` was not queued from a playlist link.
    NotInOne,
    Failed(String),
}

/// The playlists that go on by themselves (`N`), one list id a line.
fn auto_file(paths: &Paths) -> PathBuf {
    paths.queue.with_file_name("playlists-auto")
}

/// Does `list` go on by itself?
pub fn is_auto(paths: &Paths, list: &str) -> bool {
    fs::read_to_string(auto_file(paths)).unwrap_or_default().lines().any(|l| l == list)
}

fn set_auto(paths: &Paths, list: &str, on: bool) {
    let mut lists: Vec<String> = fs::read_to_string(auto_file(paths)).unwrap_or_default().lines().filter(|l| *l != list && !l.is_empty()).map(str::to_string).collect();
    if on {
        lists.push(list.to_string());
    }
    let path = auto_file(paths);
    if lists.is_empty() {
        let _ = fs::remove_file(path);
    } else {
        let _ = fs::write(path, lists.join("\n") + "\n");
    }
}

/// The playlist's videos, in its order. Read with yt-dlp when a person asks
/// (`fresh`), and kept beside the queue, so that going on after each video
/// does not read a 500-video playlist again every time.
fn ids_of(paths: &Paths, list: &str, fresh: bool) -> Result<Vec<String>, String> {
    let cache = paths.queue.with_file_name(format!("playlist-{list}"));
    if !fresh {
        if let Ok(t) = fs::read_to_string(&cache) {
            return Ok(t.lines().map(str::to_string).collect());
        }
    }
    let list_url = format!("https://www.youtube.com/playlist?list={list}");
    log(paths, &format!("reading playlist {list}"));
    let ids: Vec<String> = match run_timeout(Command::new("yt-dlp").args(["--flat-playlist", "--no-warnings", "--print", "id", "--", &list_url]), 180) {
        Ran::Done(0, out, _) => splitlines(&out).into_iter().map(|l| py_strip(l).to_string()).filter(|l| first_id(&format!("https://youtu.be/{l}")).as_deref() == Some(l.as_str())).collect(),
        Ran::Done(code, _, err) => {
            let why = splitlines(py_strip(&err)).last().map_or(format!("yt-dlp exited {code}"), |l| l.to_string());
            log(paths, &format!("playlist {list}: {why}"));
            return Err(why);
        }
        Ran::TimedOut => return Err("yt-dlp did not answer in 180 s".into()),
        Ran::Failed(e) => return Err(format!("cannot run yt-dlp: {e}")),
    };
    let _ = fs::write(&cache, ids.iter().map(|i| format!("{i}\n")).collect::<String>());
    Ok(ids)
}

/// The first of `rest` that is not in the queue, with its 1-based place.
fn first_new(paths: &Paths, ids: &[String], rest: &[String]) -> Option<(String, usize)> {
    let queued: Vec<String> = queue::snapshot(paths).iter().map(|i| queue::text(i, "url").to_string()).collect();
    rest.iter().map(|i| format!("https://www.youtube.com/watch?v={i}")).find(|u| !queued.contains(u)).map(|u| {
        let id = first_id(&u).unwrap_or_default();
        let at = ids.iter().position(|i| *i == id).map_or(0, |p| p + 1);
        (u, at)
    })
}

/// Note `next` as #`index` of `me`'s playlist, mark `me` continued, and queue it.
fn queue_one(paths: &Paths, me: &Place, next: &str, index: usize) -> Result<(), String> {
    let mut places = all(paths);
    if places.iter().all(|p| p.url != next) {
        places.push(Place { url: next.to_string(), list: me.list.clone(), index: Some(index as u32), queued: false });
    }
    for p in places.iter_mut().filter(|p| p.url == me.url) {
        p.queued = true;
    }
    // Noted before it is queued, so a runner that takes it at once already
    // finds it in the playlist.
    write(paths, &places);
    queue::enqueue(paths, &RunLock::default(), &[next.to_string()], false).map(|_| ()).map_err(|e| e.to_string())
}

/// `ytq next URL` and the window's `n` (Want::One), `ytq next --auto URL` and
/// its `N` (Want::Auto): the next video of the playlist `url` was queued from
/// that is not in the queue already. Auto on a playlist already going on
/// stops it; on one with more than CONFIRM_ABOVE left it asks first, unless
/// `confirmed`.
pub fn next(paths: &Paths, url: &str, want: Want, confirmed: bool) -> Next {
    let Some(me) = place(&all(paths), url) else { return Next::NotInOne };
    let Some(id) = first_id(url) else { return Next::NotInOne };
    if want == Want::Auto && is_auto(paths, &me.list) {
        set_auto(paths, &me.list, false);
        log(paths, &format!("{}: playlist {} stopped going on", short(url), me.list));
        return Next::Stopped;
    }
    let ids = match ids_of(paths, &me.list, true) {
        Ok(ids) => ids,
        Err(why) => return Next::Failed(why),
    };
    let rest = after(&ids, &id, me.index);
    if want == Want::Auto && rest.len() > CONFIRM_ABOVE && !confirmed {
        return Next::Confirm(rest.len());
    }
    let Some((next, index)) = first_new(paths, &ids, &rest) else { return Next::Nothing { left: rest.len() } };
    if let Err(e) = queue_one(paths, &me, &next, index) {
        return Next::Failed(e);
    }
    let auto = want == Want::Auto;
    if auto {
        set_auto(paths, &me.list, true);
    }
    log(paths, &format!("{}: playlist {}: queued #{index}{}", short(url), me.list, if auto { ", and it goes on by itself" } else { "" }));
    Next::Queued { index, left: ids.len().saturating_sub(index), auto }
}

/// After `url` is done: if its playlist goes on by itself, queue the next.
/// The playlist as it was last read is used, and its end turns the going-on
/// off. Silent for any video not queued from a playlist that goes on.
pub fn on_done(paths: &Paths, url: &str) {
    let Some(me) = place(&all(paths), url) else { return };
    if !is_auto(paths, &me.list) {
        return;
    }
    let Some(id) = first_id(url) else { return };
    let Ok(ids) = ids_of(paths, &me.list, false) else { return };
    let rest = after(&ids, &id, me.index);
    match first_new(paths, &ids, &rest) {
        Some((next, index)) => {
            if queue_one(paths, &me, &next, index).is_ok() {
                log(paths, &format!("{}: playlist {} goes on: queued #{index} of {}", short(url), me.list, ids.len()));
            }
        }
        None => {
            set_auto(paths, &me.list, false);
            log(paths, &format!("{}: playlist {} is at its end; it no longer goes on", short(url), me.list));
        }
    }
}

/// What a person is told of [`next`].
pub fn say(n: &Next) -> String {
    match n {
        Next::Queued { index, left, auto: false } => format!("queued #{index} of the playlist ({left} more after it): n for the next, N to go on by itself"),
        Next::Queued { index, left, auto: true } => format!("queued #{index}; the playlist goes on by itself, one at a time ({left} more after it). N stops it"),
        Next::Nothing { left: 0 } => "it is the last in its playlist: nothing after it".into(),
        Next::Nothing { left } => format!("the {left} after it in the playlist are all in the queue already"),
        Next::Confirm(n) => format!("{n} videos after it: press N again within 10 s to go on through them, one at a time"),
        Next::Stopped => "the playlist no longer goes on by itself; n or N resumes it from any of its videos".into(),
        Next::NotInOne => "n and N are for entries queued from a playlist link".into(),
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
        assert!(matches!(next(&paths, "https://www.youtube.com/watch?v=jNQXAC9IVRw", Want::One, false), Next::NotInOne));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn what_n_did_is_said_plainly() {
        assert_eq!(say(&Next::Queued { index: 3, left: 521, auto: false }), "queued #3 of the playlist (521 more after it): n for the next, N to go on by itself");
        assert_eq!(say(&Next::Nothing { left: 0 }), "it is the last in its playlist: nothing after it");
        assert_eq!(say(&Next::Confirm(523)), "523 videos after it: press N again within 10 s to go on through them, one at a time");
    }


    /// N's going on, from the playlist as last read: each video done queues
    /// the next not in the queue, and the end turns it off.
    #[test]
    fn a_playlist_going_on_queues_one_after_each_done_and_stops_at_its_end() {
        let home = std::env::temp_dir().join(format!("ytq-playlist-auto-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        let paths = Paths::from_lookup(|k| (k == "HOME").then(|| home.clone())).unwrap();
        fs::create_dir_all(paths.queue.parent().unwrap()).unwrap();
        let ids = ["aaaaaaaaaaa", "bbbbbbbbbbb", "ccccccccccc", "ddddddddddd"];
        let u = |i: &str| format!("https://www.youtube.com/watch?v={i}");
        fs::write(paths.queue.with_file_name("playlist-PLx"), ids.join("\n") + "\n").unwrap();
        note(&paths, &format!("{}&list=PLx&index=1", u(ids[0])));
        queue::enqueue(&paths, &RunLock::default(), &[u(ids[0]), u(ids[2])], false).unwrap();
        let queued = || queue::snapshot(&paths).iter().map(|i| queue::text(i, "url").to_string()).collect::<Vec<_>>();
        // Not going on: a done video queues nothing.
        on_done(&paths, &u(ids[0]));
        assert_eq!(queued().len(), 2);
        set_auto(&paths, "PLx", true);
        on_done(&paths, &u(ids[0]));
        assert_eq!(queued(), vec![u(ids[0]), u(ids[2]), u(ids[1])]);
        assert_eq!(place(&all(&paths), &u(ids[1])).unwrap().index, Some(2));
        // b done: c is queued already, so d.
        on_done(&paths, &u(ids[1]));
        assert_eq!(queued().last(), Some(&u(ids[3])));
        // d done: the end, and going on is off.
        on_done(&paths, &u(ids[3]));
        assert!(!is_auto(&paths, "PLx"));
        assert_eq!(queued().len(), 4);
        let _ = fs::remove_dir_all(&home);
    }

}
