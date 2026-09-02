//! Mend: smooth a jump cut. The frames on either side of the cut are
//! bridged by a short optical-flow interpolation (ffmpeg minterpolate —
//! the same engine as smooth slow motion), materialized like a freeze
//! still under media/mend/ and inserted at the cut as an ordinary clip.
//! On a trimmed interview, the quarter-second morph reads as continuous
//! motion instead of a visible jump.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::model::{Clip, Project};
use crate::ops::OpError;
use crate::time::Time;

#[derive(Debug, thiserror::Error)]
pub enum MendError {
    #[error("failed to run ffmpeg (is ffmpeg installed?): {0}")]
    Spawn(#[from] std::io::Error),
    #[error("bridge generation failed: {0}")]
    Ffmpeg(String),
    #[error("mend needs a cut: index must be 1..{0}")]
    NoCut(usize),
    #[error(transparent)]
    Op(#[from] OpError),
}

/// Extract one frame as PNG.
fn grab_frame(src: &Path, at: Time, dest: &Path) -> Result<(), MendError> {
    let out = Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error"])
        .args(["-ss", &format!("{}", at.0 as f64 / 1e9)])
        .arg("-i")
        .arg(src)
        .args(["-frames:v", "1"])
        .arg(dest)
        .output()?;
    if !out.status.success() || !dest.exists() {
        return Err(MendError::Ffmpeg(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    Ok(())
}

/// Build (or reuse) the morph bridge between two frames.
pub fn build_bridge(
    project_dir: &Path,
    left_src: &Path,
    left_end: Time,
    right_src: &Path,
    right_start: Time,
    dur: Time,
    fps: f64,
    res: [u32; 2],
) -> Result<PathBuf, MendError> {
    let rel = Path::new("media/mend").join(format!(
        "bridge-{}-{}-{}.mp4",
        left_end.0, right_start.0, dur.0
    ));
    let dest = project_dir.join(&rel);
    if dest.exists() {
        return Ok(rel);
    }
    std::fs::create_dir_all(dest.parent().unwrap())?;
    let a = dest.with_extension("a.png");
    let b = dest.with_extension("b.png");
    // The frame just inside the left clip's end, and the right clip's first.
    grab_frame(left_src, Time(left_end.0.saturating_sub(40_000_000)), &a)?;
    grab_frame(right_src, right_start, &b)?;

    // minterpolate needs runway: each endpoint holds for 1.5x the morph
    // length at 8 fps, the optical flow blends across the join, and the
    // trim keeps only the centered dur-long transition.
    let secs = dur.0 as f64 / 1e9;
    let hold = secs * 1.5;
    let out = Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error"])
        .args(["-loop", "1", "-framerate", "8", "-t", &format!("{hold}")])
        .arg("-i")
        .arg(&a)
        .args(["-loop", "1", "-framerate", "8", "-t", &format!("{hold}")])
        .arg("-i")
        .arg(&b)
        .args(["-f", "lavfi", "-i", "anullsrc=channel_layout=stereo:sample_rate=48000"])
        .args([
            "-filter_complex",
            &format!(
                "[0:v][1:v]concat=n=2:v=1:a=0,scale={}:{},\
                 minterpolate=fps={fps}:mi_mode=mci:mc_mode=aobmc,\
                 trim=start={}:end={},setpts=PTS-STARTPTS[v]",
                res[0],
                res[1],
                hold - secs / 2.0,
                hold + secs / 2.0,
            ),
        ])
        .args(["-map", "[v]", "-map", "2:a"])
        .args(["-t", &format!("{secs}")])
        .args(["-c:v", "libx264", "-preset", "veryfast", "-pix_fmt", "yuv420p"])
        .args(["-c:a", "aac", "-shortest"])
        .arg(&dest)
        .output()?;
    let _ = std::fs::remove_file(&a);
    let _ = std::fs::remove_file(&b);
    if !out.status.success() {
        let _ = std::fs::remove_file(&dest);
        return Err(MendError::Ffmpeg(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    Ok(rel)
}

/// The verb: bridge the cut between main clips `index - 1` and `index`.
/// Returns the bridge clip's index.
pub fn mend_at(
    project: &mut Project,
    project_dir: &Path,
    index: usize,
    dur: Time,
) -> Result<usize, MendError> {
    let n = project.main().clips.len();
    if index == 0 || index >= n {
        return Err(MendError::NoCut(n));
    }
    let (left, right) = {
        let clips = &project.main().clips;
        (clips[index - 1].clone(), clips[index].clone())
    };
    let abs = |src: &Path| {
        if src.is_absolute() {
            src.to_path_buf()
        } else {
            project_dir.join(src)
        }
    };
    let rel = build_bridge(
        project_dir,
        &abs(&left.src),
        left.out,
        &abs(&right.src),
        right.in_,
        dur,
        project.project.fps,
        project.project.resolution,
    )?;
    project
        .main_mut()
        .clips
        .insert(index, Clip::media(rel, Time::ZERO, dur));
    Ok(index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Track, TrackKind};

    #[test]
    fn mend_needs_a_real_cut() {
        let mut p = Project::new("m", 30.0, [320, 180]);
        let mut t = Track::new("main", TrackKind::Av);
        t.clips.push(Clip::media("media/a.mp4".into(), Time::ZERO, Time(2_000_000_000)));
        p.tracks[0] = t;
        for bad in [0, 1, 5] {
            let err = mend_at(&mut p, Path::new("/nonexistent"), bad, Time(250_000_000));
            assert!(err.is_err(), "index {bad} should refuse");
        }
    }
}
