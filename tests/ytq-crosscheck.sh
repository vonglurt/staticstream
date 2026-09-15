#!/bin/sh
# SPDX-License-Identifier: MIT
# Copyright (c) 2026 Paul Richeson
#
# ytq-crosscheck.sh -- make ytq-crosscheck: the Rust ytq (sstr ytq) against the
# Python ytq it replaces, for step 2a of docs/phase-2.md.
#
#   1. Which text is a video: youtube_urls, as_url and short, the Python ytq's
#      own functions against sstr ytq probe urls, over a corpus -- every case
#      in the ytq report, and thousands generated around them.
#   2. Settings: settings() under a set of homes, and the help's settings lines.
#   3. One queue: entries added by either, listed by both; the status panel of a
#      download under way; clear; thirty adds from both at once; a file the Rust
#      ytq rewrote, byte for byte as Python's json.dump writes it.
#   4. The clipboard: clip on a bookmarks excerpt, a plain URL and plain words,
#      with the same queue, message and exit.
#
# The Python ytq is cut from ../copal/copal-prep.sh (or $STATICSTREAM_COPAL),
# so it is the one a clean install writes. Every run has its own HOME, and
# stub wl-paste, xclip and notify-send, so the real clipboard, queue and
# notifications are never touched.

set -u
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
COPAL=${STATICSTREAM_COPAL:-$ROOT/../copal}
SSTR=${SSTR:-$ROOT/target/release/sstr}

[ -f "$COPAL/copal-prep.sh" ] || { printf '  --      ytq-crosscheck skipped: no copal checkout at %s\n' "$COPAL"; exit 0; }
command -v python3 >/dev/null 2>&1 || { printf '  --      ytq-crosscheck skipped: no python3\n'; exit 0; }
[ -x "$SSTR" ] || { printf 'ytq-crosscheck: no %s -- make build\n' "$SSTR"; exit 2; }

W=$(mktemp -d "${TMPDIR:-/tmp}/sstr-ytq-crosscheck.XXXXXX")
trap 'rm -rf "$W"' EXIT INT TERM
FAILED=0
PASSED=0
ok() { PASSED=$((PASSED + 1)); [ -n "${VERBOSE:-}" ] && printf '  ok      %s\n' "$1"; return 0; }
bad() { FAILED=$((FAILED + 1)); printf '  FAIL    %s\n' "$1"; }
same() { # <label> <file a> <file b>
    if cmp -s "$2" "$3"; then ok "$1"; else bad "$1:"; diff -u "$2" "$3" | head -20 | sed 's/^/          /'; fi
}

awk '/cat > \/usr\/local\/bin\/ytq <<.YTQ./{f=1;next} /^YTQ$/{f=0} f' "$COPAL/copal-prep.sh" > "$W/ytq.py"
[ -s "$W/ytq.py" ] || { bad "could not cut the Python ytq out of copal-prep.sh"; exit 1; }

