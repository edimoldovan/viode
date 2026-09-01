//! GUI for Viode — a client of viode-core like every other interface, and
//! deliberately shaped like the TUI crate: `state` owns the transport
//! state machine (tested), `player` owns the GES preview pipeline
//! (appsink -> texture, headless-testable), `layout` the pure timeline
//! geometry, `ui` the egui rendering, `welcome` the no-project start
//! screen. This file is only the window lifecycle: one window that starts
//! as either the welcome screen or the editor, and swaps welcome -> editor
//! in place when a project is chosen.

pub mod edit;
pub mod layout;
pub mod player;
pub mod state;
pub mod theme;
pub mod ui;
pub mod welcome;

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use eframe::egui;

use viode_core::Project;

enum App {
    Welcome(welcome::WelcomeApp),
    Editor(ui::GuiApp),
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        let picked = match self {
            App::Welcome(w) => w.update(ctx),
            App::Editor(e) => {
                e.update(ctx, frame);
                None
            }
        };
        if let Some(file) = picked {
            match open_editor(ctx, &file) {
                Ok(editor) => *self = App::Editor(editor),
                Err(e) => {
                    if let App::Welcome(w) = self {
                        w.error = Some(e.to_string());
                    }
                }
            }
        }
    }
}

fn open_editor(ctx: &egui::Context, project_file: &Path) -> Result<ui::GuiApp> {
    let project = Project::load(project_file)?;
    let project_dir = project_file
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    welcome::remember(project_file);
    ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
        "viode — {}",
        project.project.name
    )));
    Ok(ui::GuiApp::new(ctx, project, project_file.to_path_buf(), project_dir))
}

fn run_app(
    title: String,
    build: impl FnOnce(&egui::Context) -> Result<App> + 'static,
) -> Result<()> {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1440.0, 860.0])
            .with_min_inner_size([720.0, 480.0])
            .with_title(&title)
            // Must match StartupWMClass/Icon in packaging/linux/viode.desktop
            // so launchers pair the window with its icon.
            .with_app_id("viode"),
        // Wayland compositors withhold frame callbacks from hidden windows
        // (other workspace, fully covered). With vsync on, the next timed
        // repaint blocks in the buffer swap waiting for a callback that
        // never comes, the event loop stops answering pings, and the
        // compositor declares the app unresponsive. We pace repaints
        // ourselves, so vsync buys nothing — keep it off.
        vsync: false,
        ..Default::default()
    };
    eframe::run_native(
        &title,
        options,
        Box::new(move |cc| build(&cc.egui_ctx).map(|app| Box::new(app) as Box<dyn eframe::App>).map_err(Into::into)),
    )
    .map_err(|e| anyhow!("gui: {e}"))
}

/// Open the editor on a project file directly (`viode gui` inside a
/// project, or `viode gui path/`).
pub fn run(project_file: &Path) -> Result<()> {
    // Fail fast in the terminal before any window appears.
    let project = Project::load(project_file)?;
    let title = format!("viode — {}", project.project.name);
    let file = project_file.to_path_buf();
    run_app(title, move |ctx| Ok(App::Editor(open_editor(ctx, &file)?)))
}

/// Open the welcome screen — the app-launcher entry point, where there is
/// no project and often no meaningful working directory.
pub fn run_welcome() -> Result<()> {
    run_app("viode".to_string(), |_ctx| Ok(App::Welcome(welcome::WelcomeApp::new())))
}
