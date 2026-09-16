// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Paul Richeson

//! A download's live record, and how it reads: the Python ytq's `stage()`,
//! `stream_of()`, `size_of()`, `describe()`, `compact()`, `bar()`,
//! `live_view()`, `fmt_size()` and `fmt_secs()`.
//!
//! The record is kept in the entry because the process downloading is often
//! not the one looking, and either may be the Python or the Rust ytq -- so the
//! words here are the Python ytq's, character for character.

use crate::format::json::Value;

use crate::ytq::sys;

/// `number(v)`: a float, 0.0 where there is none.
pub fn number(v: Option<&Value>) -> f64 {
    match v {
        Some(Value::Num(n)) => *n,
        Some(Value::Bool(b)) => f64::from(u8::from(*b)),
        Some(Value::Str(s)) => s.trim().parse().unwrap_or(0.0),
        _ => 0.0,
    }
}

/// Python's `str(v)` for a JSON value.
pub fn py_str(v: &Value) -> String {
    match v {
        Value::Str(s) => s.clone(),
        Value::Num(n) if n.fract() == 0.0 && n.abs() < 1e16 => format!("{}", *n as i64),
        Value::Num(n) => format!("{n}"),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Null => "None".into(),
        other => other.to_json(),
    }
}

/// Python truthiness.
pub fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Num(n)) => *n != 0.0,
        Some(Value::Str(s)) => !s.is_empty(),
        Some(Value::Arr(a)) => !a.is_empty(),
        Some(Value::Obj(o)) => !o.is_empty(),
    }
}

/// `na(v)`: a value from a yt-dlp template, or "" where yt-dlp had none to give.
pub fn na(v: Option<&Value>) -> String {
    if !truthy(v) {
        return String::new();
    }
    let s = py_str(v.unwrap());
    let s = s.trim();
    if matches!(s, "NA" | "None" | "none") || s.starts_with("Unknown") {
        String::new()
    } else {
        s.to_string()
    }
}

/// `fmt_size(n)`.
pub fn fmt_size(n: f64) -> String {
    let mut n = n;
    for unit in ["B", "KiB", "MiB", "GiB"] {
        if n < 1024.0 || unit == "GiB" {
            return if unit == "B" { format!("{} {}", n.trunc() as i64, unit) } else { format!("{:.1} {}", n, unit) };
        }
        n /= 1024.0;
    }
    unreachable!()
}

/// `fmt_secs(s)`.
pub fn fmt_secs(s: f64) -> String {
    let s = s.max(0.0) as i64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

pub const PROGRESS_KEYS: [&str; 14] = ["pct", "got_b", "total_b", "est_b", "speed", "eta", "elapsed", "frag", "frags", "format", "vcodec", "acodec", "height", "state"];

/// Python's `\S+` at the start of s: its length in bytes.
fn nonspace_run(s: &str) -> usize {
    s.find(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)).unwrap_or(s.len())
}

/// `stage(line, lv)`: read one line of yt-dlp's own output into lv. True when
/// the line begins a new step.
pub fn stage(line: &str, lv: &mut Value, now: f64) -> bool {
    let before = (lv.get("phase").cloned(), lv.get("part").cloned());
    if let Some(formats) = info_formats(line) {
        let parts = formats.split('+').count();
        lv.set("phase", Value::str("starting"));
        lv.set("formats", Value::str(formats));
        lv.set("parts", Value::num(parts as u64));
    } else if let Some(dest) = line.strip_prefix("[download] Destination: ") {
        if let Value::Obj(m) = lv {
            m.retain(|(k, _)| !PROGRESS_KEYS.contains(&k.as_str()));
        }
        let part = number(lv.get("part")) as u64 + 1;
        lv.set("phase", Value::str("downloading"));
        lv.set("part", Value::num(part));
        lv.set("dest", Value::str(dest));
    } else if let Some(dest) = line.strip_prefix("[download] ").and_then(|r| r.strip_suffix(" has already been downloaded")) {
        let part = number(lv.get("part")) as u64 + 1;
        lv.set("phase", Value::str("already on disk"));
        lv.set("part", Value::num(part));
        lv.set("dest", Value::str(dest));
    } else if let Some(dest) = line.strip_prefix("[Merger] Merging formats into ") {
        lv.set("phase", Value::str("merging"));
        lv.set("dest", Value::str(dest.trim_matches('"')));
    } else if let Some(name) = postprocessor(line) {
        lv.set("phase", Value::str(format!("post-processing ({name})")));
    } else if line.starts_with("Deleting original file") {
        lv.set("phase", Value::str("removing the part files"));
    }
    if (lv.get("phase").cloned(), lv.get("part").cloned()) == before {
        return false;
    }
    lv.set("since", Value::Num(now));
    true
}

