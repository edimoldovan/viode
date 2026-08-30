//! GUI state and key grammar — pure logic, no GStreamer, no egui. Every
//! key becomes a state transition plus a list of player commands, which is
//! what the unit tests drive (the same reducer pattern as the TUI's app.rs).
//!
//! G1 is the read-only viewer, so the grammar is transport-only:
//!
//!   space      play/pause          J/K/L  shuttle (reverse/pause/forward)
//!   left/right seek ±1s            ,/.    frame step back/forward
//!   home/end   jump to start/end   ?      help    q  quit

use viode_core::Time;

const SEEK_STEP: u64 = 1_000_000_000; // 1s (shift+arrows)
const SMALL_STEP: u64 = 100_000_000; // 0.1s (arrows), like the TUI's h/l
const MAX_SHUTTLE: f64 = 8.0;

/// Commands for the player — the reducer never touches GStreamer itself.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Cmd {
    Seek(Time),
    Play,
    Pause,
    SetRate(f64),
}

/// Keys the viewer understands, already translated from the toolkit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Space,
    J,
    K,
    L,
    Left,
    Right,
    SmallLeft,
    SmallRight,
    Comma,
    Period,
    Home,
    End,
    Help,
    /// `[` — mark the range in-point at the playhead.
    MarkIn,
    /// `]` — mark the range out-point at the playhead.
    MarkOut,
    /// Clear both marks.
    ClearMarks,
}

pub struct State {
    pub playhead: Time,
    pub playing: bool,
    /// Shuttle rate: 1.0 = normal forward; negative = reverse.
    pub rate: f64,
    pub total: Time,
    pub fps: f64,
    pub show_help: bool,
    /// Marked range for range verbs (multicam take). `[` and `]` set the
    /// ends at the playhead, in either order.
    pub mark_in: Option<Time>,
    pub mark_out: Option<Time>,
}

impl State {
    pub fn new(total: Time, fps: f64) -> State {
        State {
            playhead: Time::ZERO,
            playing: false,
            rate: 1.0,
            total,
            fps: if fps > 0.0 { fps } else { 30.0 },
            show_help: false,
            mark_in: None,
            mark_out: None,
        }
    }

    /// The marked range, ordered, if both ends are set and distinct.
    pub fn marked_range(&self) -> Option<(Time, Time)> {
        let (a, b) = (self.mark_in?, self.mark_out?);
        if a == b {
            return None;
        }
        Some((a.min(b), a.max(b)))
    }

    /// One video frame in nanoseconds.
    pub fn frame(&self) -> Time {
        Time((1_000_000_000f64 / self.fps).round() as u64)
    }

    fn clamp(&self, t: Time) -> Time {
        if self.total == Time::ZERO {
            return Time::ZERO;
        }
        // Keep the playhead on the last frame, like the TUI does.
        let max = Time(self.total.0.saturating_sub(1));
        if t >= max {
            max
        } else {
            t
        }
    }

    /// An absolute seek (arrow keys, clicking/dragging the timeline).
    pub fn seek_to(&mut self, t: Time) -> Vec<Cmd> {
        self.playhead = self.clamp(t);
        vec![Cmd::Seek(self.playhead)]
    }

    /// While playing, the pipeline owns the playhead — the UI feeds the
    /// polled position back through here every frame.
    pub fn follow(&mut self, position: Time) {
        if self.playing {
            self.playhead = self.clamp(position);
        }
    }

    /// The pipeline reached the end.
    pub fn on_eos(&mut self) {
        self.playing = false;
        self.rate = 1.0;
        self.playhead = self.clamp(self.total);
    }

