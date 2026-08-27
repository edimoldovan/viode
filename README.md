# Viode

**The mission: get Lex Fridman off macOS.**

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
hand-editable, git-diffable. Clips play back-to-back in file order (the
gapless-sequence model); positions are derived, never stored.

```toml
[project]
name = "mycut"
fps = 30.0
resolution = [1920, 1080]

[[clip]]
src = "media/interview.mp4"
in = "00:00:04.200"   # where playback starts in the source file
out = "00:01:12.000"  # where it ends

[[clip]]
src = "media/broll.mp4"
out = 2.5             # times accept numbers or timecodes; `in` defaults to 0
```

## AI editing over MCP

`viode serve --mcp` speaks MCP on stdio. Register it with your client (for
Claude Code, this repo ships [.mcp.json](.mcp.json)) and the AI gets every
CLI verb as a tool, plus senses:

- `frame_grab` — returns the frame at any timeline position as an image,
  so the model can *look at* a cut before judging it
- `thumbs` / `waveform` — contact sheets and waveform images
- `audio_levels`, `silence_detect` / `silence_cut`, `scene_detect` /
  `scene_split`
- `render_preview` — fast sub-range render for checking a section

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
| 5 | Multi-track, multicam, transcript-driven editing | ⏳ |

## Development

```bash
cargo build && cargo test   # 48 tests: unit, property-based, end-to-end
```

Tests generate their own media with ffmpeg and self-skip on machines
without ffmpeg/GES. Read `crates/viode-cli/tests/cli.rs`
(`full_edit_workflow`) for the product walkthrough in test form, and
`crates/viode-cli/tests/mcp.rs` for a worked MCP session.

## License

[MIT](LICENSE) © Eduard Moldovan
