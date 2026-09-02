//! TUI state and key grammar — pure logic, no terminal. Every key becomes a
//! state transition here, which is what the unit tests drive.
//!
//! The grammar is playhead-centric (vim-adjacent): move the playhead, and
//! verbs act on the clip under it.
//!
//!   h/l  ±0.1s   H/L  ±1s     j/k  next/prev clip edge
//!   s    split at playhead    d    delete clip
//!   i/o  trim clip start/end to playhead
//!   </>  move clip left/right u/U  undo/redo
//!   space play clip (mpv)     P    preview timeline   r  render
//!   w    save                 q    quit (twice if unsaved)   ?  help

use std::path::{Path, PathBuf};
use std::process::{Child, Command};

use anyhow::Result;
use ratatui::crossterm::event::KeyCode;

use viode_core::{ops, GesBackend, Project, RenderBackend, Time, PROJECT_FILE};

const STEP: u64 = 100_000_000; // 0.1s
const BIG_STEP: u64 = 1_000_000_000; // 1s

#[derive(Debug, PartialEq)]
pub enum Action {
    None,
    Quit,
}

pub struct App {
    pub project: Project,
    pub project_file: PathBuf,
    pub project_dir: PathBuf,
    pub playhead: Time,
    pub dirty: bool,
    pub message: String,
    pub show_help: bool,
    /// Terminal can draw real images (kitty/ghostty).
    pub graphics: bool,
    pub media: crate::media::MediaCache,
    /// Pane the inline player renders into (set by ui::draw each frame).
    pub preview_area: Option<ratatui::layout::Rect>,
    preview: Option<crate::preview::Preview>,
    last_play_target: Option<PathBuf>,
    image_refresh: bool,
    confirm_quit: bool,
    file_mtime: Option<std::time::SystemTime>,
    undo: Vec<Project>,
    redo: Vec<Project>,
    children: Vec<Child>,
}

impl App {
    pub fn open(project_file: &Path) -> Result<App> {
        let project = Project::load(project_file)?;
        let project_dir = project_file
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        Ok(App {
            project,
            project_file: project_file.to_path_buf(),
            project_dir: project_dir.clone(),
            playhead: Time::ZERO,
            dirty: false,
            message: String::from("? for help"),
            show_help: false,
            graphics: crate::graphics::detect(),
            media: crate::media::MediaCache::new(&project_dir),
            preview_area: None,
            preview: None,
            last_play_target: None,
            image_refresh: false,
            confirm_quit: false,
            file_mtime: std::fs::metadata(project_file)
                .and_then(|m| m.modified())
                .ok(),
            undo: Vec::new(),
            redo: Vec::new(),
            children: Vec::new(),
        })
    }

    /// Approximate pixels per terminal cell (crisp enough for strips).
    const CELL_PX_W: u32 = 10;
    const CELL_PX_H: u32 = 20;

    fn artifact(
        &mut self,
        index: usize,
        kind: crate::media::Kind,
        cols: u16,
        rows: u16,
    ) -> Option<PathBuf> {
        let clip = self.project.main().clips.get(index)?.clone();
        let src = viode_core::proxy_for(&self.project_dir, &clip.src)
            .unwrap_or_else(|| self.project_dir.join(&clip.src));
        let px_w = cols as u32 * Self::CELL_PX_W;
        let px_h = rows as u32 * Self::CELL_PX_H;
        // One 16:9 frame is about px_h * 16/9 wide; tile enough to fill.
        let frame_w = (px_h * 16) / 9;
        let frames = (px_w / frame_w.max(1)).max(1);
        self.media.get(crate::media::Spec {
            kind,
            src,
            in_s: clip.in_.as_secs_f64(),
            out_s: clip.out.as_secs_f64(),
            px_w,
            px_h,
            frames,
        })
    }

    /// Filmstrip PNG for a main-track clip sized for cols x rows cells,
    /// if generated yet (proxy-aware).
    pub fn strip(&mut self, index: usize, cols: u16, rows: u16) -> Option<PathBuf> {
        self.artifact(index, crate::media::Kind::Strip, cols, rows)
    }

