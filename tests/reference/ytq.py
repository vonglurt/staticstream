#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
# Copyright (c) 2026 Paul Richeson -- part of Copal Linux.
#
# ytq -- a yt-dlp download queue that watches the clipboard.
#
#   ytq                the queue window. The URL on the clipboard right now is
#                      queued at once; while the window has focus, every URL
#                      copied afterwards is checked and queued too.
#   ytq clip           queue the URL on the clipboard. This is Super+Shift+Y.
#                      A clipboard holding more than a URL -- a bookmarks
#                      export, a page of notes -- queues every YouTube video
#                      in it, each once. A YouTube link may lack its https://
#                      (youtube.com/shorts/ID), here and everywhere else.
#   ytq add URL...     the same, for URLs typed in a shell
#   ytq run            download what is queued, in this terminal
#   ytq status         what is downloading, the step it is on, what is left
#   ytq cookies        retry what was waiting on a Brave sign-in
#   ytq transcript URL...  just the captions, as text, for YouTube videos
#   ytq list           every entry: queued, done, waiting, failed
#   ytq clear          forget finished, rejected and failed entries (h twice in
#                      the window); the files and the log stay
#
# FILENAMES are the first word of the uploader's name, the title and the video
# id, in the URL-safe base64 alphabet (A-Z a-z 0-9 - _) and nothing else, then
# the extension: 'Café Tour: Part 2/3 [4K]' by Rick Astley is
# Rick-Cafe_Tour_Part_2_3_4K_dQw4w9WgXcQ.mp4. A YouTube video also gets
# Rick-Cafe_Tour_Part_2_3_4K_dQw4w9WgXcQ.txt beside it: Notes (full title,
# author, URL, published and downloaded times, any license the site states,
# the video's filename), the description, then the captions as plain text,
# fetched once the video is done. The .mp4 carries the same notes in its
# metadata. See NAME_OPTS, META_OPTS and transcript().
#
# CITATIONS. The notes are the parts of a reference someone else can check --
# author, full title, when it was published, the watch URL, when you fetched
# it -- taken at download time, because a title or description can be edited
# and a video made private or deleted after you saw it. The id, in the name
# and the URL, is what finds it again. Automatic captions are speech
# recognition, not a verbatim record, and the Captions line says whether a
# transcript came from them or from the uploader: check a quote against the
# audio. The Author line is the uploader, not necessarily the speaker, and
# Published is the upload, not the event recorded. The merge and the tagging
# copy the streams without re-encoding, and the transcript is the lossy part
# (no timings, a repeated caption line dropped). A faithful copy with good
# notes is still a copy: the guide's "What the copy is" says what that does
# and does not settle, and the License line shows the Creative Commons cases.
# The yt-dlp guide ('guide') covers dates, archiving, quoting and
# accessibility.
#
# AUTOSTART is off until you ask for it:  touch ~/.config/ytq/auto
# With that file there, clip, add and cookies also start downloading in the
# background whenever nothing is downloading already. Without it they only
# queue, and 'ytq run' or the window does the downloading. --run starts a
# runner this once whatever the file says; --no-run never does.
#
# HOW IT WORKS. One file, ~/.local/share/ytq/queue.json, is the queue, and any
# number of ytq processes use it at once. Every change takes queue.lock, reads
# the file afresh, changes what it came to change, writes it back and lets go.
# (Each process used to keep its own copy, and a 'ytq add' made while 'ytq
# run' was downloading vanished at the runner's next save.)
#
# Exactly one process downloads: whichever holds run.lock, with its pid
# written inside. That is 'ytq run', or the window, or the background runner
# that autostart starts when nobody holds the lock. The runner checks
# each new URL (can yt-dlp get it as an MP4, and at what height?) and
# downloads one at a time, best video plus best audio merged by ffmpeg. A
# background runner leaves when the queue is empty, and the next 'ytq clip'
# starts another. It lets go of run.lock while holding queue.lock, and 'ytq
# clip' adds its URL and looks for a runner under that same lock -- so a URL
# cannot land in the gap between "nothing left" and "gone" and sit there.
#
# WHEN IT FAILS. A failed download goes to the back of the queue for one more
# try. If yt-dlp's complaint is about a login, an age gate, a "confirm you are
# not a bot" page or cookies, the retry is the cookie retry, and there is only
# ever one of those per entry: Brave is opened on the URL so you can sign in
# or pass the check, and the entry waits, marked 'cookies'. 'ytq cookies' (or
# 'c' in the window) then runs it again through yt-brave, which finds Brave's
# profile (Flatpak or native) and hands yt-dlp its cookies. A second failure
# after that is final. A runner with a terminal asks for Enter instead, once
# the rest of the queue is done; one in the background sends a notification.
#
# WHAT IT IS DOING. The process that downloads keeps a live record in the
# entry -- the step (looking it up, downloading part 1 of 2, merging, the
# transcript), bytes, speed, ETA, yt-dlp's last line -- and the window shows
# it above the list for whichever process is downloading; 'ytq status' too.
# Quitting the window stops its download, merge and all.
#
# THE LOG, ~/.local/share/ytq/ytq.log, is for finding out what happened. Every
# line carries the pid of the ytq that wrote it, and a download's lines its
# video id. It has every line yt-dlp printed, progress every 30 seconds, each
# part's size and speed, how long each step took, and why a download stopped;
# a runner begins by naming its ytq, its yt-dlp and its settings. Past 4 MiB
# it moves to ytq.log.1 and starts again.
#
# WHY "WHILE IT HAS FOCUS". The clipboard is a shared thing, and a queue that
# grabbed every link you copied for any reason would be a nuisance. So the
# window's watcher only acts while the window is the focused one: copy a link,
# click here, and it is queued; copy a link for a note and nothing happens.
# Focus is asked of the compositor (hyprctl on Hyprland, xdotool on X11);
# where neither answers, the watcher stays on. 'ytq clip' is the other way in:
# it takes the clipboard once, when you ask.
#
# Settings, if you want them, in ~/.config/ytq/config as KEY=VALUE:
#   DIR       where files go            (default ~/Downloads/SharedVM if the
#                                        share is mounted, else XDG_VIDEOS_DIR
#                                        or ~/Videos)
#   FORMAT    yt-dlp -f selector        (default: best MP4 video + M4A audio)
#   PROFILE   Brave profile, passed to yt-brave --profile   (default: Default)
#   KEYRING   passed to yt-brave --keyring, e.g. basictext, for a desktop with no keyring
#   POLL      clipboard poll, seconds   (default 1)
#   SUBS      caption languages for the transcript, a yt-dlp --sub-langs list
#             (default en,en-orig,en-US,en-GB: exact names, since en.* also
#             takes YouTube's translations into English); SUBS= turns it off
# and next to it the empty file ~/.config/ytq/auto, which switches autostart on.
import contextlib, curses, fcntl, glob, html, json, os, re, shlex, shutil, signal, subprocess, sys, textwrap, threading, time

HOME = os.path.expanduser("~")
CONF = os.path.join(os.environ.get("XDG_CONFIG_HOME") or os.path.join(HOME, ".config"), "ytq", "config")
AUTO = os.path.join(os.path.dirname(CONF), "auto")
DATA = os.path.join(os.environ.get("XDG_DATA_HOME") or os.path.join(HOME, ".local", "share"), "ytq")
QUEUE = os.path.join(DATA, "queue.json")
QLOCK = os.path.join(DATA, "queue.lock")
RUNLOCK = os.path.join(DATA, "run.lock")
LOG = os.path.join(DATA, "ytq.log")
DEFAULT_FORMAT = ("bv*[ext=mp4][vcodec^=avc1]+ba[ext=m4a]/bv*[ext=mp4]+ba[ext=m4a]"
                  "/b[ext=mp4]/bv*+ba/b")
# Exact names, not en.*: that also takes YouTube's machine translations into
# English (en-de is English from German), each one more caption request.
DEFAULT_SUBS = "en,en-orig,en-US,en-GB"
COOKIE_WORDS = ("sign in", "log in", "login", "cookies", "age", "bot", "private video",
                "members", "premium", "confirm you", "403", "restricted", "subscriber")
