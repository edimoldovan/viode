//! Edit operations — pure functions over the Project model. No I/O here;
//! side effects (probing, rendering, file copies) live at the edges.

use crate::model::{Clip, Project};
use crate::time::Time;

#[derive(Debug, thiserror::Error)]
pub enum OpError {
    #[error("clip index {0} out of range (timeline has {1} clips)")]
    BadIndex(usize, usize),
    #[error("invalid trim: in {0} must be before out {1}")]
    BadRange(Time, Time),
    #[error("split point {0} outside clip (length {1})")]
    BadSplit(Time, Time),
}

fn check_index(project: &Project, index: usize) -> Result<(), OpError> {
    if index >= project.clips.len() {
        return Err(OpError::BadIndex(index, project.clips.len()));
    }
    Ok(())
}

pub fn add(project: &mut Project, clip: Clip) -> Result<(), OpError> {
    if clip.in_ >= clip.out {
        return Err(OpError::BadRange(clip.in_, clip.out));
    }
    project.clips.push(clip);
    Ok(())
}

pub fn remove(project: &mut Project, index: usize) -> Result<Clip, OpError> {
    check_index(project, index)?;
    Ok(project.clips.remove(index))
}

pub fn move_clip(project: &mut Project, from: usize, to: usize) -> Result<(), OpError> {
    check_index(project, from)?;
    check_index(project, to)?;
    let clip = project.clips.remove(from);
    project.clips.insert(to, clip);
    Ok(())
}

/// Adjust a clip's source in/out points. `None` leaves a bound unchanged.
pub fn trim(
    project: &mut Project,
    index: usize,
    in_: Option<Time>,
    out: Option<Time>,
) -> Result<(), OpError> {
    check_index(project, index)?;
    let clip = &mut project.clips[index];
    let new_in = in_.unwrap_or(clip.in_);
    let new_out = out.unwrap_or(clip.out);
    if new_in >= new_out {
        return Err(OpError::BadRange(new_in, new_out));
    }
    clip.in_ = new_in;
    clip.out = new_out;
    Ok(())
}

/// Split a clip at `at` (offset from the clip's own start) into two clips.
pub fn split(project: &mut Project, index: usize, at: Time) -> Result<(), OpError> {
    check_index(project, index)?;
    let clip = &project.clips[index];
    if at == Time::ZERO || at >= clip.len() {
        return Err(OpError::BadSplit(at, clip.len()));
    }
    let mid = clip.in_ + at;
    let mut right = clip.clone();
    right.in_ = mid;
    project.clips[index].out = mid;
    project.clips.insert(index + 1, right);
    Ok(())
}

/// Map a timeline position to (clip index, time within that clip's source).
/// Returns None past the end of the timeline.
pub fn source_at(project: &Project, at: Time) -> Option<(usize, Time)> {
    let mut cursor = Time::ZERO;
    for (i, clip) in project.clips.iter().enumerate() {
        let end = cursor + clip.len();
        if at < end {
            return Some((i, clip.in_ + (at - cursor)));
        }
        cursor = end;
    }
    None
}

/// A new project containing only the [start, end) range of the timeline,
/// with boundary clips trimmed. Pure timeline math — used for previews.
pub fn extract_range(project: &Project, start: Time, end: Time) -> Result<Project, OpError> {
    if start >= end || end > project.total_duration() {
        return Err(OpError::BadRange(start, end));
    }
    let mut out = Project::new(
        &project.project.name,
        project.project.fps,
        project.project.resolution,
    );
    let mut cursor = Time::ZERO;
    for clip in &project.clips {
        let clip_start = cursor;
        let clip_end = cursor + clip.len();
        cursor = clip_end;
        if clip_end <= start || clip_start >= end {
            continue;
        }
        let mut trimmed = clip.clone();
        if start > clip_start {
            trimmed.in_ = clip.in_ + (start - clip_start);
        }
        if end < clip_end {
            trimmed.out = clip.in_ + (end - clip_start);
        }
        out.clips.push(trimmed);
    }
    Ok(out)
}

/// Split a clip at several SOURCE times (positions in the media file).
/// Times outside the clip's (in, out) range are ignored. Returns the number
/// of resulting segments.
pub fn split_at_source_times(
    project: &mut Project,
    index: usize,
    times: &[Time],
) -> Result<usize, OpError> {
    check_index(project, index)?;
    let clip = project.clips[index].clone();

    let mut cuts: Vec<Time> = times
        .iter()
        .copied()
        .filter(|t| *t > clip.in_ && *t < clip.out)
        .collect();
    cuts.sort();
    cuts.dedup();
    if cuts.is_empty() {
        return Ok(1);
    }

    let mut segments = Vec::with_capacity(cuts.len() + 1);
    let mut prev = clip.in_;
    for cut in cuts.iter().chain(std::iter::once(&clip.out)) {
        let mut seg = clip.clone();
        seg.in_ = prev;
        seg.out = *cut;
        segments.push(seg);
        prev = *cut;
    }
    let count = segments.len();
    project.clips.splice(index..=index, segments);
    Ok(count)
}

