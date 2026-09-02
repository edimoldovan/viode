//! The egui shell — rendering and input translation only. Transport logic
//! lives in state.rs, edit logic in edit.rs (both tested reducers), all
//! GStreamer in player.rs; this file draws what they say and forwards
//! keys and mouse gestures, the same dumb-renderer split as the TUI.
//!
//! Layout follows the NLE convention (Premiere, Resolve): the preview
//! dominates, the inspector docks right, the timeline docks below with a
//! prominent timecode, track headers on the left, video lanes stacked
//! above V1 and audio lanes below A1. Colors come from the Omarchy theme
//! (theme.rs).

use std::collections::HashMap;
use std::path::PathBuf;

use eframe::egui::{self, Align2, Color32, CornerRadius, CursorIcon, FontId, Pos2, Rect, Sense, Stroke, Vec2};

use viode_core::artifacts::{Kind, MediaCache, Spec};
use viode_core::{ops, proxy_for, Clip, Project, Time, Track, TrackKind};

use crate::actions::Action;
use crate::palette::Palette as CmdPalette;

use crate::edit::{DragKind, Editor, Selection};
use crate::layout::{tick_step, timeline_end, TimelineMap};
use crate::player::{Player, PlayerEvent};
use crate::state::{Cmd, Key, State};
use crate::theme::Palette;

const GUTTER: f32 = 92.0;
const HEADER_H: f32 = 34.0;
const RULER_H: f32 = 20.0;
const TITLE_LANE_H: f32 = 20.0;
const VIDEO_LANE_H: f32 = 40.0;
const STRIP_LANE_H: f32 = 56.0;
const WAVE_LANE_H: f32 = 44.0;
const AUDIO_LANE_H: f32 = 32.0;
const LANE_GAP: f32 = 2.0;
const EDGE_W: f32 = 6.0;
const INSPECTOR_W: f32 = 270.0;

pub struct GuiApp {
    editor: Editor,
    project_file: PathBuf,
    project_dir: PathBuf,
    state: State,
    player: Player,
    player_err: Option<String>,
    media: MediaCache,
    theme: Palette,
    textures: HashMap<PathBuf, egui::TextureHandle>,
    preview_tex: Option<egui::TextureHandle>,
    preview_seq: u64,
    file_mtime: Option<std::time::SystemTime>,
    last_mtime_check: std::time::Instant,
    theme_watch: crate::theme::ThemeWatcher,
    reload_blocked_note: bool,
    /// Debounced pipeline rebuild after model edits. The build itself
    /// runs on the player's actor thread — the UI never blocks on it.
    rebuild_at: Option<std::time::Instant>,
    /// Mouse-drag bookkeeping (the model side lives in the Editor).
    drag_start_x: f32,
    title_drag: Option<(usize, f32)>,
    confirm_quit: bool,
    key_prop: String,
    key_value: f64,
    // -- the pro surface (G3) --
    /// Missing media, recomputed on load/reload/edit.
    missing: Vec<(usize, usize, PathBuf)>,
    /// Engine gaps found once at startup (see viode_core::doctor).
    engine_gaps: Vec<viode_core::doctor::Check>,
    /// Announcement from the developer, provided by the official
    /// binary's license check (VIODE_NOTICE). Empty in source builds.
    notice: String,
    /// Whether any AI client on this machine already knows Viode —
    /// drives the left-panel connect hint.
    ai_connected: bool,
    palette: CmdPalette,
    show_doctor: bool,
    show_relink: bool,
    relink_dir: String,
    scopes_on: bool,
    /// (source path, source time) of the scope images currently shown.
    scope_key: Option<(PathBuf, Time)>,
    scope_rx: Option<std::sync::mpsc::Receiver<Result<(PathBuf, PathBuf), String>>>,
    scope_tex: [Option<egui::TextureHandle>; 2],
    /// Loaded transcript: (clip index, file mtime, segments).
    transcript: Option<(usize, std::time::SystemTime, Vec<viode_core::Segment>)>,
    transcribe_rx: Option<(usize, std::sync::mpsc::Receiver<Result<usize, String>>)>,
    show_render: bool,
    r_preset: String,
    r_codec: String,
    r_bitrate: u32,
    r_output: String,
    render_rx: Option<std::sync::mpsc::Receiver<Result<String, String>>>,
    captions_rx: Option<std::sync::mpsc::Receiver<Result<Vec<viode_core::captions::Caption>, String>>>,
    duck_rx: Option<(usize, std::sync::mpsc::Receiver<Result<Vec<(Time, Time)>, String>>)>,
    silence_rx: Option<(usize, std::sync::mpsc::Receiver<Result<Vec<(Time, Time)>, String>>)>,
    scenes_rx: Option<(usize, std::sync::mpsc::Receiver<Result<Vec<Time>, String>>)>,
    angle_rx: Option<std::sync::mpsc::Receiver<Result<(PathBuf, Time, f64), String>>>,
    proxy_rx: Option<std::sync::mpsc::Receiver<Result<usize, String>>>,
    r_reframe: bool,
    ramp_from: f64,
    ramp_to: f64,
    refit_to: f64,
    mask_region: String,
    mask_kind: String,
    mask_follow: bool,
}

impl GuiApp {
    pub fn new(
        ctx: &egui::Context,
        project: Project,
        project_file: PathBuf,
        project_dir: PathBuf,
    ) -> GuiApp {
        let theme = crate::theme::load();
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = theme.bg;
        visuals.window_fill = theme.lane;
        visuals.override_text_color = Some(theme.fg);
        ctx.set_visuals(visuals);
        let total = timeline_end(&project);
        let state = State::new(total, project.project.fps);
        let repaint = ctx.clone();
        let player = Player::spawn(move || repaint.request_repaint());
        if total != Time::ZERO {
            player.load(&project, &project_dir);
        }
        let mut app = GuiApp {
            media: MediaCache::new(&project_dir),
            file_mtime: std::fs::metadata(&project_file)
                .and_then(|m| m.modified())
                .ok(),
            last_mtime_check: std::time::Instant::now(),
            theme_watch: crate::theme::ThemeWatcher::new(),
            reload_blocked_note: false,
            editor: Editor::new(project),
            project_file,
            project_dir,
            state,
            player,
            player_err: None,
            theme,
            textures: HashMap::new(),
            preview_tex: None,
            preview_seq: 0,
            rebuild_at: None,
            drag_start_x: 0.0,
            title_drag: None,
            confirm_quit: false,
            key_prop: "volume".into(),
            key_value: 1.0,
            missing: Vec::new(),
            engine_gaps: viode_core::doctor::problems(),
            notice: std::env::var("VIODE_NOTICE").unwrap_or_default(),
            ai_connected: viode_core::connect::detect().iter().any(|c| c.connected),
            palette: CmdPalette::default(),
            show_doctor: false,
            show_relink: false,
            relink_dir: String::new(),
            scopes_on: false,
            scope_key: None,
            scope_rx: None,
            scope_tex: [None, None],
            transcript: None,
            transcribe_rx: None,
            show_render: false,
            r_preset: "master".into(),
            r_codec: "h264".into(),
            r_bitrate: 8000,
            r_output: String::new(),
            render_rx: None,
            captions_rx: None,
            duck_rx: None,
            silence_rx: None,
            scenes_rx: None,
            angle_rx: None,
            proxy_rx: None,
            r_reframe: false,
            ramp_from: 1.0,
            ramp_to: 2.0,
            refit_to: 60.0,
            mask_region: "0.6,0.1,0.25,0.3".into(),
            mask_kind: "blur".into(),
            mask_follow: false,
        };
        app.missing = viode_core::media::missing(&app.editor.project, &app.project_dir);
        app
    }

    /// The model changed: redraw is automatic (it reads the model), but the
    /// GES pipeline needs a rebuild — debounced so slider drags coalesce.
    fn model_changed(&mut self) {
        self.state.total = timeline_end(&self.editor.project);
        self.rebuild_at = Some(std::time::Instant::now() + std::time::Duration::from_millis(300));
        self.missing = viode_core::media::missing(&self.editor.project, &self.project_dir);
    }

    /// Reload the pipeline from the current model, keeping the transport
    /// where it was. The heavy lifting happens on the player's actor
    /// thread; commands queue behind the load in order, so the seek and
    /// resume apply to the fresh pipeline.
    fn rebuild_player(&mut self, _ctx: &egui::Context) {
        let total = timeline_end(&self.editor.project);
        let playhead = self.state.playhead;
        let playing = self.state.playing;
        let rate = self.state.rate;
        self.state = State::new(total, self.editor.project.project.fps);
        self.player_err = None;
        if total == Time::ZERO {
            self.preview_tex = None; // everything was deleted: go dark
            self.player.load(&self.editor.project, &self.project_dir);
            return;
        }
        self.player.load(&self.editor.project, &self.project_dir);
        let cmds = self.state.seek_to(playhead);
        self.apply(cmds);
        if playing {
            self.state.playing = true;
            self.state.rate = rate;
            self.apply(vec![Cmd::SetRate(rate), Cmd::Play]);
        }
    }

    /// Live-reload, same contract as the TUI: when ANOTHER process (the
    /// MCP server, the CLI, an editor) rewrites the project file, pick it
    /// up — this is what makes the GUI a live monitor of an AI edit
    /// session. Unsaved local edits are never clobbered silently.
    fn check_external_change(&mut self, ctx: &egui::Context) {
        if self.last_mtime_check.elapsed() < std::time::Duration::from_millis(500) {
            return;
        }
        self.last_mtime_check = std::time::Instant::now();
        let Ok(mtime) = std::fs::metadata(&self.project_file).and_then(|m| m.modified()) else {
            return;
        };
        if Some(mtime) == self.file_mtime {
            return;
        }
        self.file_mtime = Some(mtime);
        if self.editor.dirty {
            if !self.reload_blocked_note {
                self.editor.message =
                    "project changed on disk — unsaved edits kept (w saves over it)".into();
                self.reload_blocked_note = true;
            }
            return;
        }
        match Project::load(&self.project_file) {
            Ok(project) => {
                self.editor.replace_project(project);
                self.reload_blocked_note = false;
                self.rebuild_player(ctx);
            }
            // A half-written file (the writer is mid-save) parses next tick.
            Err(_) => {}
        }
    }

    fn save(&mut self) {
        if self.editor.save(&self.project_file) {
            self.file_mtime = std::fs::metadata(&self.project_file)
                .and_then(|m| m.modified())
                .ok();
            self.reload_blocked_note = false;
        }
    }

    fn apply(&mut self, cmds: Vec<Cmd>) {
        for cmd in cmds {
            match cmd {
                Cmd::Seek(t) => self.player.seek(t),
                Cmd::Play => self.player.play(),
                Cmd::Pause => self.player.pause(),
                Cmd::SetRate(r) => self.player.set_rate(r),
            }
        }
    }

    /// Source time of the playhead within the selected clip (for keyframes).
    fn selected_source_time(&self) -> Option<Time> {
        let (track, index) = self.editor.selected_clip()?;
        if track == 0 {
            match ops::source_at(&self.editor.project, self.state.playhead) {
                Some((i, t)) if i == index => Some(t),
                _ => None,
            }
        } else {
            let clip = &self.editor.project.tracks[track].clips[index];
            let (start, end) = clip.span();
            if self.state.playhead >= start && self.state.playhead < end {
                Some(clip.in_ + clip.src_offset(self.state.playhead - start))
            } else {
                None
            }
        }
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        // A focused text field owns the keyboard (the palette included).
        if ctx.wants_keyboard_input() {
            return;
        }
        use egui::Key as EKey;
        let mut actions: Vec<Action> = Vec::new();
        ctx.input(|i| {
            let shift = i.modifiers.shift;
            let command = i.modifiers.command;
            let key_map = [
                (EKey::Space, Action::PlayPause),
                (EKey::J, Action::ShuttleReverse),
                (EKey::K, Action::ShuttlePause),
                (EKey::L, Action::ShuttleForward),
                (EKey::Home, Action::GoToStart),
                (EKey::End, Action::GoToEnd),
                (EKey::Questionmark, Action::Help),
                (EKey::ArrowUp, Action::PreviousEdge),
                (EKey::ArrowDown, Action::NextEdge),
                (EKey::I, Action::TrimInToPlayhead),
                (EKey::O, Action::TrimOutToPlayhead),
                (EKey::T, Action::AddTitle),
                (EKey::F, Action::Freeze),
                (EKey::M, Action::AddMarker),
                (EKey::OpenBracket, Action::MarkIn),
                (EKey::CloseBracket, Action::MarkOut),
                (EKey::R, Action::RenderDialog),
                (EKey::Escape, Action::ClearMarks),
                (EKey::Q, Action::Quit),
            ];
            for (ek, a) in key_map {
                if i.key_pressed(ek) {
                    actions.push(a);
                }
            }
            if i.key_pressed(EKey::ArrowLeft) {
                actions.push(if shift { Action::JumpBack } else { Action::NudgeBack });
            }
            if i.key_pressed(EKey::ArrowRight) {
                actions.push(if shift { Action::JumpForward } else { Action::NudgeForward });
            }
            if i.key_pressed(EKey::Comma) {
                actions.push(if shift { Action::MoveEarlier } else { Action::FrameBack });
            }
            if i.key_pressed(EKey::Period) {
                actions.push(if shift { Action::MoveLater } else { Action::FrameForward });
            }
            if i.key_pressed(EKey::S) && !command {
                actions.push(Action::Split);
            }
            if i.key_pressed(EKey::D)
                || i.key_pressed(EKey::Delete)
                || i.key_pressed(EKey::Backspace)
            {
                actions.push(Action::Delete);
            }
            if i.key_pressed(EKey::U) && !command {
                actions.push(if shift { Action::Redo } else { Action::Undo });
            }
            if command && i.key_pressed(EKey::Z) {
                actions.push(if shift { Action::Redo } else { Action::Undo });
            }
            if i.key_pressed(EKey::W) || (command && i.key_pressed(EKey::S)) {
                actions.push(Action::Save);
            }
            if command && (i.key_pressed(EKey::K) || i.key_pressed(EKey::P)) {
                actions.push(Action::CommandPalette);
            }
        });
        // Multicam: number keys take that angle over the range — the
        // keyboard half of the angle wall.
        let mut takes: Vec<usize> = Vec::new();
        ctx.input(|i| {
            for (n, key) in [
                (1, EKey::Num1), (2, EKey::Num2), (3, EKey::Num3),
                (4, EKey::Num4), (5, EKey::Num5), (6, EKey::Num6),
                (7, EKey::Num7), (8, EKey::Num8), (9, EKey::Num9),
            ] {
                if i.key_pressed(key) {
                    takes.push(n);
                }
            }
        });
        for n in takes {
            if n < self.editor.project.tracks.len() {
                self.do_take(n);
            }
        }
        for a in actions {
            self.perform(ctx, a);
        }
    }

