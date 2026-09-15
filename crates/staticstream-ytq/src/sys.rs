// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Paul Richeson

//! The few C library calls ytq needs that std does not offer: local time with
//! its UTC offset, `flock`, and signalling a process group.
//!
//! Each is the C library's own, which std already links -- no crate. They
//! matter for sharing with the Python ytq: Python's `fcntl.flock` is this same
//! `flock`, so a lock one takes the other honours, and `time.strftime` is this
//! same `strftime`, so the two write the same times into the log and the notes.

use std::ffi::{CStr, CString};
use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::os::raw::{c_char, c_int, c_long};

#[repr(C)]
struct Tm {
    tm_sec: c_int,
    tm_min: c_int,
    tm_hour: c_int,
    tm_mday: c_int,
    tm_mon: c_int,
    tm_year: c_int,
    tm_wday: c_int,
    tm_yday: c_int,
    tm_isdst: c_int,
    // Present in musl, glibc and macOS alike, in this order.
    tm_gmtoff: c_long,
    tm_zone: *const c_char,
}

extern "C" {
    fn localtime_r(t: *const i64, out: *mut Tm) -> *mut Tm;
    fn strftime(s: *mut c_char, max: usize, format: *const c_char, tm: *const Tm) -> usize;
    fn tzset();
    fn flock(fd: c_int, operation: c_int) -> c_int;
    fn kill(pid: c_int, sig: c_int) -> c_int;
    fn pipe(fds: *mut c_int) -> c_int;
    fn fcntl(fd: c_int, cmd: c_int, ...) -> c_int;
    fn signal(signum: c_int, handler: extern "C" fn(c_int)) -> usize;
}

const F_SETFD: c_int = 2;
const FD_CLOEXEC: c_int = 1;

/// A pipe whose two ends are not inherited by other children: the read end
/// for this process, the write end to give one child as both stdout and
/// stderr -- Python's `stdout=PIPE, stderr=STDOUT`, lines in the order they
/// were written. Without close-on-exec a yt-dlp started at the same moment by
/// another thread would hold the write end open, and the reader would never
/// see the end of the stream.
pub fn pipe_pair() -> io::Result<(File, File)> {
    use std::os::fd::FromRawFd;
    let mut fds = [0 as c_int; 2];
    // SAFETY: fds is a two-int array, as pipe(2) requires; both descriptors
    // are new and owned by the Files made from them.
    unsafe {
        if pipe(fds.as_mut_ptr()) != 0 {
            return Err(io::Error::last_os_error());
        }
        for fd in fds {
            fcntl(fd, F_SETFD, FD_CLOEXEC);
        }
        Ok((File::from_raw_fd(fds[0]), File::from_raw_fd(fds[1])))
    }
}

/// Run `handler` on each of `signals`. The handler may only set a flag.
pub fn on_signals(signals: &[i32], handler: extern "C" fn(c_int)) {
    for &s in signals {
        // SAFETY: installing a handler that does nothing but store to an atomic.
        unsafe {
            signal(s, handler);
        }
    }
}

pub const SIGHUP: i32 = 1;
pub const SIGINT: i32 = 2;

pub const LOCK_SH: i32 = 1;
pub const LOCK_EX: i32 = 2;
pub const LOCK_NB: i32 = 4;
pub const LOCK_UN: i32 = 8;
pub const SIGTERM: i32 = 15;

/// Seconds since the epoch, as a float, as Python's `time.time()`.
pub fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// `time.strftime(format, time.localtime(t))`.
pub fn strftime_local(format: &str, t: f64) -> String {
    let secs = t.floor() as i64;
    let Ok(fmt) = CString::new(format) else { return String::new() };
    let mut tm = Tm {
        tm_sec: 0,
        tm_min: 0,
        tm_hour: 0,
        tm_mday: 0,
        tm_mon: 0,
        tm_year: 0,
        tm_wday: 0,
        tm_yday: 0,
        tm_isdst: 0,
        tm_gmtoff: 0,
        tm_zone: std::ptr::null(),
    };
    let mut buf = vec![0u8; 256];
    // SAFETY: tm and buf outlive the calls; strftime writes at most buf.len()
    // bytes including the terminator and returns 0 when it would not fit.
    let n = unsafe {
        tzset();
        if localtime_r(&secs, &mut tm).is_null() {
            return String::new();
        }
        strftime(buf.as_mut_ptr() as *mut c_char, buf.len(), fmt.as_ptr(), &tm)
    };
    buf.truncate(n);
    String::from_utf8_lossy(&buf).into_owned()
}

/// The zone's abbreviation, for tests that want to know which zone applied.
pub fn zone_name(t: f64) -> String {
    let secs = t.floor() as i64;
    let mut tm: Tm = unsafe { std::mem::zeroed() };
    // SAFETY: as above; tm_zone points into the C library's static zone data.
    unsafe {
        tzset();
        if localtime_r(&secs, &mut tm).is_null() || tm.tm_zone.is_null() {
            return String::new();
        }
        CStr::from_ptr(tm.tm_zone).to_string_lossy().into_owned()
    }
}

/// `fcntl.flock(file, operation)`: Ok, or the error -- EWOULDBLOCK for a
/// non-blocking attempt at a lock someone else holds.
pub fn file_lock(file: &File, operation: i32) -> io::Result<()> {
    loop {
        // SAFETY: the descriptor belongs to `file`, which is alive.
        let r = unsafe { flock(file.as_raw_fd(), operation) };
        if r == 0 {
            return Ok(());
        }
        let e = io::Error::last_os_error();
        if e.kind() != io::ErrorKind::Interrupted {
            return Err(e);
        }
    }
}

/// `os.killpg(pgid, sig)`: yt-dlp and the ffmpeg it started go together.
pub fn kill_group(pgid: u32, sig: i32) -> io::Result<()> {
    // SAFETY: plain system call; a negative pid names the process group.
    if unsafe { kill(-(pgid as c_int), sig) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_like_python() {
        // The zone varies by machine; compare against `date`, which is the same libc.
        let t = 1_126_000_000.0; // 2005-09-06
        let mine = strftime_local("%Y-%m-%d %H:%M:%S %z", t);
        let date = std::process::Command::new("date")
            .args(["-d", "@1126000000", "+%Y-%m-%d %H:%M:%S %z"])
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty());
        if let Some(date) = date {
            assert_eq!(mine, date);
        }
        assert_eq!(mine.len(), 25, "{mine}");
    }

    #[test]
    fn a_lock_is_exclusive_across_handles() {
        let path = std::env::temp_dir().join(format!("sstr-lock-test-{}", std::process::id()));
        let a = File::create(&path).unwrap();
        let b = File::options().append(true).open(&path).unwrap();
        file_lock(&a, LOCK_EX).unwrap();
        assert!(file_lock(&b, LOCK_EX | LOCK_NB).is_err());
        file_lock(&a, LOCK_UN).unwrap();
        file_lock(&b, LOCK_EX | LOCK_NB).unwrap();
        let _ = std::fs::remove_file(path);
    }
}
