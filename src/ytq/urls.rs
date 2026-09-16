// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Paul Richeson

//! Which text is a video: the Python ytq's `YT_RE`, `URL_RE`, `youtube_urls()`,
//! `as_url()` and `short()`, matched by hand -- there is no regex crate here --
//! and step for step as Python's regular expression engine matches them.
//!
//! ```text
//! YT_RE  = (?:https?://|(?<![\w.@/-]))(?:(?:www|m|music)\.)?
//!          (?:youtube\.com/(?:watch\?(?:[^\s"'<>#]*?&)?v=|shorts/|live/|embed/)|youtu\.be/)
//!          ([A-Za-z0-9_-]{11})(?![A-Za-z0-9_-])
//! URL_RE = ^https?://(?:[A-Za-z0-9-]+\.)+[A-Za-z0-9-]+(?::\d+)?(?:/\S*)?$
//!        | ^https?://localhost(?::\d+)?(?:/\S*)?$
//! ```
//!
//! The order in which the engine tries things decides which id it finds, so
//! that order is kept. `(?:[^…]*?&)?` is a greedy optional group around a lazy
//! run: the engine tries the group first, with the shortest run up to an `&`,
//! then longer runs, and only then no group at all. On `watch?v=A…&v=B…` that
//! finds B, and so must this.
//!
//! The ytq report's reasons for each part -- the scheme optional only for
//! YouTube hosts, the lookbehind, channels and playlists never matching --
//! stand; `make ytq-crosscheck` holds this to the Python ytq on a corpus.

/// Python's `\s` in a str pattern: Unicode whitespace, which counts the
/// information separators U+001C to U+001F that Rust's does not.
fn is_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// Python's `\w` in a str pattern: letters and digits of any script, and `_`.
fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn is_id_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

fn starts(c: &[char], at: usize, lit: &str) -> bool {
    let mut i = at;
    for ch in lit.chars() {
        if c.get(i) != Some(&ch) {
            return false;
        }
        i += 1;
    }
    true
}

/// The 11-character id at `p`, not followed by a twelfth id character.
fn id_at(c: &[char], p: usize) -> Option<(usize, String)> {
    let end = p + 11;
    if end > c.len() || !c[p..end].iter().all(|&ch| is_id_char(ch)) {
        return None;
    }
    if c.get(end).map_or(false, |&ch| is_id_char(ch)) {
        return None;
    }
    Some((end, c[p..end].iter().collect()))
}

/// After `youtube.com/` or `youtu.be/`, from `p`.
fn path_at(c: &[char], p: usize) -> Option<(usize, String)> {
    if starts(c, p, "youtube.com/") {
        let q = p + "youtube.com/".len();
        if starts(c, q, "watch?") {
            let s0 = q + "watch?".len();
            // The group first: after each `&` in a run with no \s " ' < > #,
            // shortest first; then without the group.
            let mut idx = s0;
            while idx < c.len() {
                let ch = c[idx];
                if is_space(ch) || matches!(ch, '"' | '\'' | '<' | '>' | '#') {
                    break;
                }
                if ch == '&' && starts(c, idx + 1, "v=") {
                    if let Some(found) = id_at(c, idx + 3) {
                        return Some(found);
                    }
                }
                idx += 1;
            }
            if starts(c, s0, "v=") {
                return id_at(c, s0 + 2);
            }
            return None;
        }
        for kw in ["shorts/", "live/", "embed/"] {
            if starts(c, q, kw) {
                return id_at(c, q + kw.len());
            }
        }
        return None;
    }
    if starts(c, p, "youtu.be/") {
        return id_at(c, p + "youtu.be/".len());
    }
    None
}

fn host_at(c: &[char], h: usize) -> Option<(usize, String)> {
    for prefix in ["www.", "m.", "music."] {
        if starts(c, h, prefix) {
            if let Some(found) = path_at(c, h + prefix.len()) {
                return Some(found);
            }
        }
    }
    path_at(c, h)
}

/// YT_RE matched at `i`: where the match ends, and the id.
fn match_at(c: &[char], i: usize) -> Option<(usize, String)> {
    // Cheap first: every match starts with h (a scheme), w, m or y.
    if !matches!(c.get(i), Some('h' | 'w' | 'm' | 'y')) {
        return None;
    }
    let scheme = if starts(c, i, "https://") {
        Some(i + 8)
    } else if starts(c, i, "http://") {
        Some(i + 7)
    } else {
        None
    };
    if let Some(h) = scheme {
        if let Some(found) = host_at(c, h) {
            return Some(found);
        }
    }
    let behind_ok = i == 0 || !(is_word(c[i - 1]) || matches!(c[i - 1], '.' | '@' | '/' | '-'));
    if behind_ok {
        return host_at(c, i);
    }
    None
}

