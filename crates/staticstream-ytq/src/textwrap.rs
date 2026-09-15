// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Paul Richeson

//! A transcript's words: the Python ytq's `vtt_text()`, and the part of
//! Python's `textwrap` it relies on, `fill(text, 78)`.
//!
//! The transcript a Rust ytq writes should be the one the Python ytq wrote,
//! line break for line break, so `fill` follows `textwrap.TextWrapper` with its
//! defaults: tabs expanded, whitespace made spaces, text split into chunks by
//! `wordsep_re` -- which breaks after a hyphen inside a word and around an
//! em-dash -- and chunks laid greedily into lines, a word longer than a line
//! broken at a hyphen where it can be.

use crate::urls::{html_unescape, py_strip};

/// textwrap's whitespace: ASCII only, as `_whitespace = '\t\n\x0b\x0c\r '`.
fn tw_space(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ')
}

/// Python's `\w`.
fn word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// textwrap's `letter = r'[^\d\W]'`: a word character that is not a digit.
fn letter(c: char) -> bool {
    word(c) && !c.is_numeric()
}

/// textwrap's `word_punct = r'[\w!"\'&.,?]'`.
fn word_punct(c: char) -> bool {
    word(c) || matches!(c, '!' | '"' | '\'' | '&' | '.' | ',' | '?')
}

/// `-{2,}` followed by `\w`, at `k`.
fn dashes_then_word(c: &[char], k: usize) -> Option<usize> {
    let n = c[k..].iter().take_while(|&&x| x == '-').count();
    (n >= 2 && c.get(k + n).map_or(false, |&x| word(x))).then_some(n)
}

/// `TextWrapper._split`, with `break_on_hyphens`: the chunks `wordsep_re` makes.
fn split_chunks(c: &[char]) -> Vec<String> {
    let n = c.len();
    let lt = |i: usize| c.get(i).map_or(false, |&x| letter(x));
    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        // Whitespace.
        if tw_space(c[i]) {
            let j = i + c[i..].iter().take_while(|&&x| tw_space(x)).count();
            out.push(c[i..j].iter().collect());
            i = j;
            continue;
        }
        // An em-dash between words: (?<=word_punct) -{2,} (?=\w).
        if i > 0 && word_punct(c[i - 1]) {
            if let Some(d) = dashes_then_word(c, i) {
                out.push(c[i..i + d].iter().collect());
                i += d;
                continue;
            }
        }
        // A word, possibly hyphenated: the shortest run of non-space that
        // ends at a hyphen joining letters, at whitespace or the end, or
        // before an em-dash.
        let mut end = n;
        let mut k = i + 1;
        while k <= n {
            if tw_space(c[k - 1]) {
                end = k - 1;
                break;
            }
            // -(?: (?<=lt{2}-) | (?<=lt-lt-) ) (?= lt -? lt)
            if k < n && c[k] == '-' {
                let behind = (k >= 2 && lt(k - 2) && lt(k - 1)) || (k >= 3 && lt(k - 3) && c[k - 2] == '-' && lt(k - 1));
                let ahead = lt(k + 1) && (lt(k + 2) || (c.get(k + 2) == Some(&'-') && lt(k + 3)));
                if behind && ahead {
                    end = k + 1;
                    break;
                }
            }
            // (?=ws|\Z)
            if k == n || tw_space(c[k]) {
                end = k;
                break;
            }
            // (?<=word_punct) (?=-{2,}\w)
            if word_punct(c[k - 1]) && dashes_then_word(c, k).is_some() {
                end = k;
                break;
            }
            k += 1;
        }
        out.push(c[i..end].iter().collect());
        i = end;
    }
    out
}

/// `str.expandtabs(8)`.
fn expandtabs(s: &str) -> String {
    if !s.contains('\t') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut col = 0;
    for ch in s.chars() {
        match ch {
            '\t' => {
                let pad = 8 - col % 8;
                out.push_str(&" ".repeat(pad));
                col += pad;
            }
            '\n' | '\r' => {
                out.push(ch);
                col = 0;
            }
            _ => {
                out.push(ch);
                col += 1;
            }
        }
    }
    out
}

fn blank(s: &str) -> bool {
    py_strip(s).is_empty()
}