    /// THE dispatch point. Keyboard, command palette, context menus, and
    /// the toolbar all end up here, so every surface stays in lockstep —
    /// that is the discoverability rule made structural.
    fn perform(&mut self, ctx: &egui::Context, action: Action) {
        match action {
            Action::PlayPause => self.transport(Key::Space),
            Action::ShuttleReverse => self.transport(Key::J),
            Action::ShuttlePause => self.transport(Key::K),
            Action::ShuttleForward => self.transport(Key::L),
            Action::FrameBack => self.transport(Key::Comma),
            Action::FrameForward => self.transport(Key::Period),
            Action::NudgeBack => self.transport(Key::SmallLeft),
            Action::NudgeForward => self.transport(Key::SmallRight),
            Action::JumpBack => self.transport(Key::Left),
            Action::JumpForward => self.transport(Key::Right),
            Action::GoToStart => self.transport(Key::Home),
            Action::GoToEnd => self.transport(Key::End),
            Action::PreviousEdge => self.jump_edge(-1),
            Action::NextEdge => self.jump_edge(1),
            Action::MarkIn => self.transport(Key::MarkIn),
            Action::MarkOut => self.transport(Key::MarkOut),
            Action::ClearMarks => {
                self.transport(Key::ClearMarks);
                self.editor.deselect();
            }
            Action::AddMedia => {
                if let Some(paths) = rfd::FileDialog::new()
                    .add_filter(
                        "Video/audio",
                        &["mp4", "mov", "mkv", "webm", "avi", "mp3", "wav", "m4a", "flac", "viode"],
                    )
                    .pick_files()
                {
                    let dir = self.project_dir.clone();
                    if self.editor.add_media(&dir, &paths) {
                        self.model_changed();
                    }
                }
            }
            Action::AddVideoTrack => {
                if self.editor.track_add(viode_core::TrackKind::Video) {
                    self.editor.end_stage();
                    self.model_changed();
                }
            }
            Action::AddMusicTrack => {
                if self.editor.track_add(viode_core::TrackKind::Audio) {
                    self.editor.end_stage();
                    self.model_changed();
                }
            }
            Action::AddAngle => self.start_angle_add(),
            Action::CutSilences => self.start_cut_silences(),
            Action::SplitScenes => self.start_split_scenes(),
            Action::BuildProxies => self.start_build_proxies(),
            Action::Split => self.edit(|e, ph| e.split(ph)),
            Action::TrimInToPlayhead => self.edit(|e, ph| e.trim_to_playhead(true, ph)),
            Action::TrimOutToPlayhead => self.edit(|e, ph| e.trim_to_playhead(false, ph)),
            Action::Delete => self.edit(|e, ph| e.delete(ph)),
            Action::MoveEarlier => self.edit(|e, ph| e.shift(ph, -1)),
            Action::MoveLater => self.edit(|e, ph| e.shift(ph, 1)),
            Action::AddTitle => {
                let ph = self.state.playhead;
                if self.editor.title_add(ph) {
                    self.editor.end_stage();
                    self.model_changed();
                }
            }
            Action::AddMarker => {
                let ph = self.state.playhead;
                if self.editor.marker_add(ph) {
                    self.editor.end_stage();
                    self.model_changed();
                }
            }
            Action::Freeze => {
                let dir = self.project_dir.clone();
                let ph = self.state.playhead;
                if self.editor.freeze(&dir, ph, Time(2_000_000_000)) {
                    self.model_changed();
                }
            }
            Action::Mend => {
                let dir = self.project_dir.clone();
                let ph = self.state.playhead;
                if self.editor.mend(&dir, ph) {
                    self.model_changed();
                }
            }
            Action::MatchPrevious => {
                let dir = self.project_dir.clone();
                if self.editor.match_previous(&dir) {
                    self.editor.end_stage();
                    self.model_changed();
                }
            }
            Action::BundleAdd => {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("Viode project", &["viode"])
                    .pick_file()
                {
                    match viode_core::Project::load(&path) {
                        Ok(sub) if sub.total_duration() > Time::ZERO => {
                            self.editor.snapshot_public();
                            let dur = sub.total_duration();
                            let clip = viode_core::Clip::media(path, Time::ZERO, dur);
                            let _ = viode_core::ops::add(self.editor.project.main_mut(), clip);
                            self.editor.dirty = true;
                            self.editor.message =
                                format!("bundled {} ({dur})", sub.project.name);
                            self.model_changed();
                        }
                        Ok(_) => self.editor.message = "bundled project is empty".into(),
                        Err(e) => self.editor.message = e.to_string(),
                    }
                }
            }
            Action::Captions => self.start_captions(),
            Action::Duck => self.start_duck(),
            Action::Undo => self.edit(|e, _| e.undo()),
            Action::Redo => self.edit(|e, _| e.redo()),
            Action::Save => self.save(),
            Action::RenderDialog => self.show_render = !self.show_render,
            Action::ToggleScopes => {
                self.scopes_on = !self.scopes_on;
                self.scope_key = None;
            }
            Action::EngineCheckup => self.show_doctor = true,
            Action::ConnectAi => {
                self.editor.message = crate::welcome::run_connect_all();
                self.ai_connected = true;
            }
            Action::CommandPalette => self.palette.open(),
            Action::Help => self.transport(Key::Help),
            Action::Quit => self.request_quit(ctx),
        }
    }

    fn transport(&mut self, k: Key) {
        let cmds = self.state.on_key(k);
        self.apply(cmds);
    }

    /// Run an Editor verb at the playhead; rebuild if it changed the model.
    fn edit(&mut self, f: impl FnOnce(&mut Editor, Time) -> bool) {
        if f(&mut self.editor, self.state.playhead) {
            self.model_changed();
            // Verbs that shorten the timeline can strand the playhead.
            let cmds = self.state.seek_to(self.state.playhead);
            self.apply(cmds);
        }
    }

    /// Jump the playhead to the next/previous clip boundary (TUI j/k).
    fn jump_edge(&mut self, dir: i64) {
        let positions = self.editor.project.positions();
        if positions.is_empty() {
            return;
        }
        let target = if dir > 0 {
            positions
                .iter()
                .find(|p| **p > self.state.playhead)
                .copied()
                .unwrap_or(self.state.total)
        } else {
            positions
                .iter()
                .rev()
                .find(|p| **p < self.state.playhead)
                .copied()
                .unwrap_or(Time::ZERO)
        };
        let cmds = self.state.seek_to(target);
        self.apply(cmds);
    }

    fn request_quit(&mut self, ctx: &egui::Context) {
        if self.editor.dirty {
            self.confirm_quit = true;
        } else {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    /// PNG artifact -> texture, decoded once and cached by path.
    fn tex_for(&mut self, ctx: &egui::Context, path: &PathBuf) -> Option<egui::TextureId> {
        if let Some(t) = self.textures.get(path) {
            return Some(t.id());
        }
        let bytes = std::fs::read(path).ok()?;
        let img = image::load_from_memory(&bytes).ok()?.to_rgba8();
        let size = [img.width() as usize, img.height() as usize];
        let color = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
        let tex = ctx.load_texture(
            path.to_string_lossy(),
            color,
            egui::TextureOptions::LINEAR,
        );
        let id = tex.id();
        self.textures.insert(path.clone(), tex);
        Some(id)
    }

    /// Request (or fetch) the strip/wave PNG for a clip at lane size.
    fn artifact(&mut self, clip: &Clip, kind: Kind, px_w: f32, px_h: f32) -> Option<PathBuf> {
        let src = proxy_for(&self.project_dir, &clip.src)
            .unwrap_or_else(|| self.project_dir.join(&clip.src));
        // Quantize the pixel budget so window resizes don't regenerate
        // artifacts continuously (the cache keys on exact size).
        let px_w = (px_w.max(1.0) as u32).next_multiple_of(128);
        let px_h = px_h.max(1.0) as u32;
        let frame_w = (px_h * 16) / 9;
        let frames = (px_w / frame_w.max(1)).max(1);
        let spec = Spec {
            kind,
            src,
            in_s: clip.in_.as_secs_f64(),
            out_s: clip.out.as_secs_f64(),
            px_w,
            px_h,
            frames,
        };
        // While a resize regenerates the exact size (a 1.5-hour film takes
        // ~20 s), keep the lane's picture: the nearest ready width is drawn
        // stretched until the fresh one lands.
        self.media.get(spec.clone()).or_else(|| self.media.nearest(&spec))
    }

    fn draw_preview(&mut self, ui: &mut egui::Ui) {
        let rect = ui.available_rect_before_wrap();
        ui.painter().rect_filled(rect, CornerRadius::ZERO, Color32::BLACK);

        // Upload the latest frame if it changed since last paint.
        let seq = self.player.frame_seq();
        if seq != self.preview_seq || self.preview_tex.is_none() {
            let img = self.player.with_frame(|f| {
                egui::ColorImage::from_rgba_unmultiplied([f.width, f.height], &f.rgba)
            });
            if let Some(img) = img {
                match &mut self.preview_tex {
                    Some(tex) => tex.set(img, egui::TextureOptions::LINEAR),
                    None => {
                        self.preview_tex = Some(ui.ctx().load_texture(
                            "preview",
                            img,
                            egui::TextureOptions::LINEAR,
                        ))
                    }
                }
                self.preview_seq = seq;
            }
        }

        if let Some(tex) = &self.preview_tex {
            let size = tex.size_vec2();
            let scale = (rect.width() / size.x).min(rect.height() / size.y);
            let draw = Vec2::new(size.x * scale, size.y * scale);
            let draw_rect = Rect::from_center_size(rect.center(), draw);
            ui.painter().image(
                tex.id(),
                draw_rect,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        } else {
            let msg = match (&self.player_err, self.state.total == Time::ZERO) {
                (Some(e), _) => format!("preview unavailable: {e}"),
                (None, true) => "empty timeline — add clips with `viode add`".to_string(),
                (None, false) => "waiting for first frame…".to_string(),
            };
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                msg,
                FontId::proportional(15.0),
                self.theme.dim,
            );
        }

        // QC scopes overlay, bottom-right (paused only — they describe
        // the frame the playhead is parked on).
        if self.scopes_on && !self.state.playing {
            let mut x = rect.right() - 8.0;
            for tex in self.scope_tex.iter().flatten() {
                let size = tex.size_vec2();
                let w = (rect.width() * 0.22).min(size.x);
                let h = w * size.y / size.x;
                let scope_rect = Rect::from_min_max(
                    Pos2::new(x - w, rect.bottom() - 8.0 - h),
                    Pos2::new(x, rect.bottom() - 8.0),
                );
                ui.painter().rect_filled(
                    scope_rect.expand(2.0),
                    CornerRadius::same(2),
                    Color32::from_black_alpha(160),
                );
                ui.painter().image(
                    tex.id(),
                    scope_rect,
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                    Color32::WHITE,
                );
                x -= w + 10.0;
            }
        }
    }

    /// Overlay tracks split into the NLE stack: video-carrying lanes go
    /// above V1 (topmost last in file order, matching GES layers), audio
    /// lanes go below A1.
    fn overlay_stacks(&self) -> (Vec<(String, usize)>, Vec<(String, usize)>) {
        let mut video = Vec::new();
        let mut audio = Vec::new();
        for (t, track) in self.editor.project.tracks.iter().enumerate().skip(1) {
            if track.kind == TrackKind::Audio {
                audio.push((format!("A{}", audio.len() + 2), t));
            } else {
                video.push((format!("V{}", video.len() + 2), t));
            }
        }
        video.reverse(); // topmost lane first, like the GES layer order
        (video, audio)
    }

    fn timeline_height(&self) -> f32 {
        let (video, audio) = self.overlay_stacks();
        let titles = if self.editor.project.titles.is_empty() {
            0.0
        } else {
            TITLE_LANE_H + LANE_GAP
        };
        HEADER_H
            + RULER_H
            + titles
            + video.len() as f32 * (VIDEO_LANE_H + LANE_GAP)
            + STRIP_LANE_H
            + LANE_GAP
            + WAVE_LANE_H
            + audio.len() as f32 * (AUDIO_LANE_H + LANE_GAP)
            + 10.0
    }

    fn draw_header(&self, painter: &egui::Painter, panel: &Rect) {
        let y = panel.top() + HEADER_H / 2.0;
        let icon = if self.state.playing { "⏸" } else { "▶" };
        painter.text(
            Pos2::new(panel.left() + 10.0, y),
            Align2::LEFT_CENTER,
            icon,
            FontId::proportional(15.0),
            self.theme.fg,
        );
        // The Premiere move: the timecode is the loudest thing on the
        // panel, in the theme accent.
        let tc_rect = painter.text(
            Pos2::new(panel.left() + 34.0, y),
            Align2::LEFT_CENTER,
            fmt_tc(self.state.playhead),
            FontId::monospace(17.0),
            self.theme.accent,
        );
        let mut x = tc_rect.right() + 8.0;
        let total_rect = painter.text(
            Pos2::new(x, y),
            Align2::LEFT_CENTER,
            format!("/ {}", fmt_tc(self.state.total)),
            FontId::monospace(12.0),
            self.theme.dim,
        );
        x = total_rect.right() + 12.0;
        if self.state.playing && self.state.rate != 1.0 {
            let r = painter.text(
                Pos2::new(x, y),
                Align2::LEFT_CENTER,
                format!("{}x", self.state.rate),
                FontId::monospace(12.0),
                self.theme.accent,
            );
            x = r.right() + 12.0;
        }
        if !self.editor.message.is_empty() {
            painter.text(
                Pos2::new(x, y),
                Align2::LEFT_CENTER,
                &self.editor.message,
                FontId::proportional(11.0),
                self.theme.dim,
            );
        }
    }

    /// The visible help trigger: a real button in the header, not a dim
    /// painter hint. Toggles the same overlay as the `?` key.
    fn help_button(&mut self, ui: &mut egui::Ui, panel: &Rect) {
        let rect = Rect::from_center_size(
            Pos2::new(panel.right() - 44.0, panel.top() + HEADER_H / 2.0),
            egui::vec2(64.0, 20.0),
        );
        let response = ui.allocate_rect(rect, Sense::click());
        let fill = if response.hovered() {
            self.theme.accent.gamma_multiply(0.35)
        } else {
            self.theme.accent.gamma_multiply(0.18)
        };
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::same(10), fill);
        painter.rect_stroke(
            rect,
            CornerRadius::same(10),
            Stroke::new(1.0_f32, self.theme.accent),
            egui::StrokeKind::Inside,
        );
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "?  Help",
            FontId::proportional(11.0),
            self.theme.accent,
        );
        if response.clicked() {
            self.state.show_help = !self.state.show_help;
        }
        response.on_hover_text("Keyboard and mouse reference (?)");
    }

