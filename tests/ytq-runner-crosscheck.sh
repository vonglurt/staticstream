#!/bin/sh
# SPDX-License-Identifier: MIT
# Copyright (c) 2026 Paul Richeson
#
# ytq-runner-crosscheck.sh -- make ytq-runner-crosscheck: the Rust runner
# (sstr ytq run) against the Python ytq's, for step 2b of docs/phase-2.md.
#
# Both ytqs work the same scenarios, each in its own HOME, against the
# stand-in yt-dlp in tests/standin, which pauses at gates so both can be looked
# at the same moment. Compared, after normalising times, pids and the home:
# the queue, the log, the files left, the notifications and the browser opened.
#
#   A  a download, with 'status' mid-part and mid-merge, and its transcript
#   B  SIGTERM to the runner mid-merge: the entry back in the queue
#   C  a video the check rejects
#   D  a bot check: waiting on cookies, Brave opened, 'cookies', then done via yt-brave
#   E  a download that fails twice: retry, then failed
#   F  'transcript': uploader's captions, automatic ones, and a 429
#   G  transcript text: vtt_text and textwrap.fill on a corpus
#   H  a real download of jNQXAC9IVRw, when the network is there (YTQ_REAL=0 skips)

set -u
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
SPEC=${SPEC:-$ROOT/tests/reference/ytq.py}
SSTR=${SSTR:-$ROOT/target/release/sstr}
# The ytq under test: the binary, or `sstr ytq`. Left unquoted where it is
# called, so that the two words of the default split into two.
YTQ=${YTQ:-$SSTR ytq}

[ -f "$SPEC" ] || { printf '  --      ytq-runner-crosscheck skipped: no specification at %s\n' "$SPEC"; exit 0; }
command -v python3 >/dev/null 2>&1 || { printf '  --      ytq-runner-crosscheck skipped: no python3\n'; exit 0; }
[ -x "$SSTR" ] || { printf 'ytq-runner-crosscheck: no %s -- make build\n' "$SSTR"; exit 2; }

W=$(mktemp -d "${TMPDIR:-/tmp}/sstr-ytq-runner.XXXXXX")
cleanup() { pkill -f "$W/" 2>/dev/null; rm -rf "$W"; }
trap cleanup EXIT INT TERM
FAILED=0
PASSED=0
ok() { PASSED=$((PASSED + 1)); [ -n "${VERBOSE:-}" ] && printf '  ok      %s\n' "$1"; return 0; }
bad() { FAILED=$((FAILED + 1)); printf '  FAIL    %s\n' "$1"; }
same() {
    if cmp -s "$2" "$3"; then ok "$1"; else bad "$1:"; diff -u "$2" "$3" | head -30 | sed 's/^/          /'; fi
}

cp "$SPEC" "$W/ytq.py"

STUB="$W/stub"
mkdir -p "$STUB"
cp "$ROOT/tests/standin/yt-dlp" "$STUB/yt-dlp"
printf '#!/bin/sh\nexit 1\n' > "$STUB/wl-paste"
printf '#!/bin/sh\nexit 1\n' > "$STUB/xclip"
printf '#!/bin/sh\nfor last; do :; done\nprintf "%%s\\n---\\n" "$last" >> "$HOME/notify"\n' > "$STUB/notify-send"
for b in brave flatpak xdg-open; do printf '#!/bin/sh\necho "%s $*" >> "$HOME/opened"\n' "$b" > "$STUB/$b"; done
cat > "$STUB/yt-brave" <<'EOF'
#!/bin/sh
skip=0
for a; do
    shift
    if [ "$skip" = 1 ]; then skip=0; continue; fi
    case $a in
        --profile|--keyring) skip=1 ;;
        --path) echo "$HOME/brave/Default"; exit 0 ;;
        *) set -- "$@" "$a" ;;
    esac