/// `\[info\] \S+: Downloading \d+ format\(s\): (\S+)` at the start of line.
fn info_formats(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("[info] ")?;
    let id = nonspace_run(rest);
    if id == 0 {
        return None;
    }
    // \S+ is greedy and backtracks: the id ends at the last ": Downloading " it can.
    let rest = &rest[..];
    for (i, _) in rest.match_indices(": Downloading ") {
        if i == 0 || i > id {
            continue;
        }
        let after = &rest[i + ": Downloading ".len()..];
        let digits = after.chars().take_while(|c| c.is_ascii_digit()).count();
        if digits == 0 {
            continue;
        }
        if let Some(tail) = after[digits..].strip_prefix(" format(s): ") {
            let n = nonspace_run(tail);
            if n > 0 {
                return Some(&tail[..n]);
            }
        }
    }
    None
}

/// `\[(Fixup\w*|FFmpeg\w*|Embed\w*|Metadata)\] ` at the start of line.
fn postprocessor(line: &str) -> Option<&str> {
    let rest = line.strip_prefix('[')?;
    let end = rest.find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(rest.len());
    let word = &rest[..end];
    if !rest[end..].starts_with("] ") {
        return None;
    }
    let ok = word == "Metadata" || word.starts_with("Fixup") || word.starts_with("FFmpeg") || word.starts_with("Embed");
    ok.then_some(word)
}

/// `stream_of(lv)`: 'video f299 avc1.64002a 1080p', as far as yt-dlp has said.
pub fn stream_of(lv: &Value) -> String {
    let v = na(lv.get("vcodec"));
    let a = na(lv.get("acodec"));
    let mut fmt = na(lv.get("format"));
    if fmt.is_empty() && truthy(lv.get("formats")) && truthy(lv.get("part")) {
        let ids: Vec<String> = py_str(lv.get("formats").unwrap()).split('+').map(str::to_string).collect();
        let part = number(lv.get("part")) as usize;
        fmt = if part >= 1 && part <= ids.len() { ids[part - 1].clone() } else { String::new() };
    }
    let kind = if !v.is_empty() && !a.is_empty() {
        "video and audio"
    } else if !v.is_empty() {
        "video"
    } else if !a.is_empty() {
        "audio"
    } else {
        ""
    };
    let height = if !v.is_empty() && !na(lv.get("height")).is_empty() { format!("{}p", na(lv.get("height"))) } else { String::new() };
    let codec = if !v.is_empty() { v.clone() } else { a.clone() };
    let fpart = if fmt.is_empty() { String::new() } else { format!("f{fmt}") };
    let words: Vec<&str> = [kind, fpart.as_str(), codec.as_str(), height.as_str()].into_iter().filter(|x| !x.is_empty()).collect();
    if words.is_empty() {
        "?".into()
    } else {
        words.join(" ")
    }
}

/// `size_of(lv)`: '369.0 MiB of 816.6 MiB', with ~ where the total is an estimate.
pub fn size_of(lv: &Value) -> String {
    let got = number(lv.get("got_b"));
    let total = number(lv.get("total_b"));
    let est = number(lv.get("est_b"));
    if total != 0.0 || est != 0.0 {
        return format!("{} of {}{}", fmt_size(got), if total != 0.0 { "" } else { "~" }, fmt_size(if total != 0.0 { total } else { est }));
    }
    if got != 0.0 {
        fmt_size(got)
    } else {
        String::new()
    }
}

fn phase(lv: &Value) -> String {
    lv.get("phase").map(py_str).filter(|s| !s.is_empty()).unwrap_or_default()
}

/// `describe(lv)`: the step a download is on, in words.
pub fn describe(lv: &Value) -> String {
    let ph = if truthy(lv.get("phase")) { phase(lv) } else { "starting".into() };
    let part = if truthy(lv.get("part")) && truthy(lv.get("parts")) {
        format!(" part {} of {}", py_str(lv.get("part").unwrap()), py_str(lv.get("parts").unwrap()))
    } else {
        String::new()
    };
    if ph == "downloading" || ph == "already on disk" {
        return format!("{ph}{part}: {}", stream_of(lv));
    }
    if ph == "merging" {
        return "merging the video and audio into one file (ffmpeg)".into();
    }
    ph
}

