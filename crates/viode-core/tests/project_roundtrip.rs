//! Integration tests for the project file — the contract that matters most.
//! project.viode is Viode's public interface: humans hand-edit it, git diffs
//! it, LLMs generate it. These tests pin down what the file format accepts
//! and that save -> load is lossless.

use viode_core::{ops, Clip, Project, Time};

fn clip(src: &str, in_s: f64, out_s: f64) -> Clip {
    Clip {
        src: src.into(),
        in_: Time::from_secs_f64(in_s).unwrap(),
        out: Time::from_secs_f64(out_s).unwrap(),
        label: None,
    }
}

#[test]
fn save_load_roundtrip_is_lossless() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("project.viode");

    let mut project = Project::new("roundtrip", 25.0, [1280, 720]);
    ops::add(&mut project, clip("media/a.mp4", 0.0, 3.0)).unwrap();
    ops::add(&mut project, clip("media/b.mp4", 1.25, 4.75)).unwrap();
    ops::split(&mut project, 0, Time::from_secs_f64(1.5).unwrap()).unwrap();

    project.save(&path).unwrap();
    let reloaded = Project::load(&path).unwrap();
    assert_eq!(project, reloaded);
}

#[test]
fn accepts_hand_written_toml_with_mixed_time_forms() {
    // A file a human might write: numeric seconds, MM:SS, full timecode.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("project.viode");
    std::fs::write(
        &path,
        r#"
[project]
name = "handmade"
fps = 30.0
resolution = [1920, 1080]

[[clip]]
src = "media/interview.mp4"
in = 4.2
out = "01:12"

[[clip]]
src = "media/broll.mp4"
out = "00:00:02.500"
"#,
    )
    .unwrap();

    let project = Project::load(&path).unwrap();
    assert_eq!(project.clips.len(), 2);
    assert_eq!(project.clips[0].in_, Time::from_secs_f64(4.2).unwrap());
    assert_eq!(project.clips[0].out, Time::from_secs_f64(72.0).unwrap());
    // `in` is optional and defaults to the start of the source.
    assert_eq!(project.clips[1].in_, Time::ZERO);
    assert_eq!(
        project.total_duration(),
        Time::from_secs_f64(67.8 + 2.5).unwrap()
    );
}

#[test]
fn missing_file_and_bad_toml_give_useful_errors() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("project.viode");
    let err = Project::load(&missing).unwrap_err().to_string();
    assert!(err.contains("viode new"), "unhelpful error: {err}");

    std::fs::write(&missing, "this is not toml [[[").unwrap();
    let err = Project::load(&missing).unwrap_err().to_string();
    assert!(err.contains("invalid project file"), "unhelpful error: {err}");
}

#[test]
fn positions_and_total_stay_consistent_through_edits() {
    // The core timeline invariant: clips form a gapless sequence, so the
    // last position + last length always equals the total duration.
    let mut project = Project::new("inv", 30.0, [640, 360]);
    ops::add(&mut project, clip("a.mp4", 0.0, 2.0)).unwrap();
    ops::add(&mut project, clip("b.mp4", 0.5, 3.0)).unwrap();
    ops::add(&mut project, clip("c.mp4", 1.0, 1.75)).unwrap();
    ops::split(&mut project, 1, Time::from_secs_f64(1.0).unwrap()).unwrap();
    ops::move_clip(&mut project, 3, 0).unwrap();
    ops::remove(&mut project, 2).unwrap();
    ops::trim(&mut project, 0, None, Some(Time::from_secs_f64(1.5).unwrap())).unwrap();

    let positions = project.positions();
    assert_eq!(positions[0], Time::ZERO);
    for i in 1..project.clips.len() {
        assert_eq!(
            positions[i],
            positions[i - 1] + project.clips[i - 1].len(),
            "gap or overlap between clip {} and {}",
            i - 1,
            i
        );
    }
    let last = project.clips.len() - 1;
    assert_eq!(
        positions[last] + project.clips[last].len(),
        project.total_duration()
    );
}