done
exec yt-dlp --cookies-from-browser "brave:$HOME/brave/Default" "$@"
EOF
chmod +x "$STUB"/*

py() { h=$1; shift; env -u XDG_CONFIG_HOME -u XDG_DATA_HOME HOME="$h" PATH="$STUB:$PATH" PYTHONIOENCODING=utf-8 python3 "$W/ytq.py" "$@"; }
rs() { h=$1; shift; env -u XDG_CONFIG_HOME -u XDG_DATA_HOME HOME="$h" PATH="$STUB:$PATH" $YTQ "$@"; }
# OUTPUT=mp4: what the Python ytq does, which is what this compares -- and is
# step 2c's "OUTPUT=mp4 leaves today's files". The Python ytq ignores the key.
home() { d="$W/$1"; rm -rf "$d"; mkdir -p "$d/.config/ytq" "$d/out"; printf 'DIR=%s/out\nOUTPUT=mp4\n' "$d" > "$d/.config/ytq/config"; echo "$d"; }
# start_runner <side> <home> <gates>: 'run --quiet' in the background, with
# RUNNER its own pid. A simple command, not the py/rs functions: a function
# put in the background runs in a subshell, and $! would be that subshell --
# a signal sent to it would never reach the ytq.
start_runner() {
    if [ "$1" = py ]; then
        env -u XDG_CONFIG_HOME -u XDG_DATA_HOME HOME="$2" PATH="$STUB:$PATH" PYTHONIOENCODING=utf-8 STANDIN_GATES="$3" \
            python3 "$W/ytq.py" run --quiet > "$2/run.out" 2>&1 &
    else
        env -u XDG_CONFIG_HOME -u XDG_DATA_HOME HOME="$2" PATH="$STUB:$PATH" STANDIN_GATES="$3" \
            $YTQ run --quiet > "$2/run.out" 2>&1 &
    fi
    RUNNER=$!
}
wait_for() { n=0; while [ ! -e "$1" ] && [ $n -lt 400 ]; do sleep 0.05; n=$((n + 1)); done; [ -e "$1" ]; }

# Everything a side leaves, normalised, into one file per kind.
collect() { # <scenario> <side> <home>
    s=$1 side=$2 h=$3
    sed -E -e "s|$h|HOME|g" -e 's/^[0-9-]+ [0-9:]+ \[[0-9]+\] //' -e '/took run\.lock/d' \
        -e 's/started a runner, pid [0-9]+/started a runner/' -e 's/runner [0-9]+: the queue/runner: the queue/' \
        -e 's/\(pid [0-9]+\)/(pid P)/' -e 's/checked in [0-9.]+ s/checked in T s/' -e 's/after [0-9:]+/after T/g' \
        -e 's/took [0-9:]+\)/took T)/g' -e 's/, [0-9:]+ in([,;])/, T in\1/' \
        -e 's/%\(\.\{[^}]*\}\)j/%(.{FIELDS})j/g' \
        -e '/: fetching the discussion,/d' -e '/--write-comments/d' \
        -e '/: discussion run exited/d' -e '/: discussion:/d' "$h/.local/share/ytq/ytq.log" > "$W/$s.log.$side" 2>/dev/null
    python3 -c '
import json, sys
try: q = json.load(open(sys.argv[1]))
except OSError: q = []
def whole(v):
    # 9961471.0 from Python and 9961471 from Rust are one number, written two ways.
    if isinstance(v, float) and v.is_integer(): return int(v)
    if isinstance(v, dict): return {k: whole(x) for k, x in v.items()}
    if isinstance(v, list): return [whole(x) for x in v]
    return v
q = whole(q)
for i in q: i.pop("added", None)
print(json.dumps(q, indent=1, sort_keys=True, ensure_ascii=False).replace(sys.argv[2], "HOME"))' "$h/.local/share/ytq/queue.json" "$h" > "$W/$s.queue.$side"
    (cd "$h/out" && for f in *; do [ -e "$f" ] && printf '%s %s\n' "$f" "$(wc -c < "$f")"; done) > "$W/$s.files.$side"
    for f in "$h"/out/*.txt; do [ -e "$f" ] && { echo "== $(basename "$f")"; notes_only "$f"; }; done > "$W/$s.txt.$side"
    { cat "$h/notify" 2>/dev/null; echo "== opened"; cat "$h/opened" 2>/dev/null; } | sed "s|$h|HOME|g" > "$W/$s.said.$side"
}
compare() { # <scenario> <label>
    for kind in queue log files txt said; do same "$2: $kind" "$W/$1.$kind.py" "$W/$1.$kind.rs"; done
}
# The log likewise. The Rust ytq makes a yt-dlp run the Python ytq has no
# equivalent of -- the comment fetch of phase 5 -- so its four lines come out of
# the log before the two are compared, and the yt-dlp field lists are collapsed
# to %(.{FIELDS})j because the Rust asks for more fields than the Python did.
# Both are done in collect() above. The step still RUNS in every scenario, which
# is the part worth having: what is filtered is the record of a step the
# specification never had, not the step itself.
#
# The .txt as both sides can be held to write it.
#
# Phase 5 gives the Rust ytq two sections the frozen Python ytq never had -- a
# stamped Stats block and a threaded Discussion -- so they come out before the
# two are compared, exactly as the Downloaded line is normalised rather than
# demanded to match. This comparison's job is that the Rust did not break what
# the Python did; a specification written before a feature cannot be that
# feature's oracle, so what the new sections CONTAIN is held by unit fixtures
# instead (capture_meta, notes_at and discussion in src/ytq/runner.rs).
notes_only() {  # <file>
    awk '
        /^Discussion  \(/ { rest = 1 }
        rest              { next }
        /^Stats  \(read /  { stats = 1; next }
        stats && /^$/     { stats = 0; next }
        stats             { next }
        /^$/              { held++; next }
                          { while (held-- > 0) print ""
                            sub(/^  Downloaded: .*/, "  Downloaded: T"); print }
    ' "$1"
}
norm_status() { sed -E -e "s|$2|HOME|g" -e 's/\(for [0-9:]+\)/(for T)/' -e 's/started [0-9:]+ ago/started T ago/' -e 's/runner pid [0-9]+/runner pid P/' -e 's/^runner: pid [0-9]+/runner: pid P/' "$1"; }