    fn draw_ruler(&self, painter: &egui::Painter, map: &TimelineMap, y: f32) {
        let secs = self.state.total.as_secs_f64();
        let step = tick_step(secs, map.width);
        let mut t = 0.0;
        while t <= secs {
            let x = map.x_of(Time((t * 1e9) as u64));
            painter.vline(
                x,
                egui::Rangef::new(y + RULER_H - 6.0, y + RULER_H),
                Stroke::new(1.0_f32, self.theme.dim),
            );
            painter.text(
                Pos2::new(x + 3.0, y + 1.0),
                Align2::LEFT_TOP,
                fmt_ruler(t),
                FontId::monospace(9.0),
                self.theme.dim,
            );
            t += step;
        }
    }

    fn draw_timeline(&mut self, ui: &mut egui::Ui) {
        let panel = ui.available_rect_before_wrap();
        ui.painter().rect_filled(panel, CornerRadius::ZERO, self.theme.bg);
        let map = TimelineMap::new(
            self.state.total,
            panel.left() + GUTTER,
            panel.width() - GUTTER - 8.0,
        );

        // The ruler scrubs (clips own their drags, so scrubbing lives on
        // the ruler like every NLE).
        let ruler_rect = Rect::from_min_max(
            Pos2::new(panel.left() + GUTTER, panel.top() + HEADER_H),
            Pos2::new(panel.right(), panel.top() + HEADER_H + RULER_H),
        );
        let response = ui.allocate_rect(ruler_rect, Sense::click_and_drag());
        if response.clicked() || response.dragged() {
            if let Some(pos) = response.interact_pointer_pos() {
                let t = map.time_at(pos.x);
                let cmds = self.state.seek_to(t);
                self.apply(cmds);
            }
        }
        // Right-click on the ruler: seek there first, then offer the
        // playhead and range verbs.
        if response.secondary_clicked() {
            if let Some(pos) = response.interact_pointer_pos() {
                let t = map.time_at(pos.x);
                let cmds = self.state.seek_to(t);
                self.apply(cmds);
            }
        }
        response.context_menu(|ui| self.ruler_menu(ui));

        let painter = ui.painter().clone();
        self.draw_header(&painter, &panel);
        self.help_button(ui, &panel);
        self.draw_toolbar(ui, &panel);
        let mut y = panel.top() + HEADER_H;
        let ruler_y = y;
        // Marked range band ([ and ]) under the ruler ticks.
        if let Some((a, b)) = self.state.marked_range() {
            let band = Rect::from_min_max(
                Pos2::new(map.x_of(a), y),
                Pos2::new(map.x_of(b), y + RULER_H),
            );
            painter.rect_filled(
                band,
                CornerRadius::ZERO,
                self.theme.accent.gamma_multiply(0.25),
            );
        }
        self.draw_ruler(&painter, &map, y);
        // Markers: diamonds on the ruler. Hover names them, click seeks,
        // right-click removes — the mouse-complete surface for `mark`.
        for (mi, marker) in self.editor.project.markers.clone().into_iter().enumerate() {
            let x = map.x_of(marker.at);
            let center = Pos2::new(x, y + RULER_H - 9.0);
            let color = marker
                .color
                .as_deref()
                .and_then(parse_hex_color)
                .unwrap_or(self.theme.accent);
            painter.text(
                center,
                Align2::CENTER_CENTER,
                "◆",
                FontId::proportional(10.0),
                color,
            );
            let hit = Rect::from_center_size(center, egui::vec2(12.0, 12.0));
            let resp = ui.interact(hit, ui.id().with(("marker", mi)), Sense::click());
            if resp.clicked() {
                let cmds = self.state.seek_to(marker.at);
                self.apply(cmds);
            }
            let resp = resp.on_hover_text(format!("{} — {}", marker.at, marker.text));
            resp.context_menu(|ui| {
                if ui.button("Remove marker").clicked() {
                    ui.close();
                    if self.editor.marker_remove(mi) {
                        self.editor.end_stage();
                        self.model_changed();
                    }
                }
            });
        }
        y += RULER_H;

        // Titles lane.
        if !self.editor.project.titles.is_empty() {
            let lane = lane_rect(&panel, y, TITLE_LANE_H);
            painter.rect_filled(lane, CornerRadius::ZERO, self.theme.lane);
            self.track_header(&painter, &panel, y, TITLE_LANE_H, "T", "titles", true);
            for (i, title) in self.editor.project.titles.clone().into_iter().enumerate() {
                let rect = Rect::from_min_max(
                    Pos2::new(map.x_of(title.at), y + 1.0),
                    Pos2::new(map.x_of(title.at + title.dur), y + TITLE_LANE_H - 1.0),
                );
                let selected = self.editor.selected_title() == Some(i);
                painter.rect_filled(rect, CornerRadius::same(2), self.theme.title);
                if selected {
                    painter.rect_stroke(
                        rect,
                        CornerRadius::same(2),
                        Stroke::new(2.0_f32, self.theme.accent),
                        egui::StrokeKind::Outside,
                    );
                }
                painter.text(
                    Pos2::new(rect.left() + 4.0, rect.center().y),
                    Align2::LEFT_CENTER,
                    title.text.clone(),
                    FontId::proportional(10.0),
                    self.theme.bg,
                );
                let resp = ui.interact(
                    rect,
                    ui.id().with(("title", i)),
                    Sense::click_and_drag(),
                );
                if resp.clicked() {
                    self.editor.select_title(i);
                }
                if resp.secondary_clicked() {
                    self.editor.select_title(i);
                }
                resp.context_menu(|ui| {
                    let ctx = ui.ctx().clone();
                    if ui.button("Delete title\tD").clicked() {
                        ui.close();
                        self.perform(&ctx, Action::Delete);
                    }
                    ui.label(
                        egui::RichText::new("text, position, color: inspector →")
                            .size(10.0)
                            .color(self.theme.dim),
                    );
                });
                if resp.drag_started() {
                    if let Some(pos) = resp.interact_pointer_pos() {
                        self.editor.select_title(i);
                        self.title_drag = Some((i, pos.x - map.x_of(title.at)));
                    }
                }
                if resp.dragged() {
                    if let (Some((di, grab)), Some(pos)) = (self.title_drag, resp.interact_pointer_pos()) {
                        if di == i {
                            let at = map.time_at(pos.x - grab);
                            if self.editor.title_edit(|t| t.at = at) {
                                self.model_changed();
                            }
                        }
                    }
                }
                if resp.drag_stopped() {
                    self.title_drag = None;
                    self.editor.end_stage();
                }
            }
            y += TITLE_LANE_H + LANE_GAP;
        }

        let (video_overlays, audio_overlays) = self.overlay_stacks();

        for (badge, t) in &video_overlays {
            y = self.draw_track_lane(ui, &painter, &panel, &map, y, VIDEO_LANE_H, badge, *t, Kind::Strip);
        }

        // V1 + A1: the main sequence, filmstrip over waveform.
        for (lane_h, badge, kind) in [
            (STRIP_LANE_H, "V1", Kind::Strip),
            (WAVE_LANE_H, "A1", Kind::Wave),
        ] {
            let lane = lane_rect(&panel, y, lane_h);
            painter.rect_filled(lane, CornerRadius::ZERO, self.theme.lane);
            let name = self.editor.project.main().name.clone();
            self.track_header(&painter, &panel, y, lane_h, badge, &name, true);
            let main = self.editor.project.main().clone();
            let positions = main.positions();
            for (index, (clip, start)) in main.clips.iter().zip(&positions).enumerate() {
                let rect = Rect::from_min_max(
                    Pos2::new(map.x_of(*start), y + 1.0),
                    Pos2::new(map.x_of(*start + clip.len()), y + lane_h - 1.0),
                );
                self.clip_widget(ui, &painter, &map, clip, rect, kind, 0, index, 255);
            }
            y += lane_h + LANE_GAP;
        }

        for (badge, t) in &audio_overlays {
            y = self.draw_track_lane(ui, &painter, &panel, &map, y, AUDIO_LANE_H, badge, *t, Kind::Wave);
        }

        // Playhead across ruler and every lane, with a handle in the ruler.
        let x = map.x_of(self.state.playhead);
        painter.vline(
            x,
            egui::Rangef::new(ruler_y, y),
            Stroke::new(2.0_f32, self.theme.accent),
        );
        let handle = [
            Pos2::new(x - 5.0, ruler_y),
            Pos2::new(x + 5.0, ruler_y),
            Pos2::new(x, ruler_y + 8.0),
        ];
        painter.add(egui::Shape::convex_polygon(
            handle.to_vec(),
            self.theme.accent,
            Stroke::NONE,
        ));
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_track_lane(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        panel: &Rect,
        map: &TimelineMap,
        y: f32,
        lane_h: f32,
        badge: &str,
        track_idx: usize,
        kind: Kind,
    ) -> f32 {
        let lane = lane_rect(panel, y, lane_h);
        painter.rect_filled(lane, CornerRadius::ZERO, self.theme.lane);
        let track: Track = self.editor.project.tracks[track_idx].clone();
        self.track_header(&painter, panel, y, lane_h, badge, &track.name, track.enabled);
        if track_idx != 0 {
            let hit = Rect::from_min_size(
                Pos2::new(panel.left(), y),
                egui::vec2(GUTTER, lane_h),
            );
            let resp = ui.interact(hit, ui.id().with(("track-head", track_idx)), Sense::click());
            let resp = resp.on_hover_text("right-click: track options");
            resp.context_menu(|ui| {
                let label = if track.enabled { "Disable track" } else { "Enable track" };
                if ui.button(label).clicked() {
                    ui.close();
                    if self.editor.track_toggle(track_idx) {
                        self.editor.end_stage();
                        self.model_changed();
                    }
                }
                let ctx = ui.ctx().clone();
                if ui.button("Add video overlay track").clicked() {
                    ui.close();
                    self.perform(&ctx, Action::AddVideoTrack);
                }
                if ui.button("Add music track").clicked() {
                    ui.close();
                    self.perform(&ctx, Action::AddMusicTrack);
                }
            });
        }
        let alpha = if track.enabled { 255 } else { 90 };
        for (index, clip) in track.clips.iter().enumerate() {
            let (start, end) = clip.span();
            let rect = Rect::from_min_max(
                Pos2::new(map.x_of(start), y + 1.0),
                Pos2::new(map.x_of(end), y + lane_h - 1.0),
            );
            self.clip_widget(ui, painter, map, clip, rect, kind, track_idx, index, alpha);
        }
        y + lane_h + LANE_GAP
    }

    /// Draw one clip and wire its mouse grammar: click selects, body-drag
    /// moves (alt: slip, shift+alt: slide), edge-drag trims (alt: roll).
    #[allow(clippy::too_many_arguments)]
    fn clip_widget(
        &mut self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        map: &TimelineMap,
        clip: &Clip,
        rect: Rect,
        kind: Kind,
        track: usize,
        index: usize,
        alpha: u8,
    ) {
        if rect.width() < 2.0 {
            return;
        }
        painter.rect_filled(
            rect,
            CornerRadius::same(2),
            self.theme.clip.gamma_multiply(alpha as f32 / 255.0),
        );
        if let Some(png) = self.artifact(clip, kind, rect.width(), rect.height()) {
            if let Some(tex) = self.tex_for(ui.ctx(), &png) {
                // Waveforms take the theme's audio color; strips stay true.
                let tint = match kind {
                    Kind::Wave => self.theme.wave.gamma_multiply(alpha as f32 / 255.0),
                    Kind::Strip => Color32::from_white_alpha(alpha),
                };
                painter.image(
                    tex,
                    rect.shrink(1.0),
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                    tint,
                );
            }
        }
        let selected = self.editor.selected_clip() == Some((track, index));
        painter.rect_stroke(
            rect,
            CornerRadius::same(2),
            Stroke::new(
                if selected { 2.0_f32 } else { 1.0_f32 },
                if selected { self.theme.accent } else { self.theme.clip_edge },
            ),
            egui::StrokeKind::Inside,
        );
        if kind == Kind::Strip && rect.width() > 60.0 {
            let name = clip
                .label
                .clone()
                .or_else(|| {
                    clip.src
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                })
                .unwrap_or_default();
            painter.text(
                Pos2::new(rect.left() + 5.0, rect.top() + 3.0),
                Align2::LEFT_TOP,
                name,
                FontId::proportional(10.0),
                self.theme.fg.gamma_multiply(alpha as f32 / 255.0),
            );
        }

        // --- interaction ---------------------------------------------------
        let (map_width, total_ns) = (map.width, self.state.total.0);
        let px_to_ns = move |dx: f32| -> i64 {
            if map_width <= 0.0 || total_ns == 0 {
                return 0;
            }
            (dx as f64 / map_width as f64 * total_ns as f64) as i64
        };
        let id = ui.id().with(("clip", track, index, kind == Kind::Wave));
        let wide = rect.width() > 3.0 * EDGE_W;
        let (left_rect, mid_rect, right_rect) = if wide {
            (
                Rect::from_min_max(rect.min, Pos2::new(rect.left() + EDGE_W, rect.bottom())),
                Rect::from_min_max(
                    Pos2::new(rect.left() + EDGE_W, rect.top()),
                    Pos2::new(rect.right() - EDGE_W, rect.bottom()),
                ),
                Rect::from_min_max(Pos2::new(rect.right() - EDGE_W, rect.top()), rect.max),
            )
        } else {
            (Rect::NOTHING, rect, Rect::NOTHING)
        };

        let mid = ui.interact(mid_rect, id.with("mid"), Sense::click_and_drag());
        if mid.clicked() {
            self.editor.select_clip(track, index);
        }
        if mid.secondary_clicked() {
            self.editor.select_clip(track, index);
        }
        mid.context_menu(|ui| self.clip_menu(ui, track));
        if mid.drag_started() {
            self.editor.select_clip(track, index);
            let alt = ui.input(|i| i.modifiers.alt);
            let shift = ui.input(|i| i.modifiers.shift);
            let kind = match (alt, shift) {
                (true, true) => DragKind::Slide,
                (true, false) => DragKind::Slip,
                _ => DragKind::Move,
            };
            self.drag_start_x = mid.interact_pointer_pos().map_or(0.0, |p| p.x);
            self.editor.drag_begin(kind, track, index);
        }
        if mid.dragged() && self.editor.dragging() {
            if let Some(pos) = mid.interact_pointer_pos() {
                let delta = px_to_ns(pos.x - self.drag_start_x);
                let drop = (track == 0).then(|| self.drop_index(map, pos.x));
                self.editor.drag_update(delta, drop.flatten());
            }
        }
        if mid.drag_stopped() && self.editor.dragging() {
            self.editor.drag_end();
            self.model_changed();
        }
        if mid.hovered() && ui.input(|i| i.modifiers.alt) {
            ui.ctx().set_cursor_icon(CursorIcon::Grab);
        }

        for (edge_rect, is_left) in [(left_rect, true), (right_rect, false)] {
            if edge_rect == Rect::NOTHING {
                continue;
            }
            let resp = ui.interact(
                edge_rect,
                id.with(if is_left { "in" } else { "out" }),
                Sense::drag(),
            );
            if resp.hovered() {
                ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
            }
            if resp.drag_started() {
                self.editor.select_clip(track, index);
                let alt = ui.input(|i| i.modifiers.alt);
                let n = self.editor.project.tracks[track].clips.len();
                // Alt on an interior main-track cut = roll the boundary.
                let kind = if alt && track == 0 && ((is_left && index > 0) || (!is_left && index + 1 < n)) {
                    DragKind::Roll
                } else if is_left {
                    DragKind::TrimIn
                } else {
                    DragKind::TrimOut
                };
                let idx = if kind == DragKind::Roll && !is_left { index + 1 } else { index };
                self.drag_start_x = resp.interact_pointer_pos().map_or(0.0, |p| p.x);
                self.editor.drag_begin(kind, track, idx);
            }
            if resp.dragged() && self.editor.dragging() {
                if let Some(pos) = resp.interact_pointer_pos() {
                    self.editor.drag_update(px_to_ns(pos.x - self.drag_start_x), None);
                }
            }
            if resp.drag_stopped() && self.editor.dragging() {
                self.editor.drag_end();
                self.model_changed();
            }
        }
    }

    /// Where a main-track clip dragged to pointer-x should land: the slot
    /// whose midpoint the pointer has crossed.
    fn drop_index(&self, map: &TimelineMap, x: f32) -> Option<usize> {
        let t = map.time_at(x);
        let main = self.editor.project.main();
        let positions = main.positions();
        for (i, (clip, start)) in main.clips.iter().zip(&positions).enumerate() {
            if t < *start + Time(clip.len().0 / 2) {
                return Some(i);
            }
        }
        Some(main.clips.len().saturating_sub(1))
    }

    /// The left gutter cell for a lane: "V1"-style badge plus track name,
    /// dimmed when the track is disabled.
    fn track_header(
        &self,
        painter: &egui::Painter,
        panel: &Rect,
        y: f32,
        h: f32,
        badge: &str,
        name: &str,
        enabled: bool,
    ) {
        let cell = Rect::from_min_max(
            Pos2::new(panel.left(), y),
            Pos2::new(panel.left() + GUTTER - 4.0, y + h),
        );
        painter.rect_filled(cell, CornerRadius::ZERO, self.theme.lane.gamma_multiply(0.7));
        let color = if enabled {
            self.theme.fg
        } else {
            self.theme.dim.gamma_multiply(0.6)
        };
        painter.text(
            Pos2::new(cell.left() + 8.0, cell.center().y),
            Align2::LEFT_CENTER,
            badge,
            FontId::monospace(11.0),
            color,
        );
        let label = if enabled {
            name.to_string()
        } else {
            format!("{name} (off)")
        };
        painter.text(
            Pos2::new(cell.left() + 32.0, cell.center().y),
            Align2::LEFT_CENTER,
            label,
            FontId::proportional(10.0),
            if enabled { self.theme.dim } else { self.theme.dim.gamma_multiply(0.6) },
        );
    }

    // -- inspector ----------------------------------------------------------

    fn draw_inspector(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        match self.editor.selection {
            Selection::Clip { .. } if self.editor.selected_clip().is_some() => {
                self.inspect_clip(ui)
            }
            Selection::Title(_) if self.editor.selected_title().is_some() => {
                self.inspect_title(ui)
            }
            _ => {
                ui.label(egui::RichText::new("Inspector").strong());
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new("Click a clip or title to edit it.")
                        .color(self.theme.dim)
                        .size(11.0),
                );
                ui.add_space(8.0);
                if ui.button("+ title at playhead").clicked() {
                    let ph = self.state.playhead;
                    if self.editor.title_add(ph) {
                        self.editor.end_stage();
                        self.model_changed();
                    }
                }
            }
        }
    }

