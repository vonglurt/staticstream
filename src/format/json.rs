// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Paul Richeson

//! Just enough JSON for the header, checkpoint and end records.
//!
//! The parser is orrery's `json.rs`, copied: a real parser with bounded depth,
//! because a damaged or hostile file is exactly what a reader of this format
//! is built to meet. The writer is new. Keys are kept in the order they were
//! inserted, as Python's `json.dumps` keeps them, so a record written here
//! reads the same to a person as one the prototype wrote; nothing depends on
//! the order, since every digest is taken over binary fields.

use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Value>),
    Obj(Vec<(String, Value)>),
}

impl Value {
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Obj(m) => m.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Arr(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_num(&self) -> Option<f64> {
        match self {
            Value::Num(n) => Some(*n),
            _ => None,
        }
    }

    /// A whole, non-negative number: sequence numbers, byte counts.
    pub fn as_u64(&self) -> Option<u64> {
        self.as_num().filter(|n| *n >= 0.0 && n.fract() == 0.0 && *n <= 9_007_199_254_740_992.0).map(|n| n as u64)
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn obj(pairs: Vec<(&str, Value)>) -> Value {
        Value::Obj(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    pub fn str(s: impl Into<String>) -> Value {
        Value::Str(s.into())
    }

    pub fn num(n: u64) -> Value {
        Value::Num(n as f64)
    }

    /// Set a key, replacing its value if present, appending if not.
    pub fn set(&mut self, key: &str, v: Value) {
        if let Value::Obj(m) = self {
            match m.iter_mut().find(|(k, _)| k == key) {
                Some(slot) => slot.1 = v,
                None => m.push((key.to_string(), v)),
            }
        }
    }

    /// Escape a string as a JSON literal, quotes included.
    pub fn quote(s: &str) -> String {
        let mut out = String::with_capacity(s.len() + 2);
        out.push('"');
        for c in s.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
                c => out.push(c),
            }
        }
        out.push('"');
        out
    }

    /// One line, as `json.dumps(obj)` writes it: ", " and ": " separators.
    pub fn to_json(&self) -> String {
        let mut s = String::new();
        self.write(&mut s, None, 0, false);
        s
    }

    /// Indented by one space a level, as `json.dumps(obj, indent=1)`.
    pub fn to_json_indented(&self) -> String {
        let mut s = String::new();
        self.write(&mut s, Some(1), 0, false);
        s
    }

    /// Exactly as Python's `json.dump(obj, f, indent=n)` writes it: every
    /// character outside ASCII as `\uXXXX` (a surrogate pair above U+FFFF),
    /// `\b` and `\f` by name. What ytq's queue.json is, so a file one ytq
    /// rewrites reads the same to the other, and to a person.
    pub fn to_python_json(&self, indent: Option<usize>) -> String {
        let mut s = String::new();
        self.write(&mut s, indent, 0, true);
        s
    }

    fn quote_ascii(s: &str) -> String {
        let mut out = String::with_capacity(s.len() + 2);
        out.push('"');
        for c in s.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                '\u{8}' => out.push_str("\\b"),
                '\u{c}' => out.push_str("\\f"),
                c if (c as u32) < 0x20 || (c as u32) > 0x7e => {
                    let mut units = [0u16; 2];
                    for u in c.encode_utf16(&mut units) {
                        out.push_str(&format!("\\u{:04x}", u));
                    }
                }
                c => out.push(c),
            }
        }
        out.push('"');
        out
    }

    fn write(&self, out: &mut String, indent: Option<usize>, level: usize, python: bool) {
        let newline = |out: &mut String, level: usize| {
            if let Some(n) = indent {
                out.push('\n');
                out.push_str(&" ".repeat(n * level));
            }
        };
        let sep = if indent.is_some() { "," } else { ", " };
        match self {
            Value::Null => out.push_str("null"),
            Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Value::Num(n) => {
                if n.fract() == 0.0 && n.abs() < 1e16 {
                    out.push_str(&format!("{}", *n as i64));
                } else {
                    out.push_str(&format!("{n}"));
                }
            }
            Value::Str(s) => out.push_str(&if python { Value::quote_ascii(s) } else { Value::quote(s) }),
            Value::Arr(v) => {
                if v.is_empty() {
                    out.push_str("[]");
                    return;
                }
                out.push('[');
                for (i, x) in v.iter().enumerate() {
                    if i > 0 {
                        out.push_str(sep);
                    }
                    newline(out, level + 1);
                    x.write(out, indent, level + 1, python);
                }
                newline(out, level);
                out.push(']');
            }
            Value::Obj(m) => {
                if m.is_empty() {
                    out.push_str("{}");
                    return;
                }
                out.push('{');
                for (i, (k, x)) in m.iter().enumerate() {
                    if i > 0 {
                        out.push_str(sep);
                    }
                    newline(out, level + 1);
                    out.push_str(&if python { Value::quote_ascii(k) } else { Value::quote(k) });
                    out.push_str(": ");
                    x.write(out, indent, level + 1, python);
                }
                newline(out, level);
                out.push('}');
            }
        }
    }
}

