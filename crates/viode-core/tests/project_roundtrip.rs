//! Integration tests for the project file — the contract that matters most.
//! project.viode is Viode's public interface: humans hand-edit it, git diffs
//! it, LLMs generate it. These tests pin down what the file format accepts,
//! that save -> load is lossless, and that pre-multitrack files still load.

use viode_core::{ops, Clip, Project, Time, Title, Track, TrackKind};

fn clip(src: &str, in_s: f64, out_s: f64) -> Clip {
    Clip::media(
        src.into(),
        Time::from_secs_f64(in_s).unwrap(),
        Time::from_secs_f64(out_s).unwrap(),
    )
}

fn t(s: f64) -> Time {
    Time::from_secs_f64(s).unwrap()
}

#[test]
fn save_load_roundtrip_is_lossless() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("project.viode");

    let mut project = Project::new("roundtrip", 25.0, [1280, 720]);
    ops::add(project.main_mut(), clip("media/a.mp4", 0.0, 3.0)).unwrap();
    ops::add(project.main_mut(), clip("media/b.mp4", 1.25, 4.75)).unwrap();
    ops::split(project.main_mut(), 0, t(1.5)).unwrap();
    ops::set_transition(project.main_mut(), 2, Some(t(0.5))).unwrap();
    project.main_mut().clips[0].effects = vec!["videobalance saturation=0.0".into()];

    let mut broll = Track::new("broll", TrackKind::Video);
    let mut over = clip("media/drone.mp4", 0.0, 2.0);
    over.at = Some(t(1.0));
    broll.clips.push(over);
    project.tracks.push(broll);

    let mut angle = Track::new("angle2", TrackKind::Av);
    angle.enabled = false;
    angle.clips.push(clip("media/cam2.mp4", 0.0, 5.0));
    project.tracks.push(angle);

    project.titles.push(Title {
        text: "Chapter One".into(),
        at: t(0.5),
        dur: t(2.0),
        font: Some("Sans Bold 64".into()),
        xpos: Some(0.1),
        ypos: Some(0.8),
        color: Some("#FFCC00".into()),
    });

    project.save(&path).unwrap();
    let reloaded = Project::load(&path).unwrap();
    assert_eq!(project, reloaded);
}

#[test]
fn legacy_single_track_files_still_load() {
    // The pre-multitrack format: [[clip]] at the root. Old projects must
    // open forever.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("project.viode");
    std::fs::write(
        &path,
        r#"
[project]
name = "old"
fps = 30.0
resolution = [1920, 1080]

[[clip]]
src = "media/a.mp4"
in = "00:00:01.000"
out = "00:00:03.000"

[[clip]]
src = "media/b.mp4"
out = 2.0
"#,
    )
    .unwrap();

    let project = Project::load(&path).unwrap();
    assert_eq!(project.tracks.len(), 1);
    assert_eq!(project.main().clips.len(), 2);
    assert_eq!(project.main().clips[0].in_, t(1.0));
    assert_eq!(project.total_duration(), t(4.0));

    // Saving writes the new format; reloading gives the same timeline.
    project.save(&path).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("[[track]]"), "saved in new format:\n{text}");
    assert_eq!(Project::load(&path).unwrap(), project);
}

#[test]
fn accepts_hand_written_multitrack_toml() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("project.viode");
    std::fs::write(
        &path,
        r#"
[project]
name = "handmade"
fps = 30.0
resolution = [1920, 1080]

[[track]]
name = "main"

[[track.clip]]
src = "media/interview.mp4"
in = 4.2
out = "01:12"

[[track.clip]]
src = "media/interview.mp4"
in = "01:30"
out = "01:40"
transition = 0.5
effects = ["videobalance saturation=1.2"]

[[track]]
name = "music"
kind = "audio"

[[track.clip]]
src = "media/theme.mp3"
out = 30.0
at = 0.0

[[title]]
text = "Handmade"
at = 1.0
dur = 3.0
"#,
    )
    .unwrap();

    let project = Project::load(&path).unwrap();
    assert_eq!(project.tracks.len(), 2);
    assert_eq!(project.tracks[1].kind, TrackKind::Audio);
    assert_eq!(project.main().clips[1].transition, Some(t(0.5)));
    assert_eq!(project.titles[0].text, "Handmade");
    // Sequence: 67.8 + 10.0 - 0.5 crossfade = 77.3.
    assert_eq!(project.main().end(), t(77.3));
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
    let mut project = Project::new("inv", 30.0, [640, 360]);
    ops::add(project.main_mut(), clip("a.mp4", 0.0, 2.0)).unwrap();
    ops::add(project.main_mut(), clip("b.mp4", 0.5, 3.0)).unwrap();
    ops::add(project.main_mut(), clip("c.mp4", 1.0, 1.75)).unwrap();
    ops::split(project.main_mut(), 1, t(1.0)).unwrap();
    ops::move_clip(project.main_mut(), 3, 0).unwrap();
    ops::remove(project.main_mut(), 2).unwrap();
    ops::trim(project.main_mut(), 0, None, Some(t(1.5))).unwrap();

    let positions = project.positions();
    let clips = &project.main().clips;
    assert_eq!(positions[0], Time::ZERO);
    for i in 1..clips.len() {
        assert_eq!(
            positions[i],
            positions[i - 1] + clips[i - 1].len(),
            "gap or overlap between clip {} and {}",
            i - 1,
            i
        );
    }
    let last = clips.len() - 1;
    assert_eq!(positions[last] + clips[last].len(), project.total_duration());
}

#[test]
fn init_scaffolds_a_loadable_project_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("mycut");
    let file = Project::init(&dir, 25.0, [1280, 720]).unwrap();

    assert_eq!(file, dir.join("project.viode"));
    for sub in ["media", "renders", "cache", "proxies"] {
        assert!(dir.join(sub).is_dir(), "missing {sub}/");
    }
    assert!(dir.join(".gitignore").is_file());
    let loaded = Project::load(&file).unwrap();
    assert_eq!(loaded.project.name, "mycut");
    assert_eq!(loaded.project.fps, 25.0);
    assert_eq!(loaded.project.resolution, [1280, 720]);
}

#[test]
fn init_refuses_existing_paths_with_a_helpful_error() {
    let tmp = tempfile::tempdir().unwrap();
    let err = Project::init(tmp.path(), 30.0, [1920, 1080]).unwrap_err().to_string();
    assert!(err.contains("already exists"), "unhelpful error: {err}");
}