    /// Waveform PNG for a main-track clip sized for cols x rows cells.
    pub fn wave(&mut self, index: usize, cols: u16, rows: u16) -> Option<PathBuf> {
        self.artifact(index, crate::media::Kind::Wave, cols, rows)
    }

    /// Index of the clip under the playhead.
    pub fn selected(&self) -> Option<usize> {
        ops::source_at(&self.project, self.playhead).map(|(i, _)| i)
    }

    /// Source time under the playhead.
    pub fn source_time(&self) -> Option<(usize, Time)> {
        ops::source_at(&self.project, self.playhead)
    }

    fn snapshot(&mut self) {
        self.undo.push(self.project.clone());
        self.redo.clear();
        self.dirty = true;
    }

    fn clamp_playhead(&mut self) {
        let total = self.project.total_duration();
        if total == Time::ZERO {
            self.playhead = Time::ZERO;
        } else if self.playhead >= total {
            self.playhead = Time(total.0 - 1); // keep it on the last frame
        }
    }

    /// Live-reload: when ANOTHER process (the MCP server, the CLI, an
    /// editor) rewrites the project file, pick it up — unless there are
    /// unsaved local edits, which are never clobbered silently.
    pub fn check_external_change(&mut self) {
        let Ok(mtime) = std::fs::metadata(&self.project_file).and_then(|m| m.modified()) else {
            return;
        };
        if Some(mtime) == self.file_mtime {
            return;
        }
        self.file_mtime = Some(mtime);
        if self.dirty {
            self.message = "project changed on disk — you have unsaved edits (w overwrites)".into();
            return;
        }
        match Project::load(&self.project_file) {
            Ok(project) => {
                self.project = project;
                self.undo.clear();
                self.redo.clear();
                self.clamp_playhead();
                self.message = "reloaded — project changed on disk".into();
            }
            Err(e) => self.message = format!("changed on disk but unreadable: {e}"),
        }
    }

    /// Reap finished players so they don't linger as zombies.
    pub fn reap(&mut self) {
        self.children
            .retain_mut(|c| !matches!(c.try_wait(), Ok(Some(_))));
        if self.preview.as_mut().is_some_and(|p| p.finished()) {
            self.preview = None;
            self.image_refresh = true;
            self.message = "playback finished".into();
        }
    }

    pub fn is_playing(&self) -> bool {
        self.preview.is_some()
    }

    /// One-shot: the loop must wipe images and fully redraw (player gone).
    pub fn take_image_refresh(&mut self) -> bool {
        std::mem::take(&mut self.image_refresh)
    }

    pub fn toggle_pause(&self) {
        if let Some(p) = &self.preview {
            p.toggle_pause();
        }
    }

    pub fn stop_preview(&mut self) {
        if let Some(mut p) = self.preview.take() {
            p.stop();
            self.image_refresh = true;
            self.message = "stopped".into();
        }
    }

    pub fn on_key(&mut self, code: KeyCode) -> Action {
        // A pending quit-confirmation is cancelled by any key but q.
        if self.confirm_quit && code != KeyCode::Char('q') {
            self.confirm_quit = false;
        }
        if self.show_help {
            self.show_help = false;
            return Action::None;
        }
        match code {
            KeyCode::Char('q') => return self.quit(),
            KeyCode::Char('?') => self.show_help = true,
            KeyCode::Char('w') => self.save(),
            KeyCode::Char('h') | KeyCode::Left => self.nudge(-(STEP as i64)),
            KeyCode::Char('l') | KeyCode::Right => self.nudge(STEP as i64),
            KeyCode::Char('H') => self.nudge(-(BIG_STEP as i64)),
            KeyCode::Char('L') => self.nudge(BIG_STEP as i64),
            KeyCode::Char('j') | KeyCode::Down => self.jump(1),
            KeyCode::Char('k') | KeyCode::Up => self.jump(-1),
            KeyCode::Char('s') => self.split(),
            KeyCode::Char('d') => self.delete(),
            KeyCode::Char('f') => self.freeze(),
            KeyCode::Char('i') => self.trim_in(),
            KeyCode::Char('o') => self.trim_out(),
            KeyCode::Char('<') => self.shift(-1),
            KeyCode::Char('>') => self.shift(1),
            KeyCode::Char('u') => self.undo(),
            KeyCode::Char('U') => self.redo(),
            KeyCode::Char(' ') => self.play(),
            KeyCode::Char('P') => self.preview_timeline(),
            KeyCode::Char('v') => self.play_live(),
            KeyCode::Char('r') => self.render(),
            _ => {}
        }
        Action::None
    }

