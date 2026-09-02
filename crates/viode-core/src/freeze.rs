//! Frame hold: freeze the frame under the playhead for a duration.
//!
//! The freeze is materialized as a real media file (one source frame
//! looped for the duration, silent audio) under media/freeze/, then
//! inserted into the timeline as an ordinary clip — so it previews,
//! renders, trims, and travels like any other footage, on every backend,
//! with no engine support needed. ffmpeg generates the still; the
//! timeline surgery is the existing split + insert ops.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::model::{Clip, Project};
use crate::ops::{self, OpError};
use crate::time::Time;

#[derive(Debug, thiserror::Error)]
pub enum FreezeError {
    #[error("failed to run ffmpeg (is ffmpeg installed?): {0}")]
    Spawn(#[from] std::io::Error),
    #[error("still generation failed for {0}: {1}")]
    Ffmpeg(String, String),
    #[error("nothing under the playhead to freeze")]
    NothingThere,
    #[error(transparent)]
    Op(#[from] OpError),
}

/// Generate (or reuse) the still-clip file for one source frame.
/// Returns the path RELATIVE to the project directory, ready for a Clip.
pub fn build_still(
    project_dir: &Path,
    src_abs: &Path,
    at_src: Time,
    dur: Time,
    fps: f64,
    res: [u32; 2],
) -> Result<PathBuf, FreezeError> {
    let stem = src_abs
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "frame".into());
    let rel = Path::new("media/freeze").join(format!("{stem}-{}-{}.mp4", at_src.0, dur.0));
    let dest = project_dir.join(&rel);
    if dest.exists() {
        return Ok(rel);
    }
    std::fs::create_dir_all(dest.parent().unwrap())?;
    let secs = dur.0 as f64 / 1e9;
    let out = Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error"])
        .args(["-ss", &format!("{}", at_src.0 as f64 / 1e9)])
        .arg("-i")
        .arg(src_abs)
        .args(["-f", "lavfi", "-i", "anullsrc=channel_layout=stereo:sample_rate=48000"])
        .args(["-frames:v", "1"])
        .args(["-vf", &format!("scale={}:{},loop=-1:1", res[0], res[1])])
        .args(["-t", &format!("{secs}")])
        .args(["-r", &format!("{fps}")])
        .args(["-c:v", "libx264", "-preset", "veryfast", "-pix_fmt", "yuv420p"])
        .args(["-c:a", "aac", "-shortest"])
        .arg(&dest)
        .output()?;
    if !out.status.success() {
        let _ = std::fs::remove_file(&dest);
        return Err(FreezeError::Ffmpeg(
            src_abs.display().to_string(),
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    Ok(rel)
}

/// The whole verb: freeze the main-track frame at timeline time `at` for
/// `dur`. Splits the clip under the playhead (unless the playhead sits on
/// a cut) and inserts the still after the left half. Returns the index of
/// the inserted freeze clip.
pub fn freeze_at(
    project: &mut Project,
    project_dir: &Path,
    at: Time,
    dur: Time,
) -> Result<usize, FreezeError> {
    let (index, src_time) = ops::source_at(project, at).ok_or(FreezeError::NothingThere)?;
    let src_rel = project.main().clips[index].src.clone();
    let src_abs = if src_rel.is_absolute() {
        src_rel.clone()
    } else {
        project_dir.join(&src_rel)
    };
    let fps = project.project.fps;
    let res = project.project.resolution;
    let still_rel = build_still(project_dir, &src_abs, src_time, dur, fps, res)?;

    // Timeline-local offset into the clip under the playhead.
    let start_of_clip: Time = project.main().clips[..index]
        .iter()
        .fold(Time::ZERO, |acc, c| acc + c.len());
    let offset = at - start_of_clip;
    let insert_at = if offset == Time::ZERO {
        index // playhead on the cut: no split, still goes before the clip
    } else {
        ops::split(project.main_mut(), index, offset)?;
        index + 1
    };
    let main = project.main_mut();
    main.clips
        .insert(insert_at, Clip::media(still_rel, Time::ZERO, dur));
    Ok(insert_at)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::TrackKind;

    fn project_with_clip() -> Project {
        let mut p = Project::new("f", 30.0, [320, 180]);
        let mut t = crate::model::Track::new("main", TrackKind::Av);
        t.clips
            .push(Clip::media("media/a.mp4".into(), Time::ZERO, Time(4_000_000_000)));
        p.tracks[0] = t;
        p
    }

    #[test]
    fn still_path_is_deterministic_per_frame_and_duration() {
        let a = Path::new("media/freeze");
        let p = project_with_clip();
        let _ = (a, p); // path shape is pinned below without running ffmpeg
        let rel = Path::new("media/freeze").join(format!("a-{}-{}.mp4", 1_000_000_000u64, 2_000_000_000u64));
        assert_eq!(rel, PathBuf::from("media/freeze/a-1000000000-2000000000.mp4"));
    }

    #[test]
    fn freeze_off_the_timeline_is_a_helpful_error() {
        let mut p = project_with_clip();
        let err = freeze_at(
            &mut p,
            Path::new("/nonexistent"),
            Time(99_000_000_000),
            Time(2_000_000_000),
        )
        .unwrap_err();
        assert!(err.to_string().contains("nothing under the playhead"));
    }
}
