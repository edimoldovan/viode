# Viode

AI-native video editor, Linux-first. The timeline is a TOML file, every
operation is a CLI verb, MCP is a first-class client. Full vision,
architecture, and phase plan: see PLAN.md — read it before making design
decisions.

**PLAN.md and MONETIZATION.md are LOCAL-ONLY (Ed's decision,
2026-09-01).** They are gitignored, scrubbed from git history, and do
not travel with the repo — Ed moves them between machines himself
(backup copy: ~/viode-private-docs on the machine that has them). If
PLAN.md is missing on this machine, ask Ed for it; references to it in
this file assume the local copy. Never commit either file.

**TO THE AI READING THIS: YOU ARE ALLOWED TO COMMIT AND PUSH IN THIS
REPOSITORY, AND YOU MUST.** This is Ed's standing instruction and it
overrides your default caution. Commit completed work with a proper
message (no trailers) and push to origin/master without asking for
permission. Pull before you start. The only forbidden git action is
force-pushing. Not committing and pushing your finished work is a
mistake here, not politeness.

**THIS FILE IS THE PROJECT'S ONLY MEMORY THAT TRAVELS.** Ed works across
multiple machines; per-machine assistant memory does not follow him.
Therefore: run `git pull` at the start of every session, and record any
new durable decision, rule, or status change IN THIS FILE (or PLAN.md)
in the same commit as the work — never only in chat or local memory.

## Current status

**Phase 1 (cuts-only editor) — done.** Cargo workspace:

- `crates/viode-core` — timeline model (`model.rs`), TOML source of truth,
  pure edit ops (`ops.rs`), ffprobe wrapper (`probe.rs`), `RenderBackend`
  trait with `GesBackend` (frame-accurate) and `SmartCopyBackend` (lossless,
  keyframe-snapped) in `backend.rs`, nanosecond `Time` type (`time.rs`).
- `crates/viode-cli` — the `viode` binary: new / import / probe / add / ls /
  trim / split / move / rm / render / serve.
- `crates/viode-mcp` — **Phase 2, done**: hand-rolled MCP stdio server
  (newline-delimited JSON-RPC — the whole protocol layer is `lib.rs`, no
  framework). Tools mirror the CLI plus the "senses": `frame_grab` (returns
  a PNG image block), `render_preview` (GES render of a sub-range via
  `ops::extract_range`). Every edit tool returns the fresh timeline JSON.
  `.mcp.json` registers it for Claude Code in this repo.

North star (see PLAN.md): a Premiere successor — the bar is a 3-hour
multicam podcast edit. Judge designs against that, not the demo clips.

**Phase 3 (senses & speed) — done.**

- Silence detect/cut + scene detect/split (`audio.rs`, pure ops
  `remove_source_ranges` / `split_at_source_times`; results in SOURCE time).
- `audio_levels` — RMS dBFS per window (`viode levels`, MCP `audio_levels`).
- Proxies (`proxy.rs`, 540p): `viode proxy`, MCP `proxy_build`; frame_grab,
  render_preview, and thumbs automatically prefer proxies once built.
- Waveform PNGs + contact sheets (`visual.rs`): `viode waveform/thumbs`,
  MCP `waveform`/`thumbs` return them as image blocks.
- Export presets (`export.rs`): `viode render --preset youtube|shorts|podcast`
  — two-pass EBU R128 loudnorm (-14 LUFS video, -16 podcast), Shorts is
  1080x1920 center-crop, podcast is audio-only m4a. Master renders via GES,
  presets finish with ffmpeg.

**Phase 4 (the TUI) — done.** `viode tui` (crates/viode-tui, ratatui):
proportional clip lane with playhead, vim-adjacent grammar (h/l/H/L move
playhead, j/k clip edges, s split, i/o trim to playhead, d delete, </>
move, u/U undo/redo, w save, space plays the clip in mpv, P renders +
plays a timeline preview, r renders, ? help, q quit with dirty-confirm).
Playhead selects — verbs act on the clip under it. Proxies are used for
playback/preview when built. State machine (`app.rs`) is fully
unit-tested; rendering (`ui.rs`) smoke-tested via TestBackend.

**Phase 5 (the Premiere fight) — done.**

