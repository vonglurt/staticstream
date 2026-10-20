#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
# Copyright (c) 2026 Paul Richeson
#
# feature-guide.py -- perform docs/FEATURE_GUIDE.md and take its pictures.
#
#   python3 tools/feature-guide.py [--keep]
#
# Every terminal picture is a real session: the commands shown are the
# commands run, in a throwaway HOME, against the binaries in target/release
# (built with `cargo build --release --offline` when they are not there), and
# the output shown is what they printed. The only edit made to it is the one a
# shell makes: the throwaway HOME is written `~`.
#
# What is not real, and the guide says so where it shows:
#   - the signing key is made here, with no passphrase, and thrown away;
#   - the video is ffmpeg's test pattern, not a download;
#   - `damage` and `noise` are this repository's tests/damage.py and
#     tests/noise.py, the tools the crosscheck breaks captures with;
#   - ytq drives tests/standin/yt-dlp, which answers as yt-dlp does and never
#     reaches the network;
#   - the player at the far end of --serve is ffprobe and ffmpeg, not VLC.
#
# Two pictures are drawn rather than captured -- 01 (the journey) and 03 (the
# file layout) -- from the layout in src/format/mod.rs and record.rs.
#
# Needs: python3, cargo (or a built target/release), ssh-keygen, ffmpeg and
# ffprobe, tmux, and chromium for the screenshots. Nothing is installed.
# Listens on 127.0.0.1:8190 only. Writes docs/features/NN-name.png and nothing
# else in the repository; the session's text goes to the work folder.

import base64
import html
import os
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, "docs", "features")
REL = os.path.join(ROOT, "target", "release")
PORT = 8190
KEEP = "--keep" in sys.argv[1:]


def need(prog):
    if shutil.which(prog) is None:
        sys.exit("feature-guide: %s is needed and is not on PATH" % prog)


for prog in ("ssh-keygen", "ffmpeg", "ffprobe", "tmux", "chromium"):
    need(prog)

if not all(os.access(os.path.join(REL, b), os.X_OK) for b in ("sstr", "ytq", "sstr-workspace")):
    need("cargo")
    subprocess.run(["cargo", "build", "--release", "--offline"], cwd=ROOT, check=True)

W = tempfile.mkdtemp(prefix="sstr-guide.")
HOME = os.path.join(W, "home")
BIN = os.path.join(W, "bin")
SHOTS = os.path.join(W, "shots")
for d in (HOME, BIN, SHOTS, OUT):
    os.makedirs(d, exist_ok=True)


def write(path, text, mode=0o644):
    with open(path, "w", encoding="utf-8") as f:
        f.write(text)
    os.chmod(path, mode)


# The commands a session types: the three binaries, the repository's two
# wrecking tools under short names, and the stand-ins ytq calls out to.
for b in ("sstr", "ytq", "sstr-workspace"):
    os.symlink(os.path.join(REL, b), os.path.join(BIN, b))
write(os.path.join(BIN, "damage"),
      '#!/bin/sh\nexec python3 "%s/tests/damage.py" "%s/tests/reference/copal-sstr.py" "$@"\n' % (ROOT, ROOT), 0o755)
write(os.path.join(BIN, "noise"), '#!/bin/sh\nexec python3 "%s/tests/noise.py" "$@"\n' % ROOT, 0o755)
shutil.copy(os.path.join(ROOT, "tests", "standin", "yt-dlp"), os.path.join(BIN, "yt-dlp"))
os.chmod(os.path.join(BIN, "yt-dlp"), 0o755)
for stub in ("wl-paste", "xclip"):
    write(os.path.join(BIN, stub), "#!/bin/sh\nexit 1\n", 0o755)
write(os.path.join(BIN, "notify-send"), "#!/bin/sh\nexit 0\n", 0o755)

ENV = {k: v for k, v in os.environ.items() if not k.startswith("XDG_")}
ENV.update(HOME=HOME, PATH=BIN + ":/usr/local/bin:/usr/bin:/bin", LC_ALL="C.UTF-8", TERM="xterm-256color")
ENV.pop("SSTR_KEY", None)
ENV.pop("SSH_AUTH_SOCK", None)


def tilde(s):
    return s.replace(HOME, "~")


# ------------------------------------------------------------- sessions ---

class Session:
    """A terminal: what was typed, what came back, and the exit status."""

    def __init__(self, title):
        self.title = title
        self.blocks = []  # (command, output, rc, marks)
        self.log = []

    def run(self, cmd, marks=(), show=None, quiet_ok=True, timeout=120):
        p = subprocess.run(["sh", "-c", "set -o pipefail\n" + cmd], cwd=HOME, env=ENV, stdout=subprocess.PIPE,
                           stderr=subprocess.STDOUT, timeout=timeout)
        out = tilde(p.stdout.decode("utf-8", "replace")).rstrip("\n")
        self.blocks.append((show or cmd, out, p.returncode, list(marks)))
        self.log.append("$ %s\n%s\n[exit %d]\n" % (show or cmd, out, p.returncode))
        return out, p.returncode

    def text(self):
        return "\n".join(self.log)