/// What `remove_source_ranges` did, for reporting.
#[derive(Debug, PartialEq)]
pub struct RemovedRanges {
    pub segments_kept: usize,
    pub removed: Time,
}

/// Remove SOURCE-time ranges (e.g. detected silences) from a clip, replacing
/// it with the segments in between. `pad` keeps that much of each range's
/// edges, so cuts don't clip the last syllable before a pause.
pub fn remove_source_ranges(
    project: &mut Project,
    index: usize,
    ranges: &[(Time, Time)],
    pad: Time,
) -> Result<RemovedRanges, OpError> {
    check_index(project, index)?;
    let clip = project.clips[index].clone();

    // Clamp to the clip, apply padding, drop ranges the padding consumed,
    // then merge overlaps so segment math below is simple.
    let mut cuts: Vec<(Time, Time)> = ranges
        .iter()
        .map(|&(s, e)| (s.max(clip.in_) + pad, (e.min(clip.out)) - pad))
        .filter(|&(s, e)| s < e)
        .collect();
    cuts.sort();
    let mut merged: Vec<(Time, Time)> = Vec::with_capacity(cuts.len());
    for (s, e) in cuts.drain(..) {
        match merged.last_mut() {
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => merged.push((s, e)),
        }
    }

    let mut segments = Vec::new();
    let mut cursor = clip.in_;
    for (s, e) in &merged {
        if *s > cursor {
            let mut seg = clip.clone();
            seg.in_ = cursor;
            seg.out = *s;
            segments.push(seg);
        }
        cursor = cursor.max(*e);
    }
    if cursor < clip.out {
        let mut seg = clip.clone();
        seg.in_ = cursor;
        seg.out = clip.out;
        segments.push(seg);
    }

    if segments.is_empty() {
        // Refuse to silently delete the whole clip — that's `rm`'s job.
        return Err(OpError::BadSplit(pad, clip.len()));
    }

    let kept: Time = segments.iter().fold(Time::ZERO, |acc, s| acc + s.len());
    let removed = clip.len() - kept;
    let segments_kept = segments.len();
    project.clips.splice(index..=index, segments);
    Ok(RemovedRanges { segments_kept, removed })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Project;

    fn clip(in_s: f64, out_s: f64) -> Clip {
        Clip {
            src: "media/x.mp4".into(),
            in_: Time::from_secs_f64(in_s).unwrap(),
            out: Time::from_secs_f64(out_s).unwrap(),
            label: None,
        }
    }

    fn project() -> Project {
        let mut p = Project::new("t", 30.0, [1920, 1080]);
        p.clips.push(clip(0.0, 3.0));
        p.clips.push(clip(1.0, 4.0));
        p
    }

    #[test]
    fn split_preserves_total() {
        let mut p = project();
        let before = p.total_duration();
        split(&mut p, 0, Time::from_secs_f64(1.5).unwrap()).unwrap();
        assert_eq!(p.clips.len(), 3);
        assert_eq!(p.total_duration(), before);
        assert_eq!(p.clips[0].out, p.clips[1].in_);
    }

    #[test]
    fn split_rejects_bounds() {
        let mut p = project();
        assert!(split(&mut p, 0, Time::ZERO).is_err());
        assert!(split(&mut p, 0, Time::from_secs_f64(3.0).unwrap()).is_err());
    }

    #[test]
    fn trim_validates() {
        let mut p = project();
        assert!(trim(&mut p, 0, None, Some(Time::from_secs_f64(0.0).unwrap())).is_err());
        trim(&mut p, 0, Some(Time::from_secs_f64(0.5).unwrap()), None).unwrap();
        assert_eq!(p.clips[0].in_, Time::from_secs_f64(0.5).unwrap());
    }

    #[test]
    fn move_and_remove() {
        let mut p = project();
        move_clip(&mut p, 1, 0).unwrap();
        assert_eq!(p.clips[0].in_, Time::from_secs_f64(1.0).unwrap());
        remove(&mut p, 1).unwrap();
        assert_eq!(p.clips.len(), 1);
        assert!(remove(&mut p, 5).is_err());
    }

    #[test]
    fn positions_are_gapless() {
        let p = project();
        let pos = p.positions();
        assert_eq!(pos[0], Time::ZERO);
        assert_eq!(pos[1], Time::from_secs_f64(3.0).unwrap());
    }

    #[test]
    fn source_at_maps_timeline_to_source() {
        // Timeline: clip0 = a[0..3] at 0..3, clip1 = a[1..4] at 3..6.
        let p = project();
        let t = |s| Time::from_secs_f64(s).unwrap();
        assert_eq!(source_at(&p, t(0.0)), Some((0, t(0.0))));
        assert_eq!(source_at(&p, t(2.5)), Some((0, t(2.5))));
        // 0.5s into clip 1, whose source in-point is 1.0 -> source 1.5.
        assert_eq!(source_at(&p, t(3.5)), Some((1, t(1.5))));
        assert_eq!(source_at(&p, t(6.0)), None, "end of timeline is exclusive");
    }

    #[test]
    fn extract_range_trims_boundary_clips() {
        let p = project();
        let t = |s| Time::from_secs_f64(s).unwrap();
        // Middle 2s spanning the cut between the clips.
        let sub = extract_range(&p, t(2.0), t(4.0)).unwrap();
        assert_eq!(sub.clips.len(), 2);
        assert_eq!(sub.total_duration(), t(2.0));
        assert_eq!(sub.clips[0].in_, t(2.0)); // tail of clip 0
        assert_eq!(sub.clips[0].out, t(3.0));
        assert_eq!(sub.clips[1].in_, t(1.0)); // head of clip 1
        assert_eq!(sub.clips[1].out, t(2.0));

        assert!(extract_range(&p, t(4.0), t(2.0)).is_err());
        assert!(extract_range(&p, t(0.0), t(99.0)).is_err());
    }

    #[test]
    fn split_at_source_times_ignores_out_of_range_cuts() {
        let mut p = project(); // clip 0 = [0..3]
        let t = |s| Time::from_secs_f64(s).unwrap();
        // 0.0 and 3.0 are boundaries (no-ops); 99 is outside; 1.0/2.0 are real.
        let n = split_at_source_times(&mut p, 0, &[t(2.0), t(0.0), t(1.0), t(3.0), t(99.0)])
            .unwrap();
        assert_eq!(n, 3);
        assert_eq!(p.clips.len(), 4); // 3 segments + the untouched clip 1
        assert_eq!(p.clips[0].out, t(1.0));
        assert_eq!(p.clips[1].in_, t(1.0));
        assert_eq!(p.clips[2].out, t(3.0));
        assert_eq!(p.total_duration(), t(6.0), "splitting never changes duration");
    }

    #[test]
    fn remove_source_ranges_cuts_middle_and_keeps_padding() {
        let mut p = project(); // clip 0 = [0..3]
        let t = |s| Time::from_secs_f64(s).unwrap();
        // Remove silence at 1.0..2.0, keeping 0.1s padding on each side.
        let stats =
            remove_source_ranges(&mut p, 0, &[(t(1.0), t(2.0))], t(0.1)).unwrap();
        assert_eq!(stats.segments_kept, 2);
        assert_eq!(stats.removed, t(0.8)); // 1.0s minus 2 x 0.1 pad
        assert_eq!(p.clips[0].out, t(1.1));
        assert_eq!(p.clips[1].in_, t(1.9));
    }

    #[test]
    fn remove_source_ranges_merges_overlaps_and_clamps() {
        let mut p = project();
        let t = |s| Time::from_secs_f64(s).unwrap();
        // Overlapping + out-of-clip ranges collapse into one cut 0.5..2.5.
        let stats = remove_source_ranges(
            &mut p,
            0,
            &[(t(0.5), t(1.5)), (t(1.0), t(2.5)), (t(90.0), t(99.0))],
            Time::ZERO,
        )
        .unwrap();
        assert_eq!(stats.segments_kept, 2);
        assert_eq!(stats.removed, t(2.0));
    }

    #[test]
    fn remove_source_ranges_refuses_to_delete_everything() {
        let mut p = project();
        let t = |s| Time::from_secs_f64(s).unwrap();
        assert!(remove_source_ranges(&mut p, 0, &[(t(0.0), t(3.0))], Time::ZERO).is_err());
    }
}

#[cfg(test)]
mod prop_tests {
    use super::*;
    use crate::model::Project;
    use proptest::prelude::*;

    proptest! {
        /// Splitting at ANY valid point never changes the total duration and
        /// never produces an inverted (in >= out) clip. If you touch split(),
        /// this is the invariant you must not break.
        #[test]
        fn split_preserves_total_duration(
            in_ms in 0u64..5_000,
            len_ms in 2u64..600_000,
            frac in 0.001f64..0.999,
        ) {
            let mut p = Project::new("prop", 30.0, [640, 360]);
            let in_ = Time(in_ms * 1_000_000);
            let out = Time((in_ms + len_ms) * 1_000_000);
            p.clips.push(Clip { src: "x.mp4".into(), in_, out, label: None });

            let at = Time(((len_ms as f64 * frac) as u64).max(1) * 1_000_000);
            prop_assume!(at < p.clips[0].len());

            let before = p.total_duration();
            split(&mut p, 0, at).unwrap();

            prop_assert_eq!(p.total_duration(), before);
            for c in &p.clips {
                prop_assert!(c.in_ < c.out);
            }
            prop_assert_eq!(p.clips[0].out, p.clips[1].in_);
        }
    }
}
