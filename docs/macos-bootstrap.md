# macOS bootstrap — instructions for the AI

You are a Claude session on macOS inside the Viode repository. Your job
is to get Viode fully working on this machine and report what broke.
The user watches; you type. Viode has never run on macOS before this —
every failure you hit is valuable, so record each one and its fix in
this file when you are done.

Do these in order:

1. Verify the toolchain: `brew --version`, `cargo --version`. Install
   what is missing (`brew install rustup` then `rustup-init -y` for
   Rust). Then `brew install gstreamer ffmpeg mpv` — Homebrew's
   gstreamer formula is monolithic and includes GES and the plugin sets.
2. Build: `cargo build --release`. If gstreamer-sys fails to find
   pkg-config files, `brew info gstreamer` shows the pkgconfig path;
   export PKG_CONFIG_PATH accordingly and retry.
3. Test: `cargo test`. Media-dependent tests generate their own clips
   and self-skip if tools are missing — the suite should be green or
   loudly explain why not.
4. Fix `.mcp.json`: it contains a Linux-machine absolute path. Point it
   at this checkout's `target/release/viode` (absolute path). Do not
   commit that change.
5. Smoke-test the engine end to end, in a throwaway directory:
   create a project, generate two test clips with ffmpeg (copy the
   commands from scripts/gen-test-clips.sh), add them, split one, render,
   and ffprobe the output. Then `viode render --preset podcast` for the
   ffmpeg post path, and `VIODE_PREVIEW_SINK=fake viode play` for the
   preview pipeline.
6. Report: a short table of what worked, what needed a fix, and what is
   genuinely broken on macOS. Append it to this file under "First run
   results", commit that (message: "docs: macOS first-run results"), and
   push.

Known expectations: VIODE_HWACCEL=vaapi is Linux-only (skip it; the
VideoToolbox equivalent is future work). The TUI's images need kitty or
ghostty. Everything else should work — prove or disprove that.

## First run results (2026-08-29)

Machine: Apple M5 Max, 128 GB unified memory, macOS 26 (Darwin 25.5),
Homebrew 6.0.19. Outcome: **fully working** — build, all 69 tests, GES
render, podcast preset, and preview pipeline all pass. Two fixes were
needed, both recorded below.

| Step | Outcome |
|---|---|
| Toolchain | brew + ffmpeg were present. Rust was a stale 2024 standalone rustup install (stable 1.80, no `rustup` on PATH; not asdf-managed). `brew install rustup` works but is **keg-only and no longer ships `rustup-init`** — step 1 above is stale. Used `/opt/homebrew/opt/rustup/bin/rustup` directly; the deprecated `rls` component blocked the channel update (`rustup component remove --toolchain stable rls`), then `rustup update stable` → 1.98.0. |
| gstreamer / mpv | `brew install gstreamer mpv` — monolithic gstreamer 1.28.6 includes GES. pkg-config finds everything with no PKG_CONFIG_PATH needed (the doc's step 2 caveat never triggered). |
| Build | `cargo build --release` — clean first try, 28s. |
| Tests | 68/69 green out of the box. `phase7_pro_editing_tools` failed: Homebrew's gstreamer bundle **omits the soundtouch plugin**, so the `pitch tempo=…` audio time effect for `Clip.rate` can't be created (`GES-CRITICAL: ges_effect_new: assertion 'asset' failed`). |
| The fix | `brew install sound-touch meson ninja`, then built just that plugin from the matching source: `gst-plugins-bad-1.28.6` tarball, `meson setup build -Dauto_features=disabled -Dsoundtouch=enabled`, `ninja -C build ext/soundtouch/libgstsoundtouch.dylib` (~1 min total), copied the dylib to `~/.local/share/gstreamer-1.0/plugins/` — scanned by default, no env var, survives `brew upgrade`. Full suite green after. |
| .mcp.json | Repointed at this checkout's `target/release/viode` (left uncommitted, per the instructions). |
| Smoke test | new → add ×2 → split → render: 6.0s timeline rendered in 0.9s, ffprobe confirms h264+aac 1920×1080. `viode render --preset podcast` → correct 6.0s m4a. `VIODE_PREVIEW_SINK=fake viode play` runs the preview pipeline to completion. |

Genuinely broken on macOS: nothing found beyond the soundtouch gap.

Cosmetic / untested:

- GStreamer-GL warns `An NSApplication needs to be running on the main
  thread` on every render. Harmless for offscreen work, but a real
  windowed preview sink may need `gst_macos_main()` — untested here
  (fake sink only). Worth checking when someone runs `viode play` with
  a window on a Mac.
- `VIODE_HWACCEL=vaapi` skipped as documented (Linux-only; VideoToolbox
  is future work).
- TUI graphics untested in this session (needs kitty/ghostty).
