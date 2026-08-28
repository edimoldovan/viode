//! Terminal UI for Viode — a client of viode-core like every other
//! interface. `app` owns the state machine (tested), `ui` the rendering
//! (smoke-tested), `graphics` the kitty image protocol, `media` the
//! background thumbnail/waveform worker; this file is only the terminal
//! lifecycle and event loop.

pub mod app;
pub mod graphics;
pub mod media;
pub mod preview;
pub mod ui;

use std::io::Write;
use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use ratatui::crossterm::cursor::MoveTo;
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use ratatui::crossterm::QueueableCommand;

use app::{Action, App};
use graphics::Placement;

pub fn run(project_file: &Path) -> Result<()> {
    let mut app = App::open(project_file)?;
    loop {
        let mut terminal = ratatui::init();
        let result = event_loop(&mut terminal, &mut app);
        ratatui::restore();
        match result? {
            Exit::Quit => return Ok(()),
            Exit::Play(target, start) => {
                // The terminal belongs to mpv now: real shuttle controls
                // (space pause, arrows seek), q comes back here.
                if let Err(e) = preview::play_blocking(&target, start) {
                    app.message = format!("mpv failed (is mpv installed?): {e}");
                }
            }
        }
    }
}

enum Exit {
    Quit,
    Play(std::path::PathBuf, f64),
}

fn event_loop(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> Result<Exit> {
    let mut shown: Vec<Placement> = Vec::new();
    loop {
        app.reap();
        app.media.pump();

        let mut placements = Vec::new();
        terminal.draw(|f| placements = ui::draw(f, app))?;

        // Images are independent of the text buffer: re-emit only when the
        // set of placements actually changed (edit, resize, thumb ready).
        if app.graphics && placements != shown {
            emit_images(&placements)?;
            shown = placements;
        }

        if !event::poll(Duration::from_millis(100))? {
            continue;
        }
        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => match app.on_key(key.code) {
                Action::Quit => return Ok(Exit::Quit),
                Action::Play(target, start) => return Ok(Exit::Play(target, start)),
                Action::None => {}
            },
            Event::Resize(..) => shown.clear(), // force re-emit at new geometry
            _ => {}
        }
    }
}

fn emit_images(placements: &[Placement]) -> Result<()> {
    let mut out = std::io::stdout();
    out.write_all(graphics::delete_all())?;
    for p in placements {
        let Ok(data) = std::fs::read(&p.png) else {
            continue;
        };
        out.queue(MoveTo(p.x, p.y))?;
        out.write_all(&graphics::encode_png_at(&data, p.id, p.cols, p.rows))?;
    }
    out.flush()?;
    Ok(())
}
