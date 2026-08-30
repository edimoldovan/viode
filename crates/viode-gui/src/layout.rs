//! Pure timeline geometry — time to pixels and back, ruler tick spacing.
//! No egui types beyond f32, so the unit tests need no toolkit.

use viode_core::{Project, Time};

/// The whole timeline fitted to the panel width (G1 has no zoom).
#[derive(Debug, Clone, Copy)]
pub struct TimelineMap {
    pub total: Time,
    pub left: f32,
    pub width: f32,
}

impl TimelineMap {
    pub fn new(total: Time, left: f32, width: f32) -> TimelineMap {
        TimelineMap {
            total,
            left,
            width: width.max(1.0),
        }
    }

    pub fn x_of(&self, t: Time) -> f32 {
        if self.total == Time::ZERO {
            return self.left;
        }
        self.left + (t.0 as f64 / self.total.0 as f64) as f32 * self.width
    }

    /// Inverse of x_of, clamped into the timeline.
    pub fn time_at(&self, x: f32) -> Time {
        let frac = ((x - self.left) / self.width).clamp(0.0, 1.0) as f64;
        Time((frac * self.total.0 as f64).round() as u64)
    }
}

/// Ruler tick step in seconds: the smallest "round" step keeping ticks at
/// least ~70px apart.
pub fn tick_step(total_secs: f64, width: f32) -> f64 {
    const STEPS: [f64; 10] = [0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0, 300.0];
    if total_secs <= 0.0 {
        return 1.0;
    }
    let px_per_sec = width as f64 / total_secs;
    for s in STEPS {
        if s * px_per_sec >= 70.0 {
            return s;
        }
    }
    600.0
}

/// End of everything visible: main sequence, overlay clips, titles.
pub fn timeline_end(project: &Project) -> Time {
    let mut end = project.total_duration();
    for track in project.tracks.iter().skip(1) {
        for clip in &track.clips {
            end = end.max(clip.span().1);
        }
    }
    for title in &project.titles {
        end = end.max(title.at + title.dur);
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_round_trips() {
        let m = TimelineMap::new(Time(10_000_000_000), 40.0, 1000.0);
        assert_eq!(m.x_of(Time::ZERO), 40.0);
        assert_eq!(m.x_of(Time(10_000_000_000)), 1040.0);
        let mid = Time(5_000_000_000);
        assert_eq!(m.time_at(m.x_of(mid)), mid);
        // Outside the panel clamps instead of extrapolating.
        assert_eq!(m.time_at(0.0), Time::ZERO);
        assert_eq!(m.time_at(9999.0), Time(10_000_000_000));
    }

    #[test]
    fn empty_timeline_is_inert() {
        let m = TimelineMap::new(Time::ZERO, 0.0, 800.0);
        assert_eq!(m.x_of(Time(123)), 0.0);
        assert_eq!(m.time_at(400.0), Time::ZERO);
    }

    #[test]
    fn ticks_stay_readable() {
        // 10s across 1000px -> 100px/s -> 1s ticks.
        assert_eq!(tick_step(10.0, 1000.0), 1.0);
        // 3 hours across 1200px -> big steps.
        assert_eq!(tick_step(3.0 * 3600.0, 1200.0), 600.0);
        // 2s across 1400px -> sub-second ticks.
        assert_eq!(tick_step(2.0, 1400.0), 0.1);
    }
}
