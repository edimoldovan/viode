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
  trim / split / move / rm / render / serve.
- `crates/viode-mcp` — **Phase 2, done**: hand-rolled MCP stdio server
  (newline-delimited JSON-RPC — the whole protocol layer is `lib.rs`, no
  framework). Tools mirror the CLI plus the "senses": `frame_grab` (returns
  a PNG image block), `render_preview` (GES render of a sub-range via
  `ops::extract_range`). Every edit tool returns the fresh timeline JSON.
  `.mcp.json` registers it for Claude Code in this repo.

North star (see PLAN.md): a Premiere successor — the bar is a 3-hour
multicam podcast edit. Judge designs against that, not the demo clips.

**Phase 3 (senses & speed) — in progress.** Done: silence detection +
auto-cutting (`viode silences` / `cut-silences`, MCP `silence_detect` /
`silence_cut`) and scene detection + splitting (`viode scenes` /
`split-scenes`, MCP `scene_detect` / `scene_split`) — analysis in
`viode-core/src/audio.rs` (ffmpeg silencedetect/scene-score, results in
SOURCE time), applied by pure ops `remove_source_ranges` (with padding,
overlap-merge) and `split_at_source_times`.

Still to do in Phase 3: proxies, thumbnails, waveforms, loudness-normalized
export presets (YouTube/Shorts/podcast).

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

## Conventions

- KISS. Cuts before effects, CLI before TUI, TUI before GUI. Phase gates in
  PLAN.md are the scope-creep defense.
- gstreamer-rs crates pinned at 0.24.x — 0.25 needs Rust ≥ 1.92, toolchain
  here is 1.90. Bump both together when upgrading.
- `assets/`, `renders/`, `cache/`, `proxies/` are generated/gitignored — never
  commit media.
- Edit operations in `viode-core` must be pure functions (timeline in →
  timeline out); side effects live at the edges (backends, CLI).