    pub fn on_key(&mut self, key: Key) -> Vec<Cmd> {
        match key {
            Key::Space => {
                if self.playing {
                    self.playing = false;
                    self.rate = 1.0;
                    vec![Cmd::Pause]
                } else {
                    self.playing = true;
                    self.rate = 1.0;
                    vec![Cmd::SetRate(1.0), Cmd::Play]
                }
            }
            Key::K => {
                self.playing = false;
                self.rate = 1.0;
                vec![Cmd::Pause]
            }
            Key::L => {
                // First press plays forward; repeats double up to 8x.
                self.rate = if !self.playing || self.rate <= 0.0 {
                    1.0
                } else {
                    (self.rate * 2.0).min(MAX_SHUTTLE)
                };
                self.playing = true;
                vec![Cmd::SetRate(self.rate), Cmd::Play]
            }
            Key::J => {
                self.rate = if !self.playing || self.rate >= 0.0 {
                    -1.0
                } else {
                    (self.rate * 2.0).max(-MAX_SHUTTLE)
                };
                self.playing = true;
                vec![Cmd::SetRate(self.rate), Cmd::Play]
            }
            Key::Left => {
                let t = Time(self.playhead.0.saturating_sub(SEEK_STEP));
                self.seek_to(t)
            }
            Key::Right => self.seek_to(Time(self.playhead.0 + SEEK_STEP)),
            Key::SmallLeft => {
                let t = Time(self.playhead.0.saturating_sub(SMALL_STEP));
                self.seek_to(t)
            }
            Key::SmallRight => self.seek_to(Time(self.playhead.0 + SMALL_STEP)),
            Key::Comma | Key::Period => {
                // Frame step always lands paused, like every NLE.
                let frame = self.frame();
                let t = if key == Key::Comma {
                    Time(self.playhead.0.saturating_sub(frame.0))
                } else {
                    Time(self.playhead.0 + frame.0)
                };
                self.playing = false;
                self.rate = 1.0;
                let mut cmds = vec![Cmd::Pause];
                cmds.extend(self.seek_to(t));
                cmds
            }
            Key::Home => self.seek_to(Time::ZERO),
            Key::End => self.seek_to(self.total),
            Key::Help => {
                self.show_help = !self.show_help;
                vec![]
            }
            Key::MarkIn => {
                self.mark_in = Some(self.playhead);
                vec![]
            }
            Key::MarkOut => {
                self.mark_out = Some(self.playhead);
                vec![]
            }
            Key::ClearMarks => {
                self.mark_in = None;
                self.mark_out = None;
                vec![]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> State {
        State::new(Time(10_000_000_000), 25.0) // 10s at 25fps
    }

    #[test]
    fn space_toggles_play_pause() {
        let mut s = state();
        assert_eq!(s.on_key(Key::Space), vec![Cmd::SetRate(1.0), Cmd::Play]);
        assert!(s.playing);
        assert_eq!(s.on_key(Key::Space), vec![Cmd::Pause]);
        assert!(!s.playing);
    }

    #[test]
    fn l_shuttles_up_to_8x_and_k_pauses() {
        let mut s = state();
        s.on_key(Key::L);
        assert_eq!(s.rate, 1.0);
        s.on_key(Key::L);
        assert_eq!(s.rate, 2.0);
        s.on_key(Key::L);
        s.on_key(Key::L);
        assert_eq!(s.rate, 8.0);
        s.on_key(Key::L);
        assert_eq!(s.rate, 8.0); // clamped
        assert_eq!(s.on_key(Key::K), vec![Cmd::Pause]);
        assert!(!s.playing);
        assert_eq!(s.rate, 1.0);
    }

    #[test]
    fn j_shuttles_in_reverse() {
        let mut s = state();
        s.on_key(Key::J);
        assert_eq!(s.rate, -1.0);
        assert!(s.playing);
        s.on_key(Key::J);
        assert_eq!(s.rate, -2.0);
        // Turning around resets to 1x forward.
        s.on_key(Key::L);
        assert_eq!(s.rate, 1.0);
    }

    #[test]
    fn arrows_seek_one_second_clamped() {
        let mut s = state();
        assert_eq!(s.on_key(Key::Left), vec![Cmd::Seek(Time::ZERO)]);
        s.on_key(Key::Right);
        assert_eq!(s.playhead, Time(1_000_000_000));
        for _ in 0..20 {
            s.on_key(Key::Right);
        }
        // Clamped to the last nanosecond before total.
        assert_eq!(s.playhead, Time(9_999_999_999));
    }

    #[test]
    fn frame_step_pauses_and_moves_one_frame() {
        let mut s = state();
        s.on_key(Key::Space);
        let cmds = s.on_key(Key::Period);
        assert!(!s.playing);
        assert_eq!(cmds[0], Cmd::Pause);
        assert_eq!(s.playhead, Time(40_000_000)); // 1/25s
        s.on_key(Key::Comma);
        assert_eq!(s.playhead, Time::ZERO);
    }

    #[test]
    fn home_end_jump() {
        let mut s = state();
        s.on_key(Key::End);
        assert_eq!(s.playhead, Time(9_999_999_999));
        s.on_key(Key::Home);
        assert_eq!(s.playhead, Time::ZERO);
    }

    #[test]
    fn follow_only_moves_while_playing() {
        let mut s = state();
        s.follow(Time(5_000_000_000));
        assert_eq!(s.playhead, Time::ZERO);
        s.on_key(Key::Space);
        s.follow(Time(5_000_000_000));
        assert_eq!(s.playhead, Time(5_000_000_000));
    }

    #[test]
    fn eos_stops_at_the_end() {
        let mut s = state();
        s.on_key(Key::Space);
        s.on_eos();
        assert!(!s.playing);
        assert_eq!(s.playhead, Time(9_999_999_999));
    }

    #[test]
    fn marks_set_order_and_clear() {
        let mut s = state();
        assert_eq!(s.marked_range(), None);
        s.on_key(Key::Right);
        s.on_key(Key::MarkOut); // out first, at 1s
        s.on_key(Key::Home);
        s.on_key(Key::MarkIn); // in second, at 0 — order still comes out sorted
        assert_eq!(s.marked_range(), Some((Time::ZERO, Time(1_000_000_000))));
        s.on_key(Key::ClearMarks);
        assert_eq!(s.marked_range(), None);
        // A zero-width range is no range.
        s.on_key(Key::MarkIn);
        s.on_key(Key::MarkOut);
        assert_eq!(s.marked_range(), None);
    }

    #[test]
    fn empty_timeline_never_moves() {
        let mut s = State::new(Time::ZERO, 30.0);
        s.on_key(Key::Right);
        s.on_key(Key::End);
        assert_eq!(s.playhead, Time::ZERO);
    }
}