/// `YT_RE.findall(text)`: every id, left to right, each match resuming after the last.
pub fn youtube_ids(text: &str) -> Vec<String> {
    let c: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < c.len() {
        match match_at(&c, i) {
            Some((end, id)) => {
                out.push(id);
                i = end.max(i + 1);
            }
            None => i += 1,
        }
    }
    out
}

/// `YT_RE.search(text)`: the first id.
pub fn first_id(text: &str) -> Option<String> {
    let c: Vec<char> = text.chars().collect();
    (0..c.len()).find_map(|i| match_at(&c, i)).map(|(_, id)| id)
}

/// `youtube_urls(text)`: every YouTube video in text, in the order first seen,
/// once each, as a plain watch URL, after HTML entities are undone.
pub fn youtube_urls(text: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    youtube_ids(&html_unescape(text))
        .into_iter()
        .filter(|id| seen.insert(id.clone()))
        .map(|id| format!("https://www.youtube.com/watch?v={id}"))
        .collect()
}

/// `short(url)`: a YouTube URL's video id, else the URL.
pub fn short(url: &str) -> String {
    first_id(url).unwrap_or_else(|| url.to_string())
}

/// Python's `str.strip()`.
pub fn py_strip(s: &str) -> &str {
    s.trim_matches(is_space)
}

/// `URL_RE.match(text)`.
pub fn is_url(t: &str) -> bool {
    let rest = if let Some(r) = t.strip_prefix("https://") {
        r
    } else if let Some(r) = t.strip_prefix("http://") {
        r
    } else {
        return false;
    };
    let tail_ok = |r: &str| -> bool {
        let r = match r.strip_prefix(':') {
            Some(after) => {
                let digits = after.chars().take_while(|c| c.is_ascii_digit()).count();
                if digits == 0 {
                    return false;
                }
                &after[digits..]
            }
            None => r,
        };
        // Python's $ also matches before one final newline.
        let r = r.strip_suffix('\n').unwrap_or(r);
        r.is_empty() || (r.starts_with('/') && !r.chars().any(is_space))
    };
    if let Some(r) = rest.strip_prefix("localhost") {
        if tail_ok(r) {
            return true;
        }
    }
    let end = rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '.')).unwrap_or(rest.len());
    let host = &rest[..end];
    let labels: Vec<&str> = host.split('.').collect();
    labels.len() >= 2 && labels.iter().all(|l| !l.is_empty()) && tail_ok(&rest[end..])
}

/// `as_url(text)`: text as one URL to queue, or None. A YouTube video at the
/// start of text with no whitespace in it is its plain watch URL; any other
/// URL stays as it is.
pub fn as_url(text: &str) -> Option<String> {
    let t = py_strip(text);
    let c: Vec<char> = t.chars().collect();
    if let Some((_, id)) = match_at(&c, 0) {
        if !t.chars().any(is_space) {
            return Some(format!("https://www.youtube.com/watch?v={id}"));
        }
    }
    if is_url(t) {
        Some(t.to_string())
    } else {
        None
    }
}

/// Python's `html.unescape`, for what matters to finding links: numeric
/// references, with Python's replacements for invalid ones, and the named
/// references a saved page or a bookmarks export uses. A name outside the
/// table is left as it was.
pub fn html_unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    const NAMED: [(&str, &str); 8] =
        [("amp", "&"), ("lt", "<"), ("gt", ">"), ("quot", "\""), ("apos", "'"), ("nbsp", "\u{a0}"), ("copy", "\u{a9}"), ("reg", "\u{ae}")];
    // Names html5 also accepts without the semicolon.
    const BARE: [&str; 7] = ["amp", "lt", "gt", "quot", "nbsp", "copy", "reg"];
    let c: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < c.len() {
        if c[i] != '&' {
            out.push(c[i]);
            i += 1;
            continue;
        }
        // &#123; &#x7b; (the semicolon optional, as html.unescape allows)
        if c.get(i + 1) == Some(&'#') {
            let hex = matches!(c.get(i + 2), Some('x' | 'X'));
            let start = i + if hex { 3 } else { 2 };
            let mut j = start;
            while j < c.len() && (if hex { c[j].is_ascii_hexdigit() } else { c[j].is_ascii_digit() }) {
                j += 1;
            }
            if j > start {
                let digits: String = c[start..j].iter().collect();
                let n = u32::from_str_radix(&digits, if hex { 16 } else { 10 }).unwrap_or(u32::MAX);
                out.push_str(&numeric_char(n));
                i = if c.get(j) == Some(&';') { j + 1 } else { j };
                continue;
            }
            out.push('&');
            i += 1;
            continue;
        }
        let mut j = i + 1;
        while j < c.len() && j - i <= 32 && !matches!(c[j], '\t' | '\n' | '\u{c}' | ' ' | '<' | '&' | '#' | ';') {
            j += 1;
        }
        let name: String = c[i + 1..j].iter().collect();
        if c.get(j) == Some(&';') {
            if let Some((_, v)) = NAMED.iter().find(|(n, _)| *n == name) {
                out.push_str(v);
                i = j + 1;
                continue;
            }
        }
        // Without a semicolon: the longest bare name the text starts with.
        if let Some(bare) = BARE.iter().filter(|b| name.starts_with(**b)).max_by_key(|b| b.len()) {
            out.push_str(NAMED.iter().find(|(n, _)| n == bare).map(|(_, v)| *v).unwrap_or(""));
            i += 1 + bare.len();
            continue;
        }
        out.push('&');
        i += 1;
    }
    out
}