echo "  --      ytq-runner-crosscheck: $YTQ run against $SPEC"

# A: a download, looked at mid-part and mid-merge.
for side in py rs; do
    H=$(home "A-$side"); G="$H/gates"; mkdir -p "$G"
    $side "$H" add --no-run 'https://www.youtube.com/watch?v=GATEDAAAAAA' > /dev/null 2>&1
    start_runner "$side" "$H" "$G"
    runner=$RUNNER
    wait_for "$G/at.1" || bad "A $side: never reached gate 1"
    sleep 0.3
    $side "$H" status > "$W/A.st1.raw.$side"; norm_status "$W/A.st1.raw.$side" "$H" > "$W/A.st1.$side"
    touch "$G/go.1"
    wait_for "$G/at.2" || bad "A $side: never reached gate 2"
    sleep 0.3
    $side "$H" status > "$W/A.st2.raw.$side"; norm_status "$W/A.st2.raw.$side" "$H" > "$W/A.st2.$side"
    touch "$G/go.2"
    wait "$runner"
    collect A "$side" "$H"
done
same "A: status mid-part" "$W/A.st1.py" "$W/A.st1.rs"
same "A: status mid-merge" "$W/A.st2.py" "$W/A.st2.rs"
compare A "A: a download and its transcript"

# B: SIGTERM mid-merge.
for side in py rs; do
    H=$(home "B-$side"); G="$H/gates"; mkdir -p "$G"
    $side "$H" add --no-run 'https://www.youtube.com/watch?v=SIGTERMAAAA' > /dev/null 2>&1
    start_runner "$side" "$H" "$G"
    runner=$RUNNER
    touch "$G/go.1"
    wait_for "$G/at.2" || bad "B $side: never reached gate 2"
    sleep 0.3
    kill -TERM "$runner"
    wait "$runner"
    echo "runner exit $?" > "$W/B.exit.$side"
    sleep 0.3
    pgrep -f "$H/out" > /dev/null && echo "yt-dlp still running" >> "$W/B.exit.$side"
    collect B "$side" "$H"
