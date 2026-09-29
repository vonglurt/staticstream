// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Paul Richeson

//! Text from outside, cleaned where it comes in.
//!
//! A title, a description, a caption, a comment and a line of yt-dlp's output
//! are all words somebody else chose. They go on to the notes, the video's
//! tags, a capture's header, the queue, the log, a notification and the
//! terminal -- and each of those gives some character a meaning. Escaping is
//! still owed at every one of them (`ffescape`, `services::quote`,
//! `term::printable`), but a control character is wanted at none, so it is
//! taken out once, here, and never exists inside the program at all.
//!
//! The rules are section VI of copal's docs/text-safety-lab-report.md, T1 and
//! T2; tests/ytq-hostile-check.sh holds ytq to them.

use crate::format::json::Value;

/// `s` without its control characters, newline and tab aside.
///
/// Every way of ending a line becomes a newline: CR LF, a lone CR, and the
/// vertical tab, form feed, NEL and the two Unicode separators that Python's
/// `splitlines()` -- and so ytq's log -- break at. Every other control
/// character, U+0000 to U+001F, U+007F and U+0080 to U+009F, is left out.
pub fn clean(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push('\n');
            }
            '\u{b}' | '\u{c}' | '\u{85}' | '\u{2028}' | '\u{2029}' => out.push('\n'),
            '\n' | '\t' => out.push(c),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// `s` as one line: cleaned, each run of newlines and tabs a single space,
/// and nothing at either end.
///
/// A title is one line. One that holds a newline is two, and the second can
/// be written to look like a row of the notes: `License:    Creative Commons`.
pub fn one_line(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut gap = false;
    for c in clean(s).chars() {
        if c == '\n' || c == '\t' {
            gap = true;
        } else {
            if gap && !out.is_empty() && !out.ends_with(' ') && c != ' ' {
                out.push(' ');
            }
            gap = false;
            out.push(c);
        }
    }
    out.trim().to_string()
}

/// Every string in `v`, cleaned. Each is one line, but for the values of the
/// keys in `long`, which keep their newlines: a description, a comment.
pub fn clean_info(v: Value, long: &[&str]) -> Value {
    match v {
        Value::Str(s) => Value::Str(one_line(&s)),
        Value::Arr(a) => Value::Arr(a.into_iter().map(|x| clean_info(x, long)).collect()),
        Value::Obj(m) => Value::Obj(
            m.into_iter()
                .map(|(k, x)| {
                    let x = match x {
                        Value::Str(s) if long.contains(&k.as_str()) => Value::Str(clean(&s)),
                        x => clean_info(x, long),
                    };
                    (one_line(&k), x)
                })
                .collect(),
        ),
        v => v,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::json;

    #[test]
    fn control_characters_go_and_line_ends_become_newlines() {
        assert_eq!(clean("plain — café\ttab\nline"), "plain — café\ttab\nline");
        assert_eq!(clean("a\x1b]0;title\x07b\x1b[2Jc\0d\x7fe\u{9b}f"), "a]0;titleb[2Jcdef");
        assert_eq!(clean("a\r\nb\rc\u{b}d\u{c}e\u{85}f\u{2028}g\u{2029}h"), "a\nb\nc\nd\ne\nf\ng\nh");
        assert_eq!(clean(""), "");
    }

    #[test]
    fn one_line_is_one_line() {
        assert_eq!(one_line("Honest title\nLicense:    Creative Commons"), "Honest title License:    Creative Commons");
        assert_eq!(one_line("one\r[CHAPTER]\rSTART=6000"), "one [CHAPTER] START=6000");
        assert_eq!(one_line("  a\n\n\tb\n c\n"), "a b c");
        assert_eq!(one_line("two  spaces stay"), "two  spaces stay");
        assert!(!one_line("a\u{2028}b\x1b\nc").contains(|c: char| c == '\n' || c.is_control()));
    }

    #[test]
    fn info_is_cleaned_all_the_way_down() {
        let v = json::parse(r#"{"title":"t\nURL: x","description":"one\r\ntwo\u001b[2J","n":3,"tags":["a\rb"],"chapters":[{"title":"c\n[CHAPTER]","start_time":0}],"sub\ntitles":{"en\n":[]}}"#).unwrap();
        let c = clean_info(v, &["description"]);
        assert_eq!(c.get("title").and_then(|v| v.as_str()), Some("t URL: x"));
        assert_eq!(c.get("description").and_then(|v| v.as_str()), Some("one\ntwo[2J"));
        assert_eq!(c.get("n").and_then(|v| v.as_num()), Some(3.0));
        assert_eq!(c.get("tags").and_then(|v| v.as_array()).and_then(|a| a[0].as_str()), Some("a b"));
        assert_eq!(c.get("chapters").and_then(|v| v.as_array()).and_then(|a| a[0].get("title")).and_then(|v| v.as_str()), Some("c [CHAPTER]"));
        assert!(c.get("sub titles").and_then(|v| v.get("en")).is_some());
    }
}