/// `compact(lv)`: '1/2 45.2% 12.3MiB/s eta 00:31', or the step.
pub fn compact(lv: &Value) -> String {
    let ph = phase(lv);
    if ph != "downloading" || na(lv.get("pct")).is_empty() {
        return ph;
    }
    let part = if truthy(lv.get("part")) && truthy(lv.get("parts")) {
        format!("{}/{} ", py_str(lv.get("part").unwrap()), py_str(lv.get("parts").unwrap()))
    } else {
        String::new()
    };
    let eta = na(lv.get("eta"));
    let pct = lv.get("pct").map(py_str).unwrap_or_default();
    format!("{}{} {}{}", part, pct, na(lv.get("speed")), if eta.is_empty() { String::new() } else { format!(" eta {eta}") }).trim().to_string()
}

/// `bar(frac, width)`.
pub fn bar(frac: f64, width: usize) -> String {
    let n = (frac.clamp(0.0, 1.0) * width as f64).py_round() as usize;
    format!("[{}{}]", "#".repeat(n), "-".repeat(width - n))
}

// Not f64::round_ties_even: that arrived in Rust 1.77, and this workspace
// builds on 1.70.
trait PyRound {
    fn py_round(self) -> f64;
}

impl PyRound for f64 {
    /// Python 3's round(): halves go to the even neighbour.
    fn py_round(self) -> f64 {
        let r = self.round();
        if (self - self.trunc()).abs() == 0.5 {
            2.0 * (self / 2.0).round()
        } else {
            r
        }
    }
}

/// `os.path.splitext`.
pub fn splitext(path: &str) -> (&str, &str) {
    let base_start = path.rfind('/').map_or(0, |i| i + 1);
    let base = &path[base_start..];
    let lead = base.len() - base.trim_start_matches('.').len();
    match base[lead..].rfind('.') {
        Some(i) => path.split_at(base_start + lead + i),
        None => (path, ""),
    }
}

/// `live_view(it, width)`: lines on a download under way, for the window and
/// `ytq status`, from the live record its runner keeps in the entry.
pub fn live_view(it: &Value, width: i64, now: f64) -> Vec<String> {
    let empty = Value::Obj(Vec::new());
    let lv = it.get("live").filter(|l| truthy(Some(l))).unwrap_or(&empty);
    let since = lv.get("since").map(|v| number(Some(v))).unwrap_or(now);
    let mut rows = vec![format!("now      {}  (for {})", describe(lv), fmt_secs(now - since))];
    let ph = lv.get("phase").map(py_str);
    let bw = (width - 64).clamp(10, 40) as usize;
    let pct = na(lv.get("pct"));
    if ph.as_deref() == Some("downloading") && !pct.is_empty() {
        let frag = if !na(lv.get("frag")).is_empty() && !na(lv.get("frags")).is_empty() {
            format!("  fragment {} of {}", py_str(lv.get("frag").unwrap()), py_str(lv.get("frags").unwrap()))
        } else {
            String::new()
        };
        let raw_pct = py_str(lv.get("pct").unwrap());
        let frac = number(Some(&Value::str(raw_pct.trim_end_matches('%')))) / 100.0;
        rows.push(format!(
            "progress {} {}  {}  {}  eta {}{}",
            bar(frac, bw),
            raw_pct,
            size_of(lv),
            Some(na(lv.get("speed"))).filter(|s| !s.is_empty()).unwrap_or_else(|| "?".into()),
            Some(na(lv.get("eta"))).filter(|s| !s.is_empty()).unwrap_or_else(|| "?".into()),
            frag
        ));
    } else if ph.as_deref() == Some("merging") && truthy(lv.get("dest")) {
        let dest = py_str(lv.get("dest").unwrap());
        let (root, ext) = splitext(&dest);
        let wrote = std::fs::metadata(format!("{root}.temp{ext}")).ok().map(|m| m.len() as f64);
        let want = number(lv.get("done_b"));
        match wrote {
            Some(w) if want != 0.0 => rows.push(format!(
                "progress {} ~{}%  {} written of about {}",
                bar(w / want, bw),
                (100.0f64).min((w * 100.0 / want).floor()) as i64,
                fmt_size(w),
                fmt_size(want)
            )),
            Some(w) => rows.push(format!("progress {} written", fmt_size(w))),
            None => {}
        }
    }
    let attempts = it.get("attempts").map(py_str).unwrap_or_else(|| "?".into());
    let parts_done = number(lv.get("parts_done")) as i64;
    let overall = [
        format!("attempt {attempts}"),
        if truthy(lv.get("cookies")) { "with Brave's cookies".into() } else { "plain yt-dlp".into() },
        if truthy(lv.get("started")) { format!("started {} ago", fmt_secs(now - number(lv.get("started")))) } else { String::new() },
        if truthy(lv.get("parts_done")) {
            format!("{} fetched in {} part{}", fmt_size(number(lv.get("done_b"))), parts_done, if parts_done == 1 { "" } else { "s" })
        } else {
            String::new()
        },
        if truthy(lv.get("pid")) { format!("runner pid {}", py_str(lv.get("pid").unwrap())) } else { String::new() },
    ];
    rows.push(format!("overall  {}", overall.iter().filter(|o| !o.is_empty()).cloned().collect::<Vec<_>>().join(", ")));
    if truthy(lv.get("dest")) {
        let dest = py_str(lv.get("dest").unwrap());
        rows.push(format!("file     {}", dest.rsplit('/').next().unwrap_or("")));
    }
    if truthy(lv.get("last")) {
        rows.push(format!("yt-dlp   {}", py_str(lv.get("last").unwrap())));
    }
    rows
}