COLOURS = [
    (r"\(matches the end record\)|trusted as \S+|everything checked out|identical|Verify ok", "ok"),
    (r"DOES NOT match the end record|no end record[^;]*|missing: [^\n]*|DAMAGED|differ[^\n]*|no good signature seen", "bad"),
    (r"\b[1-9]\d* records rebuilt from parity|repaired +[1-9][\d ,a-z]*records|\b[1-9]\d* header bytes, [1-9]\d* body bytes in \d+ records", "fix"),
    (r"\b[1-9]\d* data records(?=, \d+ records of unknown)|\b[1-9]\d* bad\b|\b[1-9]\d* mismatched|\b[1-9]\d* unaccounted", "bad"),
]

CSS = """
:root{--bg:#0f1720;--fg:#d7dde5;--dim:#7d8b99;--ok:#7fd99a;--bad:#ff8a80;--fix:#ffd27a;--accent:#84d6e8}
*{box-sizing:border-box}
html,body{margin:0;background:#e9eef2}
body{padding:22px;font:15px/1.45 "DejaVu Sans",sans-serif;color:#1d2731}
.win{background:var(--bg);border-radius:10px;box-shadow:0 8px 28px rgba(15,23,32,.28);overflow:hidden}
.bar{background:#1b2631;color:#aab6c2;font-size:13px;padding:8px 14px;display:flex;align-items:center;gap:8px}
.dot{width:11px;height:11px;border-radius:50%;background:#46525e;display:inline-block}
.bar b{font-weight:600;color:#dfe7ee;margin-left:8px}
pre{margin:0;padding:14px 18px 16px;font:13px/1.5 "DejaVu Sans Mono",monospace;color:var(--fg);white-space:pre}
.p{color:var(--accent)}
.c{color:#fff;font-weight:bold}
.ok{color:var(--ok)}.bad{color:var(--bad)}.fix{color:var(--fix)}
.rc{color:var(--bad);font-size:12px}
.n{display:inline-block;min-width:20px;height:20px;line-height:20px;border-radius:10px;background:#ffd27a;color:#141c24;
   font:bold 12px/20px "DejaVu Sans",sans-serif;text-align:center;margin-left:12px;vertical-align:1px}
.side{display:flex;gap:18px;align-items:flex-start}
.side figure{margin:0;background:var(--bg);border-radius:10px;padding:10px;box-shadow:0 8px 28px rgba(15,23,32,.28)}
.side figcaption{color:#aab6c2;font-size:12px;padding:8px 2px 0}
.side img{display:block;width:420px}
"""


def colour(line):
    esc = html.escape(line)
    for pat, cls in COLOURS:
        esc = re.sub(pat, lambda m: '<span class="%s">%s</span>' % (cls, m.group(0)), esc)
    return esc


def badge(n):
    return '<span class="n">%s</span>' % n


def render_lines(blocks):
    lines = []
    for cmd, out, rc, marks in blocks:
        cl = cmd.split("\n")
        for i, c in enumerate(cl):
            t = ('<span class="p">~ $</span> ' if i == 0 else '<span class="p">&gt;</span> ') + '<span class="c">%s</span>' % html.escape(c)
            lines.append([c, t])
        if out:
            for o in out.split("\n"):
                lines.append([o, colour(o)])
        if rc:
            lines.append(["", '<span class="rc">[exit %d]</span>' % rc])
        # A mark goes on the first line of this block's output that holds its
        # text, or with a leading $, on the first line of the command.
        start = len(lines) - len(cl) - (len(out.split("\n")) if out else 0) - (1 if rc else 0)
        for needle, n in marks:
            span = lines[start:start + len(cl)] if needle.startswith("$") else lines[start + len(cl):]
            needle = needle.lstrip("$")
            for ln in span:
                if needle in ln[0]:
                    ln[1] += badge(n)
                    break
            else:
                raise SystemExit("feature-guide: mark %r not found in: %s" % (needle, cmd))
    return lines


def terminal_html(title, lines, extra=""):
    body = "\n".join(l[1] for l in lines)
    return ('<!doctype html><meta charset="utf-8"><style>%s</style>'
            '<div class="side"><div class="win"><div class="bar"><span class="dot"></span><span class="dot"></span>'
            '<span class="dot"></span><b>%s</b></div><pre>%s</pre></div>%s</div>') % (CSS, html.escape(title), body, extra)


