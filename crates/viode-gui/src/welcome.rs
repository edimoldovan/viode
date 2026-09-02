//! The welcome window — what `viode gui` shows when it starts without a
//! project, which is exactly what an app-launcher start looks like. It
//! offers recent projects, New Project, and Open Project, and funnels every
//! choice into one `PathBuf` that lib.rs turns into an editor in place.
//! The logic (recent-projects list, new-project validation) is pure and
//! unit-tested; the egui rendering stays dumb. File dialogs run on worker
//! threads (rfd via the xdg-desktop-portal, so no GTK dependency) because
//! blocking the update loop is how Wayland decides an app is dead.

use std::path::{Path, PathBuf};
use std::sync::mpsc;

use eframe::egui;
use viode_core::{Project, PROJECT_FILE};

const MAX_RECENTS: usize = 10;

// --- recent projects: a TOML list of project files, newest first ----------

/// Platform state file for the recents list. Linux: XDG state dir;
/// macOS: Application Support. None only when HOME itself is unset.
pub fn recents_file() -> Option<PathBuf> {
    let base = if cfg!(target_os = "macos") {
        std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
    };
    base.map(|b| b.join("viode").join("recent.toml"))
}

pub fn load_recents(file: &Path) -> Vec<PathBuf> {
    let Ok(text) = std::fs::read_to_string(file) else {
        return Vec::new();
    };
    text.parse::<toml::Value>()
        .ok()
        .and_then(|v| v.get("recent").and_then(|r| r.as_array().cloned()))
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(PathBuf::from)).collect())
        .unwrap_or_default()
}

pub fn save_recents(file: &Path, list: &[PathBuf]) {
    let arr = toml::Value::Array(
        list.iter().map(|p| toml::Value::String(p.display().to_string())).collect(),
    );
    let mut table = toml::value::Table::new();
    table.insert("recent".into(), arr);
    if let Some(parent) = file.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let Ok(text) = toml::to_string(&table) else { return };
    let _ = std::fs::write(file, text);
}

/// Move (or insert) `path` to the front, dropping duplicates and capping
/// the list. Pure so it can be tested without a filesystem.
pub fn push_recent(list: &mut Vec<PathBuf>, path: PathBuf) {
    list.retain(|p| p != &path);
    list.insert(0, path);
    list.truncate(MAX_RECENTS);
}

/// Record a project file in the recents list, best effort — the editor
/// calls this on every open, so terminal-opened projects show up in the
/// welcome window too.
pub fn remember(project_file: &Path) {
    let Some(file) = recents_file() else { return };
    let path = project_file
        .canonicalize()
        .unwrap_or_else(|_| project_file.to_path_buf());
    let mut list = load_recents(&file);
    push_recent(&mut list, path);
    save_recents(&file, &list);
}

// --- new-project validation (mirrors `viode new`) --------------------------

/// Validate a project name against a parent directory and return the
/// directory the project would scaffold into.
pub fn new_project_dir(parent: &Path, name: &str) -> Result<PathBuf, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("give the project a name".into());
    }
    if name.contains(['/', '\\']) || name == "." || name == ".." {
        return Err(format!("{name:?} is not a usable project name"));
    }
    let dir = parent.join(name);
    if dir.exists() {
        return Err(format!("{} already exists", dir.display()));
    }
    Ok(dir)
}

/// Parse WIDTHxHEIGHT exactly like `viode new --res`.
pub fn parse_res(res: &str) -> Result<[u32; 2], String> {
    res.split_once('x')
        .and_then(|(w, h)| Some([w.trim().parse::<u32>().ok()?, h.trim().parse::<u32>().ok()?]))
        .filter(|[w, h]| *w > 0 && *h > 0)
        .ok_or_else(|| format!("invalid resolution {res:?}, expected WIDTHxHEIGHT"))
}

// --- the window -------------------------------------------------------------

enum Pick {
    Project(Option<PathBuf>),
    Folder(Option<PathBuf>),
}

/// Connect every AI client found on this machine; one plain sentence out.
pub fn run_connect_all() -> String {
    let statuses = viode_core::connect::detect();
    let mut done = Vec::new();
    for s in &statuses {
        if !s.found {
            continue;
        }
        if s.connected || viode_core::connect::connect(&s.id).is_ok() {
            done.push(s.name.clone());
        }
    }
    if done.is_empty() {
        "No compatible AI app found — Viode works with Claude Desktop, Claude Code, \
         Cursor, Windsurf, Gemini CLI, and opencode."
            .into()
    } else {
        format!("Connected: {}. Restart the app, then just talk to it.", done.join(", "))
    }
}

pub struct WelcomeApp {
    theme: crate::theme::Palette,
    recents: Vec<PathBuf>,
    pub error: Option<String>,
    new_name: String,
    new_fps: String,
    new_res: String,
    new_parent: PathBuf,
    pick_rx: Option<mpsc::Receiver<Pick>>,
    /// Some(true) once every found AI client is connected; None until
    /// first drawn. Drives the "Connect your AI" card.
    ai_connected: Option<bool>,
    connect_result: Option<String>,
}

