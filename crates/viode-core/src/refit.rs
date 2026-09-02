//! Refit: retime a music bed to a target duration with an invisible
//! seam. Shortening removes one interior span; lengthening repeats one.
//! The seam lands where the track is quietest (from the cached loudness
//! analysis), and the two halves overlap by the fade time — GES
//! auto-transition crossfades overlapping clips on the same track, so
//! the seam renders as a crossfade with no new engine work. The result
//! is ordinary clips: diffable, trimmable, undoable.

use crate::model::{Project, TrackKind};
use crate::time::Time;

#[derive(Debug, Clone, PartialEq)]
pub enum RefitPlan {
    /// Remove SOURCE span [from, to).
    Cut { from: Time, to: Time },
    /// Play [in_, from+len) then again from `from` — i.e. repeat the
    /// SOURCE span [from, to) once.
    Repeat { from: Time, to: Time },
}

#[derive(Debug, thiserror::Error)]
pub enum RefitError {
    #[error("track {0} is not a music overlay (refit works on overlay tracks with audio)")]
    BadTrack(usize),
    #[error("clip index {0} out of range")]
    BadIndex(usize),
    #[error("target {0} is out of reach: refit can shorten to half or stretch to double, clip is {1}")]
    OutOfReach(Time, Time),
    #[error("already within a window of the target — nothing to do")]
    AlreadyFits,
    #[error(transparent)]
    Analyze(#[from] crate::audio::AnalyzeError),
}

/// Choose the quietest seam. `levels` are SOURCE-time loudness windows;
/// only windows inside [in_, out) are considered. Pure.
pub fn plan(
    levels: &[(Time, f64)],
    in_: Time,
    out: Time,
    target: Time,
) -> Result<RefitPlan, RefitError> {
    let len = out - in_;
    let window = if levels.len() >= 2 {
        levels[1].0 - levels[0].0
    } else {
        Time(500_000_000)
    };
    if target.0 > len.0.saturating_mul(2) || target.0 < len.0 / 2 {
        return Err(RefitError::OutOfReach(target, len));
    }
    let delta = if target > len { target - len } else { len - target };
    if delta <= window {
        return Err(RefitError::AlreadyFits);
    }
    let inside: Vec<(Time, f64)> = levels
        .iter()
        .filter(|(t, _)| *t >= in_ && *t < out)
        .copied()
        .collect();
    let level_at = |t: Time| -> f64 {
        inside
            .iter()
            .min_by_key(|(wt, _)| wt.0.abs_diff(t.0))
            .map(|(_, db)| *db)
            .unwrap_or(0.0)
    };
    // The seam joins the audio at `w` with the audio at `w + delta`
    // (cut) or repeats [w, w+delta) (lengthen) — in both cases the two
    // stitch points should be as quiet as possible.
    let mut best: Option<(f64, Time)> = None;
    for (w, db) in &inside {
        let other = *w + delta;
        if other >= out {
            continue;
        }
        let score = db + level_at(other);
        if best.is_none() || score < best.unwrap().0 {
            best = Some((score, *w));
        }
    }
    let (_, from) = best.ok_or(RefitError::OutOfReach(target, len))?;
    let to = from + delta;
    Ok(if target < len {
        RefitPlan::Cut { from, to }
    } else {
        RefitPlan::Repeat { from, to }
    })
}

/// Apply a plan to one overlay clip: it becomes two clips overlapping by
/// `fade`, which GES renders as a crossfade at the seam.
pub fn apply(
    project: &mut Project,
    track: usize,
    index: usize,
    plan: &RefitPlan,
    fade: Time,
) -> Result<(), RefitError> {
    let t = project.tracks.get_mut(track).ok_or(RefitError::BadTrack(track))?;
    if track == 0 || t.kind == TrackKind::Video {
        return Err(RefitError::BadTrack(track));
    }
    let clip = t.clips.get(index).ok_or(RefitError::BadIndex(index))?.clone();
    let at = clip.at.unwrap_or(Time::ZERO);
    let (left_out, right_in) = match plan {
        RefitPlan::Cut { from, to } => (*from, *to),
        RefitPlan::Repeat { from, to } => (*to, *from),
    };
    let mut left = clip.clone();
    left.out = left_out;
    let mut right = clip;
    right.in_ = right_in;
    right.at = Some(at + left.len() - fade);
    t.clips.splice(index..=index, [left, right]);
    Ok(())
}

/// The verb: analyze, plan, and apply in one call. Returns the plan.
pub fn refit(
    project: &mut Project,
    project_dir: &std::path::Path,
    track: usize,
    index: usize,
    target: Time,
    fade: Time,
) -> Result<RefitPlan, RefitError> {
    let t = project.tracks.get(track).ok_or(RefitError::BadTrack(track))?;
    if track == 0 || t.kind == TrackKind::Video {
        return Err(RefitError::BadTrack(track));
    }
    let clip = t.clips.get(index).ok_or(RefitError::BadIndex(index))?;
    let abs = if clip.src.is_absolute() {
        clip.src.clone()
    } else {
        project_dir.join(&clip.src)
    };
    let levels = crate::audio::audio_scan(project_dir, &abs, -30.0, 0.35, 0.5)?.levels;
    let p = plan(&levels, clip.in_, clip.out, target)?;
    apply(project, track, index, &p, fade)?;
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Clip, Track};