# A URL, strictly enough that a pasted sentence or a path never qualifies:
# scheme, a host with at least one dot or a port, then anything without spaces.
URL_RE = re.compile(r"^https?://(?:[A-Za-z0-9-]+\.)+[A-Za-z0-9-]+(?::\d+)?(?:/\S*)?$|^https?://localhost(?::\d+)?(?:/\S*)?$")
# A YouTube video anywhere in a larger text: watch?v= (v need not come first),
# shorts/, live/, embed/ and youtu.be/, on www., m. or music. The match is the
# 11-character id and nothing after it, so a mix's &list= or a &t=5330s never
# makes the same video a second entry. Channel and playlist pages do not match:
# one bookmark of those would be hundreds of downloads. The https:// may be
# missing, as in a link copied from an address bar or typed: youtube.com/shorts/ID.
# Without it the host must not follow a letter, digit, ., @, / or -, so
# notyoutube.com, foo.youtube.com and evil.com/youtube.com/... do not match.
YT_RE = re.compile(r"(?:https?://|(?<![\w.@/-]))(?:(?:www|m|music)\.)?"
                   r"(?:youtube\.com/(?:watch\?(?:[^\s\"'<>#]*?&)?v=|shorts/|live/|embed/)|youtu\.be/)"
                   r"([A-Za-z0-9_-]{11})(?![A-Za-z0-9_-])")
ORDER = ["downloading", "cookies", "retry-cookies", "checking", "queued", "retry", "done", "failed", "rejected"]
# The history: entries that are over. 'ytq clear', or h twice in the window,
# forgets them; the files they downloaded and their lines in the log stay.
HISTORY = ("done", "failed", "rejected")
# Work a runner has still to do. 'cookies' is not in it: that waits on a person.
PENDING = ("checking", "queued", "retry", "retry-cookies", "downloading")
DOWNLOADABLE = ("retry-cookies", "queued", "retry")
# The filename: Author-Title_id in A-Z a-z 0-9 - _ only. /etc/yt-dlp.conf gives
# plain yt-dlp the same rule; change one, change both. The author is the first
# run of A-Z a-z 0-9 in the uploader's name (the channel's if there is no
# uploader), after #S, and ends at the -; with none the name starts at the
# title, as it did before there was an author. The title starts from
# %(title)#S -- --restrict-filenames for that one field -- which turns é into
# e and blanks what has no ASCII form. The regexes then keep only the
# alphabet, squeeze each run of separators holding a _ into one _ (so ' - '
# goes and Spider-Man stays), cut at 120 and trim the ends. The id gets only
# the first regex, because #S would trim a YouTube id's own leading - or _.
# Both are copies, so --print %(title)s and yt-dlp's own use of id are
# untouched; a title with nothing left (all CJK, say) names the file by id alone.
NAME_OPTS = ["--parse-metadata", "%(uploader,channel|)#S:(?s)(?P<safe_author>.+)",
             "--replace-in-metadata", "safe_author", "^[^A-Za-z0-9]+", "",
             "--replace-in-metadata", "safe_author", "(?s)[^A-Za-z0-9].*", "",
             "--parse-metadata", "%(title)#S:(?s)(?P<safe_title>.+)",
             "--replace-in-metadata", "safe_title", "[^A-Za-z0-9_-]+", "_",
             "--replace-in-metadata", "safe_title", "[-_]*_[-_]*", "_",
             "--replace-in-metadata", "safe_title", "(?<=^.{120}).+", "",
             "--replace-in-metadata", "safe_title", "^[-_]+|[-_]+$", "",
             "--parse-metadata", "id:(?s)(?P<safe_id>.+)",
             "--replace-in-metadata", "safe_id", "[^A-Za-z0-9_-]+", "_"]
NAME = "%(safe_author&{}-|)s%(safe_title&{}_|)s%(safe_id)s.%(ext)s"
# The notes, in the video file itself. --embed-metadata writes the full title,
# the uploader's full name (as artist), the upload date and the description;
# the comment gets the lines a transcript's Notes begin with, the URL and the
# published and downloaded times and the license among them, in UTC -- the
# only zone yt-dlp formats times in. A literal : would end --parse-metadata's
# FROM, so labels and times are written with = and . there and turned back by
# --replace-in-metadata, whose arguments are separate. It needs ffmpeg, so
# download() leaves it out on a machine without one.
META_OPTS = ["--embed-metadata",
             "--parse-metadata", "Title = %(title)s\nAuthor = %(uploader,channel|unknown)s\n"
                                 "URL = %(webpage_url)s\n"
                                 "Published = %(timestamp>%Y-%m-%d %H.%M.%S UTC,upload_date>%Y-%m-%d|unknown)s\n"
                                 "Downloaded = %(epoch>%Y-%m-%d %H.%M.%S UTC)s\n"
                                 "License = %(license|not stated)s:(?s)(?P<meta_comment>.+)",
             "--replace-in-metadata", "meta_comment", "(?m)^(Title|Author|URL|Published|Downloaded|License) = ", r"\1: ",
             "--replace-in-metadata", "meta_comment", r"(\d\d)\.(\d\d)\.(\d\d) UTC", r"\1:\2:\3 UTC"]


def videos_dir():
    # The folder shared with the Mac first, when the share is really mounted:
    # a download there is on both sides at once. The link is made by the
    # shared-folder stage and points at /mnt/share; a link whose share is not
    # mounted would fill the empty mount point instead, so that falls through.
    shared = os.path.join(HOME, "Downloads", "SharedVM")
    if os.path.ismount(os.path.realpath(shared)):
        return shared
    try:
        for line in open(os.path.join(HOME, ".config", "user-dirs.dirs")):
            if line.startswith("XDG_VIDEOS_DIR="):
                return os.path.expandvars(line.split("=", 1)[1].strip().strip('"'))
    except OSError:
        pass
    return os.path.join(HOME, "Videos")


def settings():
    s = {"DIR": videos_dir(), "FORMAT": DEFAULT_FORMAT, "PROFILE": "Default", "KEYRING": "", "POLL": "1", "SUBS": DEFAULT_SUBS}
    try:
        for line in open(CONF):
            line = line.strip()
            if line and not line.startswith("#") and "=" in line:
                k, v = line.split("=", 1)
                s[k.strip().upper()] = os.path.expanduser(v.strip().strip('"'))
    except OSError:
        pass
    return s


S = settings()


LOG_MAX = 4 * 1024 * 1024


def log(msg):
    """Timestamped lines, each tagged with the pid: several ytq processes write here."""
    os.makedirs(DATA, exist_ok=True)
    with contextlib.suppress(OSError):
        if os.path.getsize(LOG) > LOG_MAX:
            os.replace(LOG, LOG + ".1")
    stamp = "%s [%d] " % (time.strftime("%Y-%m-%d %H:%M:%S"), os.getpid())
    with open(LOG, "a") as f:
        f.write("".join(stamp + line + "\n" for line in (str(msg).splitlines() or [""])))


def log_start():
    """A runner's first line: which ytq and yt-dlp did what follows, with what settings."""
    try:
        ver = subprocess.run(["yt-dlp", "--version"], capture_output=True, text=True, timeout=20).stdout.strip()
    except (OSError, subprocess.SubprocessError) as e:
        ver = "(cannot run: %s)" % e
    me = os.path.realpath(__file__)
    try:
        mtime = time.strftime("%Y-%m-%d %H:%M", time.localtime(os.path.getmtime(me)))
    except OSError:
        mtime = "?"
    log("runner '%s' took run.lock: ytq %s of %s, yt-dlp %s, python %s; DIR=%s FORMAT=%s SUBS=%s PROFILE=%s; "
        "/etc/yt-dlp.conf %s" % (" ".join(sys.argv[1:]) or "window", me, mtime, ver or "?", sys.version.split()[0],
                                 S["DIR"], S["FORMAT"], S["SUBS"] or "(off)", S["PROFILE"],
                                 "present" if os.path.exists("/etc/yt-dlp.conf") else "absent"))


def short(url):
    """A YouTube URL's video id, else the URL: the tag on a download's log lines."""
    m = YT_RE.search(url)
    return m.group(1) if m else url


def number(v):
    try:
        return float(v)
    except (TypeError, ValueError):
        return 0.0


def na(v):
    """A value from a yt-dlp template, or '' where yt-dlp had none to give."""
    v = str(v or "").strip()
    return "" if v in ("NA", "None", "none") or v.startswith("Unknown") else v