/// `textwrap.wrap(text, width)`.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let text: String = expandtabs(text).chars().map(|c| if tw_space(c) { ' ' } else { c }).collect();
    let chars: Vec<char> = text.chars().collect();
    let mut chunks: Vec<String> = split_chunks(&chars).into_iter().filter(|c| !c.is_empty()).collect();
    chunks.reverse();
    let len = |s: &String| s.chars().count();
    let mut lines = Vec::new();
    while !chunks.is_empty() {
        let mut cur: Vec<String> = Vec::new();
        let mut cur_len = 0;
        if blank(chunks.last().unwrap()) && !lines.is_empty() {
            chunks.pop();
        }
        while let Some(last) = chunks.last() {
            let l = len(last);
            if cur_len + l <= width {
                cur_len += l;
                cur.push(chunks.pop().unwrap());
            } else {
                break;
            }
        }
        if let Some(last) = chunks.last() {
            if len(last) > width {
                // _handle_long_word, with break_long_words and break_on_hyphens.
                // A line already full takes nothing of the word: it goes whole
                // to the next line, and this line's trailing space is dropped.
                let space_left = if width < 1 { 1 } else { width - cur_len };
                if space_left > 0 {
                    let chunk: Vec<char> = last.chars().collect();
                    let mut end = space_left;
                    if chunk.len() > space_left {
                        if let Some(h) = chunk[..space_left].iter().rposition(|&x| x == '-') {
                            if h > 0 && chunk[..h].iter().any(|&x| x != '-') {
                                end = h + 1;
                            }
                        }
                    }
                    cur.push(chunk[..end].iter().collect());
                    *chunks.last_mut().unwrap() = chunk[end..].iter().collect();
                } else if cur.is_empty() {
                    cur.push(chunks.pop().unwrap());
                }
            }
        }
        if cur.last().map_or(false, |c| blank(c)) {
            cur.pop();
        }
        if !cur.is_empty() {
            lines.push(cur.concat());
        }
    }
    lines
}

/// `textwrap.fill(text, width)`.
pub fn fill(text: &str, width: usize) -> String {
    wrap(text, width).join("\n")
}

/// `re.sub(r"<[^>]*>", "", s)`.
fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(open) = rest.find('<') {
        match rest[open..].find('>') {
            Some(close) => {
                out.push_str(&rest[..open]);
                rest = &rest[open + close + 1..];
            }
            None => break,
        }
    }
    out.push_str(rest);
    out
}

/// `vtt_text(path)`'s work on the file's text: the words of a WebVTT file as
/// one wrapped paragraph. YouTube's automatic captions show each line again in
/// the next cue, with word timings as inline tags: the tags go, and a line the
/// same as the one before it is dropped.
pub fn vtt_words(raw: &str) -> String {
    // Python reads the file in universal-newline mode.
    let text = raw.replace("\r\n", "\n").replace('\r', "\n");
    let mut lines: Vec<String> = Vec::new();
    let mut header = true;
    let mut all: Vec<&str> = text.split('\n').collect();
    if text.ends_with('\n') {
        all.pop();
    }
    for raw_line in all {
        let line = py_strip(raw_line);
        if header {
            header = !line.is_empty();
        } else if !line.contains("-->") {
            let t = html_unescape(&strip_tags(line));
            let t = py_strip(&t);
            if !t.is_empty() && lines.last().map_or(true, |l| l != t) {
                lines.push(t.to_string());
            }
        }
    }
    fill(&lines.join(" "), 78)
}

/// `vtt_text(path)`.
pub fn vtt_text(path: &std::path::Path) -> std::io::Result<String> {
    let bytes = std::fs::read(path)?;
    Ok(vtt_words(&String::from_utf8_lossy(&bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hyphens_and_dashes_split_as_textwrap_does() {
        let c: Vec<char> = "a well-known so--called x-ray 1-2 e-mail --flag".chars().collect();
        assert_eq!(split_chunks(&c), ["a", " ", "well-", "known", " ", "so", "--", "called", " ", "x-ray", " ", "1-2", " ", "e-mail", " ", "--flag"]);
    }

    #[test]
    fn wraps_at_78() {
        let words = "All right, so here we are, in front of the elephants the cool thing about these guys is that they have really, really, really long trunks, and that's, that's cool";
        let lines = wrap(words, 78);
        assert!(lines.iter().all(|l| l.chars().count() <= 78));
        assert_eq!(lines[0], "All right, so here we are, in front of the elephants the cool thing about");
        assert_eq!(wrap(&"x".repeat(200), 78).len(), 3);
    }

    #[test]
    fn automatic_captions_lose_their_repeats() {
        let vtt = "WEBVTT\nKind: captions\n\n00:00:00.000 --> 00:00:01.000\ncome<00:00:00.500><c> on</c>\n\n00:00:01.000 --> 00:00:01.010\ncome on\n\n00:00:01.010 --> 00:00:02.000\ncome on\nlet&#39;s go\n";
        assert_eq!(vtt_words(vtt), "come on let's go");
    }
}
