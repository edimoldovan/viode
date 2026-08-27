//! Edit operations — pure functions over the model. No I/O here; side
//! effects (probing, rendering, file copies) live at the edges.
//!
//! Sequence ops take a `&mut Track` (usually the main track). Timeline-wide
//! queries (source_at, extract_range) take the whole Project.

use crate::model::{Clip, Project, Track};
use crate::time::Time;

#[derive(Debug, thiserror::Error)]
pub enum OpError {
    #[error("clip index {0} out of range (track has {1} clips)")]
    BadIndex(usize, usize),
    #[error("invalid trim: in {0} must be before out {1}")]
    BadRange(Time, Time),
    #[error("split point {0} outside clip (length {1})")]
    BadSplit(Time, Time),
    #[error("track index {0} out of range ({1} tracks)")]
    BadTrack(usize, usize),
}

fn check_index(track: &Track, index: usize) -> Result<(), OpError> {
    if index >= track.clips.len() {
        return Err(OpError::BadIndex(index, track.clips.len()));
    }
    Ok(())
}

pub fn track<'p>(project: &'p Project, index: usize) -> Result<&'p Track, OpError> {
    project
        .tracks
        .get(index)
        .ok_or(OpError::BadTrack(index, project.tracks.len()))
}

pub fn track_mut<'p>(project: &'p mut Project, index: usize) -> Result<&'p mut Track, OpError> {
    let n = project.tracks.len();
    project.tracks.get_mut(index).ok_or(OpError::BadTrack(index, n))
}

pub fn add(track: &mut Track, clip: Clip) -> Result<(), OpError> {
    if clip.in_ >= clip.out {
        return Err(OpError::BadRange(clip.in_, clip.out));
    }
    track.clips.push(clip);
    Ok(())
}

pub fn remove(track: &mut Track, index: usize) -> Result<Clip, OpError> {
    check_index(track, index)?;
    Ok(track.clips.remove(index))
}

pub fn move_clip(track: &mut Track, from: usize, to: usize) -> Result<(), OpError> {
    check_index(track, from)?;
    check_index(track, to)?;
    let clip = track.clips.remove(from);
    track.clips.insert(to, clip);
    Ok(())
}

/// Adjust a clip's source in/out points. `None` leaves a bound unchanged.
pub fn trim(
    track: &mut Track,
    index: usize,
    in_: Option<Time>,
    out: Option<Time>,
) -> Result<(), OpError> {
    check_index(track, index)?;
    let clip = &mut track.clips[index];
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
/// The left piece keeps any crossfade; the pieces join seamlessly.
pub fn split(track: &mut Track, index: usize, at: Time) -> Result<(), OpError> {
    check_index(track, index)?;
    let clip = &track.clips[index];
    if at == Time::ZERO || at >= clip.len() {
        return Err(OpError::BadSplit(at, clip.len()));
    }
    let mid = clip.in_ + at;
    let mut right = clip.clone();
    right.in_ = mid;
    right.transition = None;
    if let Some(at_pos) = right.at {
        right.at = Some(at_pos + at);
    }
    track.clips[index].out = mid;
    track.clips.insert(index + 1, right);
    Ok(())
}

/// Set (or clear) a clip's crossfade with the previous clip.
pub fn set_transition(
    track: &mut Track,
    index: usize,
    duration: Option<Time>,
) -> Result<(), OpError> {
    check_index(track, index)?;
    if index == 0 {
        return Err(OpError::BadIndex(0, track.clips.len()));
    }
    if let Some(d) = duration {
        let max = track.clips[index].len().min(track.clips[index - 1].len());
        if d == Time::ZERO || d >= max {
            return Err(OpError::BadRange(d, max));
        }
    }
    track.clips[index].transition = duration;
    Ok(())
}

/// Split a clip at several SOURCE times (positions in the media file).
/// Times outside the clip's (in, out) range are ignored. Returns the number
/// of resulting segments.
pub fn split_at_source_times(
    track: &mut Track,
    index: usize,
    times: &[Time],
) -> Result<usize, OpError> {
    check_index(track, index)?;
    let clip = track.clips[index].clone();

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
        if prev != clip.in_ {
            seg.transition = None;
        }
        segments.push(seg);
        prev = *cut;
    }
    let count = segments.len();
    track.clips.splice(index..=index, segments);
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
    track: &mut Track,
    index: usize,
    ranges: &[(Time, Time)],
    pad: Time,
) -> Result<RemovedRanges, OpError> {
    check_index(track, index)?;
    let clip = track.clips[index].clone();

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
            if cursor != clip.in_ {
                seg.transition = None;
            }
            segments.push(seg);
        }
        cursor = cursor.max(*e);
    }
    if cursor < clip.out {
        let mut seg = clip.clone();
        seg.in_ = cursor;
        seg.out = clip.out;
        if cursor != clip.in_ {
            seg.transition = None;
        }
        segments.push(seg);
    }

    if segments.is_empty() {
        // Refuse to silently delete the whole clip — that's `rm`'s job.
        return Err(OpError::BadSplit(pad, clip.len()));
    }

    let kept: Time = segments.iter().fold(Time::ZERO, |acc, s| acc + s.len());
    let removed = clip.len() - kept;
    let segments_kept = segments.len();
    track.clips.splice(index..=index, segments);
    Ok(RemovedRanges { segments_kept, removed })
}