# chromium puts its singleton socket in TMPDIR, and a socket path has a limit
# of about a hundred bytes: a long TMPDIR makes it abort before it draws.
CHROME_ENV = {k: v for k, v in os.environ.items() if k != "TMPDIR"}


def shoot(name, page, width, height):
    src = os.path.join(SHOTS, name + ".html")
    write(src, page)
    png = os.path.join(OUT, name + ".png")
    if os.path.exists(png):
        os.remove(png)
    # A profile of its own, so no browser of the person's is touched. chromium
    # can abort on its way out after writing the file, so the file is the test.
    said = ""
    for _ in range(2):
        p = subprocess.run(["chromium", "--headless=new", "--no-sandbox", "--hide-scrollbars", "--disable-gpu",
                        "--user-data-dir=" + os.path.join(W, "chromium"), "--no-first-run",
                        "--force-device-scale-factor=1.5", "--window-size=%d,%d" % (width, height),
                        "--screenshot=" + png, "file://" + src],
                       env=CHROME_ENV, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, timeout=90)
        if os.path.exists(png):
            break
        said = p.stderr.decode("utf-8", "replace")
    if not os.path.exists(png):
        sys.exit("feature-guide: chromium wrote no %s:\n%s" % (png, said[-2000:]))
    print("  %-24s %4d KB" % (name + ".png", os.path.getsize(png) // 1024))


def shoot_session(name, s, extra="", extra_w=0, min_h=0):
    lines = render_lines(s.blocks)
    width = max(len(l[0]) for l in lines)
    w = int(width * 7.9) + 120 + extra_w
    h = max(len(lines) * 19.5 + 110, min_h)
    shoot(name, terminal_html(s.title, lines, extra), max(w, 760), int(h))
    write(os.path.join(SHOTS, name + ".txt"), s.text())


# ----------------------------------------------------------- the stream ---

FF = ("ffmpeg -v error -re -f lavfi -i testsrc2=size=640x360:rate=25 "
      "-f lavfi -i sine=frequency=440 -t 8 -c:v mpeg2video -q:v 4 -c:a mp2 -f mpegts -")

print("feature-guide: work in %s" % W)

# 02: a key made for the demo, a stream recorded live, and what is on disk.
s = Session("Recording: a live stream into clip.sstr")
s.run("ssh-keygen -q -t ed25519 -N '' -C demo@example -f demo_key", marks=[("$ssh-keygen", 1)])
s.run('echo "demo@example $(cat demo_key.pub)" > allowed_signers')
s.run(FF + " \\\n  | sstr record clip.sstr --type video/mp2t --key demo_key \\\n"
      "      --source https://example.org/test-pattern --license CC0-1.0 --note 'ffmpeg testsrc2, 8 s'",
      marks=[("$sstr record", 2)])
s.run("ls -l clip.sstr && head -c 8 clip.sstr | od -c | head -1", marks=[("0000000", 3)])
shoot_session("02-record", s)

# 04: the clean verify, trusted.
s = Session("Verifying: every check, in one report")
s.run("sstr verify clip.sstr --allowed-signers allowed_signers",
      marks=[("stream ", 1), ("key ", 2), ("records ", 3), ("payload ", 4), ("checkpoints ", 5), ("signatures ", 6)])
s.run("sstr verify clip.sstr", marks=[("no --allowed-signers given", 7)])
shoot_session("04-verify", s)

# 05: bit errors and a burst, corrected by the inner code.
s = Session("Damage the inner code corrects: bit errors and a burst")
s.run("damage ber clip.sstr ber.sstr 0.0001", marks=[("bits flipped", 1)])
s.run("sstr verify ber.sstr --allowed-signers allowed_signers | grep -E 'payload|repaired|lost|signatures'",
      marks=[("repaired", 2)])
# A burst inside the third data record's body, found by walking the records
# as a reader does: zeroes over about ten bytes of every codeword it has, well
# inside the sixteen each one corrects.
with open(os.path.join(HOME, "clip.sstr"), "rb") as f:
    cap = f.read()
pos, seen = 8, 0
while True:
    typ = cap[pos + 4:pos + 5]
    plain = int.from_bytes(cap[pos + 4 + 20:pos + 4 + 24], "little")
    full, rest = divmod(plain, 223)
    if typ == b"D":
        seen += 1
        if seen == 3:
            break
    pos += 136 + full * 255 + (rest + 32 if rest else 0)
at, span = pos + 136 + 100, 10 * (full + (1 if rest else 0))
s.run("damage burst clip.sstr burst.sstr %d %d" % (at, span), marks=[("bytes zeroed", 3)])
s.run("sstr verify burst.sstr | grep -E 'payload|repaired|lost'", marks=[("repaired", 4)])
s.run("sstr play clip.sstr > clip.ts && sstr play burst.sstr | cmp - clip.ts && echo identical",
      marks=[("identical", 5)])
shoot_session("05-bit-errors", s)

# 06: two whole records of a group, at version 1 and at version 0.
s = Session("Whole records lost: version 1 against version 0")
s.run("sstr record clip-v0.sstr --format 0 --type video/mp2t --key demo_key --input clip.ts", marks=[("$--format 0", 1)])
s.run("ls -l clip.ts clip.sstr clip-v0.sstr", marks=[("clip-v0.sstr", 2)])
s.run("damage wipe clip.sstr two.sstr 3 4 && damage wipe clip-v0.sstr two-v0.sstr 3 4", marks=[("randomised", 3)])
s.run("sstr verify two.sstr | grep -E 'payload|repaired|lost'", marks=[("records rebuilt", 4)])
s.run("sstr verify two-v0.sstr | grep -E 'payload|repaired|lost'", marks=[("2 data records", 5)])
s.run("sstr play two.sstr | cmp - clip.ts && echo identical", marks=[("identical", 6)])
shoot_session("06-two-lost", s)

# 07: what is not damage: an edit, and a capture cut short.
s = Session("Tampering and truncation: found, and said")
s.run("damage tamper clip.sstr tamper.sstr 7", marks=[("re-encoded", 1)])
s.run("sstr verify tamper.sstr --allowed-signers allowed_signers | grep -E 'payload|repaired|checkpoints|signatures'",
      marks=[("checkpoints", 2), ("payload", 3)])
s.run("damage truncate clip.sstr cut.sstr 0.6")
s.run("sstr verify cut.sstr --allowed-signers allowed_signers | grep -E 'payload|signatures|end '",
      marks=[("end ", 4)])
s.run("sstr play cut.sstr > cut.ts; echo \"exit $?\"; ls -l cut.ts", marks=[("-rw-", 5)])
shoot_session("07-tamper", s)

# 08: the pace a stream was recorded at, given back.
s = Session("Pace: played back as it arrived")
s.run("for i in 1 2 3 4 5; do date +\"tick $i  %T\"; sleep 1; done \\\n  | sstr record clock.sstr --type text/plain --flush-ms 50",
      marks=[("$for i in", 1)])
s.run("sstr play clock.sstr", marks=[("$sstr play clock.sstr", 2)])
stamp = "while IFS= read -r l; do echo \"$l   arrived $(date +%T.%N | cut -c1-12)\"; done"
s.run("sstr play clock.sstr --paced | " + stamp, show="sstr play clock.sstr --paced | stamp", marks=[("$--paced | stamp", 3)])
s.run("sstr play clock.sstr --paced --speed 2 --start 2 | " + stamp, show="sstr play clock.sstr --paced --speed 2 --start 2 | stamp",
      marks=[("$--speed 2", 4)])
shoot_session("08-paced", s)

# 09: served while it is still being recorded, to a player that joins late.
s = Session("Serving: a capture still being written, to a client that joins late")
live = subprocess.Popen(["sh", "-c", FF.replace("-t 8", "-t 12") + " | sstr record live.sstr --type video/mp2t --key demo_key"],
                        cwd=HOME, env=ENV, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
s.blocks.append(("ffmpeg ... -t 12 -f mpegts - | sstr record live.sstr --type video/mp2t --key demo_key &", "", 0, [("$sstr record live", 1)]))
time.sleep(3)
srv_log = os.path.join(HOME, "serve.log")
srv = subprocess.Popen(["sh", "-c", "exec sstr play live.sstr --serve 127.0.0.1:%d --follow 2> serve.log" % PORT],
                       cwd=HOME, env=ENV)
s.blocks.append(("sstr play live.sstr --serve 127.0.0.1:%d --follow 2> serve.log &" % PORT, "", 0, [("$--serve", 2)]))
time.sleep(2)
s.run("ffprobe -v error -show_entries stream=codec_type,codec_name,width,height -of compact http://127.0.0.1:%d/" % PORT,
      marks=[("codec_type=video", 3)])
s.run("ffmpeg -v error -y -i http://127.0.0.1:%d/ -ss 2 -frames:v 1 -vf scale=420:-1 frame.png && ls -l live.sstr" % PORT,
      marks=[("$frame.png", 4)])
live.wait(timeout=60)
time.sleep(1)
srv.send_signal(signal.SIGTERM)
srv.wait(timeout=10)
s.run("cat serve.log", marks=[("serving", 5)])
s.run("sstr verify live.sstr --allowed-signers allowed_signers | grep -E 'payload|signatures'", marks=[("payload", 6)])
with open(os.path.join(HOME, "frame.png"), "rb") as f:
    frame = base64.b64encode(f.read()).decode()
fig = ('<figure><img src="data:image/png;base64,%s" alt=""><figcaption>frame.png: the frame ffmpeg took from '
       'http://127.0.0.1:%d/, two seconds in</figcaption></figure>') % (frame, PORT)
shoot_session("09-serve", s, extra=fig, extra_w=470)

# 10: the armor, over a line that drops some of it.
s = Session("Armor: a capture over a serial line that loses lines")
s.run("cp %s/README.md notes.md && sstr record notes.sstr --input notes.md --type text/markdown --key demo_key" % ROOT,
      show="sstr record notes.sstr --input notes.md --type text/markdown --key demo_key")
s.run("sstr armor notes.sstr > notes.armor && wc -l < notes.armor && head -3 notes.armor && tail -1 notes.armor",
      marks=[("BEGIN SSTR ARMOR", 1), ("END SSTR ARMOR", 2)])
s.run("noise 0.03 0 40 < notes.armor > line && grep -c usb line", marks=[("$noise", 3)])
s.run("sstr recv line --verify --allowed-signers allowed_signers 2>&1 >/dev/null | grep -E 'recv:|payload|repaired|signatures'",
      marks=[("recv:", 4), ("repaired", 5)])
s.run("sstr recv line 2>/dev/null | cmp - notes.md && echo identical", marks=[("identical", 6)])
shoot_session("10-armor", s)

# 11: a folder of captures back out as their files.
s = Session("Export: a folder of captures, back out as files")
s.run("mkdir archive && cp clip.sstr notes.sstr clock.sstr archive/ && damage wipe clip.sstr archive/broken.sstr 3 4 5",
      marks=[("randomised", 1)])
s.run("sstr export archive -n", marks=[("would   broken", 2)])
s.run("sstr export archive", marks=[("DAMAGED", 3), ("export  clip.sstr", 4)])
s.run("ls archive", marks=[("broken.ts.part", 5)])
shoot_session("11-export", s)
s.run("rm -f archive/*.ts archive/*.txt archive/*.md archive/*.part archive/*.bin")

# 12: ytq, archiving what it downloads (the stand-in yt-dlp).
s = Session("ytq: a queued video kept as a signed capture")
os.makedirs(os.path.join(HOME, ".config", "ytq"), exist_ok=True)
s.run("printf 'DIR=~/archive\\nOUTPUT=sstr\\nSSTR_KEY=~/demo_key\\n' > ~/.config/ytq/config && sstr config get OUTPUT",
      marks=[("$printf", 1)])
s.run("ytq add --no-run https://www.youtube.com/watch?v=DEMOVIDEOAA https://youtu.be/DEMOVIDEOBB && ytq list", marks=[("$ytq add", 2)])
s.run("ytq run --quiet; ytq list")
s.run("grep -E 'AA: (archived|removed)|done: .*AA' ~/.local/share/ytq/ytq.log | cut -c 12- | sed \"s|$HOME/||\" | fold -s -w 120",
      marks=[("archived", 3), ("removed", 4), ("done:", 5)])
s.run("ls archive | grep DEMOVIDEOAA && sstr verify archive/Fake-Fake_Title_DEMOVIDEOAA.sstr --allowed-signers allowed_signers | sed -n 1,4p",
      marks=[("source", 6)])
shoot_session("12-ytq", s)

# 13 and 14: the Workspace, in a private tmux server.
SOCK = "sstr-guide-%d" % os.getpid()


def tmux(*a, **kw):
    return subprocess.run(["tmux", "-L", SOCK] + list(a), env=ENV, stdout=subprocess.PIPE, check=kw.get("check", True))


SGR = {1: "font-weight:bold", 2: "opacity:.55", 4: "text-decoration:underline"}


def ansi_html(text):
    """tmux's capture-pane -e, as spans: bold, dim, reverse and the 8 colours."""
    pal = ["#0f1720", "#ff8a80", "#7fd99a", "#ffd27a", "#82aaff", "#c792ea", "#84d6e8", "#d7dde5"]
    out, st = [], {}
    for part in re.split(r"(\x1b\[[0-9;]*m)", text):
        m = re.match(r"\x1b\[([0-9;]*)m", part)
        if m:
            for c in [int(x) if x else 0 for x in m.group(1).split(";")]:
                if c == 0:
                    st = {}
                elif c in SGR:
                    st[c] = SGR[c]
                elif c == 7:
                    st[7] = True
                elif 30 <= c <= 37:
                    st["fg"] = pal[c - 30]
                elif 40 <= c <= 47:
                    st["bg"] = pal[c - 40]
                elif c in (22,):
                    st.pop(1, None); st.pop(2, None)
                elif c == 27:
                    st.pop(7, None)
            continue
        if not part:
            continue
        css = [v for k, v in st.items() if k in SGR]
        fg, bg = st.get("fg"), st.get("bg")
        if st.get(7):
            fg, bg = bg or "#0f1720", fg or "#d7dde5"
        if fg:
            css.append("color:" + fg)
        if bg:
            css.append("background:" + bg)
        e = html.escape(part)
        out.append('<span style="%s">%s</span>' % (";".join(css), e) if css else e)
    return "".join(out)


def screen():
    return tilde(tmux("capture-pane", "-p").stdout.decode("utf-8", "replace"))


def shoot_tmux(name, title, cols, rows, marks):
    raw = tilde(tmux("capture-pane", "-e", "-p").stdout.decode("utf-8", "replace")).rstrip("\n").split("\n")
    plain = [re.sub(r"\x1b\[[0-9;]*m", "", l) for l in raw]
    lines = [[p, ansi_html(r)] for p, r in zip(plain, raw)]
    for needle, n in marks:
        for ln in lines:
            if needle in ln[0]:
                ln[1] += badge(n)
                break
        else:
            raise SystemExit("feature-guide: mark %r not on the %s screen" % (needle, name))
    wide = max(len(p.rstrip()) for p in plain)
    shoot(name, terminal_html(title, lines), int(wide * 7.9) + 150, int(len(lines) * 19.5) + 110)
    write(os.path.join(SHOTS, name + ".txt"), "\n".join(plain))


COLS, ROWS = 200, 30
tmux("-f", "/dev/null", "new-session", "-d", "-x", str(COLS), "-y", str(ROWS), "sstr-workspace")
try:
    time.sleep(1.5)
    if "the Queue" in screen().split("\n")[0]:
        tmux("send-keys", "h")
        time.sleep(0.8)
    for _ in range(12):
        top = screen().split("\n")
        if len(top) > 3 and "clip.sstr" in top[3][COLS * 2 // 3 - 2:]:
            break
        tmux("send-keys", "j")
        time.sleep(0.6)
    time.sleep(1.5)
    shoot_tmux("13-workspace", "sstr-workspace: the Browser and the Inspector", COLS, ROWS,
               [("Workspace -- ~/archive", 1), ("Shelf:", 2), ("signatures", 3), ("Transcript", 4), ("Services:", 5)])
    for _ in range(12):
        top = screen().split("\n")
        if len(top) > 3 and "broken.sstr" in top[3][COLS * 2 // 3 - 2:]:
            break
        tmux("send-keys", "k")
        time.sleep(0.6)
    time.sleep(1)
    tmux("send-keys", "v")
    time.sleep(2.5)
    shoot_tmux("14-service", "sstr-workspace: the Verify Service, sent with v", COLS, ROWS,
               [("sstr verify ~/archive/broken.sstr", 1), ("DOES NOT", 2), ("press a key", 3)])
    tmux("send-keys", "Enter")
    time.sleep(0.5)
    tmux("send-keys", "q")
finally:
    tmux("kill-server", check=False)

# ------------------------------------------------------------- diagrams ---

DCSS = CSS + """
body{background:#f4f6f8}
h1{font-size:20px;margin:0 0 4px}.sub{color:#5b6875;margin:0 0 18px;font-size:14px}
.row{display:flex;align-items:stretch;gap:0}
.box{border-radius:9px;padding:12px 14px;background:#fff;border:1.5px solid #c9d3dc;font-size:13.5px}
.box h3{margin:0 0 6px;font-size:14.5px}.box code,.mono{font:12.5px/1.45 "DejaVu Sans Mono",monospace}
.arrow{align-self:center;color:#7d8b99;font-size:24px;padding:0 8px}
.k{color:#5b6875}
.tag{display:inline-block;font:bold 11px "DejaVu Sans Mono",monospace;padding:2px 6px;border-radius:5px;margin-right:4px;color:#141c24}
.H{background:#b9e3f0}.D{background:#d9e8c4}.P{background:#ffe1a8}.C{background:#e5d4f5}.E{background:#f6c9c3}.M{background:#d7dde5}
.strip{display:flex;gap:3px;flex-wrap:nowrap;margin:8px 0 4px}
.strip span{font:bold 11px "DejaVu Sans Mono",monospace;padding:6px 0;border-radius:4px;text-align:center;color:#141c24}
table{border-collapse:collapse;font-size:13px;margin-top:6px}td,th{border:1px solid #c9d3dc;padding:5px 8px;text-align:left;vertical-align:top}
th{background:#eef2f5}
.n{margin-left:6px}
"""

journey = """<!doctype html><meta charset="utf-8"><style>%s
.lane{display:grid;grid-template-columns:repeat(5,1fr);gap:14px;margin-top:6px}
.step{background:#fff;border:1.5px solid #c9d3dc;border-radius:10px;padding:12px 13px;font-size:13px;position:relative}
.step h3{margin:0 0 4px;font-size:15px}.step .v{font:bold 12.5px "DejaVu Sans Mono",monospace;color:#1f6f86;margin-bottom:6px}
.step ul{margin:6px 0 0 16px;padding:0}.step li{margin:2px 0}
.under{display:grid;grid-template-columns:2fr 3fr;gap:14px;margin-top:14px}
</style>
<h1>A static stream, start to finish</h1>
<p class="sub">What a person does with <code>sstr</code>, and what each step relies on. Drawn from the README and <code>src/bin/sstr.rs</code>.</p>
<div class="lane">
 <div class="step"><h3>1. Record <span class="n">1</span></h3><div class="v">sstr record OUT</div>
   A stream from stdin or <code>--input</code>, cut into records of 64 KiB (<code>--chunk</code>), each stamped with the moment it arrived.
   <ul><li>header: type, source, licence, note</li><li><code>--key</code>: checkpoints signed</li><li><code>--format 0|1</code>: the outer code</li></ul></div>
 <div class="step"><h3>2. Keep <span class="n">2</span></h3><div class="v">clip.sstr</div>
   On a disk, a share, or a serial line as armor. Every record carries Reed–Solomon parity; every group of 16 a parity record.
   <ul><li><code>sstr armor</code> / <code>unarmor</code> / <code>recv</code>: 80-column lines with CRCs</li></ul></div>
 <div class="step"><h3>3. Damage <span class="n">3</span></h3><div class="v">what happens anyway</div>
   Flipped bits, a burst of zeroes, records lost outright, a file cut short, lines dropped on a wire, or an edit someone made.</div>
 <div class="step"><h3>4. Verify <span class="n">4</span></h3><div class="v">sstr verify IN</div>
   One report: what was repaired, what was rebuilt, what was lost, whether the chain of checkpoints holds and who signed it
   (<code>--allowed-signers</code>). Exit 0 only when the payload is whole.</div>
 <div class="step"><h3>5. Let go <span class="n">5</span></h3><div class="v">sstr play IN</div>
   <ul><li>stdout or <code>-o FILE</code>: the bytes, as recorded</li><li><code>--paced</code>, <code>--speed</code>, <code>--start</code>: at the recorded pace</li>
   <li><code>--serve HOST:PORT</code>: to VLC or mpv as clients, <code>--follow</code> while still recording</li><li><code>sstr export DIR</code>: a folder back out as files</li></ul></div>
</div>
<div class="under">
 <div class="box"><h3>Who calls it <span class="n">6</span></h3>
   <b>ytq</b>, the download queue that watches the clipboard, records each finished download into a <code>.sstr</code>, reads it back,
   and removes the MP4 only when the capture verifies and its payload's SHA-256 is the MP4's (<code>OUTPUT=sstr</code>, the crate's default).</div>
 <div class="box"><h3>Who looks at it <span class="n">7</span></h3>
   <b>sstr-workspace</b>: a Browser over the archive folder, an Inspector that shows what <code>sstr verify</code> says in the same words,
   a Transcript, a Shelf, and Services &mdash; each one a <code>sstr</code> command line printed before it runs.</div>
</div>
""" % DCSS
shoot("01-journey", journey, 1240, 545)

bar = "".join('<span class="%s" style="width:%dpx">%s</span>' % (c, w, t) for c, w, t in [
    ("M", 46, "MAGIC"), ("H", 34, "H"), ("H", 34, "H")] +
    [("D", 26, "D")] * 16 + [("P", 26, "P")] + [("D", 26, "D")] * 16 + [("P", 26, "P")] +
    [("D", 26, "D")] * 6 + [("P", 26, "P"), ("E", 30, "E"), ("E", 30, "E"), ("C", 30, "C"), ("C", 30, "C")])

layout = """<!doctype html><meta charset="utf-8"><style>%s</style>
<h1>The .sstr file, record by record</h1>
<p class="sub">From <code>src/format/mod.rs</code>, <code>record.rs</code> and <code>writer.rs</code>. Little-endian throughout.</p>
<div class="box"><h3>The stream <span class="n">1</span></h3>
<div class="strip">%s</div>
<div class="k" style="font-size:12.5px">The signature <code>\\x89SST\\r\\n\\x1a\\n</code>, the stream header twice, then data records. After every 16 data records (<code>--group</code>),
or at a checkpoint, a parity record. Every 64 hashed records or 10 s, a signed checkpoint, twice, straight after a parity record.
At the end: the last parity, the end record twice, the last checkpoint.</div>
<div style="margin-top:8px"><span class="tag H">H</span>head: the stream header, JSON, written twice
<span class="tag D" style="margin-left:14px">D</span>data: payload bytes, optionally deflated
<span class="tag P" style="margin-left:14px">P</span>parity
<span class="tag C" style="margin-left:14px">C</span>check: a signed checkpoint, twice
<span class="tag E" style="margin-left:14px">E</span>end: length and SHA-256, twice</div>
</div>
<div class="row" style="margin-top:14px;gap:14px">
<div class="box" style="flex:1.25"><h3>One record <span class="n">2</span></h3>
<div class="strip">
<span class="M" style="width:56px">SYNC</span><span class="H" style="width:140px">hdr32</span><span class="P" style="width:112px">rs32</span>
<span class="M" style="width:56px">SYNC2</span><span class="H" style="width:140px">hdr32</span><span class="P" style="width:112px">rs32</span>
<span class="D" style="width:110px">body &hellip;</span></div>
<div class="mono">SYNC <b>\\xa7SSR</b> &middot; SYNC2 <b>\\xa7SSr</b> &middot; 4 + 32 + 32 = 68 bytes a copy, 136 for both</div>
<table><tr><th>hdr32 (32 bytes)</th><th>bytes</th></tr>
<tr><td class="mono">type u8 &nbsp;H D P C E</td><td>1</td></tr><tr><td class="mono">flags u8 &nbsp;(1 = deflate)</td><td>1</td></tr>
<tr><td class="mono">reserved u16</td><td>2</td></tr><tr><td class="mono">seq u64</td><td>8</td></tr>
<tr><td class="mono">t_us u64 &nbsp;microseconds since the start</td><td>8</td></tr>
<tr><td class="mono">plain_len u32 &nbsp;(at most 16 MiB)</td><td>4</td></tr><tr><td class="mono">body_crc u32</td><td>4</td></tr>
<tr><td class="mono">hdr_crc u32 &nbsp;CRC-32 of the 28 above</td><td>4</td></tr></table>
<p style="font-size:12.5px;margin:8px 0 0">The header is one RS codeword of 64 bytes, not interleaved, so it is written twice: a burst longer than 32 bytes leaves one copy repairable.</p>
</div>
<div class="box" style="flex:1"><h3>The body: RS(255,223), interleaved <span class="n">3</span></h3>
<p style="margin:2px 0 6px;font-size:12.5px">The plain body in pieces of 223, each followed by 32 parity bytes; a last piece shortened. Then written column by column &mdash;
every first byte, every second byte &hellip; &mdash; so a burst of B bytes lands as about B&nbsp;/&nbsp;codewords in each.</p>
<div class="mono" style="font-size:12px;line-height:1.35">
cw 0 &nbsp;[d d d &hellip; 223 &hellip; d | p &hellip; 32 &hellip; p]<br>cw 1 &nbsp;[d d d &hellip; &hellip; &hellip; &hellip; d | p &hellip; &hellip; &hellip; p]<br>
&hellip;<br>on disk: cw0[0] cw1[0] &hellip; cw0[1] cw1[1] &hellip;</div>
<p style="margin:8px 0 0;font-size:12.5px">A 64 KiB record is 294 codewords, 74,944 bytes. Each corrects 16 unknown bytes, or 32 known-lost (erasures, as armor's missing lines are).</p>
<h3 style="margin-top:12px">The parity record <span class="n">4</span></h3>
<p style="margin:2px 0 0;font-size:12.5px">The group's count and each record's seq, t_us, flags, plain_len and body_crc (25 bytes each), then the rows.
<b>Version 0</b>: P, the XOR of the bodies &mdash; any one lost record rebuilt. <b>Version 1</b> (what <code>sstr record</code> writes): P, then Q over GF(256)
with coefficient g<sup>i</sup> for the record in place i &mdash; any two.</p>
</div></div>
<div class="box" style="margin-top:14px"><h3>The chain and the signature <span class="n">5</span></h3>
<div class="mono" style="font-size:12.5px">record_hash = SHA-256(28-byte header &#8214; plain body)
&nbsp;&middot;&nbsp; genesis = SHA-256("sstr-v0 genesis" &#8214; stream_id)<br>
checkpoint(n) = SHA-256("sstr-v0 checkpoint" &#8214; checkpoint(n-1) &#8214; for each entry: seq &#8214; record_hash)<br>
signed: ssh-keygen -Y sign -n sstr@copal over "sstr-v0 " &#8214; stream_id &#8214; digest &nbsp;&middot;&nbsp; checked: ssh-keygen -Y verify with --allowed-signers</div>
<p style="margin:6px 0 0;font-size:12.5px">A checkpoint also carries the stream header, as MPEG-TS repeats its program tables: a reader that lost the head, or joined late, has it again.</p>
</div>
""" % (DCSS, bar)
shoot("03-layout", layout, 1240, 880)

print("feature-guide: pictures in %s; sessions in %s" % (OUT, SHOTS))
if not KEEP:
    print("feature-guide: (--keep to keep the work folder; removing it)")
    shutil.rmtree(W, ignore_errors=True)