    fn inspect_clip(&mut self, ui: &mut egui::Ui) {
        let (track, index) = self.editor.selected_clip().unwrap();
        let clip = self.editor.project.tracks[track].clips[index].clone();
        let name = clip
            .src
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        ui.label(egui::RichText::new(format!("{name}")).strong());
        ui.label(
            egui::RichText::new(format!(
                "track {track} clip {index} · src {} – {} · {} on timeline",
                clip.in_, clip.out, clip.len()
            ))
            .color(self.theme.dim)
            .size(10.0),
        );
        ui.separator();

        let mut changed = false;
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.label("Speed");
            let mut rate = clip.rate.unwrap_or(1.0);
            if ui
                .add(egui::Slider::new(&mut rate, 0.25..=4.0).logarithmic(true).text("rate"))
                .changed()
            {
                changed |= self.editor.set_rate(rate);
            }
            ui.horizontal(|ui| {
                ui.label("LUT");
                let current = clip
                    .lut
                    .as_ref()
                    .and_then(|p| p.file_name())
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "none".into());
                ui.label(egui::RichText::new(current).color(self.theme.dim).size(10.0));
                if ui.button("pick…").clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("3D LUT", &["cube"])
                        .pick_file()
                    {
                        if self.editor.set_lut(Some(path)) {
                            self.editor.end_stage();
                            self.model_changed();
                        }
                    }
                }
                if clip.lut.is_some() && ui.button("clear").clicked() && self.editor.set_lut(None) {
                    self.editor.end_stage();
                    self.model_changed();
                }
            });
            let mut steady_on = clip.steady.is_some();
            let mut smoothing = clip.steady.unwrap_or(10);
            ui.horizontal(|ui| {
                if ui.checkbox(&mut steady_on, "stabilize").changed() {
                    changed |= self.editor.set_steady(steady_on.then_some(smoothing));
                }
                if steady_on {
                    let mut v = smoothing as i32;
                    if ui.add(egui::DragValue::new(&mut v).range(1..=100).prefix("smoothing ")).changed() {
                        smoothing = v as u32;
                        changed |= self.editor.set_steady(Some(smoothing));
                    }
                }
            });
            let mut clean_on = clip.clean.is_some();
            let mut clean_db = clip.clean.unwrap_or(12.0);
            ui.horizontal(|ui| {
                if ui.checkbox(&mut clean_on, "clean voice").changed() {
                    changed |= self.editor.set_clean(clean_on.then_some(clean_db));
                }
                if clean_on {
                    if ui
                        .add(egui::DragValue::new(&mut clean_db).range(0.01..=97.0).prefix("nr dB "))
                        .changed()
                    {
                        changed |= self.editor.set_clean(Some(clean_db));
                    }
                }
            });
            if track != 0 {
                ui.horizontal(|ui| {
                    ui.label("refit to");
                    ui.add(
                        egui::DragValue::new(&mut self.refit_to)
                            .range(1.0..=36_000.0)
                            .suffix(" s"),
                    );
                    if ui.button("apply").clicked() {
                        let dir = self.project_dir.clone();
                        if let (Ok(target), Ok(fade)) = (
                            Time::from_secs_f64(self.refit_to),
                            Time::from_secs_f64(0.5),
                        ) {
                            if self.editor.refit_selected(&dir, target, fade) {
                                self.model_changed();
                            }
                        }
                    }
                });
            }
            if track != 0 {
                ui.horizontal(|ui| {
                    ui.label("matte");
                    let current = clip.matte.clone().unwrap_or_else(|| "off".into());
                    let mut sel = current.clone();
                    egui::ComboBox::from_id_salt("matte")
                        .selected_text(sel.clone())
                        .show_ui(ui, |ui| {
                            for m in ["off", "green", "blue"] {
                                ui.selectable_value(&mut sel, m.to_string(), m);
                            }
                        });
                    if sel != current {
                        changed |= self
                            .editor
                            .set_matte((sel != "off").then_some(sel));
                    }
                });
            }
            ui.horizontal(|ui| {
                ui.label("mask");
                ui.add(
                    egui::TextEdit::singleline(&mut self.mask_region)
                        .hint_text("x,y,w,h")
                        .desired_width(110.0),
                );
                egui::ComboBox::from_id_salt("mask_kind")
                    .selected_text(self.mask_kind.clone())
                    .show_ui(ui, |ui| {
                        for k in ["blur", "pixelate"] {
                            ui.selectable_value(&mut self.mask_kind, k.to_string(), k);
                        }
                    });
                ui.checkbox(&mut self.mask_follow, "follow");
                if ui.button("apply").clicked() {
                    let parts: Vec<f64> = self
                        .mask_region
                        .split(',')
                        .filter_map(|v| v.trim().parse().ok())
                        .collect();
                    if parts.len() == 4 {
                        let mask = viode_core::Mask {
                            region: [parts[0], parts[1], parts[2], parts[3]],
                            kind: self.mask_kind.clone(),
                            follow: self.mask_follow,
                        };
                        if self.editor.set_mask(Some(mask)) {
                            self.editor.end_stage();
                            self.model_changed();
                        }
                    } else {
                        self.editor.message = "mask region: four numbers x,y,w,h".into();
                    }
                }
                if clip.mask.is_some() && ui.button("clear").clicked() && self.editor.set_mask(None) {
                    self.editor.end_stage();
                    self.model_changed();
                }
            });
            ui.horizontal(|ui| {
                ui.label("ramp");
                ui.add(egui::DragValue::new(&mut self.ramp_from).speed(0.05).range(0.05..=20.0).prefix("from "));
                ui.add(egui::DragValue::new(&mut self.ramp_to).speed(0.05).range(0.05..=20.0).prefix("to "));
                if ui.button("apply").clicked() && self.editor.ramp(self.ramp_from, self.ramp_to, 8) {
                    self.editor.end_stage();
                    self.model_changed();
                }
            });

            ui.add_space(6.0);
            ui.label("Audio");
            let mut vol = clip.volume.unwrap_or(1.0);
            if ui
                .add(egui::Slider::new(&mut vol, 0.0..=2.0).text("gain"))
                .changed()
            {
                changed |= self.editor.set_volume(vol);
            }
            let mut pan = clip.pan.unwrap_or(0.0);
            if ui
                .add(egui::Slider::new(&mut pan, -1.0..=1.0).text("pan"))
                .changed()
            {
                changed |= self.editor.set_pan(pan);
            }

            if track == 0 && index > 0 {
                ui.add_space(6.0);
                ui.label("Transition (with previous clip)");
                let mut fade = clip.transition.map(|t| t.as_secs_f64()).unwrap_or(0.0);
                if ui
                    .add(egui::Slider::new(&mut fade, 0.0..=3.0).text("fade s"))
                    .changed()
                {
                    let d = (fade > 0.0)
                        .then(|| Time::from_secs_f64(fade).ok())
                        .flatten();
                    changed |= self.editor.set_fade(d, clip.transition_kind.clone());
                }
                let mut kind = clip
                    .transition_kind
                    .clone()
                    .unwrap_or_else(|| "crossfade".into());
                egui::ComboBox::from_label("kind")
                    .selected_text(kind.clone())
                    .show_ui(ui, |ui| {
                        for k in viode_core::TRANSITION_KINDS.iter().copied() {
                            if ui.selectable_value(&mut kind, k.to_string(), k).changed() {
                                changed |= self
                                    .editor
                                    .set_fade(clip.transition, Some(kind.clone()));
                            }
                        }
                    });
            }

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("Place");
                if ui.small_button("reset").clicked() {
                    changed |= self.editor.clear_place();
                }
            });
            let [mut px, mut py] = clip.pos.unwrap_or([0.0, 0.0]);
            if ui
                .add(egui::Slider::new(&mut px, -1.0..=1.0).text("x"))
                .changed()
                | ui.add(egui::Slider::new(&mut py, -1.0..=1.0).text("y"))
                    .changed()
            {
                changed |= self.editor.set_pos(px, py);
            }
            let mut scale = clip.scale.unwrap_or(1.0);
            if ui
                .add(egui::Slider::new(&mut scale, 0.05..=2.0).text("scale"))
                .changed()
            {
                changed |= self.editor.set_scale(scale);
            }
            let mut rot = clip.rotate.unwrap_or(0.0);
            if ui
                .add(egui::Slider::new(&mut rot, -180.0..=180.0).text("rotate"))
                .changed()
            {
                changed |= self.editor.set_rotate(rot);
            }
            let mut op = clip.opacity.unwrap_or(1.0);
            if ui
                .add(egui::Slider::new(&mut op, 0.0..=1.0).text("opacity"))
                .changed()
            {
                changed |= self.editor.set_opacity(op);
            }

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("Color");
                if ui.small_button("reset").clicked() {
                    changed |= self.editor.clear_color();
                }
            });
            let g = clip.color.clone().unwrap_or(viode_core::ColorGrade {
                brightness: None,
                contrast: None,
                saturation: None,
                hue: None, gamma: None,
            });
            for (label, field, cur, lo, hi) in [
                ("brightness", "brightness", g.brightness.unwrap_or(0.0), -1.0, 1.0),
                ("contrast", "contrast", g.contrast.unwrap_or(1.0), 0.0, 2.0),
                ("saturation", "saturation", g.saturation.unwrap_or(1.0), 0.0, 2.0),
                ("hue", "hue", g.hue.unwrap_or(0.0), -1.0, 1.0),
                ("gamma", "gamma", g.gamma.unwrap_or(1.0), 0.1, 4.0),
            ] {
                let mut v = cur;
                if ui
                    .add(egui::Slider::new(&mut v, lo..=hi).text(label))
                    .changed()
                {
                    changed |= self.editor.set_grade(field, v);
                }
            }

            ui.add_space(6.0);
            ui.label("Keyframes (source time)");
            let mut remove: Option<usize> = None;
            for (k, key) in clip.keys.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(format!("{} @ {} = {}", key.prop, key.at, key.value))
                            .monospace()
                            .size(10.0),
                    );
                    if ui.small_button("✕").clicked() {
                        remove = Some(k);
                    }
                });
            }
            if let Some(k) = remove {
                changed |= self.editor.key_remove(k);
            }
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("key_prop")
                    .selected_text(self.key_prop.clone())
                    .width(70.0)
                    .show_ui(ui, |ui| {
                        for p in ["volume", "alpha", "x", "y", "scale"] {
                            ui.selectable_value(&mut self.key_prop, p.into(), p);
                        }
                    });
                ui.add(egui::DragValue::new(&mut self.key_value).speed(0.05).range(0.0..=10.0));
                if ui.small_button("+ at playhead").clicked() {
                    if let Some(at) = self.selected_source_time() {
                        let (prop, value) = (self.key_prop.clone(), self.key_value);
                        changed |= self.editor.key_add(&prop, at, value);
                    } else {
                        self.editor.message = "playhead is not over this clip".into();
                    }
                }
            });
        });
        if changed {
            self.model_changed();
        }
    }

    fn inspect_title(&mut self, ui: &mut egui::Ui) {
        let i = self.editor.selected_title().unwrap();
        let title = self.editor.project.titles[i].clone();
        ui.label(egui::RichText::new("Title").strong());
        ui.separator();
        let mut changed = false;

        let mut text = title.text.clone();
        if ui.text_edit_singleline(&mut text).changed() {
            changed |= self.editor.title_edit(|t| t.text = text.clone());
        }
        let mut at = title.at.as_secs_f64();
        let mut dur = title.dur.as_secs_f64();
        ui.horizontal(|ui| {
            ui.label("at");
            if ui.add(egui::DragValue::new(&mut at).speed(0.05).range(0.0..=f64::MAX)).changed() {
                if let Ok(t) = Time::from_secs_f64(at) {
                    changed |= self.editor.title_edit(|ti| ti.at = t);
                }
            }
            ui.label("dur");
            if ui.add(egui::DragValue::new(&mut dur).speed(0.05).range(0.1..=f64::MAX)).changed() {
                if let Ok(t) = Time::from_secs_f64(dur) {
                    changed |= self.editor.title_edit(|ti| ti.dur = t);
                }
            }
        });
        let mut font = title.font.clone().unwrap_or_default();
        ui.horizontal(|ui| {
            ui.label("font");
            if ui.text_edit_singleline(&mut font).changed() {
                let f = (!font.is_empty()).then(|| font.clone());
                changed |= self.editor.title_edit(|t| t.font = f);
            }
        });
        let (mut x, mut y) = (title.xpos.unwrap_or(0.5), title.ypos.unwrap_or(0.5));
        if ui.add(egui::Slider::new(&mut x, 0.0..=1.0).text("x")).changed() {
            changed |= self.editor.title_edit(|t| t.xpos = Some(x));
        }
        if ui.add(egui::Slider::new(&mut y, 0.0..=1.0).text("y")).changed() {
            changed |= self.editor.title_edit(|t| t.ypos = Some(y));
        }
        let mut color = title.color.clone().unwrap_or_default();
        ui.horizontal(|ui| {
            ui.label("color");
            if ui.text_edit_singleline(&mut color).changed() {
                let valid = color.is_empty()
                    || (color.starts_with('#') && matches!(color.len(), 7 | 9));
                if valid {
                    let c = (!color.is_empty()).then(|| color.clone());
                    changed |= self.editor.title_edit(|t| t.color = c);
                }
            }
        });
        ui.add_space(6.0);
        if ui.button("delete title").clicked() {
            let ph = self.state.playhead;
            changed |= self.editor.delete(ph);
        }
        if changed {
            self.model_changed();
        }
    }

    // -- the pro surface (G3) ----------------------------------------------

    /// The range a take applies to: the marked range, else the span of
    /// the main clip under the playhead.
    fn take_range(&self) -> Option<(Time, Time)> {
        self.state.marked_range().or_else(|| {
            let (i, _) = ops::source_at(&self.editor.project, self.state.playhead)?;
            let main = self.editor.project.main();
            let start = main.positions()[i];
            Some((start, start + main.clips[i].len()))
        })
    }

    /// A one-second pseudo-clip of angle `track` at the playhead, for
    /// the wall thumbnails. Falls back to the angle's first clip.
    fn angle_clip_at(&self, track: usize, playhead: Time) -> Option<Clip> {
        let t = self.editor.project.tracks.get(track)?;
        let sec = Time((playhead.0 / 1_000_000_000) * 1_000_000_000);
        for c in &t.clips {
            let at = c.at.unwrap_or(Time::ZERO);
            if sec >= at && sec < at + c.len() {
                let src_t = c.in_ + Time(((sec - at).0 as f64 * c.rate.unwrap_or(1.0)) as u64);
                let mut thumb = c.clone();
                thumb.in_ = src_t;
                thumb.out = (src_t + Time(1_000_000_000)).min(c.out);
                return Some(thumb);
            }
        }
        t.clips.first().cloned()
    }

    fn do_take(&mut self, track: usize) {
        let Some((s, e)) = self.take_range() else {
            self.editor.message = "mark a range ([ and ]) or park the playhead on a clip".into();
            return;
        };
        if self.editor.take(track, s, e) {
            self.model_changed();
        }
    }

    fn transcript_file(&self, index: usize) -> PathBuf {
        self.project_dir
            .join("cache")
            .join(format!("transcript_{index}.json"))
    }

    /// Load (or reload) the cached transcript for a clip, keyed by mtime.
    fn load_transcript(&mut self, index: usize) -> bool {
        let path = self.transcript_file(index);
        let Ok(mtime) = std::fs::metadata(&path).and_then(|m| m.modified()) else {
            return false;
        };
        if matches!(&self.transcript, Some((i, t, _)) if *i == index && *t == mtime) {
            return true;
        }
        let Ok(json) = std::fs::read_to_string(&path) else {
            return false;
        };
        match serde_json::from_str::<Vec<viode_core::Segment>>(&json) {
            Ok(segments) => {
                self.transcript = Some((index, mtime, segments));
                true
            }
            Err(_) => false,
        }
    }

    /// Run whisper on a worker thread and cache the result where the CLI
    /// caches its (cache/transcript_N.json — one contract, every client).
    fn start_transcribe(&mut self, index: usize) {
        let Some(clip) = self.editor.project.main().clips.get(index) else {
            return;
        };
        let src = self.project_dir.join(&clip.src);
        let cache = self.project_dir.join("cache");
        let dest = self.transcript_file(index);
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = viode_core::transcribe(&src, &cache, None)
                .map_err(|e| e.to_string())
                .and_then(|segments| {
                    let json = serde_json::to_string_pretty(&segments)
                        .map_err(|e| e.to_string())?;
                    std::fs::write(&dest, json).map_err(|e| e.to_string())?;
                    Ok(segments.len())
                });
            let _ = tx.send(result);
        });
        self.transcribe_rx = Some((index, rx));
    }

    /// Seek the playhead to where a transcript segment plays on the
    /// timeline (segment times are SOURCE times).
    fn seek_segment(&mut self, index: usize, src_time: Time) {
        let main = self.editor.project.main();
        let Some(clip) = main.clips.get(index) else {
            return;
        };
        if src_time < clip.in_ || src_time >= clip.out {
            self.editor.message = "that segment is already cut out".into();
            return;
        }
        let offset_src = src_time - clip.in_;
        let rate = clip.rate.unwrap_or(1.0);
        let offset = Time((offset_src.0 as f64 / rate).round() as u64);
        let pos = main.positions()[index] + offset;
        let cmds = self.state.seek_to(pos);
        self.apply(cmds);
    }

    /// Regenerate the scope images when paused on a new frame.
    fn update_scopes(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.scope_rx {
            match rx.try_recv() {
                Ok(Ok((wave, vector))) => {
                    self.scope_rx = None;
                    for (slot, path) in [(0, wave), (1, vector)] {
                        if let Ok(bytes) = std::fs::read(&path) {
                            if let Ok(img) = image::load_from_memory(&bytes) {
                                let img = img.to_rgba8();
                                let size = [img.width() as usize, img.height() as usize];
                                self.scope_tex[slot] = Some(ctx.load_texture(
                                    format!("scope{slot}"),
                                    egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw()),
                                    egui::TextureOptions::LINEAR,
                                ));
                            }
                        }
                    }
                }
                Ok(Err(e)) => {
                    self.scope_rx = None;
                    self.scopes_on = false;
                    self.editor.message = format!("scopes: {e}");
                }
                Err(_) => return, // still working — one job at a time
            }
        }
        if !self.scopes_on || self.state.playing {
            return;
        }
        let Some((i, src_time)) = ops::source_at(&self.editor.project, self.state.playhead) else {
            return;
        };
        let clip = &self.editor.project.main().clips[i];
        let src = proxy_for(&self.project_dir, &clip.src)
            .unwrap_or_else(|| self.project_dir.join(&clip.src));
        if self.scope_key.as_ref() == Some(&(src.clone(), src_time)) {
            return;
        }
        self.scope_key = Some((src.clone(), src_time));
        let cache = self.project_dir.join("cache");
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let wave = cache.join("gui_scope_waveform.png");
            let vector = cache.join("gui_scope_vector.png");
            let result = viode_core::scope_png(&src, src_time, "waveform", &wave)
                .and_then(|()| viode_core::scope_png(&src, src_time, "vector", &vector))
                .map(|()| (wave, vector))
                .map_err(|e| e.to_string());
            let _ = tx.send(result);
        });
        self.scope_rx = Some(rx);
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }

    /// Run one render job (master via GES, then preset/codec finish) —
    /// the same recipe the CLI and the queue use.
    fn start_render(&mut self, preset: Option<String>, codec: Option<String>, bitrate: Option<u32>, output: Option<PathBuf>) {
        let reframe = self.r_reframe;
        if self.render_rx.is_some() {
            self.editor.message = "a render is already running".into();
            return;
        }
        let project = self.editor.project.clone();
        let dir = self.project_dir.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(run_render_job(&project, &dir, preset.as_deref(), codec.as_deref(), bitrate, reframe, output));
        });
        self.render_rx = Some(rx);
        self.editor.message = "rendering…".into();
    }

    fn draw_left_panel(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        let dirty = if self.editor.dirty { " ●" } else { "" };
        ui.label(
            egui::RichText::new(format!("{}{dirty}", self.editor.project.project.name)).strong(),
        );
        if !self.missing.is_empty() {
            let label = format!("⚠ {} media file(s) missing — relink…", self.missing.len());
            if ui
                .button(egui::RichText::new(label).color(self.theme.title))
                .clicked()
            {
                self.show_relink = true;
            }
        }
        if !self.notice.is_empty() {
            let notice = self.notice.clone();
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new("📣").size(12.0));
                ui.label(
                    egui::RichText::new(notice)
                        .color(self.theme.accent)
                        .size(11.0),
                );
            });
            ui.add_space(4.0);
        }
        if !self.ai_connected {
            let ctx2 = ui.ctx().clone();
            if ui
                .button(egui::RichText::new("🤖 Let your AI edit for you — connect").size(11.0))
                .clicked()
            {
                self.perform(&ctx2, Action::ConnectAi);
            }
        }
        if !self.engine_gaps.is_empty() {
            let label = format!(
                "⚠ {} engine feature(s) unavailable — details…",
                self.engine_gaps.len()
            );
            if ui
                .button(egui::RichText::new(label).color(self.theme.title))
                .clicked()
            {
                self.show_doctor = true;
            }
        }
        ui.horizontal(|ui| {
            if ui.button("render…").clicked() {
                self.show_render = !self.show_render;
            }
            let mut on = self.scopes_on;
            if ui.checkbox(&mut on, "scopes").changed() {
                self.scopes_on = on;
                self.scope_key = None; // force a fresh pair
            }
        });
        ui.separator();

        // Angles: every non-main track is a potential take source; the
        // disabled ones are multicam angles waiting their turn.
        let angles: Vec<(usize, String, bool)> = self
            .editor
            .project
            .tracks
            .iter()
            .enumerate()
            .skip(1)
            .map(|(i, t)| (i, t.name.clone(), t.enabled))
            .collect();
        if !angles.is_empty() {
            ui.label(egui::RichText::new("Angles").strong());
            let range_text = match self.take_range() {
                Some((a, b)) => format!("take range: {a} – {b}"),
                None => "take range: none (use [ and ])".into(),
            };
            ui.label(
                egui::RichText::new(range_text)
                    .color(self.theme.dim)
                    .size(10.0),
            );
            // The wall: each angle shows the frame AT THE PLAYHEAD
            // (bucketed to the second so the artifact cache stays calm),
            // and number keys take without touching the mouse.
            let ph = self.state.playhead;
            for (idx, name, enabled) in angles {
                let wall_clip = self.angle_clip_at(idx, ph);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(format!("{idx}"))
                            .monospace()
                            .color(self.theme.accent),
                    );
                    if let Some(clip) = &wall_clip {
                        if let Some(png) = self.artifact(clip, Kind::Strip, 160.0, 42.0) {
                            if let Some(tex) = self.tex_for(ui.ctx(), &png) {
                                ui.image((tex, egui::Vec2::new(74.0, 42.0)));
                            }
                        }
                    }
                    let tag = if enabled { name.clone() } else { format!("{name} (angle)") };
                    if ui
                        .button(tag)
                        .on_hover_text(format!(
                            "take this angle over the range (or press {idx})"
                        ))
                        .clicked()
                    {
                        self.do_take(idx);
                    }
                });
            }
            ui.separator();
        }

        // Transcript for the main clip under the playhead.
        ui.label(egui::RichText::new("Transcript").strong());
        let under = ops::source_at(&self.editor.project, self.state.playhead).map(|(i, _)| i);
        match under {
            None => {
                ui.label(
                    egui::RichText::new("no clip under the playhead")
                        .color(self.theme.dim)
                        .size(10.0),
                );
            }
            Some(index) => {
                if let Some((busy_index, _)) = &self.transcribe_rx {
                    if *busy_index == index {
                        ui.label(
                            egui::RichText::new("transcribing… (whisper)")
                                .color(self.theme.dim),
                        );
                        return;
                    }
                }
                if !self.load_transcript(index) {
                    ui.label(
                        egui::RichText::new(format!("clip {index}: no transcript yet"))
                            .color(self.theme.dim)
                            .size(10.0),
                    );
                    if ui.button("transcribe (whisper)").clicked() {
                        self.start_transcribe(index);
                    }
                    return;
                }
                let (clip_in, clip_out) = {
                    let c = &self.editor.project.main().clips[index];
                    (c.in_, c.out)
                };
                let segments = self.transcript.as_ref().unwrap().2.clone();
                let mut cut: Option<(Time, Time)> = None;
                let mut seek: Option<Time> = None;
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for seg in &segments {
                        let alive = seg.start >= clip_in && seg.end <= clip_out;
                        ui.horizontal(|ui| {
                            if alive && ui.small_button("✕").on_hover_text("cut this sentence").clicked() {
                                cut = Some((seg.start, seg.end));
                            }
                            let text = egui::RichText::new(&seg.text).size(11.0);
                            let text = if alive { text } else { text.strikethrough().color(self.theme.dim) };
                            if ui.add(egui::Label::new(text).sense(Sense::click())).clicked() && alive {
                                seek = Some(seg.start);
                            }
                        });
                    }
                });
                if let Some((a, b)) = cut {
                    // Same 50ms breathing pad as the CLI's cut-text default.
                    if self.editor.cut_segments(index, &[(a, b)], Time(50_000_000)) {
                        self.model_changed();
                    }
                }
                if let Some(t) = seek {
                    self.seek_segment(index, t);
                }
            }
        }
    }

    fn draw_render_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_render {
            return;
        }
        let mut open = true;
        egui::Window::new("Render")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                egui::ComboBox::from_label("target")
                    .selected_text(self.r_preset.clone())
                    .show_ui(ui, |ui| {
                        for p in ["master", "youtube", "shorts", "podcast", "custom codec"] {
                            ui.selectable_value(&mut self.r_preset, p.to_string(), p);
                        }
                    });
                if self.r_preset == "shorts" {
                    ui.checkbox(&mut self.r_reframe, "follow the subject (reframe)");
                }
                if self.r_preset == "custom codec" {
                    egui::ComboBox::from_label("codec")
                        .selected_text(self.r_codec.clone())
                        .show_ui(ui, |ui| {
                            for c in ["h264", "hevc", "av1", "prores", "dnxhr"] {
                                ui.selectable_value(&mut self.r_codec, c.to_string(), c);
                            }
                        });
                    ui.horizontal(|ui| {
                        ui.label("bitrate (kbps)");
                        ui.add(egui::DragValue::new(&mut self.r_bitrate).range(500..=100_000));
                    });
                }
                ui.horizontal(|ui| {
                    ui.label("output (blank = renders/<name>)");
                    ui.text_edit_singleline(&mut self.r_output);
                });
                ui.add_space(6.0);
                let (preset, codec, bitrate) = self.render_options();
                let output = (!self.r_output.is_empty())
                    .then(|| PathBuf::from(self.r_output.clone()));
                ui.horizontal(|ui| {
                    let busy = self.render_rx.is_some();
                    if ui.add_enabled(!busy, egui::Button::new(if busy { "rendering…" } else { "render now" })).clicked() {
                        self.start_render(preset.clone(), codec.clone(), bitrate, output.clone());
                    }
                    if ui.button("add to queue").clicked() {
                        let mut q = viode_core::queue::load(&self.project_dir).unwrap_or_default();
                        q.jobs.push(viode_core::queue::QueueJob {
                            preset: preset.clone(),
                            codec: codec.clone(),
                            bitrate,
                            output: output.clone(),
                        });
                        match viode_core::queue::save(&self.project_dir, &q) {
                            Ok(()) => self.editor.message = format!("queued job {}", q.jobs.len()),
                            Err(e) => self.editor.message = format!("queue: {e}"),
                        }
                    }
                });
                let q = viode_core::queue::load(&self.project_dir).unwrap_or_default();
                if !q.jobs.is_empty() {
                    ui.separator();
                    ui.label(format!("queue: {} job(s)", q.jobs.len()));
                    for (i, j) in q.jobs.iter().enumerate() {
                        ui.label(
                            egui::RichText::new(format!(
                                "[{i}] {}",
                                j.preset.clone().or(j.codec.clone()).unwrap_or_else(|| "master".into())
                            ))
                            .color(self.theme.dim)
                            .size(10.0),
                        );
                    }
                    ui.horizontal(|ui| {
                        let busy = self.render_rx.is_some();
                        if ui.add_enabled(!busy, egui::Button::new("run queue")).clicked() {
                            self.start_queue_run();
                        }
                        if ui.button("clear queue").clicked() {
                            let _ = viode_core::queue::save(
                                &self.project_dir,
                                &viode_core::queue::RenderQueue::default(),
                            );
                        }
                    });
                }
            });
        if !open {
            self.show_render = false;
        }
    }

    fn render_options(&self) -> (Option<String>, Option<String>, Option<u32>) {
        match self.r_preset.as_str() {
            "master" => (None, None, None),
            "custom codec" => (None, Some(self.r_codec.clone()), Some(self.r_bitrate)),
            p => (Some(p.to_string()), None, None),
        }
    }

    /// Run every queued job in order on the worker thread, then clear the
    /// queue — the CLI's `queue run` contract.
    fn start_queue_run(&mut self) {
        if self.render_rx.is_some() {
            return;
        }
        let project = self.editor.project.clone();
        let dir = self.project_dir.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = (|| {
                let q = viode_core::queue::load(&dir).map_err(|e| e.to_string())?;
                if q.jobs.is_empty() {
                    return Err("queue empty".to_string());
                }
                let n = q.jobs.len();
                let mut last = String::new();
                for j in &q.jobs {
                    last = run_render_job(
                        &project,
                        &dir,
                        j.preset.as_deref(),
                        j.codec.as_deref(),
                        j.bitrate,
                        false,
                        j.output.clone(),
                    )?;
                }
                viode_core::queue::save(&dir, &viode_core::queue::RenderQueue::default())
                    .map_err(|e| e.to_string())?;
                Ok(format!("queue complete ({n} render(s), last: {last})"))
            })();
            let _ = tx.send(result);
        });
        self.render_rx = Some(rx);
        self.editor.message = "running queue…".into();
    }

    fn draw_doctor_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_doctor {
            return;
        }
        let mut open = true;
        egui::Window::new("Engine checkup")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label(
                    "This machine's GStreamer build is missing some pieces. \
                     Everything else works; the features below will error \
                     until their piece is installed.",
                );
                ui.add_space(6.0);
                for c in &self.engine_gaps {
                    ui.label(
                        egui::RichText::new(format!("✗ {} ({})", c.feature, c.probe)).strong(),
                    );
                    ui.label(egui::RichText::new(format!("   {}", c.fix)).size(11.0));
                }
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new("`viode doctor` prints this report in the terminal.")
                        .size(10.0),
                );
            });
        if !open {
            self.show_doctor = false;
        }
    }

    /// Right-click menu for a clip. The clicked clip is already selected;
    /// entries dispatch through perform() like every other surface.
    fn clip_menu(&mut self, ui: &mut egui::Ui, track: usize) {
        let ctx = ui.ctx().clone();
        let mut chosen: Option<Action> = None;
        let mut item = |ui: &mut egui::Ui, a: Action| {
            let text = if a.shortcut().is_empty() {
                a.label().to_string()
            } else {
                format!("{}\t{}", a.label(), a.shortcut())
            };
            if ui.button(text).clicked() {
                chosen = Some(a);
                ui.close();
            }
        };
        item(ui, Action::Split);
        item(ui, Action::TrimInToPlayhead);
        item(ui, Action::TrimOutToPlayhead);
        item(ui, Action::Freeze);
        item(ui, Action::Mend);
        item(ui, Action::MatchPrevious);
        item(ui, Action::Delete);
        if track == 0 {
            ui.separator();
            item(ui, Action::MoveEarlier);
            item(ui, Action::MoveLater);
        }
        ui.separator();
        ui.label(
            egui::RichText::new("speed, gain, place, color: inspector →")
                .size(10.0)
                .color(self.theme.dim),
        );
        if let Some(a) = chosen {
            self.perform(&ctx, a);
        }
    }

    /// Right-click menu for the ruler: playhead and range verbs.
    fn ruler_menu(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let mut chosen: Option<Action> = None;
        for a in [
            Action::MarkIn,
            Action::MarkOut,
            Action::ClearMarks,
            Action::AddTitle,
            Action::AddMarker,
            Action::Split,
            Action::Freeze,
            Action::AddMedia,
        ] {
            let text = if a.shortcut().is_empty() {
                a.label().to_string()
            } else {
                format!("{}\t{}", a.label(), a.shortcut())
            };
            if ui.button(text).clicked() {
                chosen = Some(a);
                ui.close();
            }
        }
        if let Some(a) = chosen {
            self.perform(&ctx, a);
        }
    }

    fn clip_under_playhead(&self) -> Option<(usize, PathBuf)> {
        let (i, _) = ops::source_at(&self.editor.project, self.state.playhead)?;
        let src = self.editor.project.main().clips[i].src.clone();
        let abs = if src.is_absolute() { src } else { self.project_dir.join(src) };
        Some((i, abs))
    }

    /// Silence scan on a worker; the cut applies on arrival (same padding
    /// as the CLI's cut-silences default).
    fn start_cut_silences(&mut self) {
        if self.silence_rx.is_some() {
            return;
        }
        let Some((index, src)) = self.clip_under_playhead() else {
            self.editor.message = "park the playhead on a clip first".into();
            return;
        };
        let dir = self.project_dir.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = viode_core::audio_scan(&dir, &src, -35.0, 0.6, 0.5)
                .map(|s| s.silences)
                .map_err(|e| e.to_string());
            let _ = tx.send(result);
        });
        self.silence_rx = Some((index, rx));
        self.editor.message = "scanning for silence…".into();
    }

    /// Scene detection on a worker; splits apply on arrival.
    fn start_split_scenes(&mut self) {
        if self.scenes_rx.is_some() {
            return;
        }
        let Some((index, src)) = self.clip_under_playhead() else {
            self.editor.message = "park the playhead on a clip first".into();
            return;
        };
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result =
                viode_core::detect_scenes(&src, 0.4).map_err(|e| e.to_string());
            let _ = tx.send(result);
        });
        self.scenes_rx = Some((index, rx));
        self.editor.message = "detecting scene changes…".into();
    }

    /// Pick a second camera file; sync analysis runs on a worker and the
    /// angle lands as a disabled track ready for the wall.
    fn start_angle_add(&mut self) {
        if self.angle_rx.is_some() {
            return;
        }
        let Some(reference) = self
            .editor
            .project
            .main()
            .clips
            .first()
            .map(|c| self.project_dir.join(&c.src))
        else {
            self.editor.message = "add main footage before angles".into();
            return;
        };
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Video", &["mp4", "mov", "mkv", "webm", "avi"])
            .pick_file()
        else {
            return;
        };
        let dir = self.project_dir.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = (|| -> Result<(PathBuf, Time, f64), String> {
                let rel = viode_core::media::bring_in(&dir, &path).map_err(|e| e.to_string())?;
                let abs = dir.join(&rel);
                let info =
                    viode_core::probe::probe_cached(&dir, &abs).map_err(|e| e.to_string())?;
                let offset = viode_core::audio_offset(&reference, &abs, 60.0)
                    .map_err(|e| e.to_string())?;
                Ok((rel, info.duration, offset))
            })();
            let _ = tx.send(result);
        });
        self.angle_rx = Some(rx);
        self.editor.message = "syncing the angle by its audio…".into();
    }

    /// Proxy every source on a worker (the parallel core builder).
    fn start_build_proxies(&mut self) {
        if self.proxy_rx.is_some() {
            return;
        }
        let mut sources: Vec<PathBuf> = Vec::new();
        for t in &self.editor.project.tracks {
            for c in &t.clips {
                if !sources.contains(&c.src) {
                    sources.push(c.src.clone());
                }
            }
        }
        if sources.is_empty() {
            self.editor.message = "nothing to proxy yet".into();
            return;
        }
        let dir = self.project_dir.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let results = viode_core::proxy::build_all(&dir, &sources, false, 4);
            let ok = results.iter().filter(|(_, r)| r.is_ok()).count();
            let _ = tx.send(if ok == 0 {
                Err("no proxies could be built".to_string())
            } else {
                Ok(ok)
            });
        });
        self.proxy_rx = Some(rx);
        self.editor.message = "building proxies…".into();
    }

    fn poll_sweep_jobs(&mut self) {
        if let Some((index, rx)) = &self.silence_rx {
            let index = *index;
            match rx.try_recv() {
                Ok(Ok(ranges)) => {
                    self.silence_rx = None;
                    if ranges.is_empty() {
                        self.editor.message = "no silences found".into();
                    } else if self.editor.cut_segments(index, &ranges, Time(150_000_000)) {
                        self.model_changed();
                    }
                }
                Ok(Err(e)) => {
                    self.silence_rx = None;
                    self.editor.message = format!("silence scan: {e}");
                }
                Err(_) => {}
            }
        }
        if let Some((index, rx)) = &self.scenes_rx {
            let index = *index;
            match rx.try_recv() {
                Ok(Ok(times)) => {
                    self.scenes_rx = None;
                    if self.editor.split_at_sources(index, &times) {
                        self.model_changed();
                    }
                }
                Ok(Err(e)) => {
                    self.scenes_rx = None;
                    self.editor.message = format!("scene detection: {e}");
                }
                Err(_) => {}
            }
        }
        if let Some(rx) = &self.angle_rx {
            match rx.try_recv() {
                Ok(Ok((rel, duration, offset))) => {
                    self.angle_rx = None;
                    if self.editor.angle_apply(rel, duration, offset) {
                        self.model_changed();
                    }
                }
                Ok(Err(e)) => {
                    self.angle_rx = None;
                    self.editor.message = format!("angle sync: {e}");
                }
                Err(_) => {}
            }
        }
        if let Some(rx) = &self.proxy_rx {
            match rx.try_recv() {
                Ok(Ok(n)) => {
                    self.proxy_rx = None;
                    self.editor.message =
                        format!("{n} prox{} built — previews now use them", if n == 1 { "y" } else { "ies" });
                }
                Ok(Err(e)) => {
                    self.proxy_rx = None;
                    self.editor.message = format!("proxies: {e}");
                }
                Err(_) => {}
            }
        }
    }

    /// Generate captions on a worker (whisper can take minutes), then
    /// burn them in as titles on arrival. Transcripts cache per source,
    /// exactly like the CLI's `viode captions`.
    fn start_captions(&mut self) {
        if self.captions_rx.is_some() {
            self.editor.message = "captions are already being generated".into();
            return;
        }
        let project = self.editor.project.clone();
        let dir = self.project_dir.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = (|| -> Result<Vec<viode_core::captions::Caption>, String> {
                let mut sources: Vec<PathBuf> = Vec::new();
                for clip in &project.main().clips {
                    if !clip.src.starts_with("media/freeze") && !sources.contains(&clip.src) {
                        sources.push(clip.src.clone());
                    }
                }
                if sources.is_empty() {
                    return Err("the timeline has no clips to caption".into());
                }
                let mut captions = Vec::new();
                for src in &sources {
                    let abs = if src.is_absolute() { src.clone() } else { dir.join(src) };
                    let stem = src
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let cache = dir.join("cache").join(format!("captions-{stem}.json"));
                    let segments: Vec<viode_core::Segment> = if cache.exists() {
                        serde_json::from_str(
                            &std::fs::read_to_string(&cache).map_err(|e| e.to_string())?,
                        )
                        .map_err(|e| e.to_string())?
                    } else {
                        let segs = viode_core::transcribe(&abs, &dir.join("cache"), None)
                            .map_err(|e| e.to_string())?;
                        let _ = std::fs::write(
                            &cache,
                            serde_json::to_string_pretty(&segs).unwrap_or_default(),
                        );
                        segs
                    };
                    captions.extend(viode_core::captions::map_segments(&project, src, &segments));
                }
                captions.sort_by_key(|c| c.start.0);
                Ok(captions)
            })();
            let _ = tx.send(result);
        });
        self.captions_rx = Some(rx);
        self.editor.message = "generating captions…".into();
    }

    /// Duck the music track: the speech mask computes on a worker (the
    /// first run decodes each dialogue source once), keys apply on
    /// arrival. Requires exactly one candidate track, or a selection.
    fn start_duck(&mut self) {
        if self.duck_rx.is_some() {
            self.editor.message = "a duck analysis is already running".into();
            return;
        }
        // The selected clip's track wins; otherwise the single overlay
        // track that carries audio.
        let candidates: Vec<usize> = self
            .editor
            .project
            .tracks
            .iter()
            .enumerate()
            .skip(1)
            .filter(|(_, t)| {
                t.kind != viode_core::TrackKind::Video && t.enabled && !t.clips.is_empty()
            })
            .map(|(i, _)| i)
            .collect();
        let track = match self.editor.selected_clip() {
            Some((t, _)) if t != 0 => t,
            _ if candidates.len() == 1 => candidates[0],
            _ => {
                self.editor.message =
                    "select a clip on the music track first (or keep one audio overlay)".into();
                return;
            }
        };
        let project = self.editor.project.clone();
        let dir = self.project_dir.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let opts = viode_core::duck::DuckOptions::default();
            let scan = |clip: &viode_core::Clip| {
                let abs = if clip.src.is_absolute() {
                    clip.src.clone()
                } else {
                    dir.join(&clip.src)
                };
                viode_core::audio_scan(&dir, &abs, -30.0, 0.35, 0.1)
                    .ok()
                    .map(|s| s.levels)
            };
            let mask =
                viode_core::duck::speech_mask(&project, scan, opts.threshold_db, opts.gap);
            let _ = tx.send(if mask.is_empty() {
                Err("no speech found on the main track".to_string())
            } else {
                Ok(mask)
            });
        });
        self.duck_rx = Some((track, rx));
        self.editor.message = "analyzing dialogue for ducking…".into();
    }

    fn poll_duck(&mut self) {
        let Some((track, rx)) = &self.duck_rx else { return };
        let track = *track;
        match rx.try_recv() {
            Ok(Ok(mask)) => {
                self.duck_rx = None;
                if self.editor.duck_track(track, &mask) {
                    self.editor.end_stage();
                    self.model_changed();
                }
            }
            Ok(Err(e)) => {
                self.duck_rx = None;
                self.editor.message = format!("duck: {e}");
            }
            Err(_) => {}
        }
    }

    fn poll_captions(&mut self) {
        let Some(rx) = &self.captions_rx else { return };
        match rx.try_recv() {
            Ok(Ok(captions)) => {
                self.captions_rx = None;
                if self.editor.captions_burn(&captions) {
                    self.model_changed();
                }
            }
            Ok(Err(e)) => {
                self.captions_rx = None;
                self.editor.message = format!("captions: {e}");
            }
            Err(_) => {}
        }
    }

    /// The small toolbar (discoverability rule): only the verbs everyone
    /// touches, right-aligned in the timeline header. Everything else is
    /// one palette or right-click away.
    fn draw_toolbar(&mut self, ui: &mut egui::Ui, panel: &Rect) {
        const ITEMS: &[(&str, Action)] = &[
            ("add", Action::AddMedia),
            ("split", Action::Split),
            ("delete", Action::Delete),
            ("undo", Action::Undo),
            ("redo", Action::Redo),
            ("save", Action::Save),
            ("render", Action::RenderDialog),
            ("⌘ cmds", Action::CommandPalette),
        ];
        let mut x = panel.right() - 116.0;
        let ctx = ui.ctx().clone();
        for (label, action) in ITEMS.iter().rev() {
            let w = 7.0 * label.chars().count() as f32 + 16.0;
            x -= w + 6.0;
            let rect = Rect::from_min_size(
                Pos2::new(x, panel.top() + (HEADER_H - 20.0) / 2.0),
                egui::vec2(w, 20.0),
            );
            let response = ui.allocate_rect(rect, Sense::click());
            let fill = if response.hovered() {
                self.theme.accent.gamma_multiply(0.30)
            } else {
                self.theme.fg.gamma_multiply(0.08)
            };
            let painter = ui.painter();
            painter.rect_filled(rect, 4.0, fill);
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                *label,
                FontId::proportional(11.0),
                self.theme.fg,
            );
            let tip = if action.shortcut().is_empty() {
                action.label().to_string()
            } else {
                format!("{} ({})", action.label(), action.shortcut())
            };
            if response.on_hover_text(tip).clicked() {
                self.perform(&ctx, *action);
            }
        }
    }

    /// The command palette: every action, searchable, shortcut shown —
    /// the 100%-coverage surface of the discoverability rule.
    fn draw_palette(&mut self, ctx: &egui::Context) {
        if !self.palette.open {
            return;
        }
        let mut submit: Option<Action> = None;
        let mut close = false;
        egui::Window::new("Commands")
            .title_bar(false)
            .resizable(false)
            .anchor(Align2::CENTER_TOP, Vec2::new(0.0, 72.0))
            .show(ctx, |ui| {
                ui.set_width(380.0);
                let edit = egui::TextEdit::singleline(&mut self.palette.query)
                    .hint_text("Type a command… (try: razor, export, title)")
                    .desired_width(f32::INFINITY);
                let response = ui.add(edit);
                response.request_focus();
                if response.changed() {
                    self.palette.selected = 0;
                }
                let results = self.palette.results();
                ui.input(|i| {
                    if i.key_pressed(egui::Key::ArrowDown) {
                        self.palette.move_selection(1, results.len());
                    }
                    if i.key_pressed(egui::Key::ArrowUp) {
                        self.palette.move_selection(-1, results.len());
                    }
                    if i.key_pressed(egui::Key::Enter) {
                        submit = self.palette.chosen();
                    }
                    if i.key_pressed(egui::Key::Escape) {
                        close = true;
                    }
                });
                ui.add_space(4.0);
                egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                    let sel = self.palette.selected.min(results.len().saturating_sub(1));
                    for (i, a) in results.iter().enumerate() {
                        let row = ui.horizontal(|ui| {
                            let text = if i == sel {
                                egui::RichText::new(a.label()).color(self.theme.accent)
                            } else {
                                egui::RichText::new(a.label())
                            };
                            let r = ui.add(
                                egui::Label::new(text).sense(Sense::click()),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        egui::RichText::new(a.shortcut())
                                            .size(10.0)
                                            .color(self.theme.dim),
                                    );
                                },
                            );
                            r
                        });
                        if row.inner.clicked() {
                            submit = Some(*a);
                        }
                    }
                    if results.is_empty() {
                        ui.label(
                            egui::RichText::new("nothing matches — the inspector holds the parametric edits")
                                .color(self.theme.dim),
                        );
                    }
                });
            });
        if let Some(a) = submit {
            self.palette.close();
            self.perform(ctx, a);
        } else if close {
            self.palette.close();
        }
    }

    fn draw_relink_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_relink {
            return;
        }
        let mut open = true;
        egui::Window::new("Relink missing media")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                for (t, c, path) in self.missing.iter().take(10) {
                    ui.label(
                        egui::RichText::new(format!("track {t} clip {c}: {}", path.display()))
                            .monospace()
                            .size(10.0),
                    );
                }
                if self.missing.len() > 10 {
                    ui.label(format!("… and {} more", self.missing.len() - 10));
                }
                ui.horizontal(|ui| {
                    ui.label("search under");
                    ui.text_edit_singleline(&mut self.relink_dir);
                });
                if ui.button("search & relink by filename").clicked() {
                    let dir = PathBuf::from(self.relink_dir.trim());
                    if dir.is_dir() {
                        let project_dir = self.project_dir.clone();
                        if self.editor.relink(&project_dir, &dir) {
                            self.model_changed();
                        }
                    } else {
                        self.editor.message = format!("{} is not a directory", dir.display());
                    }
                }
            });
        if !open || self.missing.is_empty() {
            self.show_relink = false;
        }
    }

    fn draw_help(&mut self, ctx: &egui::Context) {
        if !self.state.show_help {
            return;
        }
        let mut open = true;
        egui::Window::new("Keys")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.monospace("space      play / pause      J/K/L  shuttle");
                ui.monospace("← / →      seek 0.1s (shift: 1s)");
                ui.monospace(", / .      frame step         ↑ / ↓  clip edges");
                ui.monospace("home / end jump               drag ruler to scrub");
                ui.separator();
                ui.monospace("s          split at playhead");
                ui.monospace("i / o      trim clip in / out to playhead");
                ui.monospace("d          delete selection (or clip at playhead)");
                ui.monospace("< / >      move clip left / right");
                ui.monospace("t          add title at playhead");
                ui.monospace("u / U      undo / redo        w  save");
                ui.separator();
                ui.monospace("click clip        select (inspector edits it)");
                ui.monospace("drag clip         move / re-position");
                ui.monospace("drag clip edge    trim");
                ui.monospace("alt+drag edge     roll cut   alt+drag clip  slip");
                ui.monospace("shift+alt+drag    slide");
                ui.separator();
                ui.monospace("[ / ]      mark range in / out (esc clears)");
                ui.monospace("           click an angle (or press its number) = take the range");
                ui.monospace("r          render dialog");
                ui.monospace("q          quit (asks when unsaved)");
            });
        if !open {
            self.state.show_help = false;
        }
    }

    fn draw_quit_confirm(&mut self, ctx: &egui::Context) {
        if !self.confirm_quit {
            return;
        }
        egui::Window::new("Unsaved changes")
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label("The project has unsaved edits.");
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Save and quit").clicked() {
                        self.save();
                        if !self.editor.dirty {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        self.confirm_quit = false;
                    }
                    if ui.button("Discard and quit").clicked() {
                        self.editor.dirty = false; // let the close through
                        self.confirm_quit = false;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    if ui.button("Cancel").clicked() {
                        self.confirm_quit = false;
                    }
                });
            });
    }
}

