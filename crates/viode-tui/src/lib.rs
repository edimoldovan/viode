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
    // Engine gaps show up in the status line at launch — the same report
    // as `viode doctor`, so nobody discovers a missing plugin mid-render.
    let gaps = viode_core::doctor::problems();
    if !gaps.is_empty() {
        app.message = format!(
            "⚠ {} engine feature(s) unavailable — run `viode doctor` (? for help)",
            gaps.len()
        );
    }
    // Announcement from the developer (official binaries set VIODE_NOTICE
    // from the license check; source builds never have it).
    if let Ok(notice) = std::env::var("VIODE_NOTICE") {
        if !notice.is_empty() {
            app.message = format!("📣 {notice}");
        }
    }
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &mut app);
    ratatui::restore();
    // mpv may have enabled terminal modes we never asked for (mouse
    // reporting); switch them off unconditionally so the shell is clean.
    use ratatui::crossterm::{event::DisableMouseCapture, execute};
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    result
}

fn event_loop(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> Result<()> {
    let mut shown: Vec<Placement> = Vec::new();
    let mut playing_ticks = 0u32;
    let mut empty_ticks = 0u32;
    let mut force_emit = false;
    let mut reflow_pending = false;
    loop {
        app.reap();
        app.media.pump();
        if !app.is_playing() {
            app.check_external_change();
        }

        // While mpv paints the pane we keep drawing TEXT. Catch: mpv
        // clears the physical screen, and ratatui renders DIFFS against
        // its back buffer — it would write nothing. So each pass we reset
        // ratatui's MEMORY of the screen (not the screen itself): every
        // text cell gets rewritten, which is invisible when unchanged and
        // never touches mpv's frame. Our own kitty images stay suppressed
        // so we don't delete mpv's.
        if app.is_playing() {
            if reflow_pending {
                // One draw first so preview_area reflects the new size.
                terminal.draw(|f| {
                    ui::draw(f, app);
                })?;
                app.reflow_playback();
                reflow_pending = false;
                playing_ticks = 0; // re-send strips at the new layout
            }
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
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                    KeyCode::Char(' ') => app.toggle_pause(),
                    KeyCode::Char('x') | KeyCode::Char('q') => app.stop_preview(),
                    _ => {}
                },
                Event::Resize(..) => {
                    // mpv's pane geometry was fixed at spawn: respawn at
                    // the new geometry, same position, same pause state —
                    // after the next draw refreshes preview_area.
                    reflow_pending = true;
                }
                _ => {}
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
        // An EMPTY set usually means a resize invalidated the pixel-sized
        // cache and regeneration is in flight — keep the old images up
        // until the replacements exist, and only truly wipe if emptiness
        // persists (e.g. all clips deleted).
        if app.graphics {
            if !placements.is_empty() && (force_emit || placements != shown) {
                emit_images(&placements, true)?;
                shown = placements;
                empty_ticks = 0;
                force_emit = false;
            } else if placements.is_empty() && !shown.is_empty() {
                empty_ticks += 1;
                if empty_ticks > 20 {
                    let mut out = std::io::stdout();
                    out.write_all(graphics::delete_all())?;
                    out.flush()?;
                    shown.clear();
                    empty_ticks = 0;
                }
            } else {
                empty_ticks = 0;
            }
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
            // The terminal drops/reflows images on resize; sizes may be
            // IDENTICAL afterwards (vertical resize), so force the next
            // available placements out regardless of the diff.
            Event::Resize(..) => force_emit = true,
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
