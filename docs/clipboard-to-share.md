<!-- SPDX-License-Identifier: MIT -->
<!-- Copyright (c) 2026 Paul Richeson -->

# From the Mac's clipboard to a file on the share

Copal runs as a guest under UTM on a Mac. This is the short version of the
one workflow that crosses the machine boundary in both directions: **a URL
copied on the Mac becomes a video file the Mac can open**, without either
side knowing anything about the other beyond a folder and a clipboard.

Five parts, each of which works alone and is worth checking alone:

| | | |
|---|---|---|
| 1 | the clipboard crosses | `copal-clip bridge`, over SPICE |
| 2 | a URL in the clipboard is queued | `ytq`, or Super+Shift+Y |
| 3 | the queue downloads | one runner, one file at a time |
| 4 | what it leaves is a **file**, not a capture | `OUTPUT=mp4` |
| 5 | the file lands where the Mac can see it | `/mnt/share`, 9p |

---

## 1. The clipboard crosses

UTM shares a clipboard over SPICE, and on a Wayland desktop that takes **two**
programs, not one.

`spice-vdagent` is an X11 program — "Spice session guest agent: X11" is its own
version banner — so it reads the selection off an X server and needs a
`DISPLAY`. Hyprland's Xwayland provides one. But vdagent shares the *Xwayland*
selection, and Hyprland does not mirror that to the Wayland one, so on its own
the host's clipboard reaches `xterm` and nothing else. **`copal-clip bridge` is
the wire between the two.**

Hyprland starts both from `exec-once`, each guarded on the SPICE port existing
and each waiting up to thirty seconds for Xwayland to bind its socket —
`exec-once` fires before it has, and an agent started that early gives up with
"Screen count is zero, are we on wayland?" and never retries.

    copal-clip copy | cut | paste     Super+C / Super+X / Super+V
                                      (also Ctrl+Alt+C / X / V)
    copal-clip history                Super+Ctrl+V — the picker
    copal-clip bridge                 the VM host wire, started by the session
    copal-clip watch                  records the history, started by the session

Copy on the Mac, **Super+V** in the guest. Copy in the guest with **Super+C**,
**Cmd+V** on the Mac. Text only.

To check it, in this order — each answers a different failure:

    ls /dev/virtio-ports/com.redhat.spice.0    the guest is a SPICE guest
    rc-service spice-vdagentd status           the root daemon is up
    rc-update show | grep spice                ...and comes back at boot
    pgrep -af 'spice-vdagent -x'               the X11 session client
    pgrep -af 'copal-clip bridge'              the wire to Wayland
    wl-paste                                   what actually arrived

If the port is missing this is not a SPICE guest and nothing else matters. If
the daemon is down, `rc-service spice-vdagentd start` and
`rc-update add spice-vdagentd`. If both are up and only `bridge` is missing,
the Wayland side is the half that is broken, and that is the one that makes
Super+V look like it does nothing.

## 2. A URL in the clipboard is queued

Three ways in, and they all reach the same `queue.json`:

    ytq                 the window. The URL on the clipboard right now is
                        queued at once, and while the window HAS FOCUS every
                        URL copied afterwards is checked and queued too
    Super+Shift+Y       take the clipboard once, now             (ytq clip)
    ytq add URL...      for URLs typed in a shell

The window only watches **while it is focused**, on purpose: a queue that
grabbed every link you copied for any other reason would be a nuisance. So the
habit that works is: copy on the Mac, click the ytq window, and it is queued.

**Sites.** YouTube — watch, shorts, live and embed links, with or without the
`https://` — plus `x.com` and `reddit.com`. Any `http(s)` URL is queued and
handed to yt-dlp, which decides whether it can fetch it; what is
YouTube-specific is that a whole *page* of text (a bookmarks export, a page of
notes) is scanned for YouTube links, and that captions are fetched.

## 3. The queue downloads

Exactly one process downloads: whichever holds `run.lock`. That is `ytq run`,
or the window, or a background runner. To have Super+Shift+Y start one by
itself:

    touch ~/.config/ytq/auto

Then a runner starts whenever nothing is downloading and leaves when the queue
is empty. `--run` and `--no-run` override the file for one command.

    ytq status          what is downloading, which step, what is left
    ytq list            every entry: queued, done, waiting, failed

A failure that reads as a login, an age gate or a bot check opens Brave on the
URL and marks the entry `cookies`; sign in there, then `ytq cookies`.

## 4. What it leaves is a file, not a capture

**This is the setting the whole workflow turns on.**

    ~/.config/copal/media.conf          OUTPUT=mp4

| `OUTPUT` | leaves | for |
|---|---|---|
| `mp4` | the video file yt-dlp downloaded | **a share another machine reads** |
| `sstr` | a Static Stream capture; the video goes once the capture verifies | an archive |
| `both` | the two | an archive you also want to watch |

