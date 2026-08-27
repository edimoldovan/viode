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
        .stdout(predicate::str::contains("crossfade: 00:00:00.500"));
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

    if ges_available() {
        // The composited render (overlay re-enabled) comes out the right length.
        viode(&proj).args(["track", "on", "1"]).assert().success();
        viode(&proj).arg("render").assert().success();
        let dur = probe_duration(&proj.join("renders/mt.mp4"));
        assert!((dur - 2.5).abs() < 0.2, "expected ~2.5s, got {dur}s");
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
}