STUB="$W/stub"
mkdir -p "$STUB"
printf '#!/bin/sh\ncat "${YTQ_CLIP:-/dev/null}"\n' > "$STUB/wl-paste"
printf '#!/bin/sh\nexit 1\n' > "$STUB/xclip"
printf '#!/bin/sh\nfor last; do :; done\nprintf "%%s\\n---\\n" "$last" >> "${YTQ_NOTIFY:-/dev/null}"\n' > "$STUB/notify-send"
chmod +x "$STUB"/*

# py|rs <home> args...: one ytq or the other, in that home, with the stubs.
py() { h=$1; shift; env -u XDG_CONFIG_HOME -u XDG_DATA_HOME HOME="$h" PATH="$STUB:$PATH" YTQ_NOTIFY="$h/notify" PYTHONIOENCODING=utf-8 python3 "$W/ytq.py" "$@"; }
rs() { h=$1; shift; env -u XDG_CONFIG_HOME -u XDG_DATA_HOME HOME="$h" PATH="$STUB:$PATH" YTQ_NOTIFY="$h/notify" "$SSTR" ytq "$@"; }
home() { d="$W/home-$1"; rm -rf "$d"; mkdir -p "$d"; echo "$d"; }

echo "  --      ytq-crosscheck: $SSTR ytq against the ytq in $COPAL/copal-prep.sh"

# 1. Which text is a video ---------------------------------------------------
H=$(home urls)
python3 - "$W/corpus.json" <<'EOF'
import json, random, sys
rnd = random.Random(20260915)
corpus = [
    # The ytq report: the bookmarks excerpt, line by line and whole.
    '<DT><A HREF="https://www.youtube.com/watch?v=sK99WuaU_k8" ICON="data:image/png;base64,iVBORw0KGgo=">x</A>',
    'https://www.youtube.com/shorts/3mxgcs10PoI', 'https://www.youtube.com/watch?v=EmFV-A5E5hk&list=RDu64NgDuis6g&index=2',
    'https://www.youtube.com/watch?v=6xlmaorRY0w&t=5330s', 'https://www.youtube.com/watch?t=807&v=s6rGdKY2xWo&feature=youtu.be',
    'https://www.youtube.com/watch?feature=share&amp;v=_m_jwz5hzzw', 'https://youtu.be/-yKLnpqfwSQ?si=abc',
    'https://www.youtube.com/@bernadettebanner', 'https://www.youtube.com/channel/UCCND6a0H56zHL4YuY226ZOQ',
    'https://www.youtube.com/watch?v=tooShort', 'https://www.youtube.com/watch?v=0eFTrOpueYEX',
    'https://www.seangoedecke.com/x', 'chrome://newtab/', 'https://vimeo.com/12345', 'plain words', '',
    # IV-P: Shorts without a scheme, the matches and the refusals, as_url's seventeen.
    'youtube.com/shorts/SWHZolxKdVU', 'www.youtube.com/shorts/SWHZolxKdVU', 'm.youtube.com/shorts/SWHZolxKdVU?feature=share',
    'http://youtube.com/shorts/SWHZolxKdVU/', 'see youtube.com/shorts/SWHZolxKdVU and https://youtu.be/jNQXAC9IVRw',
    '(youtube.com/shorts/SWHZolxKdVU)', 'HREF="youtube.com/shorts/SWHZolxKdVU"', 'notyoutube.com/shorts/SWHZolxKdVU',
    'foo.youtube.com/shorts/SWHZolxKdVU', 'evil.com/youtube.com/shorts/SWHZolxKdVU', 'youtube.com.evil.com/shorts/SWHZolxKdVU',
    'xyoutu.be/SWHZolxKdVU', 'user@youtube.com/shorts/SWHZolxKdVU', '  \nyoutu.be/SWHZolxKdVU \n',
    'https://web.archive.org/web/2024/https://www.youtube.com/watch?v=SWHZolxKdVU', 'youtube.com/shorts/SWHZolxKdVU and more words',
    'example.com/shorts/SWHZolxKdVU', 'youtube.com/@channel', 'youtube.com/watch?v=AAAAAAAAAAA&v=BBBBBBBBBBB',
    # URL_RE's edges.
    'https://a.b', 'http://localhost', 'http://localhost:8080/x y', 'https://localhost.localdomain:1/', 'https://a..b', 'https://.a.b',
    'https://a.b.', 'https://a.b:', 'https://a.b:12x', 'https://a.b/ x', 'https://a.b/\tx', 'ftp://a.b', 'https://a_b.c', 'https://a-b.c-d',
    'https://a.b?x=1', 'https://a.b/?x=1', 'HTTPS://A.B', 'https://a.b\n', 'https://xn--bcher-kva.example/', 'https://a.b:99999/',
    # Entities and the like.
    'https://www.youtube.com/watch?v=dQw4w9WgXcQ&amp;t=1', 'https://www.youtube.com/watch?x=1&#38;v=dQw4w9WgXcQ',
    'youtube.com/watch?x=&quot;&amp;v=dQw4w9WgXcQ', '&#x79;outube.com/shorts/dQw4w9WgXcQ', 'caf&eacute; youtu.be/dQw4w9WgXcQ',
    'youtu.be/dQw4w9WgXcQ&ampx', 'youtu.be/dQw4w9WgXcQ&#128;', 'é youtu.be/dQw4w9WgXcQ', 'éyoutu.be/dQw4w9WgXcQ', '_youtu.be/dQw4w9WgXcQ',
]
schemes = ['', 'http://', 'https://', 'HTTPS://', 'https:/']
prefixes = ['', 'www.', 'm.', 'music.', 'x.', 'WWW.']
hosts = ['youtube.com', 'youtu.be', 'youtube.co', 'YouTube.com', 'youtube.com.au']
paths = ['/watch?v=', '/watch?t=1&v=', '/watch?feature=a&amp;v=', '/watch?x="&v=', '/watch?x=a b&v=', '/watch?x=#&v=', '/watch?v=x&v=',
         '/shorts/', '/live/', '/embed/', '/', '/v/', '/watch/?v=', '/watch?V=', '/playlist?list=']
ids = ['dQw4w9WgXcQ', 'dQw4w9WgXc', 'dQw4w9WgXcQQ', '-_-_-_-_-_-', 'AAAAAAAAAAA']
befores = ['', ' ', 'a', '.', '@', '/', '-', 'é', '(', ' ', '"', '=', '9', '_', '　']
afters = ['', ' ', '&t=5', 'x', '-', '_', '#frag', '\n', '?si=1', '/', '"', 'é']
for _ in range(6000):
    corpus.append(rnd.choice(befores) + rnd.choice(schemes) + rnd.choice(prefixes) + rnd.choice(hosts) + rnd.choice(paths) + rnd.choice(ids) + rnd.choice(afters))
for _ in range(400):
    corpus.append(' '.join(corpus[rnd.randrange(len(corpus))] for _ in range(rnd.randrange(2, 6))))
corpus.append('\n'.join(corpus[:60]))
json.dump(corpus, open(sys.argv[1], "w"))
EOF
env -u XDG_CONFIG_HOME -u XDG_DATA_HOME HOME="$H" PYTHONIOENCODING=utf-8 python3 - "$W/ytq.py" "$W/corpus.json" > "$W/urls.py" <<'EOF'
import importlib.util, json, sys
spec = importlib.util.spec_from_file_location("ytq", sys.argv[1]); y = importlib.util.module_from_spec(spec); spec.loader.exec_module(y)
for t in json.load(open(sys.argv[2])):
    print(json.dumps({"youtube_urls": y.youtube_urls(t), "as_url": y.as_url(t), "short": y.short(t)}))
EOF
rs "$H" probe urls "$W/corpus.json" > "$W/urls.rs"
same "urls: youtube_urls, as_url and short on $(wc -l < "$W/urls.py") texts" "$W/urls.py" "$W/urls.rs"

# 2. Settings -------------------------------------------------------------------
settings_py() { env -u XDG_CONFIG_HOME -u XDG_DATA_HOME HOME="$1" python3 -c "
import importlib.util, json, sys
spec = importlib.util.spec_from_file_location('ytq', sys.argv[1]); y = importlib.util.module_from_spec(spec); spec.loader.exec_module(y)
print(json.dumps(y.S, sort_keys=True))" "$W/ytq.py"; }
for case in none config userdirs share-unmounted share-mounted; do
    H=$(home "settings-$case")
    case $case in
        config)
            mkdir -p "$H/.config/ytq"
            printf '# mine\n  dir = "~/Archive"  \nprofile=Profile 1\nKEYRING=basictext\nSUBS=\nnot a setting\n=x\npoll="2"\n' > "$H/.config/ytq/config" ;;
        userdirs)
            mkdir -p "$H/.config"
            printf 'XDG_DESKTOP_DIR="$HOME/Desktop"\nXDG_VIDEOS_DIR="${HOME}/Films/$NOPE"\n' > "$H/.config/user-dirs.dirs" ;;
        share-unmounted)
            mkdir -p "$H/Downloads" "$H/mnt"; ln -s "$H/mnt" "$H/Downloads/SharedVM" ;;
        share-mounted)
            mkdir -p "$H/Downloads"; ln -s /proc "$H/Downloads/SharedVM" ;;
    esac
    settings_py "$H" > "$W/set.py"
    rs "$H" probe settings > "$W/set.rs"
    same "settings: $case" "$W/set.py" "$W/set.rs"
    py "$H" --help 2>/dev/null | grep -E '^(config|auto):|in effect|--run or' > "$W/help.py"
    rs "$H" --help 2>/dev/null | grep -E '^(config|auto):|in effect|--run or' > "$W/help.rs"
    same "help's settings lines: $case" "$W/help.py" "$W/help.rs"
done

# 3. One queue ---------------------------------------------------------------------
H=$(home queue)
py "$H" add --no-run youtube.com/shorts/SWHZolxKdVU https://vimeo.com/12345 > /dev/null 2>&1
rs "$H" add --no-run 'https://www.youtube.com/watch?v=SWHZolxKdVU&t=5' https://example.com/a > /dev/null 2>&1
py "$H" add --no-run https://example.com/a 'https://youtu.be/jNQXAC9IVRw' > /dev/null 2>&1
py "$H" list > "$W/list.py"; rs "$H" list > "$W/list.rs"
same "list: entries added by both ytqs, each once ($(grep -c checking "$W/list.py") entries)" "$W/list.py" "$W/list.rs"
py "$H" status > "$W/status.py"; rs "$H" status > "$W/status.rs"
same "status: the same queue" "$W/status.py" "$W/status.rs"
python3 - "$H/.local/share/ytq/queue.json" <<'EOF'
import json, sys, time
q = json.load(open(sys.argv[1])); now = time.time()
q[0].update(status="downloading", title="Fake Title — café", quality="1080p mp4", attempts=1, progress="1/2 62.5% 2.00MiB/s eta 00:03",
            live={"phase": "downloading", "part": 1, "parts": 2, "formats": "299+140", "since": now - 2.4, "started": now - 3.4, "pid": 13298,
                  "cookies": False, "done_b": 0, "parts_done": 0, "pct": "62.5%", "got_b": "5033164", "total_b": "7969177", "est_b": "NA",
                  "speed": "2.00MiB/s", "eta": "00:03", "elapsed": "00:00:02", "frag": "NA", "frags": "NA", "format": "299",
                  "vcodec": "avc1.64002a", "acodec": "none", "height": "1080", "state": "downloading",
                  "dest": "/d/Fake_Title_FAKEID0000A.f299.mp4", "last": "[download] Destination: /d/Fake_Title_FAKEID0000A.f299.mp4"})
q[1].update(status="cookies", error="Sign in to confirm you're not a bot", title="Needs a sign-in")
q[2].update(status="failed", error="HTTP Error 403: Forbidden " * 8)
q.append(dict(q[3], url="https://example.com/merge", status="downloading", title="Merging one",
              live={"phase": "merging", "since": now - 61, "started": now - 200, "pid": 7, "cookies": True, "done_b": 10000000,
                    "parts_done": 2, "part": 2, "parts": 2, "dest": sys.argv[1].rsplit("/", 1)[0] + "/Merge.mp4"}))
open(sys.argv[1].rsplit("/", 1)[0] + "/Merge.temp.mp4", "wb").write(b"x" * 4000000)
json.dump(q, open(sys.argv[1], "w"), indent=1)
EOF
py "$H" status | sed -E 's/\(for [0-9:]+\)/(for T)/; s/started [0-9:]+ ago/started T ago/' > "$W/status.py"
rs "$H" status | sed -E 's/\(for [0-9:]+\)/(for T)/; s/started [0-9:]+ ago/started T ago/' > "$W/status.rs"
same "status: two downloads under way, one merging, a sign-in and a failure" "$W/status.py" "$W/status.rs"
py "$H" list > "$W/list.py"; rs "$H" list > "$W/list.rs"
same "list: with an error and a non-ASCII title" "$W/list.py" "$W/list.rs"

# The file itself: the Rust ytq rewrites it as Python's json.dump would.
rs "$H" add --no-run https://example.com/rust-wrote-this > /dev/null 2>&1
python3 -c "
import json, sys
p = sys.argv[1]; raw = open(p).read(); again = json.dumps(json.loads(raw), indent=1)
sys.exit(0 if raw == again else 1)" "$H/.local/share/ytq/queue.json" \
    && ok "queue.json rewritten by the Rust ytq is byte for byte what json.dump(indent=1) writes" \
    || bad "queue.json rewritten by the Rust ytq differs from json.dump(indent=1)"

H2=$(home clear); cp -r "$H/.local" "$H2/"
py "$H" clear > "$W/clear.py"; rs "$H2" clear > "$W/clear.rs"
same "clear: the same message" "$W/clear.py" "$W/clear.rs"
same "clear: the same queue left" "$H/.local/share/ytq/queue.json" "$H2/.local/share/ytq/queue.json"

H=$(home race)
i=0
while [ $i -lt 30 ]; do
    if [ $((i % 2)) = 0 ]; then py "$H" add --no-run "https://example.com/race/$i" > /dev/null 2>&1 &
    else rs "$H" add --no-run "https://example.com/race/$i" > /dev/null 2>&1 &
    fi
    i=$((i + 1))
done
wait
n=$(python3 -c "import json,sys; q=json.load(open(sys.argv[1])); print(len(set(i['url'] for i in q)), len(q))" "$H/.local/share/ytq/queue.json")
[ "$n" = "30 30" ] && ok "thirty adds from both ytqs at once: 30 entries, each once" || bad "thirty adds at once left $n (distinct, total)"

# 4. The clipboard and the messages --------------------------------------------------------
cat > "$W/bookmarks.html" <<'EOF'
<DT><A HREF="https://www.youtube.com/watch?v=sK99WuaU_k8" ADD_DATE="1" ICON="data:image/png;base64,iVBORw0KGgo=">a</A>
<DT><A HREF="https://www.youtube.com/shorts/3mxgcs10PoI">b</A>
<DT><A HREF="https://www.youtube.com/watch?v=EmFV-A5E5hk&list=RDu64NgDuis6g&index=2">c</A>
<DT><A HREF="https://www.youtube.com/watch?v=6xlmaorRY0w">d</A><DT><A HREF="https://www.youtube.com/watch?v=6xlmaorRY0w&t=5330s">d</A>
<DT><A HREF="https://www.youtube.com/watch?t=807&v=s6rGdKY2xWo&feature=youtu.be">e</A>
<DT><A HREF="https://www.youtube.com/watch?feature=share&amp;v=_m_jwz5hzzw">f</A>
<DT><A HREF="https://youtu.be/-yKLnpqfwSQ?si=abc">g</A>
<DT><A HREF="https://www.youtube.com/@bernadettebanner">channel</A>
EOF
printf 'https://vimeo.com/12345\n' > "$W/one-url.txt"
printf 'nothing to see here\n' > "$W/words.txt"
printf 'youtube.com/shorts/SWHZolxKdVU\n' > "$W/one-short.txt"
for clip in bookmarks.html one-url.txt words.txt one-short.txt; do
    for side in py rs; do
        H=$(home "clip-$clip-$side")
        YTQ_CLIP="$W/$clip" $side "$H" clip --no-run > "$W/clip.out.$side" 2>&1; echo "exit $?" >> "$W/clip.out.$side"
        cat "$H/notify" >> "$W/clip.out.$side" 2>/dev/null
        python3 -c "import json,sys
try: print('\n'.join(i['url'] + ' ' + i['status'] for i in json.load(open(sys.argv[1]))))
except OSError: print('(no queue)')" "$H/.local/share/ytq/queue.json" > "$W/clip.q.$side"
        YTQ_CLIP="$W/$clip" $side "$H" clip --no-run > /dev/null 2>&1
        cat "$H/notify" > "$W/clip.again.$side" 2>/dev/null
    done
    same "clip $clip: exit and message" "$W/clip.out.py" "$W/clip.out.rs"
    same "clip $clip: the queue" "$W/clip.q.py" "$W/clip.q.rs"
    same "clip $clip again: 'already queued'" "$W/clip.again.py" "$W/clip.again.rs"
done
for side in py rs; do
    H=$(home "notaurl-$side")
    $side "$H" add --no-run 'not a url' example.com/x https://a.b/ok > /dev/null 2>&1; echo "exit $?" > "$W/nu.$side"
    # Both ytqs start notify-send and do not wait for it, so two notifications
    # can arrive in either order: compare them as a set, and the log -- which
    # each writes in order before notifying -- line for line.
    awk 'BEGIN { RS = "---\n" } { print }' "$H/notify" | sort >> "$W/nu.$side"
    sed -E 's/^[0-9-]+ [0-9:]+ \[[0-9]+\] //' "$H/.local/share/ytq/ytq.log" >> "$W/nu.$side"
done
same "add: not a URL, and a mix (exit, notifications, log)" "$W/nu.py" "$W/nu.rs"

if [ "$FAILED" -eq 0 ]; then
    printf '  ok      ytq-crosscheck: %d comparisons agree\n' "$PASSED"
else
    printf '\033[31merror:\033[0m ytq-crosscheck: %d of %d comparisons disagree\n' "$FAILED" $((FAILED + PASSED))
    exit 1
fi