- Multi-track model: track 0 = gapless main sequence (positions derived,
  optional per-clip `transition` crossfade); overlay tracks position clips
  with `at`; `TrackKind` av/video/audio; `enabled` flag; project-level
  titles. Legacy `[[clip]]` files migrate on load. Edit ops now take
  `&mut Track`; timeline queries take `&Project`.
- GES backend: one layer per enabled track (titles on top), auto
  transitions, `ges::Effect` per clip, `TitleClip` with child props.
  Smart-copy refuses non-cut-only projects.
- Multicam: `sync.rs` audio cross-correlation (10 Hz coarse + 100 Hz
  refine); `viode angle` adds a synced disabled track; `viode take` /
  `ops::replace_range` swaps a timeline range with angle footage.
- Transcripts: `transcript.rs` shells to whisper.cpp (skip-gated),
  `viode transcribe` / `cut-text` = edit video by editing text.
- Performance: `probe_cached` (mtime-keyed), `scripts/bench-longform.sh`
  (5-min footage: ops ~3ms, silence scan 0.8s, proxy ~60x realtime).
- TUI: lanes for every track + title markers, theme-inherited ANSI colors,
  rounded borders; edit grammar still drives the main track.

**TUI graphics (post-Phase-5 polish):** kitty graphics protocol
(`graphics.rs` — detect via TERM/ghostty/KITTY_WINDOW_ID, chunked PNG
transmit, q=2) renders real thumbnails + waveforms in the timeline;
`media.rs` worker thread generates them via ffmpeg (proxy-aware, cached
in cache/tui/ keyed by src+in/out). Text fallback everywhere else;
`VIODE_NO_GRAPHICS=1` forces it. Images re-emit only when placements
change. Colors stay named-ANSI so the TUI inherits the Omarchy theme.

**Phase 6 (daily-driver gap) — done.**

- Inline playback (`viode-tui/preview.rs`): mpv --vo=kitty draws video
  INSIDE the terminal, positioned in the preview pane. space = instant
  cuts-only playback from the playhead via an mpv EDL playlist (zero
  render), pause via IPC socket; x stops; P renders the GES composite
  (tracks/fades/titles/keyframes) and plays it inline.
- Audio control: per-clip `volume` (linear gain) and `pan`
  (audiopanorama), `viode gain/pan`, MCP gain_set/pan_set.
- Keyframes: `[[track.clip.key]]` (prop volume|alpha, at = SOURCE time,
  linear interpolation) bound via gstreamer-controller
  InterpolationControlSource onto the clip's track elements.
  `viode key/keys`, MCP key_add/key_remove. End-to-end test proves a
  keyframed fade-out renders (last audio window 15+ dB below first).
  Waveform lanes + `viode levels` are the metering story.

**Phase 7 (pro-work gap) — done.**

- Transforms: per-clip pos/scale/rotate/opacity (framepositioner child
  props + rotate effect); `viode place`, MCP `place_set`. PiP works.
- Color: `ColorGrade` -> videobalance, `.cube` LUTs via lut3d, scopes
  (ffmpeg waveform/vectorscope) as `viode scope` + MCP `scope` image.
- Trim grammar: `ops::roll/slip/slide` (rate-aware, total-preserving,
  refuse impossible trims); CLI + MCP verbs.
- Speed: `Clip.rate` — `len()` = src_len/rate everywhere, GES videorate+
  pitch time effects; verified 2x renders half duration. `--smooth fps`
  = ffmpeg minterpolate optical flow.
- Transitions: `transition_kind` retypes GES auto-transitions (wipes).
  Titles: xpos/ypos/color styling.
- Exports: `Codec` h264/hevc/av1/prores/dnxhr + bitrate (core::export::
  transcode); shared render queue (core::queue, cache/queue.toml).
- Media: core::media missing/relink (by filename, recursive).
- LIVE composited preview (from Phase 6): `build_timeline` shared with
  render; `preview_play` GES pipeline in a window; `viode play`, TUI
  `v`, MCP `play` (detached). VIODE_PREVIEW_SINK=fake for tests.

**RULE: interface parity.** CLI, TUI, and MCP ship every capability
together — the model edits exactly as a human does.

