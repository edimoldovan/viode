//! Terminal UI for Viode — a client of viode-core like every other
//! interface. `app` owns the state machine (tested), `ui` the rendering
//! (smoke-tested); this file is only the terminal lifecycle and event loop.

pub mod app;
pub mod ui;

use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyEventKind};

use app::{Action, App};

pub fn run(project_file: &Path) -> Result<()> {
    let mut app = App::open(project_file)?;
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &mut app);
    ratatui::restore();
    result
}

fn event_loop(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> Result<()> {
    loop {
        app.reap();
        terminal.draw(|f| ui::draw(f, app))?;
        if !event::poll(Duration::from_millis(100))? {
            continue;
        }
        if let Event::Key(key) = event::read()? {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            if app.on_key(key.code) == Action::Quit {
                return Ok(());
            }
        }
    }
}