staticstream's own default is `sstr`, and Copal installs `mp4`. Both are right
about different questions. A capture is the better thing to *keep*: it is
signed, it carries the source URL, the license and the notes in its header, and
about 21% more bytes let it come back from damage that would take an MP4 with
it. But **a `.sstr` is not a file a player opens** — and the entire point of
this workflow is that something on the other side of the share can open what
lands there. A share full of captures looks empty to everything but `sstr`.

`media.conf` is read *before* `~/.config/ytq/config`, so a key set there covers
`sstr`, `ytq` and the Workspace together, and ytq's own file still wins over it.
Check what is actually in force with:

    ytq --help | grep archive:

One download can go the other way without editing anything:

    ytq --sstr URL      this one kept as a capture
    ytq --mp4 URL       this one kept as the file
    ytq --both URL      both

And to change it for good, without opening an editor:

    sstr config set OUTPUT both        ~/.config/copal/media.conf
    sstr config                        every setting, and which file set it

or **`,`** in the Workspace, which is the same commands with keys on them.

## 5. The file lands where the Mac can see it

`DIR` defaults to `~/Downloads/SharedVM` **when that share is really mounted** —
a link to an unmounted share would quietly fill the empty mount point instead —
and otherwise to `XDG_VIDEOS_DIR` or `~/Videos`.

    HOST      UTM -> the VM -> Edit -> Sharing -> Directory Share Path
              (utm/utm-vm.sh defaults to ~/Downloads/SharedVM). UTM keeps
              this as a security-scoped bookmark only the app can mint, so
              it is chosen once, by hand, and cannot be scripted.
    GUEST     /mnt/share, by autofs, with ~/Shared and
              ~/Downloads/SharedVM as symlinks to it

`~/Downloads/SharedVM` is deliberately the same path on both sides, so a note
or a script that names it stays true whichever machine runs it.

Each download leaves two files beside each other:

    Author-Title_ID.mp4     the video, with the notes in its metadata
    Author-Title_ID.txt     Notes, the description, and the captions as text

The Notes block is the citation — the parts someone else can check, taken at
download time, because a title can be edited and a video made private after you
saw it:

    Title, Author, Where, Site, URL, Published, Downloaded, Duration,
    License, Video, Captions

`Where` is read from whichever field the site in hand means by it: `r/SUBREDDIT`
for Reddit, the `@handle` for x.com, the channel for YouTube when that is not
simply the uploader's name again. Reddit is also the one site that gives
downvotes, so it is the one whose Stats block shows them.

---

## If you already have a folder of captures

This is the other direction, and the reason it exists: a machine that ran with
`OUTPUT=sstr` has a share full of `.sstr` files that the Mac cannot open.

    sstr export /mnt/share              every capture in the folder
    sstr export /mnt/share -n           what it would do, writing nothing
    sstr export /mnt/share -r           and every folder under it
    sstr export DIR --into ELSEWHERE    write the files somewhere else
    sstr export DIR --remove            delete each capture once its file
                                        is written and verified

Or, in the Workspace (**Super+Shift+A**), select the folder and press **X**.
That is the same command line, printed into the Transcript before it runs.

What it does, and does not do:

- **The name comes from the capture**, not from you: the `content_type` in its
  header decides `.mp4`, `.webm`, `.mkv`, `.m4a`, `.txt` and the rest.
- **A file already there is skipped and counted**, never overwritten and never
  quietly renamed to a `-1` name. In bulk a surprise copy is as bad as a
  surprise overwrite. `--force` overwrites and says so.
- **Nothing is renamed to the real name until the capture has verified.** A
  damaged capture's recovered bytes are still worth having, so they stay as
  `NAME.part`, named in the report, with the damage counts printed under it.
  Exit status is 1 if any capture failed.
- **`--remove` is the only way a capture is deleted**, it happens only after
  that verified rename, and it is never the default.

A single capture, when that is all you want, is still:

    sstr play FILE.sstr -o FILE.mp4

---

## Checking the whole chain

    pgrep -af 'copal-clip bridge'        1. the clipboard wire is running
    wl-paste                             1. and the Mac's copy arrived
    sstr config                          4. every setting, and who set it
    ytq --help | grep archive:           4. OUTPUT is what ytq will act on
    mountpoint /mnt/share                5. the share is really mounted
    ytq add 'https://www.youtube.com/watch?v=jNQXAC9IVRw' --run
    ls -l ~/Downloads/SharedVM           an .mp4 and a .txt, on the Mac too

And when something did not happen, `~/.local/share/ytq/ytq.log` has every line
yt-dlp printed, tagged with the pid that wrote it and the video id, each step's
duration, and why a download stopped.