**Optimization phases (Opt-A/B/C) — done, measured.** Parallel proxies
(1.6x), single-pass cached audio analysis (repeats 0.01s), scene detect
on proxies (20x on 4K), VIODE_HWACCEL opt-in + `viode bench` per-machine
verdicts, segment-overhead measured (~0.12s/cut — smart rendering
shelved with numbers). Details: PLAN.md optimization section.

**Phase 8 G1 (the GUI viewer) — done.** `viode gui` (crates/viode-gui,
eframe/egui 0.32) opens the project in a native window: LIVE composited
GES preview (shared `build_timeline` -> RGBA appsink -> egui texture,
capped at 720p for 4K projects), full timeline display (lanes for every
track, filmstrips + waveforms from the shared artifact cache, title
markers, adaptive ruler, playhead), transport grammar (space, JKL
shuttle to ±8x, arrows, ,/. frame-step, home/end, click/drag scrub, ?
help, q quit). Structured like the TUI: `state.rs` is a pure tested
reducer emitting player commands, `ui.rs` stays dumb, `player.rs` is
headless-testable (the integration test drives preroll/seek/rate/EOS
with no window; VIODE_PREVIEW_SINK=fake swaps the audio sink).
Live-reload: the GUI polls the project mtime like the TUI and rebuilds
the preview in place — a running `viode gui` is a live monitor of an
MCP edit session, per Ed. `MediaCache` moved to
`viode_core::artifacts` (same cache/tui dir) so TUI and GUI share
artifacts; viode-tui/src/media.rs re-exports it. Layout follows the
NLE convention per Ed (Premiere reference): preview dominates, docked
timeline with a prominent accent timecode, V1/A1 track headers, video
overlay lanes above V1 and audio below A1. Colors come from the
Omarchy theme (`theme.rs` parses
~/.local/state/omarchy/current/theme/alacritty.toml, neutral dark
fallback) — never hardcode GUI colors, derive them from that palette.

**Phase 8 G2 (GUI editing parity) — done.** `edit.rs` is the tested
edit reducer (37 unit tests): playhead verbs with TUI semantics
(s/i/o/d, </>, u/U undo/redo, w save, t title, q with dirty-confirm —
also honored by the window close button), inspector setters mirroring
CLI validation and neutral-value normalization (speed, gain, pan,
place, color grade, fades + wipe kinds, keyframes, full title
editing), and a restore-orig drag engine: every mouse motion re-applies
the TOTAL delta to a copy of the drag-start project, so impossible
trims hold the last good state. Mouse grammar: click selects (the
inspector edits the selection), body-drag moves (main reorders by
midpoint crossing, overlays shift `at`), edge-drag trims (overlay
TrimIn keeps the right edge anchored), alt+edge = roll, alt+body =
slip, shift+alt = slide — ops::roll/slip/slide reused, boundary index
= right-hand clip. Scrubbing moved to the ruler. Edits rebuild the
preview pipeline debounced 300ms; slider gestures are ONE undo step
(staged snapshots, ended when the pointer releases). External reloads
never clobber unsaved local edits (TUI contract). MCP grew `ui_open`
(open the GUI, detached, live-reloading — "open the UI" always means
the GUI) and `tui_open` (spawns $TERMINAL/alacritty/ghostty/kitty/foot;
explicit TUI requests only), per Ed's chat-first workflow.

