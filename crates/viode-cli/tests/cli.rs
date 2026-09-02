//! End-to-end tests that drive the real `viode` binary the way a user (or
//! Claude over MCP) would. New contributors: read `full_edit_workflow` top to
//! bottom — it IS the product walkthrough.
//!
//! Media-dependent tests generate tiny clips with ffmpeg and skip themselves
//! (with a note on stderr) when ffmpeg or GES is not installed, so `cargo
//! test` stays green on minimal machines.

use std::path::{Path, PathBuf};
use std::process::Command as Proc;

use assert_cmd::Command;
use predicates::prelude::*;

fn viode(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("viode").unwrap();
    cmd.current_dir(dir);
    cmd
}

fn ffmpeg_available() -> bool {
    Proc::new("ffmpeg").arg("-version").output().is_ok()
}

/// True when a GStreamer element exists on this machine. Used to
/// self-skip checks that need optional plugins (house rule: the suite
/// never goes red on a minimal machine).
fn gst_element_available(name: &str) -> bool {
    Proc::new("gst-inspect-1.0")
        .arg(name)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn ges_available() -> bool {
    Proc::new("pkg-config")
        .args(["--exists", "gst-editing-services-1.0"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Generate a small test clip: `dur` seconds of test pattern + sine tone.
fn make_clip(path: &Path, dur: f64) {
    let status = Proc::new("ffmpeg")
        .args([
            "-y", "-loglevel", "error",
            "-f", "lavfi", "-i", &format!("testsrc2=duration={dur}:size=320x180:rate=30"),
            "-f", "lavfi", "-i", &format!("sine=frequency=440:duration={dur}"),
            "-c:v", "libx264", "-pix_fmt", "yuv420p", "-preset", "ultrafast",
            "-c:a", "aac", "-shortest",
        ])
        .arg(path)
        .status()
        .expect("failed to run ffmpeg");
    assert!(status.success(), "ffmpeg could not create {path:?}");
}

/// Average luma (0-255) of the frame at `at` seconds, via signalstats.
fn frame_mean_luma(path: &Path, at: f64) -> f64 {
    let out = Proc::new("ffmpeg")
        .args([
            "-loglevel", "info", "-ss", &format!("{at}"),
        ])
        .arg("-i")
        .arg(path)
        .args([
            "-frames:v", "1",
            "-vf", "signalstats,metadata=print",
            "-f", "null", "-",
        ])
        .output()
        .unwrap();
    let log = String::from_utf8_lossy(&out.stderr);
    log.lines()
        .find_map(|l| l.split("signalstats.YAVG=").nth(1))
        .expect("no YAVG in ffmpeg output")
        .trim()
        .parse()
        .unwrap()
}

fn probe_duration(path: &Path) -> f64 {
    let out = Proc::new("ffprobe")
        .args(["-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0"])
        .arg(path)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().parse().unwrap()
}

// ---------------------------------------------------------------------------

#[test]
fn new_creates_project_skeleton() {
    let tmp = tempfile::tempdir().unwrap();
    viode(tmp.path())
        .args(["new", "film", "--fps", "24", "--res", "640x360"])
        .assert()
        .success()
        .stdout(predicate::str::contains("created film/ (640x360 @ 24 fps)"));

    let dir = tmp.path().join("film");
    for sub in ["media", "renders", "cache", "proxies"] {
        assert!(dir.join(sub).is_dir(), "missing {sub}/");
    }
    let toml = std::fs::read_to_string(dir.join("project.viode")).unwrap();
    assert!(toml.contains("name = \"film\""));

    // Refuses to clobber an existing directory.
    viode(tmp.path())
        .args(["new", "film"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("already exists"));
}

#[test]
fn commands_outside_a_project_fail_helpfully() {
    let tmp = tempfile::tempdir().unwrap();
    viode(tmp.path())
        .arg("ls")
        .assert()
        .failure()
        .stderr(predicate::str::contains("viode new"));
}

#[test]
fn full_edit_workflow() {
    if !ffmpeg_available() {
        eprintln!("SKIP full_edit_workflow: ffmpeg not installed");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();

    // 1. Create a project and two source clips OUTSIDE it.
    viode(tmp.path()).args(["new", "demo"]).assert().success();
    let proj = tmp.path().join("demo");
    make_clip(&tmp.path().join("a.mp4"), 2.0);
    make_clip(&tmp.path().join("b.mp4"), 1.0);

    // 2. `add` copies external files into media/ and appends to the timeline.
    viode(&proj)
        .args(["add", "../a.mp4"])
        .assert()
        .success()
        .stdout(predicate::str::contains("imported").and(predicate::str::contains("media/a.mp4")));
    viode(&proj).args(["add", "../b.mp4"]).assert().success();

    // 3. Timeline is a gapless sequence: 2s + 1s = 3s total.
    viode(&proj)
        .arg("ls")
        .assert()
        .success()
        .stdout(predicate::str::contains("total 00:00:03.000"));

    // 4. Split clip 0 at 0.5s -> three clips, same total.
    viode(&proj).args(["split", "0", "0.5"]).assert().success();
    viode(&proj)
        .arg("ls")
        .assert()
        .stdout(predicate::str::contains("total 00:00:03.000"));

    // 5. Trim the middle clip's source out-point: 0.5..2.0 -> 0.5..1.0.
    viode(&proj)
        .args(["trim", "1", "--out", "1.0"])
        .assert()
        .success();
    viode(&proj)
        .arg("ls")
        .assert()
        .stdout(predicate::str::contains("total 00:00:02.000"));

    // 6. Move and remove keep the sequence dense.
    viode(&proj).args(["move", "2", "0"]).assert().success();
    viode(&proj).args(["rm", "1"]).assert().success();
    viode(&proj)
        .arg("ls")
        .assert()
        .stdout(predicate::str::contains("total 00:00:01.500"));

    // 7. The project file stays human-readable after all of that.
    let toml = std::fs::read_to_string(proj.join("project.viode")).unwrap();
    assert!(toml.contains("[[track.clip]]"), "unexpected project file:\n{toml}");

    // 8. Error paths speak clearly and exit non-zero.
    viode(&proj)
        .args(["rm", "99"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("out of range"));
    viode(&proj)
        .args(["split", "0", "0"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("split point"));
    viode(&proj)
        .args(["trim", "0", "--in", "5", "--out", "1"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid trim"));
}

#[test]
fn add_rejects_out_beyond_source_duration() {
    if !ffmpeg_available() {
        eprintln!("SKIP add_rejects_out_beyond_source_duration: ffmpeg not installed");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    viode(tmp.path()).args(["new", "p"]).assert().success();
    let proj = tmp.path().join("p");
    make_clip(&tmp.path().join("short.mp4"), 1.0);

    viode(&proj)
        .args(["add", "../short.mp4", "--out", "10"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("beyond source duration"));
}

/// A clip whose audio is: tone 0-1s, silence 1-2s, tone 2-3s.
fn make_clip_with_silence(path: &Path) {
    let status = Proc::new("ffmpeg")
        .args([
            "-y", "-loglevel", "error",
            "-f", "lavfi", "-i", "testsrc2=duration=3:size=320x180:rate=30",
            "-f", "lavfi", "-i",
            "aevalsrc=if(between(t\\,1\\,2)\\,0\\,0.5*sin(440*2*PI*t)):d=3",
            "-c:v", "libx264", "-pix_fmt", "yuv420p", "-preset", "ultrafast",
            "-c:a", "aac", "-shortest",
        ])
        .arg(path)
        .status()
        .unwrap();
    assert!(status.success());
}

/// A clip that hard-cuts from a test pattern to SMPTE bars at 1s.
fn make_clip_with_scene_change(path: &Path) {
    let status = Proc::new("ffmpeg")
        .args([
            "-y", "-loglevel", "error",
            "-f", "lavfi", "-i", "testsrc2=duration=1:size=320x180:rate=30",
            "-f", "lavfi", "-i", "smptebars=duration=1:size=320x180:rate=30",
            "-filter_complex", "[0:v][1:v]concat=n=2:v=1[v]",
            "-map", "[v]",
            "-c:v", "libx264", "-pix_fmt", "yuv420p", "-preset", "ultrafast",
        ])
        .arg(path)
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn silence_detection_and_cutting() {
    if !ffmpeg_available() {
        eprintln!("SKIP silence_detection_and_cutting: ffmpeg not installed");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    viode(tmp.path()).args(["new", "pod"]).assert().success();
    let proj = tmp.path().join("pod");
    make_clip_with_silence(&tmp.path().join("talk.mp4"));
    viode(&proj).args(["add", "../talk.mp4"]).assert().success();

    // The 1s gap in the middle is found...
    viode(&proj)
        .args(["silences", "0", "--min", "0.5"])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 silences"));

    // ...and cut, with padding: ~0.7s removed (1s minus 2 x 0.15 pad),
    // leaving two segments and a shorter timeline.
    viode(&proj)
        .args(["cut-silences", "0", "--min", "0.5"])
        .assert()
        .success()
        .stdout(predicate::str::contains("2 segments kept"));
    viode(&proj)
        .arg("ls")
        .assert()
        .stdout(predicate::str::contains("total 00:00:02.3"));
}

#[test]
fn scene_detection_and_splitting() {
    if !ffmpeg_available() {
        eprintln!("SKIP scene_detection_and_splitting: ffmpeg not installed");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    viode(tmp.path()).args(["new", "footage"]).assert().success();
    let proj = tmp.path().join("footage");
    make_clip_with_scene_change(&tmp.path().join("raw.mp4"));
    viode(&proj).args(["add", "../raw.mp4"]).assert().success();

    // The pattern -> bars hard cut at ~1.0s registers as a scene change.
    viode(&proj)
        .args(["scenes", "0"])
        .assert()
        .success()
        .stdout(predicate::str::contains("00:00:01.0"));

    viode(&proj)
        .args(["split-scenes", "0"])
        .assert()
        .success()
        .stdout(predicate::str::contains("2 segments"));
    viode(&proj)
        .arg("ls")
        .assert()
        .stdout(predicate::str::contains("total 00:00:02.000"));
}

#[test]
fn proxies_waveforms_thumbs_and_levels() {
    if !ffmpeg_available() {
        eprintln!("SKIP proxies_waveforms_thumbs_and_levels: ffmpeg not installed");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    viode(tmp.path()).args(["new", "senses"]).assert().success();
    let proj = tmp.path().join("senses");
    make_clip_with_silence(&tmp.path().join("talk.mp4"));
    viode(&proj).args(["add", "../talk.mp4"]).assert().success();

    // Proxy: built at <=540p under proxies/, named after the media file.
    viode(&proj)
        .arg("proxy")
        .assert()
        .success()
        .stdout(predicate::str::contains("proxies/talk.mp4"));
    let proxy = proj.join("proxies/talk.mp4");
    assert!(proxy.exists());
    // 320x180 source is already under 540p — proxy must not upscale.
    let out = Proc::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v", "-show_entries", "stream=height", "-of", "csv=p=0"])
        .arg(&proxy)
        .output()
        .unwrap();
    let height: u32 = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap();
    assert!(height <= 540, "proxy is {height}p, must be <= 540p");

    // Second run skips existing proxies (no --force).
    viode(&proj).arg("proxy").assert().success();

    // Waveform + contact sheet come out as PNGs in cache/.
    viode(&proj).args(["waveform", "0"]).assert().success();
    viode(&proj).args(["thumbs", "0"]).assert().success();
    for name in ["cache/waveform_0.png", "cache/thumbs_0.png"] {
        let bytes = std::fs::read(proj.join(name)).unwrap();
        assert_eq!(&bytes[..4], b"\x89PNG", "{name} is not a PNG");
    }

    // Levels: the 1-2s silent window reads far quieter than the tone.
    let assert = viode(&proj)
        .args(["levels", "0", "--window", "0.5"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).to_string();
    let db_at = |prefix: &str| -> f64 {
        stdout
            .lines()
            .find(|l| l.starts_with(prefix))
            .unwrap_or_else(|| panic!("no window at {prefix} in:\n{stdout}"))
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap()
    };
    let loud = db_at("00:00:00.500");
    let quiet = db_at("00:00:01.500");
    assert!(
        quiet < loud - 30.0,
        "silence ({quiet} dB) should be much quieter than tone ({loud} dB)"
    );
}

#[test]
fn render_presets_finish_the_master() {
    if !ffmpeg_available() || !ges_available() {
        eprintln!("SKIP render_presets_finish_the_master: ffmpeg/GES not installed");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    viode(tmp.path()).args(["new", "ep", "--res", "320x180"]).assert().success();
    let proj = tmp.path().join("ep");
    make_clip(&tmp.path().join("a.mp4"), 2.0);
    viode(&proj).args(["add", "../a.mp4"]).assert().success();

    // podcast: audio-only m4a.
    viode(&proj)
        .args(["render", "--preset", "podcast"])
        .assert()
        .success()
        .stdout(predicate::str::contains("podcast preset"));
    let m4a = proj.join("renders/ep-podcast.m4a");
    let out = Proc::new("ffprobe")
        .args(["-v", "error", "-show_entries", "stream=codec_type", "-of", "csv=p=0"])
        .arg(&m4a)
        .output()
        .unwrap();
    let streams = String::from_utf8_lossy(&out.stdout);
    assert!(streams.contains("audio") && !streams.contains("video"), "podcast export must be audio-only, got: {streams}");

    // shorts: 1080x1920 vertical.
    viode(&proj)
        .args(["render", "--preset", "shorts"])
        .assert()
        .success();
    let out = Proc::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v", "-show_entries", "stream=width,height", "-of", "csv=p=0"])
        .arg(proj.join("renders/ep-shorts.mp4"))
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "1080,1920");

    // Bad preset name fails helpfully.
    viode(&proj)
        .args(["render", "--preset", "tiktok"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown preset"));
}

/// Noise with a fixed seed is reproducible — two "recordings" of the same
/// event. The angle has 1s of silence first, so it "started 1s early".
fn make_noise_clip(path: &Path, lead_silence: f64, noise_dur: f64) {
    let total = lead_silence + noise_dur;
    let status = Proc::new("ffmpeg")
        .args([
            "-y", "-loglevel", "error",
            "-f", "lavfi", "-i", &format!("anullsrc=r=44100:cl=mono:d={lead_silence}"),
            "-f", "lavfi", "-i",
            &format!("anoisesrc=color=pink:seed=7:d={noise_dur}:r=44100"),
            "-f", "lavfi", "-i", &format!("testsrc2=duration={total}:size=320x180:rate=30"),
            "-filter_complex", "[0][1]concat=n=2:v=0:a=1[a]",
            "-map", "2:v", "-map", "[a]",
            "-c:v", "libx264", "-pix_fmt", "yuv420p", "-preset", "ultrafast",
            "-c:a", "aac", "-shortest",
        ])
        .arg(path)
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn multitrack_titles_fades_and_effects() {
    if !ffmpeg_available() {
        eprintln!("SKIP multitrack_titles_fades_and_effects: ffmpeg not installed");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    viode(tmp.path()).args(["new", "mt", "--res", "320x180"]).assert().success();
    let proj = tmp.path().join("mt");
    make_clip(&tmp.path().join("a.mp4"), 2.0);
    make_clip(&tmp.path().join("b.mp4"), 1.0);

    // Main sequence with a crossfade.
    viode(&proj).args(["add", "../a.mp4"]).assert().success();
    viode(&proj).args(["add", "../b.mp4"]).assert().success();
    viode(&proj)
        .args(["fade", "1", "0.5"])
        .assert()
        .success()
        .stdout(predicate::str::contains("transition: 00:00:00.500"));
    // 2 + 1 - 0.5 overlap = 2.5s.
    viode(&proj)
        .arg("ls")
        .assert()
        .stdout(predicate::str::contains("total 00:00:02.500"));

    // Overlay track with positioned B-roll; effects; a title.
    viode(&proj)
        .args(["track", "add", "broll", "--kind", "video"])
        .assert()
        .success();
    viode(&proj)
        .args(["add", "../b.mp4", "--track", "1", "--at", "0.5"])
        .assert()
        .success();
    viode(&proj)
        .args(["add", "../b.mp4", "--track", "1"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--at"));
    viode(&proj)
        .args(["fx", "0", "videobalance saturation=0.0"])
        .assert()
        .success();
    viode(&proj)
        .args(["title", "Chapter One", "--at", "0.2", "--dur", "1.0"])
        .assert()
        .success();

    // The file records all of it, in the documented format.
    let toml = std::fs::read_to_string(proj.join("project.viode")).unwrap();
    for needle in [
        "[[track]]",
        "transition = \"00:00:00.500\"",
        "kind = \"video\"",
        "at = \"00:00:00.500\"",
        "videobalance saturation=0.0",
        "[[title]]",
        "Chapter One",
    ] {
        assert!(toml.contains(needle), "missing {needle} in:\n{toml}");
    }

    // Smart-copy refuses compositing projects instead of producing garbage.
    viode(&proj)
        .args(["render", "--smart"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("single-track"));

    // Disabled tracks are honored (and the main track can't be disabled).
    viode(&proj).args(["track", "off", "1"]).assert().success();
    viode(&proj)
        .args(["track", "off", "0"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("main track"));

    // Overlapping titles must render (stacked end cards) — regression for
    // the one-layer-per-title fix.
    viode(&proj)
        .args(["title", "Second line", "--at", "0.4", "--dur", "1.2", "--y", "0.6"])
        .assert()
        .success();

    if ges_available() {
        // The composited render (overlay re-enabled) comes out the right length.
        viode(&proj).args(["track", "on", "1"]).assert().success();
        viode(&proj).arg("render").assert().success();
        let dur = probe_duration(&proj.join("renders/mt.mp4"));
        assert!((dur - 2.5).abs() < 0.2, "expected ~2.5s, got {dur}s");
        // The video must show through active titles — GES defaults the
        // title background to opaque white, which once blanked the frame.
        let luma = frame_mean_luma(&proj.join("renders/mt.mp4"), 0.5);
        assert!(
            luma < 200.0,
            "frame under title is near-white (YAVG {luma}) — opaque title background?"
        );
    } else {
        eprintln!("SKIP multitrack render check: GES not installed");
    }
}

#[test]
fn multicam_sync_and_take() {
    if !ffmpeg_available() {
        eprintln!("SKIP multicam_sync_and_take: ffmpeg not installed");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    // Reference: noise from t=0. Angle: same noise, but its recorder ran 1s
    // of silence first — it started 1 second "early".
    make_noise_clip(&tmp.path().join("ref.mp4"), 0.0, 3.0);
    make_noise_clip(&tmp.path().join("cam2.mp4"), 1.0, 2.0);

    // Standalone offset detection.
    let assert = viode(tmp.path())
        .args(["sync", "ref.mp4", "cam2.mp4", "--max-lag", "5"])
        .assert()
        .success();
    let out = String::from_utf8_lossy(&assert.get_output().stdout).to_string();
    let offset: f64 = out.split('s').next().unwrap().trim().parse().unwrap();
    assert!(
        (offset - (-1.0)).abs() < 0.15,
        "expected ~-1.0s offset, got {offset} ({out})"
    );

    // Full multicam flow: angle lands synced and disabled; take swaps it in.
    viode(tmp.path()).args(["new", "mc"]).assert().success();
    let proj = tmp.path().join("mc");
    viode(&proj).args(["add", "../ref.mp4"]).assert().success();
    viode(&proj)
        .args(["angle", "../cam2.mp4"])
        .assert()
        .success()
        .stdout(predicate::str::contains("synced"));

    let toml = std::fs::read_to_string(proj.join("project.viode")).unwrap();
    assert!(toml.contains("enabled = false"), "angle starts disabled:\n{toml}");
    // Started early -> aligned by skipping its head (~1s in-point).
    assert!(toml.contains("in = \"00:00:0"), "angle has an in-point:\n{toml}");

    viode(&proj).args(["take", "1", "0.5", "1.5"]).assert().success();
    let assert = viode(&proj).arg("ls").assert().success();
    let ls = String::from_utf8_lossy(&assert.get_output().stdout).to_string();
    assert!(ls.contains("cam2.mp4"), "take put angle footage on main:\n{ls}");
    assert!(ls.contains("total 00:00:03.000"), "takes keep duration:\n{ls}");
}

#[test]
fn audio_gain_pan_and_keyframes() {
    if !ffmpeg_available() {
        eprintln!("SKIP audio_gain_pan_and_keyframes: ffmpeg not installed");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    viode(tmp.path()).args(["new", "aud", "--res", "320x180"]).assert().success();
    let proj = tmp.path().join("aud");
    make_clip(&tmp.path().join("tone.mp4"), 2.0);
    viode(&proj).args(["add", "../tone.mp4"]).assert().success();

    // Gain, pan, and a fade-out (volume 1 -> 0 over the clip) land in TOML.
    viode(&proj).args(["gain", "0", "0.8"]).assert().success();
    viode(&proj).args(["pan", "0", "-0.5"]).assert().success();
    viode(&proj)
        .args(["key", "0", "volume", "0", "1.0"])
        .assert()
        .success();
    viode(&proj)
        .args(["key", "0", "volume", "2.0", "0.0"])
        .assert()
        .success();
    let toml = std::fs::read_to_string(proj.join("project.viode")).unwrap();
    for needle in ["volume = 0.8", "pan = -0.5", "[[track.clip.key]]", "prop = \"volume\""] {
        assert!(toml.contains(needle), "missing {needle} in:\n{toml}");
    }

    // Validation speaks up.
    viode(&proj)
        .args(["key", "0", "sparkle", "1", "1"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("volume, alpha"));
    viode(&proj)
        .args(["pan", "0", "3"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("out of range"));

    if !ges_available() {
        eprintln!("SKIP audio keyframe render check: GES not installed");
        return;
    }
    // The proof: rendered audio actually fades out. The tone starts loud
    // and the volume keyframes take it to zero, so the last window must be
    // drastically quieter than the first.
    viode(&proj).arg("render").assert().success();
    let levels_out = Proc::new("ffprobe") // ensure file exists before analysis
        .arg(proj.join("renders/aud.mp4"))
        .output()
        .unwrap();
    assert!(levels_out.status.success());
    let levels = viode_core_levels(&proj.join("renders/aud.mp4"));
    let (first, last) = (levels.first().unwrap().1, levels.last().unwrap().1);
    // 10 dB is the invariant, not a tuning: a fade to zero must land the
    // final window at a fraction of the first. The exact figure varies by
    // GStreamer version (Arch ~18 dB, Ubuntu 24.04 ~12 dB) because the
    // last analysis window catches a different slice of the fade tail.
    assert!(
        last < first - 10.0,
        "fade-out did not render: first window {first} dB, last {last} dB"
    );
}

/// RMS levels via the ffmpeg astats sidecar (same math as `viode levels`).
fn viode_core_levels(path: &Path) -> Vec<(String, f64)> {
    let out = Proc::new(assert_cmd::cargo::cargo_bin("viode"))
        .args(["probe"])
        .arg(path)
        .output()
        .unwrap();
    assert!(out.status.success());
    // Use the library through the CLI `levels` verb on a throwaway project
    // is overkill here — shell out to ffmpeg astats directly.
    let out = Proc::new("ffmpeg")
        .args(["-hide_banner", "-nostats", "-i"])
        .arg(path)
        .args([
            "-af",
            "aresample=8000,asetnsamples=n=4000,astats=metadata=1:reset=1,\
             ametadata=print:key=lavfi.astats.Overall.RMS_level:file=-",
            "-f", "null", "-",
        ])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut at = String::new();
    let mut levels = Vec::new();
    for line in stdout.lines() {
        if let Some(rest) = line.split("pts_time:").nth(1) {
            at = rest.split_whitespace().next().unwrap_or("").to_string();
        } else if let Some(v) = line.strip_prefix("lavfi.astats.Overall.RMS_level=") {
            levels.push((at.clone(), v.trim().parse::<f64>().unwrap_or(-100.0).max(-100.0)));
        }
    }
    levels
}

#[test]
fn phase7_pro_editing_tools() {
    if !ffmpeg_available() {
        eprintln!("SKIP phase7_pro_editing_tools: ffmpeg not installed");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    viode(tmp.path()).args(["new", "pro", "--res", "320x180"]).assert().success();
    let proj = tmp.path().join("pro");
    make_clip(&tmp.path().join("a.mp4"), 2.0);
    make_clip(&tmp.path().join("b.mp4"), 2.0);
    viode(&proj).args(["add", "../a.mp4"]).assert().success();
    viode(&proj).args(["add", "../b.mp4"]).assert().success();

    // Transforms, color, speed, typed transition land in the file.
    viode(&proj)
        .args(["place", "0", "--x", "0.7", "--y", "0.05", "--scale", "0.25", "--opacity", "0.9"])
        .assert()
        .success();
    viode(&proj)
        .args(["color", "0", "--saturation", "0.0", "--contrast", "1.2"])
        .assert()
        .success();
    viode(&proj).args(["speed", "1", "2.0"]).assert().success();
    viode(&proj)
        .args(["fade", "1", "0.5", "--kind", "bar-wipe-lr"])
        .assert()
        .success();
    let toml = std::fs::read_to_string(proj.join("project.viode")).unwrap();
    for needle in [
        "scale = 0.25", "opacity = 0.9", "saturation = 0.0", "contrast = 1.2",
        "rate = 2.0", "transition_kind = \"bar-wipe-lr\"",
    ] {
        assert!(toml.contains(needle), "missing {needle} in:\n{toml}");
    }
    // rate 2 halves clip 1 on the timeline: 2 + 1 - 0.5 fade = 2.5s.
    viode(&proj)
        .arg("ls")
        .assert()
        .stdout(predicate::str::contains("total 00:00:02.500"));

    // Trim grammar: give the boundary room first (interior in/out points,
    // like real footage), then roll/slip must keep the total constant.
    viode(&proj).args(["trim", "0", "--in", "0.2", "--out", "1.8"]).assert().success();
    viode(&proj).args(["trim", "1", "--in", "0.5", "--out", "1.9"]).assert().success();
    // 1.6 + (1.4 source / 2x) - 0.5 fade = 1.8s.
    viode(&proj)
        .arg("ls")
        .assert()
        .stdout(predicate::str::contains("total 00:00:01.800"));
    viode(&proj).args(["roll", "1", "-0.25"]).assert().success();
    viode(&proj).args(["slip", "0", "0.1"]).assert().success();
    viode(&proj)
        .arg("ls")
        .assert()
        .stdout(predicate::str::contains("total 00:00:01.800"));
    // Rolling past the source start is refused, not silently clamped.
    viode(&proj)
        .args(["roll", "1", "-5"])
        .assert()
        .failure();

    // Scopes are real PNGs.
    viode(&proj).args(["scope", "0", "--kind", "waveform"]).assert().success();
    viode(&proj).args(["scope", "0", "--kind", "vector"]).assert().success();
    let bytes = std::fs::read(proj.join("cache/scope_0.png")).unwrap();
    assert_eq!(&bytes[..4], b"\x89PNG");

    // Media management: break a source, find it, relink it.
    let hideout = tmp.path().join("moved");
    std::fs::create_dir_all(&hideout).unwrap();
    std::fs::rename(proj.join("media/b.mp4"), hideout.join("b.mp4")).unwrap();
    viode(&proj)
        .args(["media", "missing"])
        .assert()
        .success()
        .stdout(predicate::str::contains("b.mp4"));
    viode(&proj)
        .args(["relink", "../moved"])
        .assert()
        .success()
        .stdout(predicate::str::contains("relinked 1"));
    viode(&proj)
        .args(["media", "missing"])
        .assert()
        .stdout(predicate::str::contains("all media present"));

    if !ges_available() {
        eprintln!("SKIP phase7 render checks: GES not installed");
        return;
    }
    // Speed renders shorter: solo the 2x clip -> ~1s output. The audio
    // half of a speed change needs the soundtouch `pitch` element, which
    // some GStreamer builds (Homebrew's, notably) do not ship — keep the
    // clip at 1x there so the rest of the phase still verifies.
    let solo = tmp.path().join("solo");
    viode(tmp.path()).args(["new", "solo", "--res", "320x180"]).assert().success();
    viode(&solo).args(["add", "../a.mp4"]).assert().success();
    if gst_element_available("pitch") {
        viode(&solo).args(["speed", "0", "2.0"]).assert().success();
        viode(&solo).arg("render").assert().success();
        let dur = probe_duration(&solo.join("renders/solo.mp4"));
        assert!((dur - 1.0).abs() < 0.25, "2x of 2s should render ~1s, got {dur}");
    } else {
        eprintln!("SKIP speed render check: GStreamer 'pitch' element not installed");
        viode(&solo).arg("render").assert().success();
    }

    // Codec breadth: ProRes comes out as ProRes in a .mov.
    viode(&solo)
        .args(["render", "--codec", "prores"])
        .assert()
        .success();
    let out = Proc::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v", "-show_entries", "stream=codec_name", "-of", "csv=p=0"])
        .arg(solo.join("renders/solo-prores.mov"))
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "prores");

    // The render queue runs jobs in order and empties itself.
    viode(&solo).args(["queue", "add", "--codec", "h264"]).assert().success();
    viode(&solo)
        .args(["queue", "run"])
        .assert()
        .success()
        .stdout(predicate::str::contains("queue complete"));
    assert!(solo.join("renders/solo-h264.mp4").exists());

    // Live composited preview: full pipeline, fake sinks, must exit clean.
    viode(&solo)
        .env("VIODE_PREVIEW_SINK", "fake")
        .args(["play"])
        .assert()
        .success();
}

#[test]
fn bench_prints_a_verdict() {
    if !ffmpeg_available() {
        eprintln!("SKIP bench_prints_a_verdict: ffmpeg not installed");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    make_clip(&tmp.path().join("a.mp4"), 2.0);
    viode(tmp.path())
        .args(["bench", "a.mp4", "--secs", "2"])
        .assert()
        .success()
        .stdout(predicate::str::contains("verdict:"));
}

#[test]
fn render_produces_frame_accurate_output() {
    if !ffmpeg_available() || !ges_available() {
        eprintln!("SKIP render_produces_frame_accurate_output: ffmpeg/GES not installed");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    viode(tmp.path()).args(["new", "r", "--res", "320x180"]).assert().success();
    let proj = tmp.path().join("r");
    make_clip(&tmp.path().join("a.mp4"), 2.0);

    viode(&proj).args(["add", "../a.mp4"]).assert().success();
    viode(&proj).args(["split", "0", "1.0"]).assert().success();
    viode(&proj).args(["rm", "1"]).assert().success();

    // Timeline is now exactly 1.0s of the 2.0s source.
    viode(&proj)
        .arg("render")
        .assert()
        .success()
        .stdout(predicate::str::contains("rendered"));

    let out: PathBuf = proj.join("renders").join("r.mp4");
    assert!(out.exists());
    let dur = probe_duration(&out);
    assert!(
        (dur - 1.0).abs() < 0.15,
        "expected ~1.0s render, got {dur}s"
    );
    // Renders must honor the PROJECT resolution, not GES's 720p default
    // (real-media testing caught this) — 320x180 here.
    let wh = Proc::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v", "-show_entries", "stream=width,height", "-of", "csv=p=0"])
        .arg(&out)
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&wh.stdout).trim(), "320,180");
}

#[test]
fn doctor_reports_the_machine_and_fails_without_core_deps() {
    // On a working dev machine the required trio exists: doctor succeeds
    // and lists every check by its probe name.
    Command::cargo_bin("viode")
        .unwrap()
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains("engine checkup"))
        .stdout(predicate::str::contains("ffmpeg"))
        .stdout(predicate::str::contains("pitch"));

    // With an empty PATH the sidecar binaries vanish; the missing pieces
    // are required, so doctor must fail loudly. Helpful errors are part
    // of the interface.
    Command::cargo_bin("viode")
        .unwrap()
        .arg("doctor")
        .env("PATH", "")
        .assert()
        .failure()
        .stdout(predicate::str::contains("MISS"))
        .stderr(predicate::str::contains("core dependencies are missing"));
}
