//! Captions: transcript segments (SOURCE time, per media file) mapped
//! through the main track into TIMELINE time, then delivered either as a
//! sidecar SRT or burned in as ordinary lower-third titles — the existing
//! title machinery renders them in preview and export alike, so there is
//! no subtitle-library dependency to install or check.

use std::path::Path;

use crate::model::{Project, Title};
use crate::time::Time;
use crate::transcript::Segment;

/// One caption in TIMELINE time.
#[derive(Debug, Clone, PartialEq)]
pub struct Caption {
    pub start: Time,
    pub end: Time,
    pub text: String,
}

/// Map one media file's transcript through every main-track clip that
/// uses it. Trims clip the text to what is actually visible, rates
/// rescale it, and clip order positions it. Results are sorted.
pub fn map_segments(project: &Project, src: &Path, segments: &[Segment]) -> Vec<Caption> {
    let mut captions = Vec::new();
    let mut clip_start = Time::ZERO;
    for clip in &project.main().clips {
        let len = clip.len();
        if clip.src == src {
            let rate = clip.rate.unwrap_or(1.0);
            for seg in segments {
                let vis_start = seg.start.max(clip.in_);
                let vis_end = seg.end.min(clip.out);
                if vis_start >= vis_end {
                    continue;
                }
                let to_timeline = |t: Time| -> Time {
                    clip_start + Time(((t - clip.in_).0 as f64 / rate) as u64)
                };
                let text = seg.text.trim();
                if text.is_empty() {
                    continue;
                }
                captions.push(Caption {
                    start: to_timeline(vis_start),
                    end: to_timeline(vis_end),
                    text: text.to_string(),
                });
            }
        }
        clip_start = clip_start + len;
    }
    captions.sort_by_key(|c| c.start.0);
    captions
}

/// Standard SRT text for a caption list.
pub fn to_srt(captions: &[Caption]) -> String {
    fn stamp(t: Time) -> String {
        let ms = t.0 / 1_000_000;
        format!(
            "{:02}:{:02}:{:02},{:03}",
            ms / 3_600_000,
            ms / 60_000 % 60,
            ms / 1_000 % 60,
            ms % 1_000
        )
    }
    let mut out = String::new();
    for (i, c) in captions.iter().enumerate() {
        out.push_str(&format!(
            "{}\n{} --> {}\n{}\n\n",
            i + 1,
            stamp(c.start),
            stamp(c.end),
            c.text
        ));
    }
    out
}

/// Burn captions in as lower-third titles on the project. Returns how
/// many were added. They are ordinary titles: styleable, deletable,
/// visible in the live preview, rendered by the existing title layers.
pub fn burn(project: &mut Project, captions: &[Caption]) -> usize {
    for c in captions {
        project.titles.push(Title {
            text: c.text.clone(),
            at: c.start,
            dur: c.end - c.start,
            font: Some("Sans Bold 36".into()),
            xpos: None,
            ypos: Some(0.88),
            color: None,
        });
    }
    captions.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Clip, Track, TrackKind};

    fn seg(start: f64, end: f64, text: &str) -> Segment {
        Segment {
            start: Time::from_secs_f64(start).unwrap(),
            end: Time::from_secs_f64(end).unwrap(),
            text: text.into(),
        }
    }

    fn t(secs: f64) -> Time {
        Time::from_secs_f64(secs).unwrap()
    }

    /// Two clips from the same source, the second trimmed and doubled in
    /// speed — the mapping must clip, offset, and rescale.
    #[test]
    fn segments_map_through_trims_order_and_rate() {
        let mut p = Project::new("c", 30.0, [320, 180]);
        let mut track = Track::new("main", TrackKind::Av);
        track.clips.push(Clip::media("media/a.mp4".into(), t(0.0), t(4.0)));
        let mut fast = Clip::media("media/a.mp4".into(), t(10.0), t(14.0));
        fast.rate = Some(2.0);
        track.clips.push(fast);
        p.tracks[0] = track;

        let segments = [
            seg(1.0, 2.0, "hello"),        // inside clip 0
            seg(3.5, 5.0, "cut short"),    // straddles clip 0's out point
            seg(11.0, 12.0, "fast part"),  // inside clip 1 (2x)
            seg(20.0, 21.0, "never used"), // outside every clip
        ];
        let caps = map_segments(&p, Path::new("media/a.mp4"), &segments);

        assert_eq!(caps.len(), 3);
        assert_eq!((caps[0].start, caps[0].end), (t(1.0), t(2.0)));
        assert_eq!((caps[1].start, caps[1].end), (t(3.5), t(4.0)), "clipped at the trim");
        // Clip 1 starts at timeline 4.0 + (11-10)/2 = 4.5, lasts 0.5s.
        assert_eq!((caps[2].start, caps[2].end), (t(4.5), t(5.0)));
        assert_eq!(caps[2].text, "fast part");
    }

    #[test]
    fn srt_output_is_standard() {
        let caps = vec![Caption { start: t(1.5), end: t(3.0), text: "hello".into() }];
        let srt = to_srt(&caps);
        assert_eq!(srt, "1\n00:00:01,500 --> 00:00:03,000\nhello\n\n");
    }

    #[test]
    fn burn_adds_lower_third_titles() {
        let mut p = Project::new("c", 30.0, [320, 180]);
        let caps = vec![
            Caption { start: t(0.0), end: t(1.0), text: "one".into() },
            Caption { start: t(1.0), end: t(2.0), text: "two".into() },
        ];
        assert_eq!(burn(&mut p, &caps), 2);
        assert_eq!(p.titles.len(), 2);
        assert_eq!(p.titles[0].ypos, Some(0.88));
        assert_eq!(p.titles[1].dur, t(1.0));
    }
}