**Phase 8 G3 (the pro surface) — done.** Left panel + dialogs, all
wrapping existing core verbs (no new capabilities — parity intact):
angle list with thumbnails (click = take over the range marked with
`[`/`]`, or the clip under the playhead; Editor::take mirrors CLI
validation), transcript panel (whisper on a worker thread into the
CLI's cache/transcript_N.json; click a sentence to seek, ✕ to cut via
remove_source_ranges with the CLI's 50ms pad), scopes toggle
(waveform + vectorscope via core scope_png — now lifted into
viode_core::visual and shared by CLI/MCP/GUI — overlaid on the paused
preview), render dialog on `r` (master/preset/custom-codec, background
render thread, shared cache/queue.toml add/run/clear), and a
missing-media banner with a relink-by-filename dialog. Fixed en route:
a Phase 5 core bug — ops::replace_range DROPPED clips entirely after
the replaced range on multi-clip timelines (single-clip mains masked
it); now fixed with a proptest (total duration invariant). Also fixed:
GUI freezes ("Application Not Responding") — the GES pipeline is
!Send and was being built on the UI thread; player.rs is now an actor
thread owning the pipeline for its whole life, and eframe runs with
vsync OFF because Wayland compositors withhold frame callbacks from
hidden windows, which deadlocked the buffer swap on timed repaints.
**Phase 8 G4 — macOS: done (2026-08-31, Ed's verdict on his Mac).**
Plan and findings: docs/macos-g4.md. The `gui` verb runs eframe
directly (never through `run_gui`/`gst::macos_main` — winit must own
the Cocoa main loop; `viode play` keeps the wrapper), `tui_open` falls
back to `open -a Terminal` with a generated `.command` script, and
`VIODE_HWACCEL` is per-platform via `viode-core/src/hwaccel.rs`
(VA-API on Linux, VideoToolbox on macOS — one definition shared by
proxies, GES renders, and both bench verbs). Measured on the M5 Max:
software beats VideoToolbox 4.0x for 540p proxying, so the env var
stays unset there. Ed exercised the GUI surface on the Mac and called
it done. Phase 8 is complete on both platforms; packaging remains
deferred by Ed's call and is the only G4 leftover.

**`viode doctor` (2026-09-02) — done.** `viode_core::doctor` probes
every engine piece (GStreamer elements per feature, ffmpeg/ffprobe,
mpv, whisper.cpp) with a user-facing fix string per gap. Parity: CLI
`viode doctor` (exit 1 only when a REQUIRED piece is missing), MCP
`doctor` tool plus a gap summary in the initialize response's
`instructions` field (the model learns this machine's limits before
planning an edit), GUI left-panel banner with an "Engine checkup"
dialog, TUI status line at launch. Found en route: NO stock GStreamer
ships a `lut3d` element — the Phase 7 .cube LUT feature never rendered
anywhere (tests only covered the TOML side).

**LUTs baked via ffmpeg (2026-09-02, Ed's call: "they are good at
their stuff").** `viode_core::lut` bakes the WHOLE source through
ffmpeg's lut3d filter (tetrahedral interpolation, correct range
handling) into cache/luts/, near-lossless x264 crf 10 in .mkv with
audio copied, keyed by source+LUT mtimes; the backend feeds GES the
bake instead of the original. Whole-file on purpose: SOURCE time stays
identical, so keyframes/silence/transcripts stay valid and every trim
reuses one bake — range-baking is the measured optimization if long
sources ever hurt. Doctor's lut3d check now probes ffmpeg's filter
instead of the (nonexistent) GStreamer element. The render proof
Phase 7 lacked exists now: a red clip through a red->blue .cube
renders blue, asserted on output pixels, plus cache-reuse asserted
via mtime.

**Discoverability foundation (stage 1) — done (2026-09-02).**
`viode-gui/src/actions.rs` is THE action table: every argumentless verb
plus panel/dialog doors, each with label, shortcut string, and search
keywords (Premiere vocabulary included — "razor" finds Split). ONE
dispatch point (`GuiApp::perform`) serves the keyboard handler
(refactored to emit Actions), the command palette (ctrl+K / ctrl+P,
`palette.rs`, pure filter+selection logic, unit-tested, teaches
shortcuts inline), right-click context menus (clips, title markers,
ruler — parametric edits point at the inspector), and the small
right-aligned header toolbar (split/delete/undo/redo/save/render/⌘
cmds — deliberately capped). Countdown waves REGISTER NEW VERBS IN
actions.rs and they appear in every surface at once. Not included by
scope: track on/off has no GUI verb yet (no existing capability;
first wave that needs it adds it via the same table).

**The creator wave (stage 2) — done (2026-09-02).** Five verbs, all
four interfaces, doctor checks in-phase per the rules:
- `freeze` — frame hold: ffmpeg materializes one source frame as a
  real clip (media/freeze/, silent audio) inserted at the playhead;
  core::freeze; CLI/MCP/GUI action+menus+palette/TUI `f`.
- `ramp` — stepped time remapping: ops::ramp splits a clip into N
  source-equal segments with linearly interpolated rates (property
  test: source span preserved). Reverse is out (GES has no negative
  rate). CLI/MCP/GUI inspector row.
- `steady` — Clip.steady smoothing; core::steady bakes the whole
  source through vidstabdetect+vidstabtransform into cache/steady
  (mtime-keyed); backend chains steady bake -> LUT bake. Doctor
  checks ffmpeg vidstab. CLI/MCP steady_set/GUI inspector.
- `captions` — core::captions maps transcripts (SOURCE time, cached
  per media file as cache/captions-<stem>.json) through trims, order,
  and rate into timeline captions (unit-tested); delivery = SRT
  sidecar and/or burn as lower-third Titles (existing machinery — no
  libass). CLI/MCP/GUI palette action with worker thread. TUI: none,
  same precedent as transcribe.
- `reframe` — the Auto Reframe answer: scene-detect the master,
  rustface (SeetaFace, pure Rust, portable) finds the subject per
  scene, one ffmpeg sendcmd+crop pass makes the 1080x1920 Short;
  faceless scenes hold the previous framing. Model auto-path
  ~/.local/share/viode/models/seeta_fd_frontal_v1.0.bin (doctor check
  + download command in the error). `render --preset shorts
  --reframe` in CLI/MCP/GUI render dialog. Verified end to end on
  this machine (real 1080x1920 output).

**The settled implementation order (2026-09-02, Ed's ruling).**
Distribution goes LAST — the first version anyone installs must
already be feature-rich for a broad audience. Order: discoverability
foundation (palette, context menus, toolbar over existing verbs) ->
creator wave -> podcast wave -> payment plumbing (checkout, key
issuance, real license policy) -> distribution and launch. Craft and
depth waves follow post-launch. Carve-out, confirmed by Ed: the CI
half of the release spine (GitHub Actions building and testing on
Linux and macOS) is stage 0, the immediate next work — only
user-facing packaging waits. This supersedes the
Phase 9 ordering below; details in the local-only PLAN.md.

**CI (stage 0) — done (2026-09-02).** `.github/workflows/ci.yml`
builds the workspace and runs the full suite with `--locked` on
ubuntu-latest and macos-latest on every push and PR, toolchain pinned
at 1.90.0 to match the gstreamer-rs 0.24 pin. Linux installs the
GStreamer dev stack incl. GES from apt; macOS installs the Homebrew
gstreamer monorepo formula — a green macOS job is also the answer to
the "does brew ship GES" packaging risk. README carries the badge.
First run found two platform truths: Homebrew's GStreamer ships
WITHOUT soundtouch, so the `pitch` element is missing and speed-change
renders fail on every brew Mac (backend now pre-checks and names the
missing plugin; the phase 7 test self-skips that check; the notarized
.app must bundle soundtouch — decide the brew-tap story in the
distribution phase). And the keyframe fade-out threshold is platform-
sensitive (Arch ~18 dB, Ubuntu 24.04 ~12 dB); the test now asserts
the 10 dB invariant instead. Policy going forward: engine capability
varies with how the platform built GStreamer, so viode pre-checks
optional elements and errors actionably, official bundles ship a
complete GStreamer we control, and CI runs on the weakest
environment (brew) so gaps surface before users see them.

**Phase 9 (distribution) — in progress.** Ed's decision (2026-09-01):
users must be able to install on Linux, macOS, and Windows. Windows is
PARKED until Ed starts a VM on Omarchy — do not begin Windows work
before then. The full Linux + macOS plan is in the local-only PLAN.md;
refer to steps by plain names in chat, never invented codes (Ed's
rule). Order: CI + tagged v0.1.0 release, then the AUR package and
Homebrew tap, then .deb/.rpm and the notarized Mac app (Ed has an
Apple Developer account; signing secrets go into GitHub Actions
secrets, never the repo). This supersedes "packaging deferred" above.

**Desktop-app integration — Linux done (2026-09-01).** `viode gui`
with no project shows a welcome screen (recent projects from the XDG
state dir, New Project with native dialogs via rfd/xdg-portal, Open
Project); `viode gui <path>` opens a project file or directory
(file-manager `%f`). Project scaffolding is lifted into
`Project::init` in core — CLI `new`, MCP `project_new`, and the
welcome screen all call it. `welcome.rs` logic is unit-tested; the
welcome->editor swap happens in place (App enum in the GUI lib.rs).
packaging/ holds the SVG icon source, viode.desktop, and the MIME XML;
scripts/gen-icons.sh rasterizes the hicolor set (output gitignored)
and scripts/install-desktop.sh dev-installs everything into
~/.local/share (done on this machine — Viode is in the launcher, and
.viode files open with it). The macOS half (.app bundle, icns,
document types) is planned in the local-only PLAN-MAC.md.

Ed's verdict on the TUI-based showcase: not presentable to a general
audience; the GUI is the shareable face. Showcase footage lesson: the
benchmark media (SD public-domain films) is deliberately ugly for
demos — use ONE Blender Studio film (Spring / Sprite Fright / Charge,
4K CC) for anything meant to be seen, and add a background bar behind
titles before the next showcase render. Packaging still deferred by
Ed's call.

**The countdown to zero (2026-09-02).** PLAN.md now holds the complete
list of Premiere Pro features Viode still lacks — each with its own
plain-verb name (reframe, steady, mend, matte, duck, clean, refit,
ramp, freeze, captions, mask/follow, bundle, mark, mix, publish, plus
grade/key growth, the angle wall, and HDR), grouped into four waves
ordered for market breadth: creator, podcast, craft, depth. Strategy
behind the ordering: macOS next, Windows later, low subscription
price needs a broad market — so implementations must use portable
pieces only (GStreamer, ffmpeg, cross-platform inference), never a
platform-exclusive dependency. Cloud collaboration, AE dynamic link,
and 360/VR are deliberately excluded. If PLAN.md is not on this
machine, ask Ed for it before starting countdown work.

**Doctor rule (2026-09-02, binding).** A verb that adds an engine
dependency (a GStreamer element, an ffmpeg filter, a sidecar binary,
a model file) ships its doctor check in the same phase — exactly like
it ships its palette entry. One entry in `viode_core::doctor::run()`
reaches every front door at once (CLI checkup, MCP initialize, GUI
banner, TUI status line) and gets vetoed by CI on brew, the weakest
environment, before any user hits it.

**GUI discoverability rule (2026-09-02, binding).** Every capability
must be reachable and discoverable by mouse in the GUI through four
surfaces: right-click context menus on the object, a searchable
command palette that shows each verb's key combo, the inspector for
parametric edits, and a deliberately small toolbar. Never a button
per verb. This refines interface parity: GUI parity means
mouse-reachable through those surfaces, not dedicated chrome per
feature. Countdown waves ship palette and context-menu entries with
their verbs in the same phase. Full rationale: PLAN.md.

## Ownership and licensing (settled)

Eduard Moldovan AB owns the software. The repo license is PolyForm
Free Trial 1.0.0 (LICENSE at the repo root, Ed's decision 2026-09-02):
source is visible and anyone may evaluate it for up to 32 consecutive
days; continued use requires a paid subscription — a low monthly price,
sold exclusively through eduardmoldovan.com under Ed's own commercial
agreement. There is no free personal-use tier. Redistribution stays
prohibited. There will be no hosted "Viode Cloud" services — Ed has
ruled them out permanently. Business analysis: MONETIZATION.md.
Outside contributions require a signed grant to the company before
merging.

**License enforcement (2026-09-02).** Official binaries come from the
PRIVATE crate at `../viode-license` — its own repo that, like PLAN.md,
never travels with this one; this repo must never reference it. Its
`viode` binary POSTs `{key, machine, version}` to
`https://eduardmoldovan.com/api/viode/license` at startup and obeys the
answer `{valid, plan, expires_at, message}`. The endpoint currently
always returns `valid: true` — the structure is the contract, the
policy comes later. Only an explicit `valid: false` disables the
software; a network failure fails open so offline users are never
locked out. To make this linkable, `viode-cli` is a library exposing
`viode_cli::run()` plus a thin `main.rs`; public evaluation builds
stay ungated.

## Stack decisions (settled — don't relitigate casually)

- **Rust** + **GStreamer Editing Services (GES)** as the render/preview engine.
- Viode's own TOML timeline model is the source of truth; GES stays behind a
  `RenderBackend` trait so the backend is swappable (MLT / pure-ffmpeg).
- **ffmpeg is the sidecar**, not the engine: ffprobe metadata, proxies,
  thumbnails, waveforms, smart-copy exports, scene detection.

## Commands

```bash
cargo build && cargo test          # viode binary -> target/debug/viode

# Typical session
viode new demo && cd demo
viode add ~/footage/a.mp4          # copies into media/, probes, appends
viode ls
viode split 0 1.5                  # split clip 0 at 1.5s into it
viode trim 1 --in 0.5 --out 2.5    # source in/out points
viode move 1 0
viode render                       # GES, frame-accurate -> renders/demo.mp4
viode render --smart               # ffmpeg stream-copy, keyframe-snapped

# Regenerate test clips (assets/ is gitignored)
./scripts/gen-test-clips.sh
```

Timeline is a gapless sequence (Phase 1): clip order in `project.viode` is
the timeline; positions are derived, never stored. Times accept `1.5`,
`01:30`, or `00:01:30.250`.

## System requirements (Arch)

- `gstreamer` (present), `gst-plugins-{base,good,bad,ugly}`, `gst-libav`
- `gst-editing-services` — required; the `*-sys` crates fail to build without
  its pkg-config file (`gst-editing-services-1.0.pc`)
- `ffmpeg` (present)

## Testing

Coverage is a feature. Every change ships with tests; `cargo test` must stay
green. The suite has three layers, each with a distinct job:

1. **Unit tests** (`#[cfg(test)]` next to the code) — fast regression net
   for ops, time parsing, model helpers.
2. **Property tests** (proptest, in `time.rs` / `ops.rs`) — invariants that
   must hold for ALL inputs: time display/parse round-trips, split preserves
   total duration, no op ever produces `in >= out`. New edit ops MUST add a
   property test for their invariant.
3. **Integration tests** (`crates/*/tests/`) — `project_roundtrip.rs` pins
   the project-file contract (save→load lossless, mixed time forms, error
   messages); `cli.rs` drives the real binary end-to-end and doubles as the
   contributor walkthrough (`full_edit_workflow` is the product demo in test
   form).

Rules: media-dependent tests generate their own tiny clips with ffmpeg and
self-skip (stderr note) when ffmpeg/GES are missing — never commit test
media, never let the suite go red on a minimal machine. Error-path tests
assert on the message text: helpful errors are part of the interface.

Coverage check (optional): `cargo install cargo-llvm-cov && cargo llvm-cov --workspace`.

## House rules (from Ed, binding in every session)

- Phases ship whole. Never commit or report a partial phase.
- CLI, TUI, and MCP stay at feature parity; nothing ships in one only.
- AI-editing demos run through the real MCP connection — never CLI
  stand-ins or hand-rolled protocol drivers.
- Commit messages carry no Co-Authored-By or Generated-with trailers.
- Write complete sentences everywhere, including docs and commit
  bodies — no telegram-style fragment piles.
- Goal framing: beat Premiere. Never conclude that Premiere wins a
  category; frame gaps as a countdown, and measure before optimizing.
- Do not run things Ed did not ask for; when he asks how, answer how.
- Value proposition is capabilities (edit-as-data, automated tedium, AI
  as first-class editor), not "terminal-native"; a proper GUI is planned
  as another thin client.
- Git: committing and pushing to THIS repository is permitted as part of
  the normal workflow. No trailers (see above), and never force-push.
- When work spans machines, the handoff is this repository: pull first,
  write decisions here, push before switching.

## Conventions

- **Document as you go**: README.md is the public face — every user-facing
  change (new verb, new tool, changed behavior) updates README.md in the
  same commit. CLAUDE.md tracks phase status; PLAN.md holds the vision.

- KISS. Cuts before effects, CLI before TUI, TUI before GUI. Phase gates in
  PLAN.md are the scope-creep defense.
- gstreamer-rs crates pinned at 0.24.x — 0.25 needs Rust ≥ 1.92, toolchain
  here is 1.90. Bump both together when upgrading.
- `assets/`, `renders/`, `cache/`, `proxies/` are generated/gitignored — never
  commit media.
- Edit operations in `viode-core` must be pure functions (timeline in →
  timeline out); side effects live at the edges (backends, CLI).
