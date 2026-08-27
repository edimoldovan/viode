# Viode

Terminal-native, AI-native video editor for Linux. The timeline is a TOML file,
every operation is a CLI verb, MCP is a first-class client. Full vision,
architecture, and phase plan: see PLAN.md — read it before making design
decisions.

## Current status

**Phase 0 (spike).** `src/main.rs` is a throwaway proof: two clips → GES
timeline → rendered MP4. Once validated, restructure into a cargo workspace
(`viode-core`, `viode-cli`, `viode-mcp` — see PLAN.md architecture) and Phase 1
begins. Don't polish the spike.

## Stack decisions (settled — don't relitigate casually)

- **Rust** + **GStreamer Editing Services (GES)** as the render/preview engine.
- Viode's own TOML timeline model is the source of truth; GES stays behind a
  `RenderBackend` trait so the backend is swappable (MLT / pure-ffmpeg).
- **ffmpeg is the sidecar**, not the engine: ffprobe metadata, proxies,
  thumbnails, waveforms, smart-copy exports, scene detection.

## Commands

```bash
# Build & run the spike (concatenates assets/clip1.mp4 + clip2.mp4)
cargo build
cargo run                          # -> renders/spike.mp4
cargo run -- a.mp4 b.mp4 -o out.mp4

# Regenerate test clips (assets/ is gitignored)
./scripts/gen-test-clips.sh

# Inspect a render
ffprobe -v error -show_format -show_streams renders/spike.mp4
```

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
