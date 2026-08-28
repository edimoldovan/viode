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
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::crossterm::QueueableCommand;

use app::{Action, App};
use graphics::Placement;

pub fn run(project_file: &Path) -> Result<()> {
    let mut app = App::open(project_file)?;
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &mut app);
    ratatui::restore();
    result
}

fn event_loop(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> Result<()> {
    let mut shown: Vec<Placement> = Vec::new();
    let mut playing_ticks = 0u32;
    loop {
        app.reap();
        app.media.pump();

        // While mpv paints the pane we keep drawing TEXT. Catch: mpv
        // clears the physical screen, and ratatui renders DIFFS against
        // its back buffer — it would write nothing. So each pass we reset
        // ratatui's MEMORY of the screen (not the screen itself): every
        // text cell gets rewritten, which is invisible when unchanged and
        // never touches mpv's frame. Our own kitty images stay suppressed
        // so we don't delete mpv's.
        if app.is_playing() {
            terminal.swap_buffers();
            terminal.current_buffer_mut().reset();
            terminal.swap_buffers();
            let mut placements = Vec::new();
            terminal.draw(|f| placements = ui::draw(f, app))?;
            shown.clear(); // mpv's startup clear may eat our images
            // Keep re-transmitting the filmstrips (no delete-all, same
            // ids — atomic, invisible) every tick for the first second,
            // so they are present immediately and outlive mpv's startup
            // wipe within 100ms, whenever it happens.
            playing_ticks += 1;
            if playing_ticks <= 10 {
                emit_images(&placements, false)?;
            }
            if !event::poll(Duration::from_millis(100))? {
                continue;
            }
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char(' ') => app.toggle_pause(),
                        KeyCode::Char('x') | KeyCode::Char('q') => app.stop_preview(),
                        _ => {}
                    }
                }
            }
            continue;
        }
        playing_ticks = 0;
        if app.take_image_refresh() {
            // The player is gone: wipe its frames and repaint everything.
            let mut out = std::io::stdout();
            out.write_all(graphics::delete_all())?;
            out.flush()?;
            shown.clear();
            terminal.clear()?;
        }

        let mut placements = Vec::new();
        terminal.draw(|f| placements = ui::draw(f, app))?;

        // Images are independent of the text buffer: re-emit only when the
        // set of placements actually changed (edit, resize, thumb ready).
        if app.graphics && placements != shown {
            emit_images(&placements, true)?;
            shown = placements;
        }

        if !event::poll(Duration::from_millis(100))? {
            continue;
        }
        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                if app.on_key(key.code) == Action::Quit {
                    return Ok(());
                }
            }
            Event::Resize(..) => shown.clear(), // force re-emit at new geometry
            _ => {}
        }
    }
}

fn emit_images(placements: &[Placement], clear_first: bool) -> Result<()> {
    let mut out = std::io::stdout();
    if clear_first {
        out.write_all(graphics::delete_all())?;
    }
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