    fn quit(&mut self) -> Action {
        if self.dirty && !self.confirm_quit {
            self.confirm_quit = true;
            self.message = "unsaved changes — q again to discard, w to save".into();
            return Action::None;
        }
        Action::Quit
    }

    fn save(&mut self) {
        match self.project.save(&self.project_file) {
            Ok(()) => {
                self.dirty = false;
                self.file_mtime = std::fs::metadata(&self.project_file)
                    .and_then(|m| m.modified())
                    .ok();
                self.message = format!("saved {}", self.project_file.display());
            }
            Err(e) => self.message = format!("save failed: {e}"),
        }
    }

    fn nudge(&mut self, delta_ns: i64) {
        let ns = self.playhead.0 as i64 + delta_ns;
        self.playhead = Time(ns.max(0) as u64);
        self.clamp_playhead();
        self.message.clear();
    }

    /// Jump to the next/previous clip boundary.
    fn jump(&mut self, dir: i64) {
        let positions = self.project.positions();
        if positions.is_empty() {
            return;
        }
        self.playhead = if dir > 0 {
            positions
                .iter()
                .find(|p| **p > self.playhead)
                .copied()
                .unwrap_or_else(|| Time(self.project.total_duration().0.saturating_sub(1)))
        } else {
            positions
                .iter()
                .rev()
                .find(|p| **p < self.playhead)
                .copied()
                .unwrap_or(Time::ZERO)
        };
        self.clamp_playhead();
        self.message.clear();
    }

    fn split(&mut self) {
        let Some((index, src_time)) = self.source_time() else {
            self.message = "nothing under playhead".into();
            return;
        };
        let offset = src_time - self.project.main().clips[index].in_;
        self.snapshot();
        match ops::split(self.project.main_mut(), index, offset) {
            Ok(()) => self.message = format!("split clip {index} at {}", self.playhead),
            Err(e) => {
                self.project = self.undo.pop().unwrap();
                self.message = e.to_string();
            }
        }
    }

    /// Frame hold: freeze the frame under the playhead for two seconds.
    /// The still is generated by ffmpeg and inserted as a normal clip.
    fn freeze(&mut self) {
        self.snapshot();
        let dur = viode_core::Time(2_000_000_000);
        match viode_core::freeze::freeze_at(&mut self.project, &self.project_dir, self.playhead, dur)
        {
            Ok(i) => self.message = format!("froze frame at {} for {dur} (clip {i})", self.playhead),
            Err(e) => {
                self.project = self.undo.pop().unwrap();
                self.message = e.to_string();
            }
        }
    }

    fn delete(&mut self) {
        let Some(index) = self.selected() else {
            self.message = "nothing under playhead".into();
            return;
        };
        self.snapshot();
        match ops::remove(self.project.main_mut(), index) {
            Ok(clip) => {
                self.message = format!("deleted [{index}] {}", clip.src.display());
                self.clamp_playhead();
            }
            Err(e) => {
                self.project = self.undo.pop().unwrap();
                self.message = e.to_string();
            }
        }
    }

    fn trim_in(&mut self) {
        self.trim(true);
    }

    fn trim_out(&mut self) {
        self.trim(false);
    }

    fn trim(&mut self, start: bool) {
        let Some((index, src_time)) = self.source_time() else {
            self.message = "nothing under playhead".into();
            return;
        };
        self.snapshot();
        let (in_, out) = if start {
            (Some(src_time), None)
        } else {
            (None, Some(src_time))
        };
        match ops::trim(self.project.main_mut(), index, in_, out) {
            Ok(()) => {
                self.message = format!(
                    "clip {index} {} set to {src_time}",
                    if start { "in" } else { "out" }
                );
                self.clamp_playhead();
            }
            Err(e) => {
                self.project = self.undo.pop().unwrap();
                self.message = e.to_string();
            }
        }
    }