done
same "B: the runner's exit, and no yt-dlp left behind" "$W/B.exit.py" "$W/B.exit.rs"
grep -q '"status": "queued"' "$W/B.queue.rs" && grep -q '"live": {}' "$W/B.queue.rs" \
    && ok "B: the Rust entry is back in the queue with its live record cleared" || bad "B: the Rust entry was not put back"
compare B "B: SIGTERM mid-merge"

# C, D, E: the check rejects; a bot check and the cookie retry; a flaky download.
for side in py rs; do
    H=$(home "C-$side")
    $side "$H" add --no-run 'https://www.youtube.com/watch?v=UNAVAIL0000' > /dev/null 2>&1
    $side "$H" run --quiet > /dev/null 2>&1
    collect C "$side" "$H"

    H=$(home "D-$side")
    $side "$H" add --no-run 'https://www.youtube.com/watch?v=BOT00000000' > /dev/null 2>&1
    $side "$H" run --quiet > /dev/null 2>&1
    collect D1 "$side" "$H"
    $side "$H" cookies --no-run > /dev/null 2>&1
    collect D2 "$side" "$H"
    $side "$H" run --quiet > /dev/null 2>&1
    collect D3 "$side" "$H"

    H=$(home "E-$side")
    $side "$H" add --no-run 'https://www.youtube.com/watch?v=FLAKY000000' > /dev/null 2>&1
    $side "$H" run --quiet > /dev/null 2>&1
    collect E "$side" "$H"
done
compare C "C: rejected at the check"
compare D1 "D: waiting on a Brave sign-in"
compare D2 "D: 'cookies'"
compare D3 "D: done with Brave's cookies"
compare E "E: retry, then failed"

# F: the transcript command.
for side in py rs; do
    H=$(home "F-$side")
    for id in TRANSCRIPTU TRANSCRIPTA NOCAPS00000; do
        $side "$H" transcript "youtu.be/$id" >> "$H/transcript.out" 2>&1; echo "exit $?" >> "$H/transcript.out"
    done
    $side "$H" transcript https://example.com/x >> "$H/transcript.out" 2>&1; echo "exit $?" >> "$H/transcript.out"
    collect F "$side" "$H"
    sed "s|$H|HOME|g" "$H/transcript.out" > "$W/F.out.$side"
done
same "F: transcript's output and exits" "$W/F.out.py" "$W/F.out.rs"
compare F "F: transcripts"

# G: transcript text on a corpus.
H=$(home G)
python3 - "$W/fill.json" "$W/vtt" <<'EOF'
import json, os, random, sys
rnd = random.Random(915)
words = ["a", "well-known", "so--called", "x-ray", "e-mail", "--flag", "1-2", "self-self-aware", "café", "naïve",
         "supercalifragilisticexpialidocious", "super-cali-fragilistic-expiali-docious-" * 3, "“quoted”", "it's", "no.", "?!",
         "\t", "word", "the", "of", "-", "--", "---", "a-", "-b", "hyphen-", "ab-cd-ef-gh", "日本語", "x" * 90, "y-" * 50]
texts = [" ".join(rnd.choice(words) for _ in range(rnd.randrange(1, 60))) for _ in range(800)]
texts += ["", " ", "a" * 78, "a" * 79, "a " * 39, "\tindented\ttext", "line\nbreaks\rand\x0cfeeds"]
json.dump(texts, open(sys.argv[1], "w"))
os.makedirs(sys.argv[2], exist_ok=True)
for n in range(60):
    cues = []
    for k in range(rnd.randrange(1, 30)):
        line = " ".join(rnd.choice(words + ["<c>tag</c>", "&amp;", "&#39;", "<00:00:01.000>", "&lt;b&gt;"]) for _ in range(rnd.randrange(1, 12)))
        cues.append("00:00:%02d.000 --> 00:00:%02d.000\n%s\n" % (k, k + 1, line))
        if rnd.random() < 0.3:
            cues.append("00:00:%02d.500 --> 00:00:%02d.510\n%s\n" % (k, k, line))
    open(os.path.join(sys.argv[2], "%02d.vtt" % n), "w").write("WEBVTT\nKind: captions\nLanguage: en\n\n" + "\n".join(cues))
