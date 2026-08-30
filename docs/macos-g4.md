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

## House rules that apply here

Every fix ships with a test or a bootstrap-doc entry, `cargo test`
stays green on both platforms, and nothing lands that makes Linux
worse.