    fn shift(&mut self, dir: i64) {
        let Some(index) = self.selected() else {
            self.message = "nothing under playhead".into();
            return;
        };
        let to = index as i64 + dir;
        if to < 0 || to >= self.project.main().clips.len() as i64 {
            self.message = "already at the edge".into();
            return;
        }
        self.snapshot();
        if let Err(e) = ops::move_clip(self.project.main_mut(), index, to as usize) {
            self.project = self.undo.pop().unwrap();
            self.message = e.to_string();
        } else {
            // Follow the clip to its new position.
            self.playhead = self.project.positions()[to as usize];
            self.message = format!("moved clip {index} -> {to}");
        }
    }

    fn undo(&mut self) {
        match self.undo.pop() {
            Some(prev) => {
                self.redo.push(std::mem::replace(&mut self.project, prev));
                self.dirty = true;
                self.clamp_playhead();
                self.message = "undo".into();
            }
            None => self.message = "nothing to undo".into(),
        }
    }

    fn redo(&mut self) {
        match self.redo.pop() {
            Some(next) => {
                self.undo.push(std::mem::replace(&mut self.project, next));
                self.dirty = true;
                self.clamp_playhead();
                self.message = "redo".into();
            }
            None => self.message = "nothing to redo".into(),
        }
    }

    /// Space: play the timeline from the playhead in the preview pane —
    /// instant, zero-render, via an mpv EDL playlist of the cuts.
    fn play(&mut self) {
        if self.project.main().clips.is_empty() {
            self.message = "timeline is empty".into();
            return;
        }
        let edl = crate::preview::edl_for(&self.project, &self.project_dir);
        let edl_path = self.project_dir.join("cache").join("preview.edl");
        let _ = std::fs::create_dir_all(edl_path.parent().unwrap());
        if let Err(e) = std::fs::write(&edl_path, edl) {
            self.message = format!("could not write EDL: {e}");
            return;
        }
        self.spawn_preview(&edl_path, self.playhead.as_secs_f64());
    }

    fn spawn_preview(&mut self, target: &Path, start: f64) {
        self.spawn_preview_state(target, start, false);
    }

    fn spawn_preview_state(&mut self, target: &Path, start: f64, paused: bool) {
        let Some(area) = self.preview_area.filter(|a| a.width > 8 && a.height > 4) else {
            self.message = "terminal too small for the preview pane".into();
            return;
        };
        let sock = self.project_dir.join("cache").join("mpv.sock");
        match crate::preview::Preview::spawn(target, area, start, paused, sock) {
            Ok(p) => {
                self.preview = Some(p);
                self.last_play_target = Some(target.to_path_buf());
                self.message = "playing — space pause · x stop".into();
            }
            Err(e) => self.message = format!("mpv failed (is mpv installed?): {e}"),
        }
    }

    /// The terminal changed shape under a live player: respawn mpv at the
    /// new pane geometry, at the same position, preserving pause state.
    pub fn reflow_playback(&mut self) {
        let Some(target) = self.last_play_target.clone() else { return };
        let state = self.preview.as_ref().and_then(|p| p.position());
        if let Some(mut p) = self.preview.take() {
            p.stop();
        }
        let (pos, paused) = state.unwrap_or((0.0, false));
        self.spawn_preview_state(&target, pos, paused);
    }