EOF
env -u XDG_CONFIG_HOME -u XDG_DATA_HOME HOME="$H" PYTHONIOENCODING=utf-8 python3 - "$W/ytq.py" "$W/fill.json" "$W/vtt" > "$W/G.py" <<'EOF'
import glob, importlib.util, json, sys, textwrap
spec = importlib.util.spec_from_file_location("ytq", sys.argv[1]); y = importlib.util.module_from_spec(spec); spec.loader.exec_module(y)
for t in json.load(open(sys.argv[2])):
    print(json.dumps(textwrap.fill(t, 78)))
for f in sorted(glob.glob(sys.argv[3] + "/*.vtt")):
    print(json.dumps(y.vtt_text(f)))
EOF
{ rs "$H" probe fill "$W/fill.json"; for f in "$W"/vtt/*.vtt; do rs "$H" probe vtt "$f"; done; } > "$W/G.rs"
same "G: textwrap.fill on $(python3 -c 'import json,sys; print(len(json.load(open(sys.argv[1]))))' "$W/fill.json") texts and vtt_text on 60 caption files" "$W/G.py" "$W/G.rs"

# H: a real download, both ways.
if [ "${YTQ_REAL:-1}" != 0 ] && timeout 60 yt-dlp --ignore-config --simulate --no-warnings --print id 'https://www.youtube.com/watch?v=jNQXAC9IVRw' > /dev/null 2>&1; then
    REAL="$W/real-stub"; mkdir -p "$REAL"
    cp "$STUB/notify-send" "$STUB/wl-paste" "$STUB/xclip" "$REAL/"
    for side in py rs; do
        H=$(home "H-$side")
        env -u XDG_CONFIG_HOME -u XDG_DATA_HOME HOME="$H" PATH="$REAL:$PATH" PYTHONIOENCODING=utf-8 sh -c '
            if [ "$1" = py ]; then set -- python3 "$2"; else set -- "$3" ytq; fi
            "$@" add --no-run https://www.youtube.com/watch?v=jNQXAC9IVRw && "$@" run --quiet' _ "$side" "$W/ytq.py" "$SSTR" > /dev/null 2>&1
        (cd "$H/out" && ls) > "$W/H.files.$side"
        for f in "$H"/out/*.mp4; do ffprobe -v error -show_entries format_tags=title,artist,date,comment -of default=nw=1 "$f" | sed -E 's/^(Downloaded: ).*/\1T/'; done > "$W/H.tags.$side"
        for f in "$H"/out/*.txt; do notes_only "$f"; done > "$W/H.txt.$side"
        python3 -c 'import json,sys; print([(i["status"], i["file"].replace(sys.argv[2], "HOME"), i["title"], i["quality"]) for i in json.load(open(sys.argv[1]))])' \
            "$H/.local/share/ytq/queue.json" "$H" > "$W/H.queue.$side"
    done
    same "H: real download: file names" "$W/H.files.py" "$W/H.files.rs"
    same "H: real download: the tags in the MP4" "$W/H.tags.py" "$W/H.tags.rs"
    same "H: real download: the transcript and its Notes" "$W/H.txt.py" "$W/H.txt.rs"
    same "H: real download: the queue entry" "$W/H.queue.py" "$W/H.queue.rs"
else
    printf '  --      H skipped: no network, or YTQ_REAL=0\n'
fi

if [ "$FAILED" -eq 0 ]; then
    printf '  ok      ytq-runner-crosscheck: %d comparisons agree\n' "$PASSED"
else
    printf '\033[31merror:\033[0m ytq-runner-crosscheck: %d of %d comparisons disagree\n' "$FAILED" $((FAILED + PASSED))
    exit 1
fi