/// For callers that want the time now, as the Python ytq's `time.time()`.
pub fn now() -> f64 {
    sys::now()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::json;

    #[test]
    fn sizes_and_times() {
        assert_eq!(fmt_size(0.0), "0 B");
        assert_eq!(fmt_size(1023.9), "1023 B");
        assert_eq!(fmt_size(422.93 * 1024.0), "422.9 KiB");
        assert_eq!(fmt_size(8e9), "7.5 GiB");
        assert_eq!(fmt_secs(2.9), "0:02");
        assert_eq!(fmt_secs(3725.0), "1:02:05");
    }

    #[test]
    fn stages_from_yt_dlp_lines() {
        let mut lv = json::parse("{}").unwrap();
        assert!(stage("[info] mkPc3DCZ-Ec: Downloading 1 format(s): 299+140", &mut lv, 1.0));
        assert_eq!(describe(&lv), "starting");
        assert!(stage("[download] Destination: /x/Fake_Title_FAKEID0000A.f299.mp4", &mut lv, 2.0));
        assert_eq!(describe(&lv), "downloading part 1 of 2: f299");
        assert!(!stage("[download]  12.5% of 7.6MiB", &mut lv, 3.0));
        assert!(stage("[Merger] Merging formats into \"/x/Fake.mp4\"", &mut lv, 4.0));
        assert!(stage("[Metadata] Adding metadata to \"/x/Fake.mp4\"", &mut lv, 5.0));
        assert_eq!(describe(&lv), "post-processing (Metadata)");
        assert!(!stage("[MetadataParser] Parsed safe_title", &mut lv, 6.0));
        assert!(stage("Deleting original file /x/a.f140.m4a (pass -k to keep)", &mut lv, 7.0));
        assert_eq!(splitext("/a/b.temp.mp4"), ("/a/b.temp", ".mp4"));
        assert_eq!(splitext("/a/.hidden"), ("/a/.hidden", ""));
    }

    #[test]
    fn the_reports_status_panel() {
        let it = json::parse(r#"{"attempts": 1, "live": {"phase": "downloading", "part": 1, "parts": 2, "formats": "299+140",
            "since": 100.0, "started": 99.0, "pid": 13298, "cookies": false, "done_b": 0, "parts_done": 0,
            "pct": "62.5%", "got_b": "5033164", "total_b": "7969177", "est_b": "NA", "speed": "2.00MiB/s", "eta": "00:03",
            "format": "299", "vcodec": "avc1.64002a", "acodec": "none", "height": "1080", "dest": "/d/Fake_Title_FAKEID0000A.f299.mp4"}}"#).unwrap();
        let rows = live_view(&it, 110, 102.5);
        assert_eq!(rows[0], "now      downloading part 1 of 2: video f299 avc1.64002a 1080p  (for 0:02)");
        assert_eq!(rows[1], "progress [#########################---------------] 62.5%  4.8 MiB of 7.6 MiB  2.00MiB/s  eta 00:03");
        assert_eq!(rows[2], "overall  attempt 1, plain yt-dlp, started 0:03 ago, runner pid 13298");
        assert_eq!(rows[3], "file     Fake_Title_FAKEID0000A.f299.mp4");
        assert_eq!(compact(it.get("live").unwrap()), "1/2 62.5% 2.00MiB/s eta 00:03");
    }
}