impl eframe::App for GuiApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Some(palette) = self.theme_watch.changed() {
            self.theme = palette;
        }
        ctx.set_visuals(crate::theme::visuals(&self.theme));
        ctx.request_repaint_after(std::time::Duration::from_secs(1));
        // Artifacts finished in the background become visible this frame.
        self.media.pump();
        // Another process (an MCP session, the CLI) may have rewritten the
        // project — poll for it, and keep an idle heartbeat running so the
        // poll happens even when nobody touches the window.
        self.check_external_change(ctx);
        ctx.request_repaint_after(std::time::Duration::from_millis(500));

        // The window's close button honors unsaved edits too.
        if ctx.input(|i| i.viewport().close_requested()) && self.editor.dirty {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.confirm_quit = true;
        }

        // Debounced pipeline rebuild after edits.
        if self.rebuild_at.is_some_and(|at| std::time::Instant::now() >= at) {
            self.rebuild_at = None;
            self.rebuild_player(ctx);
        }

        for ev in self.player.poll_events() {
            match ev {
                PlayerEvent::Eos => self.state.on_eos(),
                PlayerEvent::Error(e) => {
                    self.player_err = Some(e);
                    self.state.playing = false;
                }
            }
        }
        if self.state.playing {
            if let Some(pos) = self.player.position() {
                self.state.follow(pos);
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(33));
        }
        if !self.media.pending().is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }

        self.handle_keys(ctx);

        // A released pointer ends any staged inspector gesture.
        if ctx.input(|i| i.pointer.any_released()) {
            self.editor.end_stage();
        }

        // Background pro-surface jobs: renders, transcription, scopes.
        if let Some(rx) = &self.render_rx {
            if let Ok(result) = rx.try_recv() {
                self.render_rx = None;
                self.editor.message = match result {
                    Ok(path) => format!("rendered {path}"),
                    Err(e) => format!("render failed: {e}"),
                };
            }
        }
        if let Some((index, rx)) = &self.transcribe_rx {
            let index = *index;
            if let Ok(result) = rx.try_recv() {
                self.transcribe_rx = None;
                self.editor.message = match result {
                    Ok(n) => format!("transcribed clip {index}: {n} segments"),
                    Err(e) => format!("transcribe failed: {e}"),
                };
            }
        }
        self.update_scopes(ctx);

        egui::TopBottomPanel::bottom("timeline")
            .exact_height(self.timeline_height())
            .frame(egui::Frame::new().fill(self.theme.bg))
            .show(ctx, |ui| self.draw_timeline(ui));
        egui::SidePanel::left("pro")
            .exact_width(230.0)
            .frame(egui::Frame::new().fill(self.theme.bg).inner_margin(8.0))
            .show(ctx, |ui| self.draw_left_panel(ui));
        egui::SidePanel::right("inspector")
            .exact_width(INSPECTOR_W)
            .frame(egui::Frame::new().fill(self.theme.bg).inner_margin(8.0))
            .show(ctx, |ui| self.draw_inspector(ui));
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(Color32::BLACK))
            .show(ctx, |ui| self.draw_preview(ui));
        self.draw_help(ctx);
        self.draw_render_dialog(ctx);
        self.poll_captions();
        self.poll_duck();
        self.poll_sweep_jobs();
        // Drag a file onto the window = add footage.
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect()
        });
        if !dropped.is_empty() {
            let dir = self.project_dir.clone();
            if self.editor.add_media(&dir, &dropped) {
                self.model_changed();
            }
        }
        self.draw_relink_dialog(ctx);
        self.draw_doctor_dialog(ctx);
        self.draw_palette(ctx);
        self.draw_quit_confirm(ctx);
    }
}