    /// v: LIVE composited preview — the GES pipeline plays the timeline
    /// in a window (no render step). Runs as a child process so the TUI
    /// stays interactive.
    fn play_live(&mut self) {
        if self.project.main().clips.is_empty() {
            self.message = "timeline is empty".into();
            return;
        }
        let Ok(exe) = std::env::current_exe() else {
            self.message = "cannot find the viode binary".into();
            return;
        };
        match Command::new(exe)
            .arg("--project")
            .arg(&self.project_file)
            .arg("play")
            .arg("--from")
            .arg(self.playhead.to_string())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(child) => {
                self.children.push(child);
                self.message = "live composite window opened (close it to end)".into();
            }
            Err(e) => self.message = format!("live preview failed: {e}"),
        }
    }

    /// P: the accurate path — GES renders the full composite (tracks,
    /// fades, titles, keyframes), then it plays in the pane.
    fn preview_timeline(&mut self) {
        if self.project.main().clips.is_empty() {
            self.message = "timeline is empty".into();
            return;
        }
        // Proxied copy for speed where proxies exist.
        let mut preview = self.project.clone();
        for track in &mut preview.tracks {
            for clip in &mut track.clips {
                if let Some(p) = viode_core::proxy_for(&self.project_dir, &clip.src) {
                    clip.src = p;
                }
            }
        }
        let out = self.project_dir.join("cache").join("tui-preview.mp4");
        self.message = "rendering composited preview…".into();
        match GesBackend.render(&preview, &self.project_dir, &out) {
            Ok(()) => self.spawn_preview(&out, 0.0),
            Err(e) => self.message = format!("preview failed: {e}"),
        }
    }

    fn render(&mut self) {
        if self.project.main().clips.is_empty() {
            self.message = "timeline is empty".into();
            return;
        }
        let out = self
            .project_dir
            .join("renders")
            .join(format!("{}.mp4", self.project.project.name));
        self.message = "rendering…".into();
        match GesBackend.render(&self.project, &self.project_dir, &out) {
            Ok(()) => self.message = format!("rendered {}", out.display()),
            Err(e) => self.message = format!("render failed: {e}"),
        }
    }
}

