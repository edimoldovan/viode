//! GUI for Viode — a client of viode-core like every other interface, and
//! deliberately shaped like the TUI crate: `state` owns the transport
//! state machine (tested), `player` owns the GES preview pipeline
//! (appsink -> texture, headless-testable), `layout` the pure timeline
//! geometry, `ui` the egui rendering. This file is only the window
//! lifecycle.

pub mod edit;
pub mod layout;
pub mod player;
pub mod state;
pub mod theme;
pub mod ui;

use std::path::Path;

use anyhow::{anyhow, Result};

use viode_core::Project;

pub fn run(project_file: &Path) -> Result<()> {
    let project = Project::load(project_file)?;
    let project_dir = project_file
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| std::path::PathBuf::from("."));

    let project_file = project_file.to_path_buf();
    let title = format!("viode — {}", project.project.name);
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1440.0, 860.0])
            .with_min_inner_size([720.0, 480.0])
            .with_title(&title),
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
        Box::new(move |cc| Ok(Box::new(ui::GuiApp::new(cc, project, project_file, project_dir)))),
    )
    .map_err(|e| anyhow!("gui: {e}"))
}