/// Master render via GES, then the preset/codec finishing pass — one
/// recipe shared by "render now" and the queue, mirroring the CLI.
fn parse_hex_color(s: &str) -> Option<egui::Color32> {
    let hex = s.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(hex, 16).ok()?;
    Some(egui::Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

fn run_render_job(
    project: &Project,
    dir: &std::path::Path,
    preset: Option<&str>,
    codec: Option<&str>,
    bitrate: Option<u32>,
    reframe: bool,
    output: Option<PathBuf>,
) -> Result<String, String> {
    use viode_core::RenderBackend;
    let name = &project.project.name;
    let master = dir.join("renders").join(format!("{name}.mp4"));
    viode_core::GesBackend
        .render(project, dir, &master)
        .map_err(|e| e.to_string())?;
    let final_path = if let Some(p) = preset {
        let preset = viode_core::Preset::parse(p).ok_or_else(|| format!("unknown preset {p:?}"))?;
        let out = output.unwrap_or_else(|| {
            dir.join("renders").join(format!("{name}-{p}.{}", preset.extension()))
        });
        if reframe && preset == viode_core::Preset::Shorts {
            viode_core::reframe::shorts_reframed(&master, &out).map_err(|e| e.to_string())?;
        } else {
            viode_core::apply_preset(&master, &out, preset).map_err(|e| e.to_string())?;
        }
        out
    } else if let Some(c) = codec {
        let codec = viode_core::Codec::parse(c).ok_or_else(|| format!("unknown codec {c:?}"))?;
        let out = output.unwrap_or_else(|| {
            dir.join("renders").join(format!("{name}-{c}.{}", codec.extension()))
        });
        viode_core::export::transcode(&master, &out, codec, bitrate).map_err(|e| e.to_string())?;
        out
    } else if let Some(out) = output {
        std::fs::rename(&master, &out)
            .or_else(|_| std::fs::copy(&master, &out).map(|_| ()))
            .map_err(|e| e.to_string())?;
        out
    } else {
        master
    };
    Ok(final_path.display().to_string())
}

fn lane_rect(panel: &Rect, y: f32, h: f32) -> Rect {
    Rect::from_min_max(
        Pos2::new(panel.left() + GUTTER, y),
        Pos2::new(panel.right() - 8.0, y + h),
    )
}

fn fmt_tc(t: Time) -> String {
    let total = t.as_secs_f64();
    let h = (total / 3600.0) as u64;
    let m = ((total / 60.0) as u64) % 60;
    let s = total % 60.0;
    format!("{h:02}:{m:02}:{s:06.3}")
}

fn fmt_ruler(secs: f64) -> String {
    if secs >= 3600.0 {
        format!("{}:{:02}:{:02}", (secs / 3600.0) as u64, ((secs / 60.0) as u64) % 60, (secs % 60.0) as u64)
    } else if secs >= 60.0 {
        format!("{}:{:02}", (secs / 60.0) as u64, (secs % 60.0) as u64)
    } else if secs.fract() == 0.0 {
        format!("{}s", secs as u64)
    } else {
        format!("{secs:.1}s")
    }
}