#[derive(Debug)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

pub fn parse(src: &str) -> Result<Value, Error> {
    let b: Vec<char> = src.chars().collect();
    let mut p = Parser { b: &b, i: 0, depth: 0 };
    p.ws();
    let v = p.value()?;
    p.ws();
    if p.i != p.b.len() {
        return Err(Error(format!("trailing input at char {}", p.i)));
    }
    Ok(v)
}

pub fn parse_bytes(src: &[u8]) -> Result<Value, Error> {
    parse(std::str::from_utf8(src).map_err(|_| Error("not UTF-8".into()))?)
}

struct Parser<'a> {
    b: &'a [char],
    i: usize,
    depth: usize,
}

const MAX_DEPTH: usize = 64;

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<char> {
        self.b.get(self.i).copied()
    }

    fn ws(&mut self) {
        while matches!(self.peek(), Some(' ' | '\t' | '\n' | '\r')) {
            self.i += 1;
        }
    }

    fn eat(&mut self, c: char) -> Result<(), Error> {
        if self.peek() == Some(c) {
            self.i += 1;
            Ok(())
        } else {
            Err(Error(format!("expected {:?} at char {}", c, self.i)))
        }
    }

    fn lit(&mut self, word: &str) -> Result<(), Error> {
        for c in word.chars() {
            self.eat(c)?;
        }
        Ok(())
    }

    fn value(&mut self) -> Result<Value, Error> {
        if self.depth > MAX_DEPTH {
            return Err(Error("nested too deeply".into()));
        }
        match self.peek() {
            Some('{') => self.object(),
            Some('[') => self.array(),
            Some('"') => Ok(Value::Str(self.string()?)),
            Some('t') => {
                self.lit("true")?;
                Ok(Value::Bool(true))
            }
            Some('f') => {
                self.lit("false")?;
                Ok(Value::Bool(false))
            }
            Some('n') => {
                self.lit("null")?;
                Ok(Value::Null)
            }
            Some(c) if c == '-' || c.is_ascii_digit() => self.number(),
            Some(c) => Err(Error(format!("unexpected {:?} at char {}", c, self.i))),
            None => Err(Error("input ended early".into())),
        }
    }

    fn object(&mut self) -> Result<Value, Error> {
        self.eat('{')?;
        self.depth += 1;
        let mut map: Vec<(String, Value)> = Vec::new();
        self.ws();
        if self.peek() == Some('}') {
            self.i += 1;
            self.depth -= 1;
            return Ok(Value::Obj(map));
        }
        loop {
            self.ws();
            let k = self.string()?;
            self.ws();
            self.eat(':')?;
            self.ws();
            let v = self.value()?;
            match map.iter_mut().find(|(key, _)| *key == k) {
                Some(slot) => slot.1 = v,
                None => map.push((k, v)),
            }
            self.ws();
            match self.peek() {
                Some(',') => self.i += 1,
                Some('}') => {
                    self.i += 1;
                    break;
                }
                _ => return Err(Error(format!("expected , or }} at char {}", self.i))),
            }
        }
        self.depth -= 1;
        Ok(Value::Obj(map))
    }

    fn array(&mut self) -> Result<Value, Error> {
        self.eat('[')?;
        self.depth += 1;
        let mut out = Vec::new();
        self.ws();
        if self.peek() == Some(']') {
            self.i += 1;
            self.depth -= 1;
            return Ok(Value::Arr(out));
        }
        loop {
            self.ws();
            out.push(self.value()?);
            self.ws();
            match self.peek() {
                Some(',') => self.i += 1,
                Some(']') => {
                    self.i += 1;
                    break;
                }
                _ => return Err(Error(format!("expected , or ] at char {}", self.i))),
            }
        }
        self.depth -= 1;
        Ok(Value::Arr(out))
    }

    fn string(&mut self) -> Result<String, Error> {
        self.eat('"')?;
        let mut out = String::new();
        loop {
            match self.peek() {
                None => return Err(Error("string never closed".into())),
                Some('"') => {
                    self.i += 1;
                    return Ok(out);
                }
                Some('\\') => {
                    self.i += 1;
                    let c = self.peek().ok_or_else(|| Error("escape at end".into()))?;
                    self.i += 1;
                    match c {
                        '"' => out.push('"'),
                        '\\' => out.push('\\'),
                        '/' => out.push('/'),
                        'b' => out.push('\u{8}'),
                        'f' => out.push('\u{c}'),
                        'n' => out.push('\n'),
                        'r' => out.push('\r'),
                        't' => out.push('\t'),
                        'u' => {
                            let mut n = 0u32;
                            for _ in 0..4 {
                                let h = self.peek().ok_or_else(|| Error("short \\u".into()))?;
                                n = n * 16 + h.to_digit(16).ok_or_else(|| Error("bad \\u digit".into()))?;
                                self.i += 1;
                            }
                            // Python writes non-ASCII as surrogate pairs by default
                            // (ensure_ascii): join a high and a low half.
                            if (0xD800..0xDC00).contains(&n) && self.b.get(self.i) == Some(&'\\') && self.b.get(self.i + 1) == Some(&'u') {
                                let save = self.i;
                                self.i += 2;
                                let mut lo = 0u32;
                                let mut ok = true;
                                for _ in 0..4 {
                                    match self.peek().and_then(|h| h.to_digit(16)) {
                                        Some(d) => {
                                            lo = lo * 16 + d;
                                            self.i += 1;
                                        }
                                        None => {
                                            ok = false;
                                            break;
                                        }
                                    }
                                }
                                if ok && (0xDC00..0xE000).contains(&lo) {
                                    n = 0x10000 + ((n - 0xD800) << 10) + (lo - 0xDC00);
                                } else {
                                    self.i = save;
                                }
                            }
                            out.push(char::from_u32(n).unwrap_or('\u{fffd}'));
                        }
                        c => return Err(Error(format!("bad escape \\{}", c))),
                    }
                }
                Some(c) => {
                    self.i += 1;
                    out.push(c);
                }
            }
        }
    }

    fn number(&mut self) -> Result<Value, Error> {
        let start = self.i;
        if self.peek() == Some('-') {
            self.i += 1;
        }
        while matches!(self.peek(), Some(c) if c.is_ascii_digit() || c == '.' || c == 'e' || c == 'E' || c == '+' || c == '-') {
            self.i += 1;
        }
        let s: String = self.b[start..self.i].iter().collect();
        s.parse::<f64>().map(Value::Num).map_err(|_| Error(format!("not a number: {}", s)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_the_prototype_writes() {
        let v = parse(r#"{"prev": "ab", "entries": [[3, "cd"], [4, "ef"]], "sig": null, "stream": {"key": "ssh-ed25519 AAAA x"}}"#).unwrap();
        assert_eq!(v.get("prev").unwrap().as_str(), Some("ab"));
        let e = v.get("entries").unwrap().as_array().unwrap();
        assert_eq!(e[1].as_array().unwrap()[0].as_u64(), Some(4));
        assert_eq!(v.get("sig"), Some(&Value::Null));
        assert_eq!(v.get("stream").unwrap().get("key").unwrap().as_str(), Some("ssh-ed25519 AAAA x"));
    }

    #[test]
    fn writes_as_python_does() {
        let v = Value::obj(vec![("a", Value::num(1)), ("b", Value::Arr(vec![Value::Bool(true), Value::Null])), ("c", Value::str("x\"y"))]);
        assert_eq!(v.to_json(), r#"{"a": 1, "b": [true, null], "c": "x\"y"}"#);
        assert_eq!(Value::obj(vec![("a", Value::num(1)), ("b", Value::str("z"))]).to_json_indented(), "{\n \"a\": 1,\n \"b\": \"z\"\n}");
        assert_eq!(parse(&v.to_json_indented()).unwrap(), v);
    }

    #[test]
    fn joins_surrogate_pairs() {
        assert_eq!(parse(r#""café 😀""#).unwrap().as_str(), Some("café 😀"));
    }

    #[test]
    fn refuses_junk() {
        for bad in ["{", "[1,]", "{\"a\"}", "tru", "\"unterminated", "{} {}", ""] {
            assert!(parse(bad).is_err(), "accepted {:?}", bad);
        }
        let deep = "[".repeat(5000) + &"]".repeat(5000);
        assert!(parse(&deep).is_err());
    }
}
