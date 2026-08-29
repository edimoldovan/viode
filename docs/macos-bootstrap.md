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
