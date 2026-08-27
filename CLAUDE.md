# Viode

Terminal-native, AI-native video editor for Linux. The timeline is a TOML file,
every operation is a CLI verb, MCP is a first-class client. Full vision,
architecture, and phase plan: see PLAN.md — read it before making design
decisions.

## Current status

**Phase 1 (cuts-only editor) — done.** Cargo workspace:

- `crates/viode-core` — timeline model (`model.rs`), TOML source of truth,
  pure edit ops (`ops.rs`), ffprobe wrapper (`probe.rs`), `RenderBackend`
  trait with `GesBackend` (frame-accurate) and `SmartCopyBackend` (lossless,
  keyframe-snapped) in `backend.rs`, nanosecond `Time` type (`time.rs`).
- `crates/viode-cli` — the `viode` binary: new / import / probe / add / ls /
  trim / split / move / rm / render.

Next: **Phase 2 — MCP server** (`viode serve --mcp`, same verbs plus
frame_grab / render_preview; see PLAN.md).

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

## Conventions

- KISS. Cuts before effects, CLI before TUI, TUI before GUI. Phase gates in
  PLAN.md are the scope-creep defense.
- gstreamer-rs crates pinned at 0.24.x — 0.25 needs Rust ≥ 1.92, toolchain
  here is 1.90. Bump both together when upgrading.
- `assets/`, `renders/`, `cache/`, `proxies/` are generated/gitignored — never
  commit media.
- Edit operations in `viode-core` must be pure functions (timeline in →
  timeline out); side effects live at the edges (backends, CLI).
