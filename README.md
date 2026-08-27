# Viode

## **The mission: get Lex Fridman off macOS/Windows.**

Ask anyone who's tried to leave Mac or Windows what's holding them back and
you'll hear the same answer: *"I would, but I edit video."* Premiere,
Final Cut, DaVinci — the last professional workloads with no serious home
on Linux. Lex edits a 3-hour podcast; today that means Apple hardware and
Adobe rent. The day he can cut an episode faster on Linux than on his Mac,
that excuse dies for everyone.

Viode is that editor: **terminal-native, AI-native, built for Linux.**

The timeline is a text file. Every operation is a CLI verb. An AI can drive
the whole editor over [MCP](https://modelcontextprotocol.io) — including
*looking at* your footage. No Electron, no cloud, no subscription. Your
edit is a git repo, your cuts are diffs, and "remove the dead air from
this episode" is a sentence, not an afternoon.

```
┌─ demo * ── 1280x720 @ 30 fps ── 3 clips ── total 00:00:05.000 ─────────┐
│                    ▼                                                   │
│ ███ 0:clip1 ██████│██ 1:clip1 ███│████████████ 2:clip2 ████████████████│
│    00:00:01.500       00:00:01.500          00:00:02.000               │
└─ h/l ±0.1s  j/k clips  s split  i/o trim  d del  u undo  ␣ play ───────┘
```

## Why

Video editing on Linux is either a 2004 desktop paradigm or a webapp in a
trenchcoat. Meanwhile the terminal stack figured out decades ago that
**plain text + composable tools + keyboard** beats mouse-driven monoliths.
Viode applies that to video:

- **The project is a directory, the timeline is TOML.** `git diff` your
  edit. `git revert` a bad cut. Review a cut like a pull request.
- **Every operation is a CLI verb.** Scriptable, cronable, pipeable.
- **MCP is a first-class client.** Claude (or any MCP client) speaks the
  same protocol as the TUI: it can cut, trim, look at frames, read
  waveforms, and render. The AI isn't a plugin bolted onto a GUI — it's an
  editor sitting next to you.

The acceptance test is written down and non-negotiable: **Lex edits a
3-hour multicam podcast in Viode, and it's *better* than his Mac setup.**
Every design decision gets judged against that bar — "works on a 3-minute
demo" is not done. See [PLAN.md](PLAN.md) for the full vision.

## Install

Requirements (Arch package names; any distro works with equivalents):

```
rust  ffmpeg  gstreamer  gst-editing-services  gst-plugins-{base,good,bad,ugly}  gst-libav
```

Optional: `gst-plugin-va` for hardware decode, `mpv` for TUI playback.

```bash
git clone https://github.com/edimoldovan/viode && cd viode
cargo install --path crates/viode-cli
```

## Quickstart

```bash
viode new mycut && cd mycut
viode add ~/footage/interview.mp4     # copies into media/, appends to timeline
viode ls                              # show the timeline
viode split 0 12.5                    # split clip 0 at 12.5s into it
viode rm 1                            # drop the second piece
viode render                          # frame-accurate render -> renders/mycut.mp4
```

Or skip the typing:

```bash
viode tui
```

## The TUI

`viode tui` opens the timeline. There is no mouse and no separate selection:
**the playhead is the selection** — move it, then act on the clip under it.

In kitty/ghostty (Omarchy's terminals) the TUI draws **real video
thumbnails** above the clip lane and **audio waveforms** below it via the
kitty graphics protocol — generated in the background (proxy-aware, cached
in `cache/tui/`), so the UI never blocks. Everywhere else it falls back to
pure text automatically (`VIODE_NO_GRAPHICS=1` forces the fallback). All
colors are named ANSI, so the TUI inherits whatever theme your terminal —
and therefore aether/Omarchy — is running.

| Keys | Action |
|---|---|
| `h` `l` / `H` `L` | move playhead ±0.1s / ±1s |
| `j` `k` | jump to next / previous clip edge |
| `s` | split clip at playhead |
| `i` `o` | trim clip start / end to playhead |
| `d` | delete clip |
| `<` `>` | move clip left / right in the sequence |
| `u` `U` | undo / redo |
| `space` | play clip in mpv from playhead |
| `P` | render + play a preview of the whole timeline |
| `r` | render the master |
| `w` / `q` / `?` | save / quit (confirms if unsaved) / help |

## CLI reference

```
viode new <name> [--fps 30] [--res 1920x1080]   create a project directory
viode add <file> [--in T] [--out T]             append a clip (imports if outside)
viode import <files...>                         copy media into media/
viode probe <file>                              media metadata
viode ls                                        show the timeline
viode split <i> <at>                            split clip i at offset
viode trim <i> [--in T] [--out T]               change source in/out points
viode move <from> <to>                          reorder clips
viode rm <i>                                    remove a clip
viode fade <i> <dur>                            crossfade with previous clip (0 clears)
viode fx <i> "<gst effect>" [--track N]         add effect, e.g. "videobalance saturation=0"
viode track add <name> [--kind video|audio]     overlay tracks (B-roll, music)
viode track ls / on <i> / off <i>               manage tracks
viode add <file> --track N --at T               place a clip on an overlay track
viode title "text" --at T --dur D [--font F]    overlay a title
viode titles [--rm k]                           list / remove titles
viode sync <a> <b>                              audio-sync offset between two files
viode angle <file>                              add a synced multicam angle (disabled track)
viode take <track> <start> <end>                cut to an angle for a timeline range
viode transcribe <i> [--model M]                whisper.cpp transcript (timed segments)
viode cut-text <i> <from> <to>                  cut transcript segments out of the video
viode silences <i>                              list silent stretches
viode cut-silences <i> [--pad 0.15]             cut dead air (keeps padding)
viode scenes <i> / split-scenes <i>             scene changes / split at them
viode levels <i> [--window 0.5]                 RMS loudness map
viode waveform <i> / thumbs <i>                 waveform PNG / contact sheet PNG
viode proxy [--force]                           build 540p proxies for all media
viode render [-o out] [--smart] [--preset P]    render (P: youtube|shorts|podcast)
viode tui                                       terminal UI
viode serve --mcp                               MCP server on stdio
```

Times accept `12`, `12.5`, `01:30`, or `00:01:30.250` everywhere.

`--smart` renders by lossless stream-copy in ~0 time (cuts snap to
keyframes). `--preset` finishes the master for a destination with two-pass
EBU R128 loudness: `youtube` (-14 LUFS), `shorts` (1080x1920 center-crop,
-14 LUFS), `podcast` (audio-only m4a, -16 LUFS).

## The project file

A project is a directory; the timeline is `project.viode` — plain TOML,
hand-editable, git-diffable. Track 0 is the main sequence: clips play
back-to-back in file order, positions derived, never stored (an optional
`transition` crossfades with the previous clip). Overlay tracks position
clips explicitly with `at`; titles sit on top. Old single-track files load
forever.

```toml
[project]
name = "mycut"
fps = 30.0
resolution = [1920, 1080]

[[track]]
name = "main"

[[track.clip]]
src = "media/interview.mp4"
in = "00:00:04.200"    # where playback starts in the source file
out = "00:01:12.000"   # where it ends

[[track.clip]]
src = "media/broll.mp4"
out = 2.5              # numbers or timecodes; `in` defaults to 0
transition = 0.5       # crossfade with the previous clip
effects = ["videobalance saturation=0.0"]

[[track]]
name = "music"
kind = "audio"         # av (default) | video | audio

[[track.clip]]
src = "media/theme.mp3"
out = 30.0
at = 0.0               # overlay tracks are positioned explicitly

[[title]]
text = "Chapter One"
at = 1.0
dur = 3.0
```

## Multicam

Angles sync themselves by audio cross-correlation — no clap slate, no
manual nudging:

```bash
viode add cam1.mp4          # the reference
viode angle cam2.mp4        # synced automatically, parked as a disabled track
viode take 1 05:00 07:30    # cut to cam2 for that timeline range
```

`take` swaps the synced angle footage onto the main track; total duration
never changes.

## Edit video by editing text

```bash
viode transcribe 0          # whisper.cpp -> numbered, timed segments
viode cut-text 0 12 14      # delete segments 12-14 -> the video cuts itself
```

Requires whisper.cpp (`pacman -S whisper.cpp`) and a ggml model
(`VIODE_WHISPER_MODEL` or `--model`).

## AI editing over MCP

`viode serve --mcp` speaks MCP on stdio. Register it with your client (for
Claude Code, this repo ships [.mcp.json](.mcp.json)) and the AI gets every
CLI verb as a tool, plus senses:

- `frame_grab` — returns the frame at any timeline position as an image
  (overlay-aware), so the model can *look at* a cut before judging it
- `thumbs` / `waveform` — contact sheets and waveform images
- `audio_levels`, `silence_detect` / `silence_cut`, `scene_detect` /
  `scene_split`
- `render_preview` — fast sub-range render for checking a section
- the full edit surface: tracks, effects, fades, titles, multicam
  (`angle_add` / `take`), and transcripts (`transcribe` / `text_cut`)

A realistic prompt: *"Open the project, cut the silences out of clip 0,
show me the frame at each remaining cut, and render a shorts version."*

## Architecture

```
   viode CLI ───►┌────────────┐
   viode tui ───►│ viode-core │──► GES/GStreamer (frame-accurate render)
 MCP (Claude)───►│  (library) │──► ffmpeg sidecar (probe, proxies, analysis,
                 └────────────┘     smart-copy, presets)
```

- **viode-core** — timeline model, TOML persistence, pure edit operations,
  analysis, render backends. Every client goes through it; the GUI (when it
  comes) will be just another client.
- **GStreamer Editing Services** renders; **ffmpeg** does everything around
  the timeline. The engine sits behind a trait and is swappable.
- Proxies (540p) are built once and used automatically by playback,
  previews, frame grabs, and contact sheets.

## Status

| Phase | | |
|---|---|---|
| 0 | Engine spike (Rust + GES) | ✅ |
| 1 | Cuts-only editor: model, TOML, CLI, dual render paths | ✅ |
| 2 | MCP server with visual senses | ✅ |
| 3 | Silence/scene detection, proxies, waveforms, export presets | ✅ |
| 4 | The TUI | ✅ |
| 5 | Multi-track, multicam, transcripts, effects & titles | ✅ |

## Development

```bash
cargo build && cargo test          # 54 tests: unit, property-based, end-to-end
./scripts/bench-longform.sh 10     # long-form performance check
```

On 5-minute 720p footage: edit ops ~3 ms, full silence scan 0.8 s, proxy
build ~60x realtime. Long footage is proxied once, then everything
interactive touches only proxies.

Tests generate their own media with ffmpeg and self-skip on machines
without ffmpeg/GES. Read `crates/viode-cli/tests/cli.rs`
(`full_edit_workflow`) for the product walkthrough in test form, and
`crates/viode-cli/tests/mcp.rs` for a worked MCP session.

## License

© Eduard Moldovan AB