    fn t(secs: f64) -> Time {
        Time::from_secs_f64(secs).unwrap()
    }

    /// 0.5s windows over 60s: quiet valleys at 10s and 30s, loud rest.
    fn levels() -> Vec<(Time, f64)> {
        (0..120)
            .map(|i| {
                let start = i as f64 * 0.5;
                let quiet = (9.5..10.5).contains(&start) || (29.5..30.5).contains(&start);
                (t(start), if quiet { -55.0 } else { -12.0 })
            })
            .collect()
    }

    #[test]
    fn shortening_cuts_between_the_two_quiet_valleys() {
        // 60s down to 40s: the only 20s spans with BOTH ends quiet sit
        // in the valleys around 10s and 30s.
        let RefitPlan::Cut { from, to } = plan(&levels(), t(0.0), t(60.0), t(40.0)).unwrap()
        else {
            panic!("expected a cut")
        };
        assert!((9.4..=10.6).contains(&(from.0 as f64 / 1e9)), "{from}");
        assert_eq!(to - from, t(20.0), "cut length equals the excess");
    }

    #[test]
    fn lengthening_repeats_the_quiet_bounded_span() {
        let RefitPlan::Repeat { from, to } = plan(&levels(), t(0.0), t(60.0), t(80.0)).unwrap()
        else {
            panic!("expected a repeat")
        };
        assert!((9.4..=10.6).contains(&(from.0 as f64 / 1e9)), "{from}");
        assert_eq!(to - from, t(20.0));
    }

    #[test]
    fn unreachable_and_noop_targets_refuse_clearly() {
        assert!(matches!(
            plan(&levels(), t(0.0), t(60.0), t(10.0)),
            Err(RefitError::OutOfReach(_, _))
        ));
        assert!(matches!(
            plan(&levels(), t(0.0), t(60.0), t(60.2)),
            Err(RefitError::AlreadyFits)
        ));
    }

    #[test]
    fn apply_splits_into_two_overlapping_clips_that_hit_the_target() {
        let mut p = Project::new("r", 30.0, [320, 180]);
        let mut music = Track::new("music", TrackKind::Audio);
        let mut c = Clip::media("media/song.mp3".into(), t(0.0), t(60.0));
        c.at = Some(t(5.0));
        music.clips.push(c);
        p.tracks.push(music);

        let fade = t(0.5);
        apply(&mut p, 1, 0, &RefitPlan::Cut { from: t(10.0), to: t(30.0) }, fade).unwrap();
        let clips = &p.tracks[1].clips;
        assert_eq!(clips.len(), 2);
        assert_eq!((clips[0].in_, clips[0].out), (t(0.0), t(10.0)));
        assert_eq!((clips[1].in_, clips[1].out), (t(30.0), t(60.0)));
        // Right half starts fade early so the two overlap into a
        // crossfade; audible span = 40s - fade.
        assert_eq!(clips[1].at, Some(t(14.5)));
        let end = clips[1].at.unwrap() + clips[1].len();
        assert_eq!(end - t(5.0), t(39.5));

        // Main track refuses.
        assert!(matches!(
            apply(&mut p, 0, 0, &RefitPlan::Cut { from: t(1.0), to: t(2.0) }, fade),
            Err(RefitError::BadTrack(0))
        ));
    }
}
