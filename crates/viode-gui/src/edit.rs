//! GUI edit state — the G2 reducer, pure logic like the TUI's app.rs and
//! this crate's state.rs. Owns the project, the undo/redo stacks, the
//! selection, and the dirty flag; every verb is a method the unit tests
//! drive with no egui and no GStreamer. The rendering layer only calls in
//! and draws what it finds.
//!
//! Three edit shapes, three snapshot disciplines:
//! - one-shot verbs (split, delete, move): snapshot, apply, roll back on
//!   error — exactly the TUI's pattern;
//! - staged edits (inspector sliders): ONE snapshot per gesture, however
//!   many times the value changes before end_stage();
//! - drags (trim/move/roll/slip/slide with the mouse): snapshot at
//!   drag_begin, then every motion re-applies the TOTAL delta to a copy of
//!   the original — impossible trims keep the last good state instead of
//!   erroring mid-drag.

use std::path::Path;

use viode_core::{ops, ColorGrade, Keyframe, Project, Time, Title};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selection {
    None,
    Clip { track: usize, index: usize },
    Title(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragKind {
    /// Drag a clip's left edge: source in-point (overlays also keep their
    /// right edge anchored by shifting `at`).
    TrimIn,
    /// Drag a clip's right edge: source out-point.
    TrimOut,
    /// Drag a clip body: reorder on the main track, reposition (`at`) on
    /// overlays.
    Move,
    /// Alt-drag an edge on the main track: move the cut between two clips
    /// (`index` is the ops boundary index — the right-hand clip).
    Roll,
    /// Alt-drag a clip body: shift source content under a fixed slot.
    Slip,
    /// Shift+alt-drag a clip body: move the slot by trimming neighbours.
    Slide,
}

struct Drag {
    kind: DragKind,
    track: usize,
    index: usize,
    orig: Project,
    good: Project,
}

pub struct Editor {
    pub project: Project,
    pub selection: Selection,
    pub dirty: bool,
    pub message: String,
    undo: Vec<Project>,
    redo: Vec<Project>,
    staging: bool,
    drag: Option<Drag>,
}

impl Editor {
    pub fn new(project: Project) -> Editor {
        Editor {
            project,
            selection: Selection::None,
            dirty: false,
            message: String::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            staging: false,
            drag: None,
        }
    }

    /// Another process rewrote the file and we hold no local edits: adopt
    /// the new project wholesale. Old undo states describe a timeline that
    /// no longer exists, so the stacks clear.
    pub fn replace_project(&mut self, project: Project) {
        self.project = project;
        self.undo.clear();
        self.redo.clear();
        self.selection = Selection::None;
        self.dirty = false;
        self.staging = false;
        self.drag = None;
    }

    pub fn save(&mut self, path: &Path) -> bool {
        match self.project.save(path) {
            Ok(()) => {
                self.dirty = false;
                self.message = format!("saved {}", path.display());
                true
            }
            Err(e) => {
                self.message = format!("save failed: {e}");
                false
            }
        }
    }

    fn snapshot(&mut self) {
        self.undo.push(self.project.clone());
        self.redo.clear();
    }

    fn rollback(&mut self, err: impl std::fmt::Display) {
        self.project = self.undo.pop().expect("rollback without snapshot");
        self.message = err.to_string();
    }

    pub fn undo(&mut self) -> bool {
        let Some(prev) = self.undo.pop() else {
            self.message = "nothing to undo".into();
            return false;
        };
        self.redo.push(std::mem::replace(&mut self.project, prev));
        self.selection = Selection::None;
        self.dirty = true;
        self.message = "undo".into();
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else {
            self.message = "nothing to redo".into();
            return false;
        };
        self.undo.push(std::mem::replace(&mut self.project, next));
        self.selection = Selection::None;
        self.dirty = true;
        self.message = "redo".into();
        true
    }

    // -- selection ---------------------------------------------------------

    pub fn select_clip(&mut self, track: usize, index: usize) {
        self.selection = Selection::Clip { track, index };
    }

    pub fn select_title(&mut self, index: usize) {
        self.selection = Selection::Title(index);
    }

    pub fn deselect(&mut self) {
        self.selection = Selection::None;
    }

    pub fn selected_clip(&self) -> Option<(usize, usize)> {
        match self.selection {
            Selection::Clip { track, index }
                if self
                    .project
                    .tracks
                    .get(track)
                    .is_some_and(|t| index < t.clips.len()) =>
            {
                Some((track, index))
            }
            _ => None,
        }
    }

    pub fn selected_title(&self) -> Option<usize> {
        match self.selection {
            Selection::Title(i) if i < self.project.titles.len() => Some(i),
            _ => None,
        }
    }

    // -- playhead verbs (TUI semantics: the playhead is the selection) -----

    pub fn split(&mut self, playhead: Time) -> bool {
        let Some((index, src_time)) = ops::source_at(&self.project, playhead) else {
            self.message = "nothing under playhead".into();
            return false;
        };
        let offset = src_time - self.project.main().clips[index].in_;
        self.snapshot();
        match ops::split(self.project.main_mut(), index, offset) {
            Ok(()) => {
                self.dirty = true;
                self.message = format!("split clip {index} at {playhead}");
                true
            }
            Err(e) => {
                self.rollback(e);
                false
            }
        }
    }

    pub fn trim_to_playhead(&mut self, start: bool, playhead: Time) -> bool {
        let Some((index, src_time)) = ops::source_at(&self.project, playhead) else {
            self.message = "nothing under playhead".into();
            return false;
        };
        self.snapshot();
        let (in_, out) = if start {
            (Some(src_time), None)
        } else {
            (None, Some(src_time))
        };
        match ops::trim(self.project.main_mut(), index, in_, out) {
            Ok(()) => {
                self.dirty = true;
                self.message = format!(
                    "clip {index} {} set to {src_time}",
                    if start { "in" } else { "out" }
                );
                true
            }
            Err(e) => {
                self.rollback(e);
                false
            }
        }
    }

    /// Delete the selection if there is one, else the main clip under the
    /// playhead.
    pub fn delete(&mut self, playhead: Time) -> bool {
        if let Some(i) = self.selected_title() {
            self.snapshot();
            let t = self.project.titles.remove(i);
            self.dirty = true;
            self.selection = Selection::None;
            self.message = format!("deleted title {:?}", t.text);
            return true;
        }
        let (track, index) = match self.selected_clip() {
            Some(sel) => sel,
            None => match ops::source_at(&self.project, playhead) {
                Some((i, _)) => (0, i),
                None => {
                    self.message = "nothing selected or under playhead".into();
                    return false;
                }
            },
        };
        self.snapshot();
        match ops::remove(&mut self.project.tracks[track], index) {
            Ok(clip) => {
                self.dirty = true;
                self.selection = Selection::None;
                self.message = format!("deleted [{index}] {}", clip.src.display());
                true
            }
            Err(e) => {
                self.rollback(e);
                false
            }
        }
    }

    /// Move the selected main-track clip (or the one under the playhead)
    /// one slot left/right.
    pub fn shift(&mut self, playhead: Time, dir: i64) -> bool {
        let index = match self.selected_clip() {
            Some((0, i)) => i,
            Some(_) => {
                self.message = "only main-track clips reorder (overlays drag by position)".into();
                return false;
            }
            None => match ops::source_at(&self.project, playhead) {
                Some((i, _)) => i,
                None => {
                    self.message = "nothing under playhead".into();
                    return false;
                }
            },
        };
        let to = index as i64 + dir;
        if to < 0 || to >= self.project.main().clips.len() as i64 {
            self.message = "already at the end".into();
            return false;
        }
        self.snapshot();
        match ops::move_clip(self.project.main_mut(), index, to as usize) {
            Ok(()) => {
                self.dirty = true;
                self.selection = Selection::Clip { track: 0, index: to as usize };
                self.message = format!("moved clip {index} -> {to}");
                true
            }
            Err(e) => {
                self.rollback(e);
                false
            }
        }
    }

    // -- staged edits (inspector widgets) ----------------------------------

    /// First change of a gesture takes the undo snapshot; the rest ride it.
    fn stage(&mut self) {
        if !self.staging {
            self.snapshot();
            self.staging = true;
        }
        self.dirty = true;
    }

    /// The gesture ended (pointer released / widget lost focus).
    pub fn end_stage(&mut self) {
        self.staging = false;
    }

    fn clip_mut(&mut self) -> Option<&mut viode_core::Clip> {
        let (t, i) = self.selected_clip()?;
        Some(&mut self.project.tracks[t].clips[i])
    }

    pub fn set_rate(&mut self, rate: f64) -> bool {
        if rate <= 0.0 || rate > 20.0 || self.selected_clip().is_none() {
            return false;
        }
        self.stage();
        self.clip_mut().unwrap().rate = (rate != 1.0).then_some(rate);
        true
    }

    /// Stabilization on the selected clip (None clears).
    pub fn set_steady(&mut self, smoothing: Option<u32>) -> bool {
        if smoothing.is_some_and(|s| !(1..=100).contains(&s)) || self.selected_clip().is_none() {
            return false;
        }
        self.stage();
        self.clip_mut().unwrap().steady = smoothing;
        true
    }

    /// Audio cleanup on the selected clip (None clears).
    pub fn set_clean(&mut self, strength: Option<f64>) -> bool {
        if strength.is_some_and(|v| !(0.01..=97.0).contains(&v)) || self.selected_clip().is_none()
        {
            return false;
        }
        self.stage();
        self.clip_mut().unwrap().clean = strength;
        true
    }

    /// Refit the selected overlay clip to a target duration.
    pub fn refit_selected(&mut self, project_dir: &Path, target: Time, fade: Time) -> bool {
        let Some((track, index)) = self.selected_clip() else {
            self.message = "select a music clip to refit".into();
            return false;
        };
        self.snapshot();
        match viode_core::refit::refit(&mut self.project, project_dir, track, index, target, fade)
        {
            Ok(_) => {
                self.dirty = true;
                self.selection = Selection::None;
                self.message = format!("refit to {target} with a crossfaded seam");
                true
            }
            Err(e) => {
                self.rollback(e);
                false
            }
        }
    }

    /// Duck one overlay track against a precomputed timeline speech mask
    /// (the mask comes from a worker thread; applying keys is instant).
    pub fn duck_track(&mut self, track: usize, mask: &[(Time, Time)]) -> bool {
        if track == 0 || track >= self.project.tracks.len() || mask.is_empty() {
            self.message = "nothing to duck".into();
            return false;
        }
        self.snapshot();
        let opts = viode_core::duck::DuckOptions::default();
        let positions: Vec<Time> = self.project.tracks[track]
            .clips
            .iter()
            .map(|c| c.at.unwrap_or(Time::ZERO))
            .collect();
        for (clip, at) in self.project.tracks[track].clips.iter_mut().zip(positions) {
            clip.keys.retain(|k| k.prop != "volume");
            let mut keys = viode_core::duck::keys_for_clip(at, clip, mask, &opts);
            clip.keys.append(&mut keys);
            clip.keys.sort_by_key(|k| k.at.0);
        }
        self.dirty = true;
        self.message = format!("ducked track {track} under {} speech window(s)", mask.len());
        true
    }

    /// Speed-ramp the selected MAIN-track clip (stepped time remapping).
    pub fn ramp(&mut self, from: f64, to: f64, steps: usize) -> bool {
        let Some((track, index)) = self.selected_clip() else {
            self.message = "select a clip to ramp".into();
            return false;
        };
        if track != 0 {
            self.message = "ramp applies to main-track clips".into();
            return false;
        }
        self.snapshot();
        match ops::ramp(self.project.main_mut(), index, from, to, steps) {
            Ok(()) => {
                self.dirty = true;
                self.selection = Selection::None;
                self.message = format!("ramped clip {index} {from}x -> {to}x in {steps} steps");
                true
            }
            Err(e) => {
                self.rollback(e);
                false
            }
        }
    }

    /// Drop a marker at the playhead. Text is editable in the file (or
    /// via the CLI); the GUI ruler shows and removes them.
    pub fn marker_add(&mut self, at: Time) -> bool {
        self.snapshot();
        let n = self.project.markers.len();
        self.project.markers.push(viode_core::Marker {
            at,
            text: format!("marker {n}"),
            color: None,
        });
        self.project.markers.sort_by_key(|m| m.at.0);
        self.dirty = true;
        self.message = format!("marker at {at}");
        true
    }

    pub fn marker_remove(&mut self, index: usize) -> bool {
        if index >= self.project.markers.len() {
            return false;
        }
        self.snapshot();
        let m = self.project.markers.remove(index);
        self.dirty = true;
        self.message = format!("removed marker {:?}", m.text);
        true
    }

    /// Frame hold at the playhead (the still is generated by ffmpeg).
    pub fn freeze(&mut self, project_dir: &Path, playhead: Time, dur: Time) -> bool {
        self.snapshot();
        match viode_core::freeze::freeze_at(&mut self.project, project_dir, playhead, dur) {
            Ok(i) => {
                self.dirty = true;
                self.message = format!("froze frame at {playhead} for {dur} (clip {i})");
                true
            }
            Err(e) => {
                self.rollback(e);
                false
            }
        }
    }

    /// Burn a caption list in as lower-third titles.
    pub fn captions_burn(&mut self, captions: &[viode_core::captions::Caption]) -> bool {
        if captions.is_empty() {
            self.message = "no speech found — nothing to caption".into();
            return false;
        }
        self.snapshot();
        let n = viode_core::captions::burn(&mut self.project, captions);
        self.dirty = true;
        self.message = format!("burned {n} captions in as titles");
        true
    }

    pub fn set_volume(&mut self, v: f64) -> bool {
        if !(0.0..=10.0).contains(&v) || self.selected_clip().is_none() {
            return false;
        }
        self.stage();
        self.clip_mut().unwrap().volume = (v != 1.0).then_some(v);
        true
    }

    pub fn set_pan(&mut self, v: f64) -> bool {
        if !(-1.0..=1.0).contains(&v) || self.selected_clip().is_none() {
            return false;
        }
        self.stage();
        self.clip_mut().unwrap().pan = (v != 0.0).then_some(v);
        true
    }

    pub fn set_pos(&mut self, x: f64, y: f64) -> bool {
        if self.selected_clip().is_none() {
            return false;
        }
        self.stage();
        self.clip_mut().unwrap().pos = ((x, y) != (0.0, 0.0)).then_some([x, y]);
        true
    }

    pub fn set_scale(&mut self, s: f64) -> bool {
        if s <= 0.0 || self.selected_clip().is_none() {
            return false;
        }
        self.stage();
        self.clip_mut().unwrap().scale = (s != 1.0).then_some(s);
        true
    }

    pub fn set_rotate(&mut self, deg: f64) -> bool {
        if self.selected_clip().is_none() {
            return false;
        }
        self.stage();
        self.clip_mut().unwrap().rotate = (deg != 0.0).then_some(deg);
        true
    }

    pub fn set_opacity(&mut self, o: f64) -> bool {
        if !(0.0..=1.0).contains(&o) || self.selected_clip().is_none() {
            return false;
        }
        self.stage();
        self.clip_mut().unwrap().opacity = (o != 1.0).then_some(o);
        true
    }

    pub fn clear_place(&mut self) -> bool {
        if self.selected_clip().is_none() {
            return false;
        }
        self.stage();
        let c = self.clip_mut().unwrap();
        (c.pos, c.scale, c.rotate, c.opacity) = (None, None, None, None);
        self.end_stage();
        true
    }

    /// One videobalance field; neutral values drop back to None so the
    /// project file stays clean (mirrors the CLI).
    pub fn set_grade(&mut self, field: &str, v: f64) -> bool {
        if self.selected_clip().is_none() {
            return false;
        }
        self.stage();
        let c = self.clip_mut().unwrap();
        let mut g = c.color.clone().unwrap_or(ColorGrade {
            brightness: None,
            contrast: None,
            saturation: None,
            hue: None,
        });
        let (slot, neutral) = match field {
            "brightness" => (&mut g.brightness, 0.0),
            "contrast" => (&mut g.contrast, 1.0),
            "saturation" => (&mut g.saturation, 1.0),
            "hue" => (&mut g.hue, 0.0),
            _ => return false,
        };
        *slot = (v != neutral).then_some(v);
        c.color = (g.brightness.is_some()
            || g.contrast.is_some()
            || g.saturation.is_some()
            || g.hue.is_some())
        .then_some(g);
        true
    }

    pub fn clear_color(&mut self) -> bool {
        if self.selected_clip().is_none() {
            return false;
        }
        self.stage();
        let c = self.clip_mut().unwrap();
        c.color = None;
        c.lut = None;
        self.end_stage();
        true
    }

    /// Crossfade with the previous clip (main track, index > 0 only).
    /// Staged like the other inspector edits; set_transition validates
    /// before it mutates, so a refused duration changes nothing.
    pub fn set_fade(&mut self, duration: Option<Time>, kind: Option<String>) -> bool {
        let Some((0, index)) = self.selected_clip() else {
            self.message = "fades live on main-track clips".into();
            return false;
        };
        self.stage();
        match ops::set_transition(self.project.main_mut(), index, duration) {
            Ok(()) => {
                self.project.main_mut().clips[index].transition_kind =
                    kind.filter(|_| duration.is_some());
                true
            }
            Err(e) => {
                self.message = e.to_string();
                false
            }
        }
    }

    pub fn key_add(&mut self, prop: &str, at: Time, value: f64) -> bool {
        if (prop != "volume" && prop != "alpha") || value < 0.0 {
            self.message = "keyframes: volume or alpha, value >= 0".into();
            return false;
        }
        if self.selected_clip().is_none() {
            return false;
        }
        self.snapshot();
        let c = self.clip_mut().unwrap();
        c.keys.push(Keyframe { prop: prop.into(), at, value });
        c.keys
            .sort_by(|a, b| (a.prop.clone(), a.at).cmp(&(b.prop.clone(), b.at)));
        self.dirty = true;
        true
    }

    pub fn key_remove(&mut self, k: usize) -> bool {
        let Some((t, i)) = self.selected_clip() else {
            return false;
        };
        if k >= self.project.tracks[t].clips[i].keys.len() {
            return false;
        }
        self.snapshot();
        self.project.tracks[t].clips[i].keys.remove(k);
        self.dirty = true;
        true
    }

    // -- the pro surface (G3) ----------------------------------------------

    /// Multicam take: replace `start..end` of the main track with the
    /// synced footage from angle track `track` — CLI semantics exactly,
    /// including the coverage check.
    pub fn take(&mut self, track: usize, start: Time, end: Time) -> bool {
        if track == 0 || track >= self.project.tracks.len() {
            self.message = "take copies FROM an angle track (1+)".into();
            return false;
        }
        if start >= end {
            self.message = "mark a range first ([ and ], or select a clip)".into();
            return false;
        }
        let Some(clip) = self.project.tracks[track].clips.first().cloned() else {
            self.message = format!("track {track} has no clip");
            return false;
        };
        let (a_start, a_end) = clip.span();
        if start < a_start || end > a_end {
            self.message = format!("angle {track} only covers {a_start}..{a_end}");
            return false;
        }
        let mut take = clip.clone();
        take.in_ = clip.in_ + (start - a_start);
        take.out = take.in_ + (end - start);
        self.snapshot();
        match ops::replace_range(self.project.main_mut(), start, end, take) {
            Ok(()) => {
                self.dirty = true;
                self.selection = Selection::None;
                self.message = format!("took {start}..{end} from track {track}");
                true
            }
            Err(e) => {
                self.rollback(e);
                false
            }
        }
    }

    /// Cut transcript segments (SOURCE-time ranges) out of a main clip —
    /// text-based editing, same op the CLI's cut-text uses.
    pub fn cut_segments(&mut self, index: usize, ranges: &[(Time, Time)], pad: Time) -> bool {
        if ranges.is_empty() {
            return false;
        }
        self.snapshot();
        match ops::remove_source_ranges(self.project.main_mut(), index, ranges, pad) {
            Ok(stats) => {
                self.dirty = true;
                self.selection = Selection::None;
                self.message = format!(
                    "cut {} ({} segments kept)",
                    stats.removed, stats.segments_kept
                );
                true
            }
            Err(e) => {
                self.rollback(e);
                false
            }
        }
    }

    /// Reconnect missing media by filename under `dir` (recursive).
    pub fn relink(&mut self, project_dir: &Path, search_dir: &Path) -> bool {
        self.snapshot();
        let n = viode_core::media::relink(&mut self.project, project_dir, search_dir);
        if n == 0 {
            self.undo.pop();
            self.message = format!("nothing relinked under {}", search_dir.display());
            return false;
        }
        self.dirty = true;
        self.message = format!("relinked {n} file(s)");
        true
    }

    // -- titles ------------------------------------------------------------

    pub fn title_add(&mut self, at: Time) -> bool {
        self.snapshot();
        self.project.titles.push(Title {
            text: "Title".into(),
            at,
            dur: Time(2_000_000_000),
            font: None,
            xpos: None,
            ypos: None,
            color: None,
        });
        self.dirty = true;
        self.selection = Selection::Title(self.project.titles.len() - 1);
        true
    }

    pub fn title_edit(&mut self, f: impl FnOnce(&mut Title)) -> bool {
        let Some(i) = self.selected_title() else {
            return false;
        };
        self.stage();
        f(&mut self.project.titles[i]);
        true
    }

    // -- mouse drags -------------------------------------------------------

    pub fn drag_begin(&mut self, kind: DragKind, track: usize, index: usize) {
        self.snapshot();
        self.drag = Some(Drag {
            kind,
            track,
            index,
            orig: self.project.clone(),
            good: self.project.clone(),
        });
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Re-apply the drag at a new TOTAL timeline delta (ns from the drag
    /// start). `drop_index` is the main-track reorder target for Move.
    pub fn drag_update(&mut self, delta_ns: i64, drop_index: Option<usize>) {
        let Some(drag) = &mut self.drag else {
            return;
        };
        let mut p = drag.orig.clone();
        match apply_drag(&mut p, drag.kind, drag.track, drag.index, delta_ns, drop_index) {
            Ok(selection) => {
                drag.good = p.clone();
                self.project = p;
                self.dirty = true;
                self.selection = selection;
            }
            // An impossible position mid-drag holds the last good state.
            Err(_) => self.project = drag.good.clone(),
        }
    }

    pub fn drag_end(&mut self) {
        if let Some(drag) = self.drag.take() {
            if self.project == drag.orig {
                // Nothing actually moved: drop the provisional undo point.
                self.undo.pop();
            }
        }
    }
}

fn shift_time(t: Time, d: i64) -> Result<Time, ops::OpError> {
    let v = t.0 as i64 + d;
    if v < 0 {
        return Err(ops::OpError::BadRange(Time::ZERO, t));
    }
    Ok(Time(v as u64))
}

fn apply_drag(
    p: &mut Project,
    kind: DragKind,
    track: usize,
    index: usize,
    delta_ns: i64,
    drop_index: Option<usize>,
) -> Result<Selection, ops::OpError> {
    let sel = Selection::Clip { track, index };
    let rate = p.tracks[track].clips[index].rate.unwrap_or(1.0);
    let src_delta = (delta_ns as f64 * rate).round() as i64;
    match kind {
        DragKind::TrimIn => {
            let clip = &p.tracks[track].clips[index];
            let mut delta_ns = delta_ns;
            let mut src_delta = src_delta;
            if let Some(at) = clip.at {
                // Overlay: the right edge stays put, so `at` absorbs the
                // timeline delta — clamped at the timeline origin.
                let eff = delta_ns.max(-(at.0 as i64));
                if eff != delta_ns {
                    delta_ns = eff;
                    src_delta = (delta_ns as f64 * rate).round() as i64;
                }
                p.tracks[track].clips[index].at = Some(shift_time(at, delta_ns)?);
            }
            let new_in = shift_time(p.tracks[track].clips[index].in_, src_delta)?;
            ops::trim(&mut p.tracks[track], index, Some(new_in), None)?;
            Ok(sel)
        }
        DragKind::TrimOut => {
            let new_out = shift_time(p.tracks[track].clips[index].out, src_delta)?;
            ops::trim(&mut p.tracks[track], index, None, Some(new_out))?;
            Ok(sel)
        }
        DragKind::Move => {
            if track == 0 {
                let to = drop_index.unwrap_or(index);
                if to != index {
                    ops::move_clip(&mut p.tracks[0], index, to)?;
                }
                Ok(Selection::Clip { track: 0, index: drop_index.unwrap_or(index) })
            } else {
                let at = p.tracks[track].clips[index].at.unwrap_or(Time::ZERO);
                let eff = delta_ns.max(-(at.0 as i64));
                p.tracks[track].clips[index].at = Some(shift_time(at, eff)?);
                Ok(sel)
            }
        }
        DragKind::Roll => {
            ops::roll(&mut p.tracks[track], index, delta_ns)?;
            Ok(sel)
        }
        DragKind::Slip => {
            ops::slip(&mut p.tracks[track], index, delta_ns)?;
            Ok(sel)
        }
        DragKind::Slide => {
            ops::slide(&mut p.tracks[track], index, delta_ns)?;
            Ok(sel)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use viode_core::{Clip, Track, TrackKind};

    const SEC: u64 = 1_000_000_000;

    fn clip(in_s: u64, out_s: u64) -> Clip {
        Clip::media("media/a.mp4".into(), Time(in_s * SEC), Time(out_s * SEC))
    }

    /// 3 main clips of 2s each, one overlay at 1s, one title.
    fn editor() -> Editor {
        let mut p = Project::new("t", 30.0, [640, 360]);
        for _ in 0..3 {
            p.main_mut().clips.push(clip(0, 2));
        }
        let mut overlay = Track::new("pip", TrackKind::Video);
        let mut oc = clip(0, 2);
        oc.at = Some(Time(SEC));
        overlay.clips.push(oc);
        p.tracks.push(overlay);
        p.titles.push(Title {
            text: "T".into(),
            at: Time::ZERO,
            dur: Time(SEC),
            font: None,
            xpos: None,
            ypos: None,
            color: None,
        });
        Editor::new(p)
    }

    #[test]
    fn split_at_playhead_and_undo() {
        let mut e = editor();
        assert!(e.split(Time(SEC)));
        assert_eq!(e.project.main().clips.len(), 4);
        assert!(e.dirty);
        assert!(e.undo());
        assert_eq!(e.project.main().clips.len(), 3);
        assert!(e.redo());
        assert_eq!(e.project.main().clips.len(), 4);
    }

    #[test]
    fn failed_split_rolls_back_cleanly() {
        let mut e = editor();
        assert!(!e.split(Time::ZERO)); // exactly on a boundary: BadSplit
        assert_eq!(e.project.main().clips.len(), 3);
        assert!(!e.undo()); // no stray undo point
    }

    #[test]
    fn trim_to_playhead_sets_source_points() {
        let mut e = editor();
        assert!(e.trim_to_playhead(true, Time(SEC / 2)));
        assert_eq!(e.project.main().clips[0].in_, Time(SEC / 2));
        assert!(e.trim_to_playhead(false, Time(SEC)));
        // Playhead 1s is 1s into clip 0, which now starts at source 0.5 —
        // so the out point lands at source 1.5.
        assert_eq!(e.project.main().clips[0].out, Time(SEC + SEC / 2));
    }

    #[test]
    fn delete_prefers_selection_over_playhead() {
        let mut e = editor();
        e.select_clip(1, 0);
        assert!(e.delete(Time::ZERO));
        assert_eq!(e.project.tracks[1].clips.len(), 0);
        assert_eq!(e.project.main().clips.len(), 3);
        // No selection: playhead rules, like the TUI.
        assert!(e.delete(Time(3 * SEC)));
        assert_eq!(e.project.main().clips.len(), 2);
    }

    #[test]
    fn delete_selected_title() {
        let mut e = editor();
        e.select_title(0);
        assert!(e.delete(Time::ZERO));
        assert!(e.project.titles.is_empty());
    }

    #[test]
    fn shift_reorders_and_follows_selection() {
        let mut e = editor();
        e.project.main_mut().clips[0].label = Some("first".into());
        e.select_clip(0, 0);
        assert!(e.shift(Time::ZERO, 1));
        assert_eq!(e.project.main().clips[1].label.as_deref(), Some("first"));
        assert_eq!(e.selected_clip(), Some((0, 1)));
        assert!(!e.shift(Time::ZERO, 5)); // out of range refused
    }

    #[test]
    fn staged_slider_gesture_is_one_undo_step() {
        let mut e = editor();
        e.select_clip(0, 0);
        assert!(e.set_volume(1.2));
        assert!(e.set_volume(1.5));
        assert!(e.set_volume(1.8));
        e.end_stage();
        assert_eq!(e.project.main().clips[0].volume, Some(1.8));
        assert!(e.undo());
        assert_eq!(e.project.main().clips[0].volume, None);
        assert!(!e.undo()); // one gesture, one step
    }

    #[test]
    fn setters_normalize_neutral_values_like_the_cli() {
        let mut e = editor();
        e.select_clip(0, 0);
        e.set_volume(1.0);
        assert_eq!(e.project.main().clips[0].volume, None);
        e.set_rate(2.0);
        assert_eq!(e.project.main().clips[0].rate, Some(2.0));
        e.set_rate(1.0);
        assert_eq!(e.project.main().clips[0].rate, None);
        e.set_grade("saturation", 0.0);
        assert!(e.project.main().clips[0].color.is_some());
        e.set_grade("saturation", 1.0);
        assert!(e.project.main().clips[0].color.is_none());
        assert!(!e.set_pan(2.0)); // out of range refused
    }

    #[test]
    fn fade_only_on_later_main_clips() {
        let mut e = editor();
        e.select_clip(0, 1);
        assert!(e.set_fade(Some(Time(SEC / 2)), Some("bar-wipe-lr".into())));
        assert_eq!(e.project.main().clips[1].transition, Some(Time(SEC / 2)));
        assert_eq!(
            e.project.main().clips[1].transition_kind.as_deref(),
            Some("bar-wipe-lr")
        );
        e.select_clip(0, 0);
        assert!(!e.set_fade(Some(Time(SEC / 2)), None)); // first clip refused
    }

    #[test]
    fn keyframes_add_sorted_and_remove() {
        let mut e = editor();
        e.select_clip(0, 0);
        assert!(e.key_add("volume", Time(SEC), 0.0));
        assert!(e.key_add("volume", Time::ZERO, 1.0));
        assert_eq!(e.project.main().clips[0].keys[0].at, Time::ZERO);
        assert!(!e.key_add("zoom", Time::ZERO, 1.0)); // unknown prop
        assert!(e.key_remove(0));
        assert_eq!(e.project.main().clips[0].keys.len(), 1);
    }

    #[test]
    fn titles_add_select_edit() {
        let mut e = editor();
        assert!(e.title_add(Time(SEC)));
        assert_eq!(e.selected_title(), Some(1));
        assert!(e.title_edit(|t| t.text = "Hello".into()));
        e.end_stage();
        assert_eq!(e.project.titles[1].text, "Hello");
    }

    #[test]
    fn drag_trim_out_is_total_delta_and_rate_aware() {
        let mut e = editor();
        e.project.main_mut().clips[0].rate = Some(2.0); // 2s source = 1s timeline
        e.drag_begin(DragKind::TrimOut, 0, 0);
        e.drag_update(-(SEC as i64) / 4, None); // -0.25s timeline = -0.5s source
        e.drag_update(-(SEC as i64) / 2, None); // total, not cumulative
        e.drag_end();
        assert_eq!(e.project.main().clips[0].out, Time(SEC));
        assert!(e.undo());
        assert_eq!(e.project.main().clips[0].out, Time(2 * SEC));
    }

    #[test]
    fn drag_past_impossible_holds_last_good() {
        let mut e = editor();
        e.drag_begin(DragKind::TrimOut, 0, 0);
        e.drag_update(-(SEC as i64), None); // out = in+1s: still valid
        e.drag_update(-(3 * SEC as i64), None); // out < in: refused
        e.drag_end();
        assert_eq!(e.project.main().clips[0].out, Time(SEC));
    }

    #[test]
    fn drag_move_reorders_main_track() {
        let mut e = editor();
        e.project.main_mut().clips[2].label = Some("last".into());
        e.drag_begin(DragKind::Move, 0, 2);
        e.drag_update(0, Some(0));
        e.drag_end();
        assert_eq!(e.project.main().clips[0].label.as_deref(), Some("last"));
        assert_eq!(e.selected_clip(), Some((0, 0)));
    }

    #[test]
    fn drag_move_overlay_shifts_at_with_floor() {
        let mut e = editor();
        e.drag_begin(DragKind::Move, 1, 0);
        e.drag_update(-(5 * SEC as i64), None); // way left: clamps at 0
        e.drag_end();
        assert_eq!(e.project.tracks[1].clips[0].at, Some(Time::ZERO));
    }

    #[test]
    fn overlay_trim_in_keeps_right_edge_anchored() {
        let mut e = editor();
        e.drag_begin(DragKind::TrimIn, 1, 0);
        e.drag_update(SEC as i64 / 2, None);
        e.drag_end();
        let c = &e.project.tracks[1].clips[0];
        assert_eq!(c.at, Some(Time(SEC + SEC / 2)));
        assert_eq!(c.in_, Time(SEC / 2));
        // Timeline end of the overlay is unchanged: at + len == 3s.
        assert_eq!(c.span().1, Time(3 * SEC));
    }

    #[test]
    fn noop_drag_leaves_no_undo_point() {
        let mut e = editor();
        e.drag_begin(DragKind::Move, 0, 1);
        e.drag_update(0, Some(1));
        e.drag_end();
        assert!(!e.undo());
    }

    #[test]
    fn roll_moves_the_boundary() {
        let mut e = editor();
        e.drag_begin(DragKind::Roll, 0, 1);
        e.drag_update(SEC as i64 / 2, None);
        e.drag_end();
        assert_eq!(e.project.main().clips[0].out, Time(2 * SEC + SEC / 2));
        assert_eq!(e.project.main().clips[1].in_, Time(SEC / 2));
        assert_eq!(e.project.total_duration(), Time(6 * SEC));
    }

    #[test]
    fn take_swaps_range_with_angle_footage() {
        let mut e = editor();
        // The overlay at track 1 covers 1s..3s; treat it as the angle.
        assert!(e.take(1, Time(SEC), Time(2 * SEC)));
        // Total duration is preserved (that is replace_range's contract).
        assert_eq!(e.project.total_duration(), Time(6 * SEC));
        // The taken piece plays the angle's source for that second.
        let main = e.project.main();
        let piece = main
            .clips
            .iter()
            .find(|c| c.src == std::path::PathBuf::from("media/a.mp4") && c.in_ == Time::ZERO && c.src_len() == Time(SEC))
            .expect("the taken clip exists");
        assert_eq!(piece.src_len(), Time(SEC));
        assert!(e.undo());
        assert_eq!(e.project.main().clips.len(), 3);
    }

    #[test]
    fn take_refuses_bad_track_and_uncovered_range() {
        let mut e = editor();
        assert!(!e.take(0, Time::ZERO, Time(SEC))); // main track refused
        assert!(!e.take(9, Time::ZERO, Time(SEC))); // no such track
        assert!(!e.take(1, Time::ZERO, Time(SEC))); // angle starts at 1s
        assert!(!e.take(1, Time(2 * SEC), Time(2 * SEC))); // empty range
        assert!(!e.undo()); // no stray undo points
    }

    #[test]
    fn cut_segments_removes_source_ranges() {
        let mut e = editor();
        let ranges = [(Time(SEC / 2), Time(SEC))];
        assert!(e.cut_segments(0, &ranges, Time::ZERO));
        // Clip 0 (source 0..2s) lost 0.5s: the timeline shrinks by that.
        assert_eq!(e.project.total_duration(), Time(5 * SEC + SEC / 2));
        assert!(e.undo());
        assert_eq!(e.project.total_duration(), Time(6 * SEC));
    }

    #[test]
    fn relink_finds_moved_media() {
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("elsewhere");
        std::fs::create_dir_all(&media).unwrap();
        std::fs::write(media.join("a.mp4"), b"x").unwrap();
        let mut e = editor();
        // media/a.mp4 does not exist under the project dir: relink finds
        // it by filename under the search dir.
        assert!(e.relink(dir.path(), dir.path()));
        assert!(e.dirty);
        assert!(e
            .project
            .main()
            .clips[0]
            .src
            .ends_with("elsewhere/a.mp4"));
        // Nothing left to relink: refused, no undo point.
        assert!(!e.relink(dir.path(), dir.path()));
    }

    #[test]
    fn save_clears_dirty(){
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("project.viode");
        let mut e = editor();
        e.split(Time(SEC));
        assert!(e.dirty);
        assert!(e.save(&path));
        assert!(!e.dirty);
        let reloaded = Project::load(&path).unwrap();
        assert_eq!(reloaded.main().clips.len(), 4);
    }

    #[test]
    fn replace_project_resets_history() {
        let mut e = editor();
        e.split(Time(SEC));
        e.replace_project(Project::new("new", 30.0, [640, 360]));
        assert!(!e.dirty);
        assert!(!e.undo());
        assert_eq!(e.selection, Selection::None);
    }
}
