//! Auto-ducking: lower the music when someone speaks. No sidechain
//! plumbing — the dialogue's cached loudness analysis becomes ordinary
//! volume keyframes on the music clips, so the duck previews, renders,
//! undoes, and diffs like any other edit, and the editor can hand-tune
//! any keyframe afterwards.

use crate::model::{Clip, Keyframe, Project};
use crate::time::Time;

#[derive(Debug, Clone, Copy)]
pub struct DuckOptions {
    /// Music volume while speech is present, as a fraction of its own
    /// base volume (0.25 = duck to a quarter).
    pub amount: f64,
    /// RMS dBFS above which a dialogue window counts as speech.
    pub threshold_db: f64,
    /// Fade time into and out of each duck.
    pub ramp: Time,
    /// Speech windows closer than this merge into one duck.
    pub gap: Time,
}

impl Default for DuckOptions {
    fn default() -> Self {
        DuckOptions {
            amount: 0.25,
            threshold_db: -35.0,
            ramp: Time(200_000_000),
            gap: Time(600_000_000),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DuckError {
    #[error("track {0} is not a duckable overlay (needs audio and clips)")]
    BadTrack(usize),
    #[error("no speech found above {0} dBFS — nothing to duck against")]
    NoSpeech(f64),
    #[error(transparent)]
    Analyze(#[from] crate::audio::AnalyzeError),
}

/// TIMELINE windows where the main track carries speech. `levels_of`
/// yields one clip's SOURCE-time loudness windows (injected so the
/// planning is pure and unit-testable; production passes the cached
/// audio_scan).
pub fn speech_mask(
    project: &Project,
    mut levels_of: impl FnMut(&Clip) -> Option<Vec<(Time, f64)>>,
    threshold_db: f64,
    gap: Time,
) -> Vec<(Time, Time)> {
    let mut windows: Vec<(Time, Time)> = Vec::new();
    let mut clip_start = Time::ZERO;
    for clip in &project.main().clips {
        let len = clip.len();
        if let Some(levels) = levels_of(clip) {
            let rate = clip.rate.unwrap_or(1.0);
            let window_len = window_length(&levels);
            for (src_t, db) in levels {
                let src_end = src_t + window_len;
                if db < threshold_db || src_end <= clip.in_ || src_t >= clip.out {
                    continue;
                }
                let vis_start = src_t.max(clip.in_);
                let vis_end = src_end.min(clip.out);
                let to_timeline =
                    |t: Time| clip_start + Time(((t - clip.in_).0 as f64 / rate) as u64);
                windows.push((to_timeline(vis_start), to_timeline(vis_end)));
            }
        }
        clip_start = clip_start + len;
    }
    windows.sort_by_key(|w| w.0 .0);
    // Merge windows that touch or sit within the gap.
    let mut merged: Vec<(Time, Time)> = Vec::new();
    for (start, end) in windows {
        match merged.last_mut() {
            Some((_, last_end)) if start.0 <= last_end.0 + gap.0 => {
                if end > *last_end {
                    *last_end = end;
                }
            }
            _ => merged.push((start, end)),
        }
    }
    merged
}

fn window_length(levels: &[(Time, f64)]) -> Time {
    if levels.len() >= 2 {
        levels[1].0 - levels[0].0
    } else {
        Time(500_000_000)
    }
}

/// Volume keyframes (SOURCE time) implementing the duck for one music
/// clip placed at `clip_at` on the timeline. Pure.
pub fn keys_for_clip(
    clip_at: Time,
    clip: &Clip,
    mask: &[(Time, Time)],
    opts: &DuckOptions,
) -> Vec<Keyframe> {
    let base = clip.volume.unwrap_or(1.0);
    let ducked = (base * opts.amount).max(0.0);
    let rate = clip.rate.unwrap_or(1.0);
    let clip_end = clip_at + clip.len();
    let to_source = |t: Time| -> Time {
        clip.in_ + Time(((t - clip_at).0 as f64 * rate) as u64)
    };
    let mut keys = Vec::new();
    let key = |at: Time, value: f64| Keyframe { prop: "volume".into(), at, value };
    for &(start, end) in mask {
        if end <= clip_at || start >= clip_end {
            continue;
        }
        let duck_in = (start - opts.ramp).max(clip_at);
        let duck_out = (end + opts.ramp).min(clip_end);
        if duck_in > clip_at {
            keys.push(key(to_source(duck_in), base));
        }
        keys.push(key(to_source(start.max(clip_at)), ducked));
        keys.push(key(to_source(end.min(clip_end)), ducked));
        if duck_out < clip_end {
            keys.push(key(to_source(duck_out), base));
        }
    }
    keys
}

/// The verb: duck every clip on `track` against the main track's speech.
/// Existing volume keyframes on the track are replaced (rerunning duck
/// re-plans); other keyframe properties are untouched.
pub fn duck(
    project: &mut Project,
    project_dir: &std::path::Path,
    track: usize,
    opts: &DuckOptions,
) -> Result<usize, DuckError> {
    use crate::model::TrackKind;
    let t = project.tracks.get(track).ok_or(DuckError::BadTrack(track))?;
    if track == 0 || t.kind == TrackKind::Video || t.clips.is_empty() {
        return Err(DuckError::BadTrack(track));
    }
    let dir = project_dir.to_path_buf();
    let mask = {
        let scan = |clip: &Clip| -> Option<Vec<(Time, f64)>> {
            let abs = if clip.src.is_absolute() {
                clip.src.clone()
            } else {
                dir.join(&clip.src)
            };
            crate::audio::audio_scan(&dir, &abs, -30.0, 0.35, 0.1)
                .ok()
                .map(|s| s.levels)
        };
        speech_mask(project, scan, opts.threshold_db, opts.gap)
    };
    if mask.is_empty() {
        return Err(DuckError::NoSpeech(opts.threshold_db));
    }
    let positions: Vec<Time> = project.tracks[track]
        .clips
        .iter()
        .map(|c| c.at.unwrap_or(Time::ZERO))
        .collect();
    for (clip, at) in project.tracks[track].clips.iter_mut().zip(positions) {
        clip.keys.retain(|k| k.prop != "volume");
        let mut keys = keys_for_clip(at, clip, &mask, opts);
        clip.keys.append(&mut keys);
        clip.keys.sort_by_key(|k| k.at.0);
    }
    Ok(mask.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Track, TrackKind};

    fn t(secs: f64) -> Time {
        Time::from_secs_f64(secs).unwrap()
    }

    fn project_with_dialogue() -> Project {
        let mut p = Project::new("d", 30.0, [320, 180]);
        let mut main = Track::new("main", TrackKind::Av);
        main.clips.push(Clip::media("media/talk.mp4".into(), t(0.0), t(10.0)));
        p.tracks[0] = main;
        p
    }

    /// 0.5s analysis windows: speech at 2.0-3.0 and 3.2-4.0 (merges into
    /// one duck), silence elsewhere.
    fn fake_levels(_c: &Clip) -> Option<Vec<(Time, f64)>> {
        let mut v = Vec::new();
        for i in 0..20 {
            let start = i as f64 * 0.5;
            let loud = (2.0..3.0).contains(&start) || (3.2..4.0).contains(&start);
            v.push((t(start), if loud { -20.0 } else { -60.0 }));
        }
        Some(v)
    }

    #[test]
    fn mask_merges_nearby_speech_and_respects_the_threshold() {
        let p = project_with_dialogue();
        let mask = speech_mask(&p, fake_levels, -35.0, Time(600_000_000));
        assert_eq!(mask.len(), 1, "{mask:?}");
        assert_eq!(mask[0], (t(2.0), t(4.0)));
        // A harsher threshold hears nothing.
        assert!(speech_mask(&p, fake_levels, -10.0, Time(600_000_000)).is_empty());
    }

    #[test]
    fn keys_duck_and_recover_with_ramps_in_source_time() {
        let mut music = Clip::media("media/song.mp3".into(), t(0.0), t(30.0));
        music.at = Some(t(0.0));
        music.volume = Some(0.8);
        let mask = [(t(2.0), t(4.0))];
        let keys = keys_for_clip(t(0.0), &music, &mask, &DuckOptions::default());
        let flat: Vec<(f64, f64)> = keys
            .iter()
            .map(|k| (k.at.0 as f64 / 1e9, k.value))
            .collect();
        assert_eq!(
            flat,
            vec![(1.8, 0.8), (2.0, 0.2), (4.0, 0.2), (4.2, 0.8)],
            "ramp in, duck to amount*base, ramp out"
        );
    }

    #[test]
    fn duck_end_to_end_writes_keys_and_refuses_bad_tracks() {
        let mut p = project_with_dialogue();
        let mut music = Track::new("music", TrackKind::Audio);
        let mut c = Clip::media("media/song.mp3".into(), t(0.0), t(10.0));
        c.at = Some(t(0.0));
        music.clips.push(c);
        p.tracks.push(music);

        // Main track is never duckable.
        assert!(matches!(
            duck(&mut p, std::path::Path::new("."), 0, &DuckOptions::default()),
            Err(DuckError::BadTrack(0))
        ));

        // Plan with injected levels through the pure pieces.
        let mask = speech_mask(&p, fake_levels, -35.0, Time(600_000_000));
        let clip = p.tracks[1].clips[0].clone();
        let keys = keys_for_clip(t(0.0), &clip, &mask, &DuckOptions::default());
        p.tracks[1].clips[0].keys = keys;
        assert_eq!(p.tracks[1].clips[0].keys.len(), 4);
    }
}