/// What html.unescape makes of &#n;.
fn numeric_char(n: u32) -> String {
    const CP1252: [(u32, char); 27] = [
        (0x80, '\u{20ac}'), (0x82, '\u{201a}'), (0x83, '\u{0192}'), (0x84, '\u{201e}'), (0x85, '\u{2026}'),
        (0x86, '\u{2020}'), (0x87, '\u{2021}'), (0x88, '\u{02c6}'), (0x89, '\u{2030}'), (0x8a, '\u{0160}'),
        (0x8b, '\u{2039}'), (0x8c, '\u{0152}'), (0x8e, '\u{017d}'), (0x91, '\u{2018}'), (0x92, '\u{2019}'),
        (0x93, '\u{201c}'), (0x94, '\u{201d}'), (0x95, '\u{2022}'), (0x96, '\u{2013}'), (0x97, '\u{2014}'),
        (0x98, '\u{02dc}'), (0x99, '\u{2122}'), (0x9a, '\u{0161}'), (0x9b, '\u{203a}'), (0x9c, '\u{0153}'),
        (0x9e, '\u{017e}'), (0x9f, '\u{0178}'),
    ];
    if let Some((_, ch)) = CP1252.iter().find(|(k, _)| *k == n) {
        return ch.to_string();
    }
    if n == 0 || (0xD800..=0xDFFF).contains(&n) || n > 0x10FFFF {
        return "\u{fffd}".into();
    }
    // Python drops a few invalid code points outright.
    if (0x1..=0x8).contains(&n) || (0xe..=0x1f).contains(&n) || (0x7f..=0x9f).contains(&n) || (0xfdd0..=0xfdef).contains(&n)
        || n == 0xb || (n & 0xfffe) == 0xfffe
    {
        return String::new();
    }
    char::from_u32(n).map(String::from).unwrap_or_else(|| "\u{fffd}".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The test excerpt of the ytq report, IV-B: eight ids, in bookmark order.
    #[test]
    fn the_reports_excerpt() {
        let text = r#"<DT><A HREF="https://www.youtube.com/watch?v=sK99WuaU_k8" ICON="data:image/png;base64,iVBORw0KGgo=">x</A>
<DT><A HREF="https://www.youtube.com/shorts/3mxgcs10PoI">s</A>
<DT><A HREF="https://www.youtube.com/watch?v=EmFV-A5E5hk&list=RDu64NgDuis6g&index=2">mix</A>
<DT><A HREF="https://www.youtube.com/watch?v=6xlmaorRY0w">a</A>
<DT><A HREF="https://www.youtube.com/watch?v=6xlmaorRY0w&t=5330s">a again</A>
<DT><A HREF="https://www.youtube.com/watch?t=807&v=s6rGdKY2xWo&feature=youtu.be">t first</A>
<DT><A HREF="https://www.youtube.com/watch?feature=share&amp;v=_m_jwz5hzzw">escaped</A>
<DT><A HREF="https://youtu.be/-yKLnpqfwSQ?si=abc">be</A>
<DT><A HREF="https://www.youtube.com/watch?v=WXpWwY4kDgI&list=RDp29-4ymTv94&index=10">mix2</A>
<DT><A HREF="https://www.youtube.com/@bernadettebanner">channel</A>
<DT><A HREF="https://www.youtube.com/channel/UCCND6a0H56zHL4YuY226ZOQ">channel id</A>
<DT><A HREF="https://www.youtube.com/watch?v=tooShort">short</A>
<DT><A HREF="https://www.youtube.com/watch?v=0eFTrOpueYEX">long</A>
<DT><A HREF="https://www.seangoedecke.com/x">other</A><DT><A HREF="chrome://newtab/">tab</A>"#;
        let ids: Vec<String> = youtube_urls(text).iter().map(|u| u.rsplit('=').next().unwrap().to_string()).collect();
        assert_eq!(ids, ["sK99WuaU_k8", "3mxgcs10PoI", "EmFV-A5E5hk", "6xlmaorRY0w", "s6rGdKY2xWo", "_m_jwz5hzzw", "-yKLnpqfwSQ", "WXpWwY4kDgI"]);
    }

    /// IV-P: eleven links the scheme-optional pattern must match, ten it must not.
    #[test]
    fn scheme_optional_links() {
        for yes in [
            "youtube.com/shorts/SWHZolxKdVU",
            "www.youtube.com/shorts/SWHZolxKdVU",
            "m.youtube.com/shorts/SWHZolxKdVU",
            "youtu.be/SWHZolxKdVU",
            "youtube.com/watch?feature=share&v=SWHZolxKdVU",
            "see youtube.com/shorts/SWHZolxKdVU here",
            "(youtube.com/shorts/SWHZolxKdVU)",
            r#"HREF="youtube.com/shorts/SWHZolxKdVU""#,
            "https://www.youtube.com/shorts/SWHZolxKdVU",
            "http://youtube.com/shorts/SWHZolxKdVU/",
            "music.youtube.com/watch?v=SWHZolxKdVU",
        ] {
            assert_eq!(first_id(yes).as_deref(), Some("SWHZolxKdVU"), "{yes}");
        }
        for no in [
            "notyoutube.com/shorts/SWHZolxKdVU",
            "foo.youtube.com/shorts/SWHZolxKdVU",
            "evil.com/youtube.com/shorts/SWHZolxKdVU",
            "youtube.com.evil.com/shorts/SWHZolxKdVU",
            "xyoutu.be/SWHZolxKdVU",
            "user@youtube.com/shorts/SWHZolxKdVU",
            "youtube.com/shorts/SWHZolxKdVUX",
            "youtube.com/shorts/SWHZolx",
            "youtube.com/@channel",
            "youtube.com/playlist?list=PL0123456789",
        ] {
            assert_eq!(first_id(no), None, "{no}");
        }
    }

    #[test]
    fn as_url_as_the_report_lists() {
        let watch = Some("https://www.youtube.com/watch?v=SWHZolxKdVU".to_string());
        assert_eq!(as_url("youtube.com/shorts/SWHZolxKdVU"), watch);
        assert_eq!(as_url("  \nyoutu.be/SWHZolxKdVU \n"), watch);
        assert_eq!(as_url("m.youtube.com/watch?v=SWHZolxKdVU&t=5s"), watch);
        assert_eq!(as_url("https://www.youtube.com/watch?v=SWHZolxKdVU&list=RDabc&index=2"), watch);
        let archive = "https://web.archive.org/web/2024/https://www.youtube.com/watch?v=SWHZolxKdVU";
        assert_eq!(as_url(archive).as_deref(), Some(archive));
        assert_eq!(as_url("https://vimeo.com/12345").as_deref(), Some("https://vimeo.com/12345"));
        assert_eq!(as_url("https://www.youtube.com/@channel").as_deref(), Some("https://www.youtube.com/@channel"));
        for none in ["notyoutube.com/shorts/SWHZolxKdVU", "example.com/shorts/SWHZolxKdVU", "youtube.com/shorts/SWHZolxKdVU and more words", "youtube.com/@channel", "plain words", ""] {
            assert_eq!(as_url(none), None, "{none}");
        }
        assert!(is_url("http://localhost:8080/x") && is_url("https://a.b:443") && !is_url("https://nodot") && !is_url("https://a..b"));
    }

    #[test]
    fn the_engine_order_is_kept() {
        assert_eq!(first_id("youtube.com/watch?v=AAAAAAAAAAA&v=BBBBBBBBBBB").as_deref(), Some("BBBBBBBBBBB"));
        assert_eq!(first_id("youtube.com/watch?v=short&v=CCCCCCCCCCC").as_deref(), Some("CCCCCCCCCCC"));
    }

    #[test]
    fn entities() {
        assert_eq!(html_unescape("a&amp;b &lt;&gt; &#61; &#x3D; &ampx &bogus; &#128;"), "a&b <> = = &x &bogus; \u{20ac}");
    }
}