def fmt_size(n):
    n = number(n)
    for unit in ("B", "KiB", "MiB", "GiB"):
        if n < 1024 or unit == "GiB":
            return ("%d %s" if unit == "B" else "%.1f %s") % (n, unit)
        n /= 1024


def fmt_secs(s):
    s = int(max(0, s))
    return "%d:%02d:%02d" % (s // 3600, s // 60 % 60, s % 60) if s >= 3600 else "%d:%02d" % (s // 60, s % 60)


# Who hears about things. A terminal gets them printed; Super+Shift+Y and the
# background runner have no terminal, so they get a notification; the window
# shows everything itself and only logs.
MODE = {"say": "print" if sys.stdout.isatty() else "notify"}


def say(msg, urgent=False):
    log(msg)
    if MODE["say"] == "print":
        print(msg, flush=True)
    elif MODE["say"] == "notify":
        try:
            subprocess.Popen(["notify-send", "-a", "ytq", "-u", "critical" if urgent else "normal", "ytq", msg],
                             stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        except OSError:
            pass


# --- the queue file ---------------------------------------------------------
def new_entry(url):
    return {"url": url, "title": "", "status": "checking", "quality": "", "attempts": 0,
            "cookie_tried": False, "error": "", "added": int(time.time()), "file": "", "progress": ""}


def find(items, url):
    return next((i for i in items if i["url"] == url), None)


class Queue:
    """queue.json, shared by every ytq process: lock, read, change, write."""

    def __init__(self):
        self.guard = threading.RLock()
        self.depth = 0
        self.fd = None
        self.items = []

    @staticmethod
    def _read():
        try:
            with open(QUEUE) as f:
                items = json.load(f)
            return items if isinstance(items, list) else []
        except (OSError, ValueError):
            return []

    @contextlib.contextmanager
    def edit(self):
        """The live list, to change in place; written back when the block ends."""
        with self.guard:
            if self.depth == 0:
                os.makedirs(DATA, exist_ok=True)
                # A signal can land between open and flock and leave the old
                # handle behind; flock between two handles in one process
                # would then wait on itself forever. Closing it lets go.
                if self.fd:
                    self.fd.close()
                self.fd = open(QLOCK, "a")
                fcntl.flock(self.fd, fcntl.LOCK_EX)
                self.items = self._read()
                self.before = json.dumps(self.items)
            self.depth += 1
            try:
                yield self.items
            finally:
                self.depth -= 1
                if self.depth == 0:
                    try:
                        if json.dumps(self.items) != self.before:
                            tmp = QUEUE + ".tmp"
                            with open(tmp, "w") as f:
                                json.dump(self.items, f, indent=1)
                            os.replace(tmp, QUEUE)
                    finally:
                        fcntl.flock(self.fd, fcntl.LOCK_UN)
                        self.fd.close()
                        self.fd = None

    def snapshot(self):
        """The queue as it stands, for looking at. Writes are renames, so no lock."""
        return self._read()


Q = Queue()


def update(url, **kw):
    """Change one entry. Its new state, or None if it was deleted meanwhile."""
    with Q.edit() as items:
        it = find(items, url)
        if it is None:
            return None
        it.update(kw)
        return dict(it)


class RunLock:
    """run.lock: held by the one process that downloads."""

    def __init__(self):
        self.fd = None

    def take(self, tries=10):
        os.makedirs(DATA, exist_ok=True)
        fd = open(RUNLOCK, "a+")
        # A few tries, because 'ytq status' looking at the lock holds it for a
        # moment, and a runner that gave up on that would leave its URL sitting.
        for n in range(tries):
            try:
                fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
                break
            except OSError:
                if n == tries - 1:
                    fd.close()
                    return False
                time.sleep(0.1)
        fd.seek(0)
        fd.truncate()
        fd.write("%d\n" % os.getpid())
        fd.flush()
        self.fd = fd
        log_start()
        # Nobody else can be downloading now, so an entry still marked so was
        # left behind by a runner that died.
        with Q.edit() as items:
            for it in items:
                if it["status"] == "downloading":
                    log("%s: left 'downloading' (%s) by a runner that is gone; back in the queue"
                        % (short(it["url"]), (it.get("live") or {}).get("phase", "no step recorded")))
                    it.update(status="queued", progress="", live={})
        return True

    def release(self):
        if self.fd:
            fcntl.flock(self.fd, fcntl.LOCK_UN)
            self.fd.close()
            self.fd = None

    def holder(self):
        """The pid holding run.lock, or None."""
        if self.fd:
            return os.getpid()
        try:
            fd = open(RUNLOCK, "a+")
        except OSError:
            return None
        with fd:
            try:
                fcntl.flock(fd, fcntl.LOCK_SH | fcntl.LOCK_NB)
            except OSError:
                fd.seek(0)
                return fd.read().strip() or "?"
            fcntl.flock(fd, fcntl.LOCK_UN)
            return None


RUN = RunLock()


def start_runner():
    """(pid, started): a background 'ytq run --quiet', unless run.lock is held.

    Call it inside Q.edit() -- see HOW IT WORKS for why that matters."""
    pid = RUN.holder()
    if pid:
        return pid, False
    os.makedirs(DATA, exist_ok=True)
    with open(LOG, "a") as logf:
        p = subprocess.Popen([sys.executable, os.path.realpath(__file__), "run", "--quiet"],
                             stdin=subprocess.DEVNULL, stdout=logf, stderr=logf, start_new_session=True)
    log("started a runner, pid %d" % p.pid)
    return p.pid, True


def autostart(flags=()):
    """Do clip, add and cookies start a runner? The auto file says, unless --run or --no-run does."""
    if "--no-run" in flags:
        return False
    return "--run" in flags or os.path.exists(AUTO)


def current_runner():
    """(pid, False) for a runner already downloading, else None."""
    pid = RUN.holder()
    return (pid, False) if pid else None


def enqueue(urls, run=False):
    """Queue whatever is new. (added, runner): runner is (pid, started) when one
    is downloading or was started for this, None when nothing is."""
    added, runner = [], None
    with Q.edit() as items:
        for u in urls:
            if find(items, u) is None:
                items.append(new_entry(u))
                added.append(u)
                log("added " + u)
        if any(i["status"] in PENDING for i in items):
            runner = start_runner() if run else current_runner()
    return added, runner


STOP = threading.Event()
PAUSED = threading.Event()
CURRENT = {"proc": None, "url": None, "cookies": False, "attempts": 0, "live": None}


# --- yt-dlp -------------------------------------------------------------------
def cookie_problem(text):
    t = text.lower()
    return any(w in t for w in COOKIE_WORDS)


def brave_cmd():
    """yt-dlp by way of yt-brave, which owns the Brave profile path and the keyring."""
    cmd = ["yt-brave", "--profile", S["PROFILE"]]
    if S["KEYRING"]:
        cmd += ["--keyring", S["KEYRING"]]
    return cmd


def brave_ready():
    """None if yt-brave can find the profile, else the reason it cannot."""
    try:
        r = subprocess.run(brave_cmd() + ["--path"], capture_output=True, text=True, timeout=10)
    except (OSError, subprocess.SubprocessError) as e:
        return "cannot run yt-brave: %s" % e
    if r.returncode == 0:
        return None
    return (r.stderr.strip().splitlines() or ["yt-brave found no Brave profile"])[0]


def open_browser(url):
    for cmd in (["brave", url], ["flatpak", "run", "com.brave.Browser", url], ["xdg-open", url]):
        try:
            subprocess.Popen(cmd, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                             stderr=subprocess.DEVNULL, start_new_session=True)
            log("opened browser: " + " ".join(cmd[:2]))
            return True
        except OSError:
            continue
    return False


def kill(proc):
    try:
        os.killpg(proc.pid, signal.SIGTERM)
    except OSError:
        pass


def check(url):
    """Can yt-dlp fetch this as an MP4, and at what height? --simulate costs one request."""
    cmd = ["yt-dlp", "--simulate", "--no-playlist", "--no-warnings", "-f", S["FORMAT"],
           "--print", "%(title)s\t%(height)s\t%(ext)s", url]
    t0 = time.time()
    try:
        r = subprocess.run(cmd, capture_output=True, text=True, timeout=180)
    except subprocess.TimeoutExpired:
        log("%s: check: yt-dlp gave no answer in 180 s" % short(url))
        update(url, status="rejected", error="timed out asking yt-dlp about it")
        say("not downloadable (timed out): " + url)
        return
    except OSError as e:
        update(url, status="rejected", error="yt-dlp: %s" % e)
        say("cannot run yt-dlp: %s" % e, urgent=True)
        return
    if r.returncode == 0 and r.stdout.strip():
        title, height, ext = (r.stdout.strip().splitlines()[0].split("\t") + ["", ""])[:3]
        q = ("%sp" % height if height not in ("NA", "", "None") else "?") + " " + ext
        update(url, status="queued", title=title[:200], quality=q, error="")
        log("queued %s [%s] %s (checked in %.1f s)" % (title, q, url, time.time() - t0))
    else:
        log("%s: check: yt-dlp exited %d after %.1f s" % (short(url), r.returncode, time.time() - t0))
        for line in r.stderr.strip().splitlines()[-4:]:
            log("%s: check: %s" % (short(url), line))
        err = (r.stderr.strip().splitlines() or ["no output"])[-1][:300]
        if cookie_problem(err):
            # Not rejected: queue it anyway. The first real attempt is what
            # opens Brave if the complaint holds -- the check is only a look.
            update(url, status="queued", title=url, quality="?", error=err)
            log("queued despite a cookie complaint at check: " + url)
        else:
            update(url, status="rejected", error=err)
            say("not downloadable: %s -- %s" % (url, err))


def vtt_text(path):
    """The words of a WebVTT file, as one wrapped paragraph. YouTube's automatic
    captions show each line again in the next cue, with word timings as inline
    tags: the tags go, and a line the same as the one before it is dropped."""
    lines, header = [], True
    with open(path, encoding="utf-8", errors="replace") as f:
        for raw in f:
            line = raw.strip()
            if header:
                header = bool(line)
            elif "-->" not in line:
                text = html.unescape(re.sub(r"<[^>]*>", "", line)).strip()
                if text and (not lines or text != lines[-1]):
                    lines.append(text)
    return textwrap.fill(" ".join(lines), 78)


def notes(meta, url, lang, video):
    """The top of a transcript: the title; Notes -- full title, author and URL,
    when it was published and downloaded, in local time, the license the site
    states, the video file it goes with, and the captions' language and whether
    the uploader wrote them; the
    video's description when it has one; the Transcript heading."""
    def when(t):
        return time.strftime("%Y-%m-%d %H:%M:%S %z", time.localtime(t))
    # yt-dlp takes the uploader's captions over automatic ones in the same
    # language (process_subtitles), so a language among 'subtitles' -- the
    # uploader's -- is theirs, and any other came from automatic_captions.
    subs = meta.get("subtitles")
    source = ("written by the uploader" if isinstance(subs, dict) and lang in subs
              else "automatic: YouTube's speech recognition, not verbatim")
    title = meta.get("title") or url
    if isinstance(meta.get("timestamp"), (int, float)):
        published = when(meta["timestamp"])
    elif re.fullmatch(r"\d{8}", meta.get("upload_date") or ""):
        d = meta["upload_date"]
        published = "%s-%s-%s" % (d[:4], d[4:6], d[6:])
    else:
        published = "unknown"
    rows = [("Title", title), ("Author", meta.get("uploader") or meta.get("channel") or "unknown"),
            ("URL", meta.get("webpage_url") or url), ("Published", published),
            ("Downloaded", when(time.time())),
            # yt-dlp's license is what the site states: on YouTube, a Creative
            # Commons license's row on the watch page, and nothing otherwise.
            ("License", meta.get("license") or "not stated"), ("Video", video or "none downloaded"),
            ("Captions", "%s, %s" % (lang, source))]
    out = "%s\n\nNotes\n%s\n" % (title, "".join("  %-11s %s\n" % (k + ":", v) for k, v in rows))
    desc = (meta.get("description") or "").strip()
    if desc:
        out += "Description\n%s\n\n" % desc
    return out + "Transcript\n"


def transcript(url, outtmpl, cmd=("yt-dlp",), video=None):
    """(path, None) for a .txt of url's captions, named by the -o template outtmpl
    as the video is; (None, why) when there is none. video is the filename the
    notes give; without it, a video already beside the .txt is named, if any.

    Its own yt-dlp run, after the video's: a caption fetch that fails -- and
    YouTube answers 429 to captions far sooner than to video -- fails the whole
    run it is part of, -i or not, and would put a finished video back in the queue."""
    cmd = list(cmd) + NAME_OPTS + [
           "--skip-download", "--no-simulate", "--no-playlist", "--no-warnings",
           "--write-subs", "--write-auto-subs", "--sub-langs", S["SUBS"] or DEFAULT_SUBS, "--sub-format", "vtt",
           "--print", "video:STEM %(filename)s",
           "--print", "video:META %(.{title,uploader,channel,webpage_url,timestamp,upload_date,license,description,subtitles})j",
           "-o", outtmpl, url]
    tag, t0 = short(url), time.time()
    log("%s: fetching the transcript, captions %s" % (tag, S["SUBS"] or DEFAULT_SUBS))
    log("run: " + " ".join(shlex.quote(c) for c in cmd))
    try:
        r = subprocess.run(cmd, stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=300)
    except (OSError, subprocess.SubprocessError) as e:
        log("%s: transcript: %s" % (tag, e))
        return None, "cannot run %s: %s" % (cmd[0], e)
    log("%s: transcript run exited %d after %s" % (tag, r.returncode, fmt_secs(time.time() - t0)))
    for line in r.stderr.strip().splitlines()[-8:]:
        log("%s: transcript: yt-dlp: %s" % (tag, line))
    stem, meta = "", {}
    for line in r.stdout.splitlines():
        if line.startswith("STEM "):
            # The extension is a guess made without choosing a format; the stem is right.
            stem = os.path.splitext(line[5:])[0]
        elif line.startswith("META "):
            with contextlib.suppress(ValueError):
                meta = json.loads(line[5:])
    # One file per language that matched: stem.en.vtt, stem.en-orig.vtt. The
    # shortest name, the plain language, is taken; for a language the uploader
    # captioned, yt-dlp has already chosen those captions over automatic ones.
    vtts = sorted(glob.glob(glob.escape(stem) + ".*.vtt"), key=lambda p: (len(p), p)) if stem else []
    log("%s: captions written: %s" % (tag, ", ".join(os.path.basename(v) for v in vtts) or "none"))
    if not vtts:
        return None, (r.stderr.strip().splitlines() or ["no captions matching " + (S["SUBS"] or DEFAULT_SUBS)])[-1][:300]
    lang = vtts[0][len(stem) + 1:-len(".vtt")]
    try:
        text = vtt_text(vtts[0])
        if text:
            with open(stem + ".txt", "w", encoding="utf-8") as f:
                if not video:
                    video = next((os.path.basename(p) for p in sorted(glob.glob(glob.escape(stem) + ".*"))
                                  if os.path.splitext(p)[1].lower() in (".mp4", ".mkv", ".webm", ".m4v", ".mov")), None)
                f.write("%s%s\n" % (notes(meta, url, lang, video), text))
            log("%s: wrote %s: %d words from the %s captions" % (tag, stem + ".txt", len(text.split()), lang))
    except OSError as e:
        return None, "cannot write the transcript: %s" % e
    finally:
        for v in vtts:
            with contextlib.suppress(OSError):
                os.remove(v)
    return (stem + ".txt", None) if text else (None, "the %s captions are empty" % lang)


def claim():
    """(entry, with_cookies) for the next download, marked as downloading in one locked step."""
    with Q.edit() as items:
        for st in DOWNLOADABLE:
            it = next((i for i in items if i["status"] == st), None)
            if it:
                cookies = st == "retry-cookies"
                it.update(status="downloading", attempts=it["attempts"] + 1, progress="starting",
                          cookie_tried=it["cookie_tried"] or cookies)
                return dict(it), cookies
    return None, False


# What yt-dlp reports as it downloads: one PROGRESS line per update, fields
# in PROGRESS_KEYS order. Byte counts come raw, so parts can be added up.
PROGRESS_KEYS = ("pct", "got_b", "total_b", "est_b", "speed", "eta", "elapsed", "frag", "frags",
                 "format", "vcodec", "acodec", "height", "state")
PROGRESS_TEMPLATE = "download:PROGRESS " + "|".join(
    "%(" + f + ")s" for f in ("progress._percent_str", "progress.downloaded_bytes", "progress.total_bytes",
                             "progress.total_bytes_estimate", "progress._speed_str", "progress._eta_str",
                             "progress._elapsed_str", "progress.fragment_index", "progress.fragment_count",
                             "info.format_id", "info.vcodec", "info.acodec", "info.height", "progress.status"))


def stage(line, lv):
    """Read one line of yt-dlp's own output into lv, a download's live record.
    True when the line begins a new step."""
    before = (lv.get("phase"), lv.get("part"))
    m = re.match(r"\[info\] \S+: Downloading \d+ format\(s\): (\S+)", line)
    if m:
        lv.update(phase="starting", formats=m.group(1), parts=len(m.group(1).split("+")))
    elif line.startswith("[download] Destination: "):
        for k in PROGRESS_KEYS:
            lv.pop(k, None)
        lv.update(phase="downloading", part=lv.get("part", 0) + 1, dest=line[len("[download] Destination: "):])
    elif line.startswith("[download] ") and line.endswith(" has already been downloaded"):
        lv.update(phase="already on disk", part=lv.get("part", 0) + 1,
                  dest=line[len("[download] "):-len(" has already been downloaded")])
    elif line.startswith("[Merger] Merging formats into "):
        lv.update(phase="merging", dest=line[len("[Merger] Merging formats into "):].strip('"'))
    elif re.match(r"\[(Fixup\w*|FFmpeg\w*|Embed\w*|Metadata)\] ", line):
        lv.update(phase="post-processing (%s)" % line[1:line.index("]")])
    elif line.startswith("Deleting original file"):
        lv.update(phase="removing the part files")
    if (lv.get("phase"), lv.get("part")) == before:
        return False
    lv["since"] = time.time()
    return True


def stream_of(lv):
    """'video f299 avc1.64002a 1080p': the part being fetched, as far as yt-dlp has said."""
    v, a, fmt = na(lv.get("vcodec")), na(lv.get("acodec")), na(lv.get("format"))
    if not fmt and lv.get("formats") and lv.get("part"):
        ids = lv["formats"].split("+")
        fmt = ids[lv["part"] - 1] if lv["part"] <= len(ids) else ""
    kind = "video and audio" if v and a else "video" if v else "audio" if a else ""
    height = "%sp" % na(lv.get("height")) if v and na(lv.get("height")) else ""
    return " ".join(x for x in (kind, "f" + fmt if fmt else "", v or a, height) if x) or "?"


def size_of(lv):
    """'369.0 MiB of 816.6 MiB', with ~ where the total is yt-dlp's estimate."""
    got, total, est = number(lv.get("got_b")), number(lv.get("total_b")), number(lv.get("est_b"))
    if total or est:
        return "%s of %s%s" % (fmt_size(got), "" if total else "~", fmt_size(total or est))
    return fmt_size(got) if got else ""


def describe(lv):
    """The step a download is on, in words."""
    ph = lv.get("phase") or "starting"
    part = " part %s of %s" % (lv["part"], lv["parts"]) if lv.get("part") and lv.get("parts") else ""
    if ph in ("downloading", "already on disk"):
        return "%s%s: %s" % (ph, part, stream_of(lv))
    if ph == "merging":
        return "merging the video and audio into one file (ffmpeg)"
    return ph


def compact(lv):
    """The one-line form, for the list: '1/2 45.2% 12.3MiB/s eta 00:31', or the step."""
    ph = lv.get("phase") or ""
    if ph != "downloading" or not na(lv.get("pct")):
        return ph
    part = "%s/%s " % (lv["part"], lv["parts"]) if lv.get("part") and lv.get("parts") else ""
    eta = na(lv.get("eta"))
    return ("%s%s %s%s" % (part, lv["pct"], na(lv.get("speed")), " eta " + eta if eta else "")).strip()


def bar(frac, width):
    n = int(round(max(0.0, min(1.0, frac)) * width))
    return "[" + "#" * n + "-" * (width - n) + "]"


def live_view(it, width):
    """Lines on a download under way, for the window and 'ytq status', from the
    live record its runner keeps in the entry -- often in another process."""
    lv, now = it.get("live") or {}, time.time()
    rows = ["now      %s  (for %s)" % (describe(lv), fmt_secs(now - lv.get("since", now)))]
    ph, bw = lv.get("phase"), max(10, min(40, width - 64))
    if ph == "downloading" and na(lv.get("pct")):
        frag = "  fragment %s of %s" % (lv["frag"], lv["frags"]) if na(lv.get("frag")) and na(lv.get("frags")) else ""
        rows.append("progress %s %s  %s  %s  eta %s%s" % (
            bar(number(lv["pct"].rstrip("%")) / 100, bw), lv["pct"], size_of(lv),
            na(lv.get("speed")) or "?", na(lv.get("eta")) or "?", frag))
    elif ph == "merging" and lv.get("dest"):
        # ffmpeg writes NAME.temp.mp4 and renames it at the end; its size
        # against the parts' is as near to a merge percentage as there is.
        root, ext = os.path.splitext(lv["dest"])
        try:
            wrote = os.path.getsize(root + ".temp" + ext)
        except OSError:
            wrote = None
        want = number(lv.get("done_b"))
        if wrote is not None and want:
            rows.append("progress %s ~%d%%  %s written of about %s" % (
                bar(wrote / want, bw), min(100, wrote * 100 // want), fmt_size(wrote), fmt_size(want)))
        elif wrote is not None:
            rows.append("progress %s written" % fmt_size(wrote))
    overall = ["attempt %s" % it.get("attempts", "?"),
               "with Brave's cookies" if lv.get("cookies") else "plain yt-dlp",
               "started %s ago" % fmt_secs(now - lv["started"]) if lv.get("started") else "",
               "%s fetched in %d part%s" % (fmt_size(lv["done_b"]), lv["parts_done"], "" if lv["parts_done"] == 1 else "s")
               if lv.get("parts_done") else "",
               "runner pid %s" % lv["pid"] if lv.get("pid") else ""]
    rows.append("overall  " + ", ".join(o for o in overall if o))
    if lv.get("dest"):
        rows.append("file     " + os.path.basename(lv["dest"]))
    if lv.get("last"):
        rows.append("yt-dlp   " + lv["last"])
    return rows


def download(it, with_cookies):
    url, title, tag = it["url"], it["title"] or it["url"], short(it["url"])
    os.makedirs(S["DIR"], exist_ok=True)
    # With cookies it is yt-brave rather than yt-dlp -- the same arguments,
    # with the Brave profile and keyring put in front by the wrapper. --print
    # makes yt-dlp quiet, and quiet hides the progress lines and every line
    # about merging; --progress and --no-quiet bring both back.
    cmd = (brave_cmd() if with_cookies else ["yt-dlp"]) + NAME_OPTS + (META_OPTS if shutil.which("ffmpeg") else []) + [
           "--no-playlist", "--newline", "--no-simulate", "-f", S["FORMAT"],
           "--merge-output-format", "mp4", "--progress", "--no-quiet",
           "--progress-template", PROGRESS_TEMPLATE,
           "--print", "after_move:FILE %(filepath)s",
           "-o", os.path.join(S["DIR"].replace("%", "%%"), NAME)]
    cmd.append(url)
    log("%s: attempt %d at %s%s, into %s" % (tag, it["attempts"], title,
                                            " with Brave's cookies" if with_cookies else "", S["DIR"]))
    log("run: " + " ".join(shlex.quote(c) for c in cmd))
    started = time.time()
    # The live record: what the window and 'ytq status' show, kept in the
    # entry because the process downloading is often not the one looking.
    live = {"phase": "looking it up", "since": started, "started": started, "pid": os.getpid(),
            "cookies": with_cookies, "done_b": 0, "parts_done": 0}
    update(url, progress=live["phase"], live=dict(live))
    try:
        p = subprocess.Popen(cmd, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                             text=True, start_new_session=True)
    except OSError as e:
        update(url, status="failed", error="cannot run %s: %s" % (cmd[0], e), progress="", live={})
        say("failed: cannot run %s: %s" % (cmd[0], e), urgent=True)
        return
    # attempts as it was before claim() counted this one, so a stop from
    # either side -- here, or stop_current() -- hands back the same number.
    CURRENT.update(proc=p, url=url, cookies=with_cookies, attempts=it["attempts"] - 1, live=live)
    tail, fname, wrote, sampled = [], "", 0.0, started
    for line in p.stdout:
        line = line.rstrip()
        if not line:
            continue
        now, changed = time.time(), False
        if line.startswith("PROGRESS "):
            live.update(zip(PROGRESS_KEYS, (v.strip() for v in line[9:].split("|"))))
            if live.get("state") == "finished":
                size = number(live.get("total_b")) or number(live.get("got_b"))
                live["done_b"] += size
                live["parts_done"] += 1
                log("%s: part %s done: %s, %s in %s at %s" % (tag, live.get("part", "?"), stream_of(live),
                    fmt_size(size), na(live.get("elapsed")) or "?", na(live.get("speed")) or "?"))
                changed = True
            elif now - sampled >= 30:
                sampled = now
                log("%s: %s, %s, %s" % (tag, compact(live), stream_of(live), size_of(live)))
        elif line.startswith("FILE "):
            fname = line[5:]
            log("%s: file %s" % (tag, fname))
        elif line.startswith("[MetadataParser] "):
            # A line per step of the filename rule, twice over with
            # /etc/yt-dlp.conf present, and the notes' steps; the name they
            # make is logged as 'file' at the end.
            continue
        else:
            tail = (tail + [line])[-6:]
            live["last"] = line
            log("%s: yt-dlp: %s" % (tag, line))
            took = fmt_secs(now - live.get("since", now))
            if stage(line, live):
                changed = True
                log("%s: now %s (the step before took %s)" % (tag, describe(live), took))
                if MODE["say"] == "print":
                    print("  " + describe(live), flush=True)
        # Once a second is plenty for the queue, and every write is a turn at
        # queue.lock that a 'ytq clip' might be waiting for. A new step goes
        # in at once.
        if changed or now - wrote >= 1:
            wrote = now
            if update(url, progress=compact(live), live=dict(live)) is None:
                log("%s: deleted from the queue while downloading; stopping yt-dlp" % tag)
                kill(p)
    p.wait()
    took = fmt_secs(time.time() - started)
    CURRENT.update(proc=None, url=None, cookies=False, live=None)
    if STOP.is_set():
        log("%s: stopped while %s, %s in; back in the queue" % (tag, live.get("phase"), took))
        update(url, status="retry-cookies" if with_cookies else "queued", attempts=it["attempts"] - 1,
               progress="", live={})
        return
    CURRENT.update(attempts=0)
    log("%s: yt-dlp exited %d after %s, last step: %s" % (tag, p.returncode, took, live.get("phase")))
    if p.returncode == 0:
        with contextlib.suppress(OSError):
            log("%s: %s is %s" % (tag, fname, fmt_size(os.path.getsize(fname))))
        also = ""
        # The transcript takes the video's name with .txt, so the two sort together.
        if fname and S["SUBS"] and YT_RE.search(url):
            live.update(phase="fetching the transcript", since=time.time(), last="",
                        dest=os.path.splitext(fname)[0] + ".txt")
            if update(url, progress="transcript", live=dict(live)) is not None:
                txt, why = transcript(url, os.path.splitext(fname)[0].replace("%", "%%") + ".%(ext)s",
                                      brave_cmd() if with_cookies else ["yt-dlp"], os.path.basename(fname))
                log("%s: transcript: %s" % (tag, txt or "none -- " + why))
                also = " + transcript" if txt else " (no transcript)"
        if update(url, status="done", file=fname, progress="", error="", live={}) is not None:
            say("done: " + (os.path.basename(fname) if fname else title) + also)
        return
    err = (tail or ["yt-dlp exited %d" % p.returncode])[-1][:300]
    if cookie_problem(err) and not it["cookie_tried"]:
        log("%s: that reads like a login, age or bot check; waiting on a Brave sign-in" % tag)
        if update(url, status="cookies", error=err, progress="", live={}) is not None:
            open_browser(url)
            say("needs a Brave sign-in: %s -- Brave is open on it; sign in, then 'ytq cookies'" % title, urgent=True)
    elif it["attempts"] < 2 and not it["cookie_tried"]:
        update(url, status="retry", error=err, progress="", live={})
        log("retry later: %s: %s" % (url, err))
    elif update(url, status="failed", error=err, progress="", live={}) is not None:
        say("failed: %s -- %s" % (title, err), urgent=True)


def stop_current(why):
    """Kill the download under way and put its entry back as it was."""
    p, url, lv = CURRENT["proc"], CURRENT["url"], CURRENT.get("live") or {}
    if p:
        log("%s: stopping yt-dlp (pid %d) while %s, %s in, because %s. Back in the queue; finished parts "
            "and .part files stay, and the next attempt picks them up." % (
                short(url), p.pid, lv.get("phase", "?"), fmt_secs(time.time() - lv.get("started", time.time())), why))
        kill(p)
        update(url, status="retry-cookies" if CURRENT["cookies"] else "queued",
               attempts=CURRENT["attempts"], progress="", live={})


def checker():
    while not STOP.is_set():
        # Looking needs no lock; only this thread moves an entry out of 'checking'.
        it = next((i for i in Q.snapshot() if i["status"] == "checking"), None)
        if it:
            check(it["url"])
        else:
            time.sleep(0.5)


def mark_cookie_retries():
    with Q.edit() as items:
        n = 0
        for i in items:
            if i["status"] == "cookies":
                i["status"] = "retry-cookies"
                n += 1
        return n


def serve(stay=False, interactive=False):
    """Work the queue. Call holding run.lock; returns having let go of it, once
    nothing is left -- or never, when told to stay (the window)."""
    threading.Thread(target=checker, daemon=True).start()
    while not STOP.is_set():
        items = Q.snapshot()
        if not PAUSED.is_set() and any(i["status"] in DOWNLOADABLE for i in items):
            it, cookies = claim()
            if it:
                if interactive:
                    print("%s  %s" % ("with Brave's cookies" if cookies else "fetching", it["title"] or it["url"]), flush=True)
                download(it, cookies)
                continue
        if stay or PAUSED.is_set() or any(i["status"] in PENDING for i in items):
            time.sleep(1)
            continue
        if interactive and any(i["status"] == "cookies" for i in items):
            for i in items:
                if i["status"] == "cookies":
                    print("needs a Brave sign-in: %s\n  %s" % (i["url"], i["error"]))
            print("  Brave has been opened on it. Sign in or pass the check there, then press Enter here.")
            try:
                input()
            except EOFError:
                interactive = False
                continue
            why = brave_ready()
            if why:
                print(why + " -- start Brave once, or set PROFILE in ~/.config/ytq/config")
                interactive = False
            else:
                mark_cookie_retries()
            continue
        # Nothing left. Let go of run.lock under queue.lock, so that a 'ytq
        # clip' either got its URL in before this look or finds the lock free.
        with Q.edit() as items:
            if any(i["status"] in PENDING for i in items):
                continue
            RUN.release()
            log("runner %d: the queue is empty, leaving" % os.getpid())
            return


# --- the clipboard and focus ---------------------------------------------------
def read_clipboard():
    for cmd in (["wl-paste", "-n", "--type", "text/plain"], ["xclip", "-o", "-selection", "clipboard"]):
        try:
            r = subprocess.run(cmd, capture_output=True, text=True, timeout=3)
            if r.returncode == 0:
                return r.stdout.strip()
        except (OSError, subprocess.SubprocessError):
            continue
    return ""


def ancestors():
    pids, p = set(), os.getpid()
    while p > 1:
        pids.add(p)
        try:
            with open("/proc/%d/stat" % p) as f:
                p = int(f.read().rsplit(")", 1)[1].split()[1])
        except (OSError, ValueError, IndexError):
            break
    return pids


ANCESTORS = ancestors()


def focused():
    """Is the terminal this runs in the focused window? Unknown counts as yes."""
    try:
        if os.environ.get("HYPRLAND_INSTANCE_SIGNATURE"):
            r = subprocess.run(["hyprctl", "activewindow", "-j"], capture_output=True, text=True, timeout=2)
            return json.loads(r.stdout).get("pid") in ANCESTORS
        if os.environ.get("DISPLAY"):
            r = subprocess.run(["xdotool", "getactivewindow", "getwindowpid"], capture_output=True, text=True, timeout=2)
            if r.returncode == 0:
                return int(r.stdout.strip()) in ANCESTORS
    except (OSError, ValueError, subprocess.SubprocessError, AttributeError):
        pass
    return True


WATCH = {"on": True, "focused": True, "last": None}


def youtube_urls(text):
    """Every YouTube video in text, in the order first seen, once each, as a plain watch URL.
    The text is unescaped first, so an href out of saved HTML matches with its &amp; as &."""
    seen, urls = set(), []
    for vid in YT_RE.findall(html.unescape(text)):
        if vid not in seen:
            seen.add(vid)
            urls.append("https://www.youtube.com/watch?v=" + vid)
    return urls


def as_url(text):
    """text as one URL to queue, or None. A YouTube video, with or without its
    https:// -- youtube.com/shorts/ID, www.youtube.com/watch?v=ID&t=5s, youtu.be/ID --
    is its plain watch URL, as 'ytq clip' queues it, so every form of one video
    is one entry. It must start the text: a web.archive.org address holding a
    YouTube URL stays an archive address. Any other URL stays as it is; other
    text with no scheme is not a URL."""
    text = text.strip()
    m = YT_RE.match(text)
    if m and not re.search(r"\s", text):
        return "https://www.youtube.com/watch?v=" + m.group(1)
    return text if URL_RE.match(text) else None


def clipboard_urls():
    """What 'ytq clip' queues: the YouTube videos in the clipboard, however much
    text they are buried in; failing those, the clipboard itself if it is one URL."""
    text = read_clipboard()
    url = as_url(text)
    return youtube_urls(text) or ([url] if url else [])


def watcher():
    # What is on the clipboard at start counts: launching ytq with a link
    # already copied is the common case, so it is queued straight away. No
    # runner is started from here -- the window's own worker takes run.lock
    # as soon as nobody else has it.
    WATCH["last"] = read_clipboard()
    if as_url(WATCH["last"]):
        enqueue([as_url(WATCH["last"])], run=False)
    while not STOP.is_set():
        time.sleep(float(S["POLL"]))
        WATCH["focused"] = focused()
        if not WATCH["on"] or not WATCH["focused"]:
            continue
        text = read_clipboard()
        if text == WATCH["last"]:
            continue
        WATCH["last"] = text
        if as_url(text):
            enqueue([as_url(text)], run=False)


# --- the window ------------------------------------------------------------------
MARK = {"downloading": ">", "queued": ".", "retry": "r", "retry-cookies": "c", "cookies": "C",
        "checking": "?", "done": "+", "failed": "x", "rejected": "-"}


def prompt(win, label):
    h, w = win.getmaxyx()
    curses.echo(); curses.curs_set(1)
    win.addstr(h - 1, 0, (label + " ").ljust(w - 1)[:w - 1])
    win.refresh()
    try:
        s = win.getstr(h - 1, len(label) + 1, w - len(label) - 3).decode("utf-8", "replace").strip()
    except Exception:
        s = ""
    curses.noecho(); curses.curs_set(0)
    return s


def window_worker():
    """Download from the window whenever nobody else is: take run.lock and keep it."""
    while not STOP.is_set():
        if RUN.take(tries=1):
            serve(stay=True)
            return
        time.sleep(1)


def tui(win):
    MODE["say"] = "quiet"
    curses.curs_set(0)
    win.nodelay(True)
    win.timeout(500)
    colors = curses.has_colors()
    if colors:
        curses.start_color(); curses.use_default_colors()
        curses.init_pair(1, curses.COLOR_GREEN, -1); curses.init_pair(2, curses.COLOR_RED, -1)
        curses.init_pair(3, curses.COLOR_YELLOW, -1); curses.init_pair(4, curses.COLOR_CYAN, -1)
    # armed: when h (or x) was pressed once, so a second press clears the history.
    sel, message, armed = 0, "", 0.0
    for t in (watcher, window_worker):
        threading.Thread(target=t, daemon=True).start()
    while True:
        items = sorted(Q.snapshot(), key=lambda i: (ORDER.index(i["status"]) if i["status"] in ORDER else 3, i["added"]))
        holder = RUN.holder()
        h, w = win.getmaxyx()
        win.erase()
        state = ("watching the clipboard" if WATCH["on"] and WATCH["focused"] else
                 "clipboard: paused (window not focused)" if WATCH["on"] else "clipboard: off")
        worker = ("[paused]" if PAUSED.is_set() else "") if RUN.fd else \
                 "[downloading in pid %s]" % holder if holder else "[no worker yet]"
        counts = "  ".join("%d %s" % (n, st) for st, n in
                           ((st, sum(1 for i in items if i["status"] == st)) for st in ORDER) if n)
        head = " ytq  %s   -> %s   %s   %s" % (state, S["DIR"].replace(HOME, "~"), worker, counts)
        win.addstr(0, 0, head[:w - 1].ljust(w - 1), curses.A_REVERSE | curses.A_BOLD)
        # Downloads under way go above the list, each with the step it is on,
        # read from the live record whichever process is downloading keeps.
        top_y = 1
        for it in [i for i in items if i["status"] == "downloading"][:2]:
            rows = live_view(it, w - 4)[:max(0, h - top_y - 10)]
            if not rows:
                break
            win.addstr(top_y, 0, (" > %s   %s" % (it["title"] or it["url"], it["quality"]))[:w - 1],
                       (curses.color_pair(4) if colors else 0) | curses.A_BOLD)
            for n, row in enumerate(rows):
                win.addstr(top_y + 1 + n, 0, ("   " + row)[:w - 1])
            top_y += len(rows) + 2
        if top_y > 1:
            win.addstr(top_y - 1, 0, "-" * (w - 1), curses.A_DIM)
        room = max(1, h - 3 - top_y)
        sel = max(0, min(sel, len(items) - 1))
        top = max(0, sel - (room - 1))
        for row, it in enumerate(items[top:top + room]):
            y = top_y + row
            st = it["status"]
            col = {"done": 1, "failed": 2, "rejected": 2, "cookies": 3, "retry": 3, "downloading": 4}.get(st, 0)
            extra = it["quality"] if st in ("queued", "done") else it["error"][:40]
            line = " %s %-13s %-9s %s" % (MARK.get(st, " "), st, extra[:9], it["title"] or it["url"])
            if st == "downloading":
                line = " %s %-13s %s  %s" % (MARK[st], st, it.get("progress", "")[:40], it["title"] or it["url"])
            attr = curses.color_pair(col) if col else 0
            if top + row == sel:
                attr |= curses.A_REVERSE
            win.addstr(y, 0, line[:w - 1].ljust(w - 1), attr)
        if not items:
            win.addstr(2, 2, "Nothing queued. Copy a video URL while this window is focused, or press a.")
        keys = " a add  d delete  r retry  c continue with Brave's cookies  o open in Brave  p pause  h h clear history  q quit"
        win.addstr(h - 2, 0, keys[:w - 1], curses.A_DIM)
        if items and 0 <= sel < len(items):
            it = items[sel]
            info = it["error"] if it["status"] in ("failed", "rejected", "cookies", "retry") else it.get("file") or it["url"]
            if it["status"] == "cookies":
                info = "Brave is open on it: sign in / pass the check, then press c.  " + it["error"]
            win.addstr(h - 1, 0, (message or info)[:w - 1])
        else:
            win.addstr(h - 1, 0, message[:w - 1])
        win.refresh()
        try:
            k = win.getch()
        except KeyboardInterrupt:
            k = ord("q")
        if k == -1:
            continue
        message = ""
        if k not in (ord("h"), ord("x")):
            armed = 0.0
        if k in (ord("q"), 27):
            break
        elif k in (curses.KEY_DOWN, ord("j")):
            sel += 1
        elif k in (curses.KEY_UP, ord("k")):
            sel -= 1
        elif k == ord("a"):
            url = prompt(win, "URL:")
            if as_url(url):
                message = "queued" if enqueue([as_url(url)], run=False)[0] else "already in the queue"
            elif url:
                message = "that is not a URL"
        elif k == ord("w"):
            WATCH["on"] = not WATCH["on"]
        elif k == ord("p"):
            if RUN.fd:
                PAUSED.clear() if PAUSED.is_set() else PAUSED.set()
            else:
                message = "p pauses this window's downloads, and pid %s has the queue" % holder if holder \
                    else "nothing to pause yet"
        elif k in (ord("h"), ord("x")):
            # Twice in a row, within 5 s: forgetting a failed entry loses its
            # error, so one stray key must not do it. x was once a single press
            # and now asks the same way.
            if armed and time.time() - armed < 5:
                with Q.edit() as live:
                    n = len(live)
                    live[:] = [i for i in live if i["status"] not in HISTORY]
                    n -= len(live)
                armed = 0.0
                log("history cleared from the window: %d entries" % n)
                message = "cleared %d from the history; the files and the log stay" % n
            else:
                counts = ["%d %s" % (c, s) for s, c in
                          ((s, sum(1 for i in items if i["status"] == s)) for s in HISTORY) if c]
                armed = time.time() if counts else 0.0
                message = ("press %s again to clear the history: %s" % (chr(k), ", ".join(counts))
                           if counts else "the history is empty")
        elif items and k in (ord("d"), curses.KEY_DC):
            url = items[sel]["url"]
            # Whoever is downloading it notices at its next progress line.
            with Q.edit() as live:
                live[:] = [i for i in live if i["url"] != url]
            log("%s: deleted from the window (was %s)" % (short(url), items[sel]["status"]))
            if CURRENT["url"] == url and CURRENT["proc"]:
                kill(CURRENT["proc"])
        elif items and k == ord("r"):
            it = items[sel]
            if it["status"] != "downloading":
                update(it["url"], status="checking" if not it["title"] else "queued", error="")
        elif items and k == ord("c"):
            if items[sel]["status"] == "cookies":
                why = brave_ready()
                if why is None:
                    n = mark_cookie_retries()
                    message = "retrying %d with Brave's cookies" % n
                else:
                    message = why + " -- start Brave once, or set PROFILE in ~/.config/ytq/config"
            else:
                message = "c is for entries marked 'cookies'"
        elif items and k == ord("o"):
            open_browser(items[sel]["url"])
    # Quitting stops this window's downloads; 'ytq run', or the next 'ytq
    # clip', carries on from where it was.
    STOP.set()
    log("window closed")
    if RUN.fd:
        stop_current("the window was closed")
        RUN.release()


# --- the command line ------------------------------------------------------------
def interrupted(*_):
    raise KeyboardInterrupt


def cmd_run(quiet):
    if quiet:
        MODE["say"] = "notify"
    if not RUN.take():
        if not quiet:
            print("already downloading in pid %s -- 'ytq status' to follow it" % RUN.holder())
        return
    signal.signal(signal.SIGTERM, interrupted)
    signal.signal(signal.SIGHUP, interrupted)
    try:
        serve(interactive=not quiet and sys.stdin.isatty() and sys.stdout.isatty())
    except KeyboardInterrupt:
        STOP.set()
        stop_current("'ytq run' was interrupted (Ctrl+C, SIGTERM or SIGHUP)")
        if not quiet:
            print()
    finally:
        RUN.release()


def report(urls, added, runner):
    """One message, so a keypress makes one notification rather than a stack."""
    if len(urls) > 5:
        # A bookmarks file is hundreds of lines; the log has each URL already.
        lines = ["queued %d of %d links; the other %d were already in the queue"
                 % (len(added), len(urls), len(urls) - len(added))]
    else:
        lines = [("queued: " if u in added else "already queued: ") + u for u in urls]
    if runner:
        pid, started = runner
        lines.append("downloading: %s pid %s" % ("started," if started else "already, in", pid))
    elif any(i["status"] in PENDING for i in Q.snapshot()):
        lines.append("nothing is downloading -- 'ytq run' starts it, or touch %s to have it start by itself"
                     % AUTO.replace(HOME, "~"))
    if lines:
        say("\n".join(lines))


def cmd_status():
    items, pid = Q.snapshot(), RUN.holder()
    print("runner: " + ("pid %s" % pid if pid else "none -- 'ytq run' or the next 'ytq clip' starts one"))
    for i in items:
        if i["status"] == "downloading":
            print("  downloading  %s   %s" % (i["title"] or i["url"], i["quality"]))
            for row in live_view(i, 110):
                print("    " + row)
    for i in items:
        if i["status"] == "cookies":
            print("  sign-in    %s  -- then 'ytq cookies'" % (i["title"] or i["url"]))
    counts = [(st, sum(1 for i in items if i["status"] == st)) for st in ORDER]
    print("  " + (", ".join("%d %s" % (n, st) for st, n in counts if n) or "the queue is empty"))


def cmd_cookies(run):
    if not any(i["status"] == "cookies" for i in Q.snapshot()):
        print("nothing is waiting on a Brave sign-in")
        return
    why = brave_ready()
    if why:
        say(why + " -- start Brave once, or set PROFILE in ~/.config/ytq/config", urgent=True)
        sys.exit(1)
    with Q.edit():
        n = mark_cookie_retries()
        runner = start_runner() if run else current_runner()
    log("retrying %d with Brave's cookies" % n)
    report([], [], runner)


def main(argv):
    flags = [a for a in argv if a in ("--run", "--no-run", "--quiet")]
    args = [a for a in argv if a not in flags]
    cmd = args[0] if args else "tui"
    run = autostart(flags)
    if cmd == "add" and args[1:]:
        urls = []
        for u in args[1:]:
            if as_url(u):
                urls.append(as_url(u))
            else:
                say("not a URL: " + u)
        if urls:
            report(urls, *enqueue(urls, run))
    elif cmd == "clip":
        urls = clipboard_urls()
        if not urls:
            say("the clipboard holds no URL and no YouTube link")
            sys.exit(1)
        report(urls, *enqueue(urls, run))
    elif cmd == "run":
        cmd_run("--quiet" in flags)
    elif cmd == "status":
        cmd_status()
    elif cmd == "cookies":
        cmd_cookies(run)
    elif cmd == "transcript" and args[1:]:
        failed = 0
        for u in args[1:]:
            u = as_url(u) or u
            if not YT_RE.search(u):
                print("not a YouTube video: " + u)
                failed = 1
                continue
            os.makedirs(S["DIR"], exist_ok=True)
            txt, why = transcript(u, os.path.join(S["DIR"].replace("%", "%%"), NAME))
            print(txt.replace(HOME, "~") if txt else "no transcript for %s -- %s" % (u, why))
            failed = failed or not txt
        sys.exit(1 if failed else 0)
    elif cmd == "list":
        items = Q.snapshot()
        for it in items:
            print("%-13s %-10s %s" % (it["status"], it["quality"], it["title"] or it["url"]))
            if it["error"]:
                print("              %s" % it["error"][:100])
        if not items:
            print("the queue is empty")
    elif cmd == "clear":
        with Q.edit() as items:
            n = len(items)
            items[:] = [i for i in items if i["status"] not in HISTORY]
            n -= len(items)
        print("removed %d" % n)
    elif cmd == "tui":
        curses.wrapper(tui)
    else:
        for line in open(__file__).read().splitlines()[1:]:
            if not line.startswith("#"):
                break
            print(line[2:])
        print("usage: ytq | ytq clip | ytq add URL... | ytq run | ytq status | ytq cookies | ytq transcript URL... | ytq list | ytq clear")
        print()
        print("config: %s (%s)" % (CONF.replace(HOME, "~"), "found" if os.path.isfile(CONF) else "not there -- the defaults apply"))
        print("  KEY=VALUE lines, # for comments: DIR, FORMAT, PROFILE, KEYRING, POLL, SUBS")
        print("  in effect: DIR=%s  PROFILE=%s%s" % (S["DIR"].replace(HOME, "~"), S["PROFILE"],
                                                 "  KEYRING=" + S["KEYRING"] if S["KEYRING"] else ""))
        print("auto:   %s (%s)" % (AUTO.replace(HOME, "~"),
              "found -- clip, add and cookies start downloading by themselves" if os.path.exists(AUTO)
              else "not there -- clip and add only queue; touch it to have them start downloading"))
        print("  --run or --no-run on clip, add or cookies overrides it for that one command")
        sys.exit(0 if cmd in ("-h", "--help", "help") else 2)


if __name__ == "__main__":
    main(sys.argv[1:])
