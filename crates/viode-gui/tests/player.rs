//! Headless integration test for the GUI preview player: the appsink
//! design means frames arrive with no window and no display, which is
//! exactly what makes the pipeline testable. Media-dependent, so the test
//! generates its own tiny clip with ffmpeg and self-skips (stderr note)
//! when ffmpeg or GES is missing.

use std::path::Path;
use std::process::Command as Proc;
use std::time::{Duration, Instant};

use viode_core::{Clip, Project, Time};
use viode_gui::player::Player;

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

fn wait_for(mut cond: impl FnMut() -> bool, timeout: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

#[test]
fn player_prerolls_seeks_and_plays_headless() {
    if !ffmpeg_available() {
        eprintln!("SKIP player_prerolls_seeks_and_plays_headless: ffmpeg not installed");
        return;
    }
    if !ges_available() {
        eprintln!("SKIP player_prerolls_seeks_and_plays_headless: GES not installed");
        return;
    }
    // No audio device in CI: the fake audio sink keeps the pipeline alive.
    std::env::set_var("VIODE_PREVIEW_SINK", "fake");

    let dir = tempfile::tempdir().unwrap();
    let media = dir.path().join("media");
    std::fs::create_dir_all(&media).unwrap();
    make_clip(&media.join("a.mp4"), 2.0);

    let mut project = Project::new("gui-test", 30.0, [320, 180]);
    project.main_mut().clips.push(Clip::media(
        "media/a.mp4".into(),
        Time::ZERO,
        Time(2_000_000_000),
    ));

    // The handle returns instantly; the actor thread builds and prerolls.
    let player = Player::spawn(|| {});
    player.load(&project, dir.path());

    // Preroll delivers the first frame while still paused.
    assert!(
        wait_for(|| player.frame_seq() > 0, Duration::from_secs(10)),
        "no preroll frame arrived"
    );
    let (w, h) = player
        .with_frame(|f| (f.width, f.height))
        .expect("a frame is stored");
    assert_eq!((w, h), (320, 180), "preview keeps small sources native");
    player
        .with_frame(|f| assert_eq!(f.rgba.len(), f.width * f.height * 4))
        .unwrap();

    // A paused accurate seek prerolls a fresh frame — that is scrubbing.
    let seq_before = player.frame_seq();
    player.seek(Time(1_000_000_000));
    assert!(
        wait_for(|| player.frame_seq() > seq_before, Duration::from_secs(10)),
        "seek did not deliver a frame"
    );

    // Play: the clock runs and the position advances past the seek point.
    player.play();
    assert!(
        wait_for(
            || player.position().is_some_and(|p| p > Time(1_100_000_000)),
            Duration::from_secs(10)
        ),
        "position did not advance during playback"
    );
    player.pause();

    // Rate change through the shuttle path keeps the pipeline healthy.
    player.set_rate(2.0);
    player.play();
    assert!(
        wait_for(|| player.frame_seq() > seq_before + 2, Duration::from_secs(10)),
        "no frames while shuttling"
    );

    // Running to the end reports EOS on the bus.
    assert!(
        wait_for(
            || player.poll_events().contains(&viode_gui::player::PlayerEvent::Eos),
            Duration::from_secs(15)
        ),
        "no EOS at the end of the timeline"
    );
}
