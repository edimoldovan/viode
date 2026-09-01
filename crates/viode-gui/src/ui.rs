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
        // A focused text field owns the keyboard.
        if ctx.wants_keyboard_input() {
            return;
        }
        use egui::Key as EKey;
        let mut transport = Vec::new();
        let mut edits: Vec<Box<dyn FnOnce(&mut GuiApp)>> = Vec::new();
        ctx.input(|i| {
            let shift = i.modifiers.shift;
            let command = i.modifiers.command;
            let transport_map = [
                (EKey::Space, Key::Space),
                (EKey::J, Key::J),
                (EKey::K, Key::K),
                (EKey::L, Key::L),
                (EKey::Home, Key::Home),
                (EKey::End, Key::End),
                (EKey::Questionmark, Key::Help),
            ];
            for (ek, k) in transport_map {
                if i.key_pressed(ek) {
                    transport.push(k);
                }
            }
            if i.key_pressed(EKey::ArrowLeft) {
                transport.push(if shift { Key::Left } else { Key::SmallLeft });
            }
            if i.key_pressed(EKey::ArrowRight) {
                transport.push(if shift { Key::Right } else { Key::SmallRight });
            }
            if i.key_pressed(EKey::ArrowUp) {
                edits.push(Box::new(|a| a.jump_edge(-1)));
            }
            if i.key_pressed(EKey::ArrowDown) {
                edits.push(Box::new(|a| a.jump_edge(1)));
            }
            if i.key_pressed(EKey::Comma) {
                if shift {
                    edits.push(Box::new(|a| a.edit(|e, ph| e.shift(ph, -1))));
                } else {
                    transport.push(Key::Comma);
                }
            }
            if i.key_pressed(EKey::Period) {
                if shift {
                    edits.push(Box::new(|a| a.edit(|e, ph| e.shift(ph, 1))));
                } else {
                    transport.push(Key::Period);
                }
            }
            if i.key_pressed(EKey::S) && !command {
                edits.push(Box::new(|a| a.edit(|e, ph| e.split(ph))));
            }
            if i.key_pressed(EKey::I) {
                edits.push(Box::new(|a| a.edit(|e, ph| e.trim_to_playhead(true, ph))));
            }
            if i.key_pressed(EKey::O) {
                edits.push(Box::new(|a| a.edit(|e, ph| e.trim_to_playhead(false, ph))));
            }
            if i.key_pressed(EKey::D) || i.key_pressed(EKey::Delete) || i.key_pressed(EKey::Backspace) {
                edits.push(Box::new(|a| a.edit(|e, ph| e.delete(ph))));
            }
            if i.key_pressed(EKey::U) && !command {
                if shift {
                    edits.push(Box::new(|a| a.edit(|e, _| e.redo())));
                } else {
                    edits.push(Box::new(|a| a.edit(|e, _| e.undo())));
                }
            }
            if command && i.key_pressed(EKey::Z) {
                if shift {
                    edits.push(Box::new(|a| a.edit(|e, _| e.redo())));
                } else {
                    edits.push(Box::new(|a| a.edit(|e, _| e.undo())));
                }
            }
            if i.key_pressed(EKey::W) || (command && i.key_pressed(EKey::S)) {
                edits.push(Box::new(|a| a.save()));
            }
            if i.key_pressed(EKey::T) {
                edits.push(Box::new(|a| {
                    let ph = a.state.playhead;
                    if a.editor.title_add(ph) {
                        a.editor.end_stage();
                        a.model_changed();
                    }
                }));
            }
            if i.key_pressed(EKey::OpenBracket) {
                transport.push(Key::MarkIn);
            }
            if i.key_pressed(EKey::CloseBracket) {
                transport.push(Key::MarkOut);
            }
            if i.key_pressed(EKey::R) {
                edits.push(Box::new(|a| a.show_render = !a.show_render));
            }
            if i.key_pressed(EKey::Escape) {
                transport.push(Key::ClearMarks);
                edits.push(Box::new(|a| a.editor.deselect()));
            }
            if i.key_pressed(EKey::Q) {
                edits.push(Box::new(|a| a.request_quit(ctx)));
            }
        });
        for k in transport {
            let cmds = self.state.on_key(k);
            self.apply(cmds);
        }
        for f in edits {
            f(self);
        }
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
        self.media.get(Spec {
            kind,
            src,
            in_s: clip.in_.as_secs_f64(),
            out_s: clip.out.as_secs_f64(),
            px_w,
            px_h,
            frames,
        })
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
        let dirty = if self.editor.dirty { "● " } else { "" };
        painter.text(
            Pos2::new(panel.right() - 88.0, y),
            Align2::RIGHT_CENTER,
            format!("{dirty}{}", self.editor.project.project.name),
            FontId::proportional(11.0),
            if self.editor.dirty { self.theme.accent } else { self.theme.dim },
        );
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

        let painter = ui.painter().clone();
        self.draw_header(&painter, &panel);
        self.help_button(ui, &panel);
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
                hue: None,
            });
            for (label, field, cur, lo, hi) in [
                ("brightness", "brightness", g.brightness.unwrap_or(0.0), -1.0, 1.0),
                ("contrast", "contrast", g.contrast.unwrap_or(1.0), 0.0, 2.0),
                ("saturation", "saturation", g.saturation.unwrap_or(1.0), 0.0, 2.0),
                ("hue", "hue", g.hue.unwrap_or(0.0), -1.0, 1.0),
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
                        ui.selectable_value(&mut self.key_prop, "volume".into(), "volume");
                        ui.selectable_value(&mut self.key_prop, "alpha".into(), "alpha");
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
        if self.render_rx.is_some() {
            self.editor.message = "a render is already running".into();
            return;
        }
        let project = self.editor.project.clone();
        let dir = self.project_dir.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(run_render_job(&project, &dir, preset.as_deref(), codec.as_deref(), bitrate, output));
        });
        self.render_rx = Some(rx);
        self.editor.message = "rendering…".into();
    }

    fn draw_left_panel(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.label(egui::RichText::new(&self.editor.project.project.name).strong());
        if !self.missing.is_empty() {
            let label = format!("⚠ {} media file(s) missing — relink…", self.missing.len());
            if ui
                .button(egui::RichText::new(label).color(self.theme.title))
                .clicked()
            {
                self.show_relink = true;
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
            for (idx, name, enabled) in angles {
                let clip = self.editor.project.tracks[idx].clips.first().cloned();
                ui.horizontal(|ui| {
                    if let Some(clip) = &clip {
                        if let Some(png) = self.artifact(clip, Kind::Strip, 120.0, 26.0) {
                            if let Some(tex) = self.tex_for(ui.ctx(), &png) {
                                ui.image((tex, egui::Vec2::new(78.0, 26.0)));
                            }
                        }
                    }
                    let tag = if enabled { name.clone() } else { format!("{name} (angle)") };
                    if ui.button(tag).on_hover_text("click = take the range from this angle").clicked() {
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
                ui.monospace("           click an angle = take the marked range");
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
        self.draw_relink_dialog(ctx);
        self.draw_quit_confirm(ctx);
    }
}

/// Master render via GES, then the preset/codec finishing pass — one
/// recipe shared by "render now" and the queue, mirroring the CLI.
fn run_render_job(
    project: &Project,
    dir: &std::path::Path,
    preset: Option<&str>,
    codec: Option<&str>,
    bitrate: Option<u32>,
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
        viode_core::apply_preset(&master, &out, preset).map_err(|e| e.to_string())?;
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