impl WelcomeApp {
    pub fn new() -> WelcomeApp {
        let recents: Vec<PathBuf> = recents_file()
            .map(|f| load_recents(&f))
            .unwrap_or_default()
            .into_iter()
            .filter(|p| p.exists())
            .collect();
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| ".".into());
        let videos = home.join("Videos");
        WelcomeApp {
            theme: crate::theme::load(),
            recents,
            error: None,
            new_name: String::new(),
            new_fps: "30".into(),
            new_res: "1920x1080".into(),
            new_parent: if videos.is_dir() { videos } else { home },
            pick_rx: None,
            ai_connected: None,
            connect_result: None,
        }
    }

    fn spawn_pick(&mut self, ctx: &egui::Context, folder: bool) {
        let (tx, rx) = mpsc::channel();
        self.pick_rx = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let pick = if folder {
                Pick::Folder(rfd::FileDialog::new().set_title("Project location").pick_folder())
            } else {
                Pick::Project(
                    rfd::FileDialog::new()
                        .set_title("Open Viode project")
                        .add_filter("Viode project", &["viode"])
                        .pick_file(),
                )
            };
            let _ = tx.send(pick);
            ctx.request_repaint();
        });
    }

    /// Validate that a picked file really is a project before handing it on.
    fn open_checked(&mut self, file: PathBuf) -> Option<PathBuf> {
        match Project::load(&file) {
            Ok(_) => Some(file),
            Err(e) => {
                self.error = Some(e.to_string());
                None
            }
        }
    }

    fn create(&mut self) -> Option<PathBuf> {
        let fps: f64 = match self.new_fps.trim().parse() {
            Ok(f) if f > 0.0 => f,
            _ => {
                self.error = Some(format!("invalid fps {:?}", self.new_fps));
                return None;
            }
        };
        let res = match parse_res(&self.new_res) {
            Ok(r) => r,
            Err(e) => {
                self.error = Some(e);
                return None;
            }
        };
        let dir = match new_project_dir(&self.new_parent, &self.new_name) {
            Ok(d) => d,
            Err(e) => {
                self.error = Some(e);
                return None;
            }
        };
        match Project::init(&dir, fps, res) {
            Ok(file) => Some(file),
            Err(e) => {
                self.error = Some(e.to_string());
                None
            }
        }
    }

    /// One frame of the welcome screen. Returns the chosen project file
    /// once the user opens or creates one.
    pub fn update(&mut self, ctx: &egui::Context) -> Option<PathBuf> {
        ctx.set_visuals(crate::theme::visuals(&self.theme));
        let mut chosen = None;
        if let Some(rx) = &self.pick_rx {
            match rx.try_recv() {
                Ok(Pick::Project(sel)) => {
                    self.pick_rx = None;
                    if let Some(f) = sel {
                        chosen = self.open_checked(f);
                    }
                }
                Ok(Pick::Folder(sel)) => {
                    self.pick_rx = None;
                    if let Some(d) = sel {
                        self.new_parent = d;
                    }
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => self.pick_rx = None,
            }
        }
        let dialog_open = self.pick_rx.is_some();
        let theme = self.theme.clone();

        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(theme.bg).inner_margin(24.0))
            .show(ctx, |ui| {
                ui.add_enabled_ui(!dialog_open, |ui| {
                    let width = ui.available_width().min(560.0);
                    ui.vertical_centered(|ui| {
                        ui.set_max_width(width);
                        ui.add_space(24.0);
                        ui.label(
                            egui::RichText::new("viode")
                                .color(theme.accent)
                                .size(42.0)
                                .strong(),
                        );
                        ui.label(
                            egui::RichText::new("AI-native video editor").color(theme.dim),
                        );
                        ui.add_space(24.0);

                        // New project.
                        egui::Frame::group(ui.style()).fill(theme.lane).show(ui, |ui| {
                            ui.set_width(width - 16.0);
                            ui.label(egui::RichText::new("New project").color(theme.fg).strong());
                            ui.add_space(6.0);
                            ui.horizontal(|ui| {
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.new_name)
                                        .hint_text("name")
                                        .desired_width(180.0),
                                );
                                ui.label(egui::RichText::new("fps").color(theme.dim));
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.new_fps)
                                        .desired_width(40.0),
                                );
                                ui.label(egui::RichText::new("res").color(theme.dim));
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.new_res)
                                        .desired_width(84.0),
                                );
                            });
                            ui.add_space(4.0);
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(format!(
                                        "in {}",
                                        self.new_parent.display()
                                    ))
                                    .color(theme.dim),
                                );
                                if ui.button("Choose…").clicked() {
                                    self.spawn_pick(ctx, true);
                                }
                                if ui
                                    .add_enabled(
                                        !self.new_name.trim().is_empty(),
                                        egui::Button::new(
                                            egui::RichText::new("Create").color(theme.accent),
                                        ),
                                    )
                                    .clicked()
                                {
                                    chosen = self.create();
                                }
                            });
                        });

                        ui.add_space(12.0);
                        if ui.button("Open project…").clicked() {
                            self.spawn_pick(ctx, false);
                        }

                        if let Some(err) = &self.error {
                            ui.add_space(8.0);
                            ui.label(egui::RichText::new(err).color(theme.title));
                        }

                        // The AI card: one click connects every AI app
                        // on this machine (Claude, Cursor, opencode, ...).
                        let connected = *self.ai_connected.get_or_insert_with(|| {
                            let s = viode_core::connect::detect();
                            s.iter().any(|c| c.connected)
                        });
                        ui.add_space(16.0);
                        if let Some(result) = &self.connect_result {
                            ui.label(egui::RichText::new(result).color(theme.accent).size(11.0));
                        } else if !connected {
                            ui.label(
                                egui::RichText::new(
                                    "Viode can be driven by your AI assistant — \
                                     it cuts, trims, and renders from plain conversation.",
                                )
                                .color(theme.dim)
                                .size(11.0),
                            );
                            if ui.button("Connect your AI").clicked() {
                                self.connect_result = Some(run_connect_all());
                                self.ai_connected = None;
                            }
                        }

                        // Recents.
                        let recents = self.recents.clone();
                        if !recents.is_empty() {
                            ui.add_space(20.0);
                            ui.label(egui::RichText::new("Recent").color(theme.dim));
                            ui.add_space(4.0);
                            for path in recents {
                                let name = path
                                    .parent()
                                    .and_then(|d| d.file_name())
                                    .map(|n| n.to_string_lossy().to_string())
                                    .unwrap_or_else(|| path.display().to_string());
                                let label = format!("{name}  —  {}", path.display());
                                if ui
                                    .button(egui::RichText::new(label).color(theme.fg))
                                    .clicked()
                                {
                                    if let Some(f) = self.open_checked(path.clone()) {
                                        chosen = Some(f);
                                    }
                                }
                            }
                        }
                        ui.add_space(24.0);
                        ui.label(
                            egui::RichText::new(format!(
                                "or from a terminal: viode new <name> && cd <name> && viode gui  ({PROJECT_FILE} is the timeline)"
                            ))
                            .color(theme.dim)
                            .size(11.0),
                        );
                    });
                });
            });
        chosen
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_recent_dedupes_fronts_and_caps() {
        let mut list = Vec::new();
        for i in 0..12 {
            push_recent(&mut list, PathBuf::from(format!("/p{i}/project.viode")));
        }
        assert_eq!(list.len(), MAX_RECENTS);
        assert_eq!(list[0], PathBuf::from("/p11/project.viode"));
        // Re-opening an older project moves it to the front without a dup.
        push_recent(&mut list, PathBuf::from("/p5/project.viode"));
        assert_eq!(list[0], PathBuf::from("/p5/project.viode"));
        assert_eq!(list.iter().filter(|p| **p == PathBuf::from("/p5/project.viode")).count(), 1);
        assert_eq!(list.len(), MAX_RECENTS);
    }

    #[test]
    fn recents_roundtrip_through_the_state_file() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("state/viode/recent.toml");
        let list = vec![PathBuf::from("/a/project.viode"), PathBuf::from("/b/project.viode")];
        save_recents(&file, &list);
        assert_eq!(load_recents(&file), list);
        // A missing or garbage file is an empty list, never an error.
        assert!(load_recents(&tmp.path().join("nope.toml")).is_empty());
        std::fs::write(&file, "not toml [[").unwrap();
        assert!(load_recents(&file).is_empty());
    }

    #[test]
    fn new_project_validation_mirrors_the_cli() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(new_project_dir(tmp.path(), "  ").is_err());
        assert!(new_project_dir(tmp.path(), "a/b").is_err());
        assert!(new_project_dir(tmp.path(), "..").is_err());
        let err = new_project_dir(tmp.path().parent().unwrap(), tmp.path().file_name().unwrap().to_str().unwrap())
            .unwrap_err();
        assert!(err.contains("already exists"), "unhelpful: {err}");
        assert_eq!(new_project_dir(tmp.path(), " cut ").unwrap(), tmp.path().join("cut"));
    }

    #[test]
    fn resolution_parses_like_viode_new() {
        assert_eq!(parse_res("1920x1080").unwrap(), [1920, 1080]);
        assert_eq!(parse_res("640 x 360").unwrap(), [640, 360]);
        assert!(parse_res("1920").is_err());
        assert!(parse_res("0x100").is_err());
        assert!(parse_res("axb").is_err());
    }
}