/// Replace the [start, end) TIMELINE range of a sequence track with `clip`
/// (whose length must equal the range) — how a multicam take lands on the
/// main track.
pub fn replace_range(
    track: &mut Track,
    start: Time,
    end: Time,
    mut clip: Clip,
) -> Result<(), OpError> {
    if start >= end || end > track.end() {
        return Err(OpError::BadRange(start, end));
    }
    if clip.len() != end - start {
        return Err(OpError::BadRange(clip.in_, clip.out));
    }
    clip.transition = None;
    clip.at = None;

    let positions = track.positions();
    let mut result: Vec<Clip> = Vec::new();
    let mut inserted = false;
    for (i, old) in track.clips.iter().enumerate() {
        let c_start = positions[i];
        let c_end = c_start + old.len();
        // Piece before the range.
        if c_start < start && start < c_end {
            let mut head = old.clone();
            head.out = old.in_ + (start - c_start);
            result.push(head);
        } else if c_end <= start {
            result.push(old.clone());
            continue;
        }
        if !inserted {
            result.push(clip.clone());
            inserted = true;
        }
        // Piece after the range.
        if c_start < end && end < c_end {
            let mut tail = old.clone();
            tail.in_ = old.in_ + (end - c_start);
            tail.transition = None;
            result.push(tail);
        }
    }
    if !inserted {
        result.push(clip);
    }
    track.clips = result;
    Ok(())
}

/// Map a timeline position to (main-track clip index, time within that
/// clip's source). Returns None past the end of the sequence.
pub fn source_at(project: &Project, at: Time) -> Option<(usize, Time)> {
    let track = project.main();
    let positions = track.positions();
    for (i, clip) in track.clips.iter().enumerate() {
        let start = positions[i];
        if at >= start && at < start + clip.len() {
            return Some((i, clip.in_ + (at - start)));
        }
    }
    None
}

