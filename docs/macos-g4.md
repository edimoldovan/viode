# G4 — macOS

The goal: `viode gui` is daily-drivable on the Mac. Linux stays the
reference platform; macOS fixes land behind `cfg(target_os = "macos")`
only when there is no portable way. Ed runs this on his own machine
(M5 Max, macOS 26, Homebrew GStreamer, Rust 1.98) and drives the fixes
from what actually breaks.

## Baseline (already proven, 2026-08-29)

The CLI, the full test suite, GES renders, presets, and the preview
pipeline all worked on the first try — see
[macos-bootstrap.md](macos-bootstrap.md). The one gap was Homebrew's
missing soundtouch plugin, fixed by building `libgstsoundtouch` from
source. The GUI did not exist yet; G4 is about the GUI.

## Order of work

1. **Build and launch `viode gui`.** The likely first fight: on macOS
   the CLI wraps commands in `run_gui` (`gst::macos_main`), which wants
   to own the Cocoa main loop — and eframe/winit also wants to own it.
   If the window doesn't appear or input dies, unwrap the `gui` verb
   from `run_gui` on macOS and let eframe run the NSApplication itself;
   the GES preview stays safe because the player actor never opens a
   window.
2. **Exercise the whole surface.** Playback, JKL, scrubbing, edits,
   inspector, angle takes, transcript panel (whisper via brew), scopes,
   render dialog, queue, relink. Note every break in this file and fix
   it on the spot; anything with logic gets a test.
3. **Live-reload from MCP.** `ui_open` spawns the GUI and edits appear
   live, same as Linux. Known gap to fix here: `tui_open` looks for
   Linux terminal emulators — teach it `open -a Terminal`.
4. **Theme.** There is no Omarchy palette on macOS, so the GUI uses its
   neutral dark fallback automatically; decide whether to read the
   system light/dark preference or leave it.
5. **Hardware acceleration.** Run `viode bench` and try VideoToolbox
   (`VIODE_HWACCEL`) — same rule as VA-API on Linux: measured win or it
   stays off.
6. **Packaging, last.** Only once daily use is stable: .app bundle with
   relocated GStreamer, codesign + notarization, and a CI build. Not
   before — packaging a moving target is wasted work.

## Findings (2026-08-31, on the M5 Max)

Step 1 — launch. The predicted fight happened exactly as written:
`run_gui` wraps the closure in `gst::macos_main`, which runs it OFF the
main thread, and winit aborts ("EventLoop must be created on the main
thread"). Fix: the `gui` verb no longer goes through `run_gui` — eframe
owns the NSApplication itself, which also satisfies GStreamer-GL (no
main-thread warning appears in the GUI's log anymore). `viode play`
keeps its `run_gui` wrapper: the GStreamer preview window has no winit
and still needs the Cocoa loop. The GUI launches, runs its event loop
at idle CPU, and live-reloads a CLI edit made while it was open.

Step 3 — `tui_open` on macOS. Fixed as planned: when no CLI terminal
emulator answers, the server writes an executable `.command` script to
the temp dir and runs `open -a Terminal` on it — verified end to end
on this machine (Terminal.app opens and runs the TUI). Paths are
single-quoted through a tested `sh_quote` helper. `$TERMINAL` and
ghostty/kitty et al. still win when present.

Step 4 — theme. Decision: keep the neutral dark fallback and do not
read the system light/dark preference. Every serious NLE is dark
regardless of OS theme, and one fewer platform branch is one fewer
thing to break; revisit only if daily use argues otherwise.

Step 5 — hardware acceleration. `VIODE_HWACCEL` is now per-platform,
defined once in `viode-core/src/hwaccel.rs` and shared by proxies, the
GES render, and both bench verbs: VA-API on Linux, VideoToolbox on
macOS (`-hwaccel videotoolbox` + `scale_vt` + `h264_videotoolbox` for
ffmpeg, `vtenc_h264` for GES — confirmed instantiated via
GST_DEBUG). The measured verdict on this machine, 30s of 4K60
Big Buck Bunny to a 540p proxy: software 1.8s, VideoToolbox 7.3s —
**software wins 4.0x, leave VIODE_HWACCEL unset on the M5 Max**. Same
rule as Linux: the hardware path exists, is one export away, and stays
off without a local win. (For GES renders the point is moot anyway:
Homebrew's vtenc_h264 ranks primary, so encodebin picks VideoToolbox
by default there.)

Step 2 — exercising the whole surface (playback, JKL, scrubbing, edits,
inspector, angles, transcript, scopes, render dialog, relink) is Ed's
part, with eyes on the window; nothing else can judge whether video
actually shows. Step 6 (packaging) stays untouched until daily use is
stable, per the order of work.

## House rules that apply here

Every fix ships with a test or a bootstrap-doc entry, `cargo test`
stays green on both platforms, and nothing lands that makes Linux
worse.

## Follow-up (2026-09-02): the window froze on macOS 26.6

After the macOS 26.6 update the editor window stopped updating a few
seconds after launch. Diagnosis on the M5 Max: the GES preview
delivered picture frames, egui repainted at about 90 updates per
second, the texture carried the frames, and playback position
advanced — yet the on-screen window kept an early frame (black
preview, timecode at zero). The August 31 baseline binary froze the
same way, which cleared Viode's own changes; eframe's glow (CGL)
backend logged no error, it simply stopped presenting. Switching the
renderer to wgpu (Metal) fixed it outright: picture, moving timecode,
70 seconds of playback verified. The renderer is chosen per platform
in `viode-gui/src/lib.rs`; Linux keeps glow.

