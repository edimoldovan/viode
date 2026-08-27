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
    assert!(toml.contains("[[clip]]"), "unexpected project file:\n{toml}");

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