/// A new project containing only the [start, end) range of the timeline —
/// main sequence trimmed at the boundaries, overlay clips and titles
/// shifted/cropped. Pure timeline math, used for previews.
pub fn extract_range(project: &Project, start: Time, end: Time) -> Result<Project, OpError> {
    if start >= end || end > project.total_duration() {
        return Err(OpError::BadRange(start, end));
    }
    let mut out = Project::new(
        &project.project.name,
        project.project.fps,
        project.project.resolution,
    );

    let main = project.main();
    let positions = main.positions();
    for (i, clip) in main.clips.iter().enumerate() {
        let clip_start = positions[i];
        let clip_end = clip_start + clip.len();
        if clip_end <= start || clip_start >= end {
            continue;
        }
        let mut trimmed = clip.clone();
        trimmed.transition = None;
        if start > clip_start {
            trimmed.in_ = clip.in_ + (start - clip_start);
        }
        if end < clip_end {
            trimmed.out = clip.in_ + (end - clip_start);
        }
        out.main_mut().clips.push(trimmed);
    }

    for track in project.tracks.iter().skip(1).filter(|t| t.enabled) {
        let mut sub = Track::new(&track.name, track.kind);
        for clip in &track.clips {
            let (c_start, c_end) = clip.span();
            if c_end <= start || c_start >= end {
                continue;
            }
            let mut trimmed = clip.clone();
            if start > c_start {
                trimmed.in_ = clip.in_ + (start - c_start);
            }
            if end < c_end {
                trimmed.out = trimmed.in_ + (end.min(c_end) - start.max(c_start));
            }
            trimmed.at = Some(c_start.max(start) - start);
            sub.clips.push(trimmed);
        }
        if !sub.clips.is_empty() {
            out.tracks.push(sub);
        }
    }

    for title in &project.titles {
        let (t_start, t_end) = (title.at, title.at + title.dur);
        if t_end <= start || t_start >= end {
            continue;
        }
        let mut t = title.clone();
        t.at = t_start.max(start) - start;
        t.dur = t_end.min(end) - t_start.max(start);
        out.titles.push(t);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Project, Title, TrackKind};

    fn clip(in_s: f64, out_s: f64) -> Clip {
        Clip::media(
            "media/x.mp4".into(),
            Time::from_secs_f64(in_s).unwrap(),
            Time::from_secs_f64(out_s).unwrap(),
        )
    }

    fn project() -> Project {
        let mut p = Project::new("t", 30.0, [1920, 1080]);
        p.main_mut().clips.push(clip(0.0, 3.0));
        p.main_mut().clips.push(clip(1.0, 4.0));
        p
    }

    fn t(s: f64) -> Time {
        Time::from_secs_f64(s).unwrap()
    }

    #[test]
    fn split_preserves_total() {
        let mut p = project();
        let before = p.total_duration();
        split(p.main_mut(), 0, t(1.5)).unwrap();
        assert_eq!(p.main().clips.len(), 3);
        assert_eq!(p.total_duration(), before);
        assert_eq!(p.main().clips[0].out, p.main().clips[1].in_);
    }

    #[test]
    fn split_rejects_bounds() {
        let mut p = project();
        assert!(split(p.main_mut(), 0, Time::ZERO).is_err());
        assert!(split(p.main_mut(), 0, t(3.0)).is_err());
    }

    #[test]
    fn trim_validates() {
        let mut p = project();
        assert!(trim(p.main_mut(), 0, None, Some(t(0.0))).is_err());
        trim(p.main_mut(), 0, Some(t(0.5)), None).unwrap();
        assert_eq!(p.main().clips[0].in_, t(0.5));
    }

    #[test]
    fn move_and_remove() {
        let mut p = project();
        move_clip(p.main_mut(), 1, 0).unwrap();
        assert_eq!(p.main().clips[0].in_, t(1.0));
        remove(p.main_mut(), 1).unwrap();
        assert_eq!(p.main().clips.len(), 1);
        assert!(remove(p.main_mut(), 5).is_err());
    }

    #[test]
    fn positions_are_gapless() {
        let p = project();
        let pos = p.positions();
        assert_eq!(pos[0], Time::ZERO);
        assert_eq!(pos[1], t(3.0));
    }

    #[test]
    fn transitions_overlap_the_sequence() {
        let mut p = project();
        set_transition(p.main_mut(), 1, Some(t(0.5))).unwrap();
        let pos = p.positions();
        assert_eq!(pos[1], t(2.5), "clip 1 starts inside clip 0's tail");
        assert_eq!(p.total_duration(), t(5.5));
        // Too-long fades and fades on clip 0 are rejected.
        assert!(set_transition(p.main_mut(), 1, Some(t(3.0))).is_err());
        assert!(set_transition(p.main_mut(), 0, Some(t(0.5))).is_err());
        // Clearing restores the plain sequence.
        set_transition(p.main_mut(), 1, None).unwrap();
        assert_eq!(p.total_duration(), t(6.0));
    }

    #[test]
    fn source_at_maps_timeline_to_source() {
        let p = project();
        assert_eq!(source_at(&p, t(0.0)), Some((0, t(0.0))));
        assert_eq!(source_at(&p, t(2.5)), Some((0, t(2.5))));
        assert_eq!(source_at(&p, t(3.5)), Some((1, t(1.5))));
        assert_eq!(source_at(&p, t(6.0)), None, "end of timeline is exclusive");
    }

    #[test]
    fn extract_range_trims_boundaries_and_shifts_overlays() {
        let mut p = project();
        let mut overlay = Track::new("broll", TrackKind::Video);
        let mut over = clip(0.0, 2.0);
        over.at = Some(t(3.0)); // covers 3..5
        overlay.clips.push(over);
        p.tracks.push(overlay);
        p.titles.push(Title { text: "hi".into(), at: t(1.0), dur: t(4.0), font: None });

        let sub = extract_range(&p, t(2.0), t(4.0)).unwrap();
        assert_eq!(sub.main().clips.len(), 2);
        assert_eq!(sub.main().end(), t(2.0));
        // Overlay covered 3..5, window is 2..4 -> 1s at position 1.0.
        assert_eq!(sub.tracks[1].clips[0].at, Some(t(1.0)));
        assert_eq!(sub.tracks[1].clips[0].len(), t(1.0));
        // Title covered 1..5 -> cropped to 0..2 of the window.
        assert_eq!(sub.titles[0].at, t(0.0));
        assert_eq!(sub.titles[0].dur, t(2.0));

        assert!(extract_range(&p, t(4.0), t(2.0)).is_err());
        assert!(extract_range(&p, t(0.0), t(99.0)).is_err());
    }

    #[test]
    fn split_at_source_times_ignores_out_of_range_cuts() {
        let mut p = project();
        let n =
            split_at_source_times(p.main_mut(), 0, &[t(2.0), t(0.0), t(1.0), t(3.0), t(99.0)])
                .unwrap();
        assert_eq!(n, 3);
        assert_eq!(p.main().clips.len(), 4);
        assert_eq!(p.main().clips[0].out, t(1.0));
        assert_eq!(p.total_duration(), t(6.0), "splitting never changes duration");
    }

    #[test]
    fn remove_source_ranges_cuts_middle_and_keeps_padding() {
        let mut p = project();
        let stats =
            remove_source_ranges(p.main_mut(), 0, &[(t(1.0), t(2.0))], t(0.1)).unwrap();
        assert_eq!(stats.segments_kept, 2);
        assert_eq!(stats.removed, t(0.8));
        assert_eq!(p.main().clips[0].out, t(1.1));
        assert_eq!(p.main().clips[1].in_, t(1.9));
    }

    #[test]
    fn remove_source_ranges_merges_overlaps_and_clamps() {
        let mut p = project();
        let stats = remove_source_ranges(
            p.main_mut(),
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
        assert!(remove_source_ranges(p.main_mut(), 0, &[(t(0.0), t(3.0))], Time::ZERO).is_err());
    }

    #[test]
    fn replace_range_swaps_in_a_take() {
        // Timeline 0..6; take angle footage for 1..4.
        let mut p = project();
        let mut take = clip(10.0, 13.0);
        take.src = "media/angle2.mp4".into();
        replace_range(p.main_mut(), t(1.0), t(4.0), take).unwrap();

        let clips = &p.main().clips;
        assert_eq!(clips.len(), 3);
        assert_eq!(clips[0].out, t(1.0), "head of original clip 0");
        assert_eq!(clips[1].src, PathBufFrom("media/angle2.mp4"));
        assert_eq!(clips[2].in_, t(2.0), "tail of original clip 1 (source 1+1)");
        assert_eq!(p.total_duration(), t(6.0), "takes never change duration");

        // Wrong-length takes and bad ranges are rejected.
        let mut p = project();
        assert!(replace_range(p.main_mut(), t(1.0), t(4.0), clip(0.0, 1.0)).is_err());
        assert!(replace_range(p.main_mut(), t(5.0), t(99.0), clip(0.0, 1.0)).is_err());
    }

    #[allow(non_snake_case)]
    fn PathBufFrom(s: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(s)
    }
}

#[cfg(test)]
mod prop_tests {
    use super::*;
    use crate::model::{Project, Track, TrackKind};
    use proptest::prelude::*;

    proptest! {
        /// Splitting at ANY valid point never changes the total duration and
        /// never produces an inverted (in >= out) clip.
        #[test]
        fn split_preserves_total_duration(
            in_ms in 0u64..5_000,
            len_ms in 2u64..600_000,
            frac in 0.001f64..0.999,
        ) {
            let mut track = Track::new("main", TrackKind::Av);
            let in_ = Time(in_ms * 1_000_000);
            let out = Time((in_ms + len_ms) * 1_000_000);
            track.clips.push(Clip::media("x.mp4".into(), in_, out));
            let mut p = Project::new("prop", 30.0, [640, 360]);
            p.tracks[0] = track;

            let at = Time(((len_ms as f64 * frac) as u64).max(1) * 1_000_000);
            prop_assume!(at < p.main().clips[0].len());

            let before = p.total_duration();
            split(p.main_mut(), 0, at).unwrap();

            prop_assert_eq!(p.total_duration(), before);
            for c in &p.main().clips {
                prop_assert!(c.in_ < c.out);
            }
            prop_assert_eq!(p.main().clips[0].out, p.main().clips[1].in_);
        }
    }
}