pub fn default_project_file() -> PathBuf {
    PathBuf::from(PROJECT_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use viode_core::Clip;

    fn app() -> App {
        // Two 2s clips on a disk-backed project, in a UNIQUE temp dir —
        // tests run in parallel and must not share a project file.
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "viode-tui-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(PROJECT_FILE);
        let mut project = Project::new("t", 30.0, [640, 360]);
        let t = |s| Time::from_secs_f64(s).unwrap();
        for _ in 0..2 {
            project
                .main_mut()
                .clips
                .push(Clip::media("media/a.mp4".into(), t(0.0), t(2.0)));
        }
        project.save(&file).unwrap();
        App::open(&file).unwrap()
    }

    fn t(s: f64) -> Time {
        Time::from_secs_f64(s).unwrap()
    }

    #[test]
    fn playhead_moves_and_clamps() {
        let mut a = app();
        a.on_key(KeyCode::Char('h'));
        assert_eq!(a.playhead, Time::ZERO, "clamps at zero");
        a.on_key(KeyCode::Char('L'));
        assert_eq!(a.playhead, t(1.0));
        for _ in 0..100 {
            a.on_key(KeyCode::Char('L'));
        }
        assert!(a.playhead < a.project.total_duration(), "clamps below total");
    }

    #[test]
    fn jump_walks_clip_boundaries() {
        let mut a = app();
        a.on_key(KeyCode::Char('j'));
        assert_eq!(a.playhead, t(2.0), "start of clip 1");
        a.on_key(KeyCode::Char('k'));
        assert_eq!(a.playhead, Time::ZERO);
    }

    #[test]
    fn split_and_undo_redo() {
        let mut a = app();
        a.playhead = t(1.0);
        a.on_key(KeyCode::Char('s'));
        assert_eq!(a.project.main().clips.len(), 3);

        // Freeze off the end of the timeline refuses with a clear message
        // and leaves the timeline untouched.
        let clips_before = a.project.main().clips.len();
        a.playhead = t(999.0);
        a.on_key(KeyCode::Char('f'));
        assert!(a.message.contains("nothing under the playhead"), "{}", a.message);
        assert_eq!(a.project.main().clips.len(), clips_before);
        assert!(a.dirty);
        a.on_key(KeyCode::Char('u'));
        assert_eq!(a.project.main().clips.len(), 2, "undo restores");
        a.on_key(KeyCode::Char('U'));
        assert_eq!(a.project.main().clips.len(), 3, "redo reapplies");
    }

    #[test]
    fn trim_in_to_playhead() {
        let mut a = app();
        a.playhead = t(2.5); // 0.5s into clip 1
        a.on_key(KeyCode::Char('i'));
        assert_eq!(a.project.main().clips[1].in_, t(0.5));
        assert_eq!(a.project.total_duration(), t(3.5));
    }

    #[test]
    fn delete_clamps_playhead() {
        let mut a = app();
        a.playhead = t(3.0);
        a.on_key(KeyCode::Char('d'));
        assert_eq!(a.project.main().clips.len(), 1);
        assert!(a.playhead < a.project.total_duration());
    }

    #[test]
    fn move_follows_the_clip() {
        let mut a = app();
        a.playhead = t(3.0); // clip 1
        a.on_key(KeyCode::Char('<'));
        assert_eq!(a.playhead, Time::ZERO, "playhead follows moved clip");
        a.on_key(KeyCode::Char('<'));
        assert_eq!(a.message, "already at the edge");
    }

    #[test]
    fn quit_requires_confirmation_when_dirty() {
        let mut a = app();
        assert_eq!(a.on_key(KeyCode::Char('q')), Action::Quit, "clean quits at once");

        let mut a = app();
        a.playhead = t(1.0);
        a.on_key(KeyCode::Char('s'));
        assert_eq!(a.on_key(KeyCode::Char('q')), Action::None, "dirty asks first");
        assert_eq!(a.on_key(KeyCode::Char('q')), Action::Quit, "second q quits");

        let mut a = app();
        a.playhead = t(1.0);
        a.on_key(KeyCode::Char('s'));
        a.on_key(KeyCode::Char('q'));
        a.on_key(KeyCode::Char('h'), );
        assert_eq!(a.on_key(KeyCode::Char('q')), Action::None, "other key resets confirm");
    }

    #[test]
    fn save_clears_dirty() {
        let mut a = app();
        a.playhead = t(1.0);
        a.on_key(KeyCode::Char('s'));
        assert!(a.dirty);
        a.on_key(KeyCode::Char('w'));
        assert!(!a.dirty);
        let reloaded = Project::load(&a.project_file).unwrap();
        assert_eq!(reloaded.main().clips.len(), 3);
    }

    #[test]
    fn external_changes_reload_unless_dirty() {
        let mut a = app();
        // Another process rewrites the file (MCP server, CLI, editor).
        std::thread::sleep(std::time::Duration::from_millis(20));
        let mut other = Project::load(&a.project_file).unwrap();
        other.main_mut().clips.truncate(1);
        other.save(&a.project_file).unwrap();

        a.check_external_change();
        assert_eq!(a.project.main().clips.len(), 1, "reloaded external edit");
        assert!(a.message.contains("reloaded"));

        // With unsaved local edits, never clobber silently.
        let mut a2 = App::open(&a.project_file).unwrap();
        a2.playhead = t(0.5);
        a2.on_key(KeyCode::Char('s')); // dirty now
        std::thread::sleep(std::time::Duration::from_millis(20));
        let mut other = Project::load(&a2.project_file).unwrap();
        other.main_mut().clips.clear();
        other.save(&a2.project_file).unwrap();
        a2.check_external_change();
        assert!(!a2.project.main().clips.is_empty(), "local edits kept");
        assert!(a2.message.contains("unsaved"));
    }

    #[test]
    fn space_writes_the_edl_and_needs_a_pane() {
        let mut a = app();
        a.playhead = t(1.0);
        a.on_key(KeyCode::Char(' ')); // no preview_area in tests -> no spawn
        assert_eq!(a.message, "terminal too small for the preview pane");
        let edl = std::fs::read_to_string(a.project_dir.join("cache/preview.edl")).unwrap();
        assert!(edl.starts_with("# mpv EDL v0"), "wrote a real EDL: {edl}");

        let mut a = app();
        a.project.main_mut().clips.clear();
        a.on_key(KeyCode::Char(' '));
        assert_eq!(a.message, "timeline is empty");
    }
}
