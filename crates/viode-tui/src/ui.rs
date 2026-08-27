//! Rendering: App state -> one ratatui frame. Pure function of the state,
//! smoke-tested against a TestBackend.
//!
//! Style rule: only named ANSI colors — the TUI inherits the terminal
//! palette, so it automatically matches the user's Omarchy/aether theme.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use ratatui::Frame;

use viode_core::{Time, TrackKind};

use crate::app::App;
use crate::graphics::Placement;

/// Rows of real imagery in graphics-capable terminals.
const THUMB_ROWS: u16 = 4;
const WAVE_ROWS: u16 = 2;

const CLIP_COLORS: [Color; 4] = [Color::Blue, Color::Cyan, Color::Magenta, Color::Green];

fn block(title: &str) -> Block<'_> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(
            format!(" {title} "),
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ))
}

pub fn draw(f: &mut Frame, app: &mut App) -> Vec<Placement> {
    let overlay_lanes = app.project.tracks.len().saturating_sub(1) as u16;
    let title_lane = u16::from(!app.project.titles.is_empty());
    let image_rows = if app.graphics { THUMB_ROWS + WAVE_ROWS } else { 0 };
    let [header, timeline, details, _filler, status] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(6 + overlay_lanes + title_lane + image_rows),
        Constraint::Length(6),
        Constraint::Min(0), // breathing room, not a giant empty box
        Constraint::Length(1),
    ])
    .areas(f.area());

    draw_header(f, &*app, header);
    let placements = draw_timeline(f, app, timeline);
    draw_details(f, &*app, details);
    draw_status(f, &*app, status);
    if app.show_help {
        draw_help(f);
    }
    placements
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let meta = &app.project.project;
    let line = Line::from(vec![
        Span::styled(
            format!(" {} ", meta.name),
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            if app.dirty { "● " } else { "" },
            Style::default().fg(Color::Red),
        ),
        Span::styled(
            format!(
                "{}x{} @ {} fps  ·  {} track{}  ·  {} clips  ·  ",
                meta.resolution[0],
                meta.resolution[1],
                meta.fps,
                app.project.tracks.len(),
                if app.project.tracks.len() == 1 { "" } else { "s" },
                app.project.main().clips.len(),
            ),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            app.project.total_duration().to_string(),
            Style::default().fg(Color::Cyan),
        ),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

/// Column of a timeline position, given total duration and lane width.
fn col(at: Time, total: u64, width: u64) -> usize {
    ((at.0 as u128 * width as u128) / total.max(1) as u128).min(width.saturating_sub(1) as u128)
        as usize
}

fn draw_timeline(f: &mut Frame, app: &mut App, area: Rect) -> Vec<Placement> {
    let outer = block("timeline");
    let inner = outer.inner(area);
    f.render_widget(outer, area);
    if app.project.main().clips.is_empty() && app.project.tracks.len() == 1 {
        f.render_widget(
            Paragraph::new("empty — `viode add <file>` some footage first")
                .style(Style::default().fg(Color::DarkGray)),
            inner,
        );
        return Vec::new();
    }

    let width = inner.width as u64;
    let total = app.project.total_duration().0.max(1);
    let selected = app.selected();
    let playhead_col = col(app.playhead, total, width);
    let mut lines: Vec<Line> = Vec::new();

    // Timecode ruler with the playhead marker.
    let mut ruler: Vec<char> = vec!['·'; width as usize];
    let tick_every = (width / 6).max(1);
    for tick in (0..width).step_by(tick_every as usize) {
        let t = Time((tick as u128 * total as u128 / width.max(1) as u128) as u64);
        let label: Vec<char> = format!("{t}").chars().skip(3).take(9).collect(); // MM:SS.mmm
        for (k, ch) in label.into_iter().enumerate() {
            let x = tick as usize + k;
            if x < ruler.len() {
                ruler[x] = ch;
            }
        }
    }
    ruler[playhead_col] = '▼';
    lines.push(Line::styled(
        ruler.into_iter().collect::<String>(),
        Style::default().fg(Color::DarkGray),
    ));

    // Title lane (markers on top).
    if !app.project.titles.is_empty() {
        let mut lane: Vec<char> = vec![' '; width as usize];
        for title in &app.project.titles {
            let from = col(title.at, total, width);
            let to = col(title.at + title.dur, total, width).max(from + 1);
            for x in from..to {
                lane[x] = '▔';
            }
            for (k, ch) in title.text.chars().take(to - from).enumerate() {
                lane[from + k] = ch;
            }
        }
        lines.push(Line::styled(
            lane.into_iter().collect::<String>(),
            Style::default().fg(Color::Yellow),
        ));
    }

    // Overlay lanes, topmost first (matching render stacking).
    for (ti, track) in app.project.tracks.iter().enumerate().skip(1).rev() {
        let mut spans: Vec<Span> = Vec::new();
        let mut cursor = 0usize;
        let color = if track.kind == TrackKind::Audio { Color::Green } else { Color::Magenta };
        let style = if track.enabled {
            Style::default().fg(color)
        } else {
            Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM)
        };
        for clip in &track.clips {
            let (s, e) = clip.span();
            let from = col(s, total, width);
            let to = col(e, total, width).max(from + 1);
            if from > cursor {
                spans.push(Span::raw(" ".repeat(from - cursor)));
            }
            let w = to - from;
            let label = truncate(
                &format!("{}{}", track.name, if track.enabled { "" } else { " (off)" }),
                w,
            );
            spans.push(Span::styled(format!("{label:▁<w$}"), style));
            cursor = to;
        }
        let _ = ti;
        lines.push(Line::from(spans));
    }

    // Geometry of every main-track clip in cells (also used for images).
    let main = app.project.main().clone();
    let positions = main.positions();
    let cells: Vec<(usize, usize)> = main
        .clips
        .iter()
        .enumerate()
        .map(|(i, clip)| {
            let from = col(positions[i], total, width);
            let to = col(positions[i] + clip.len(), total, width).max(from + 1);
            (from, to)
        })
        .collect();

    // Thumbnail strip: real frames in graphics terminals. The rows are left
    // blank in the text buffer; kitty images float over them.
    let mut placements: Vec<Placement> = Vec::new();
    if app.graphics {
        let thumb_y = area.y + 1 + lines.len() as u16;
        for (i, &(from, to)) in cells.iter().enumerate() {
            let cols = (to - from) as u16;
            if cols < 2 {
                continue;
            }
            if let Some(png) = app.strip(i, cols, THUMB_ROWS) {
                placements.push(Placement {
                    png,
                    id: i as u32 + 1,
                    x: inner.x + from as u16,
                    y: thumb_y,
                    cols,
                    rows: THUMB_ROWS,
                });
            }
        }
        for _ in 0..THUMB_ROWS {
            lines.push(Line::raw(""));
        }
    }

    // Main lane: proportional blocks, selected clip highlighted.
    let mut lane: Vec<Span> = Vec::new();
    let mut labels: Vec<Span> = Vec::new();
    let mut cursor = 0usize;
    for (i, clip) in main.clips.iter().enumerate() {
        let (from, to) = cells[i];
        let w = to - from.min(to);
        if from > cursor {
            lane.push(Span::raw(" ".repeat(from - cursor)));
            labels.push(Span::raw(" ".repeat(from - cursor)));
        }
        let mut style = Style::default()
            .bg(CLIP_COLORS[i % CLIP_COLORS.len()])
            .fg(Color::Black);
        if Some(i) == selected {
            style = style.add_modifier(Modifier::BOLD | Modifier::REVERSED);
        }
        let name = clip
            .src
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let fade = if clip.transition.is_some() { "⤬" } else { "" };
        let text = truncate(&format!("{fade}{i}:{name}"), w);
        lane.push(Span::styled(format!("{text:^w$}"), style));
        labels.push(Span::styled(
            truncate(&format!("{:^w$}", clip.len().to_string()), w),
            Style::default().fg(Color::DarkGray),
        ));
        cursor = to;
    }
    lines.push(Line::from(lane));
    lines.push(Line::from(labels));

    // Waveform strip under the labels.
    if app.graphics {
        let wave_y = area.y + 1 + lines.len() as u16;
        for (i, &(from, to)) in cells.iter().enumerate() {
            let cols = (to - from) as u16;
            if cols < 2 {
                continue;
            }
            if let Some(png) = app.wave(i, cols, WAVE_ROWS) {
                placements.push(Placement {
                    png,
                    id: i as u32 + 1001, // distinct id space from strips
                    x: inner.x + from as u16,
                    y: wave_y,
                    cols,
                    rows: WAVE_ROWS,
                });
            }
        }
        for _ in 0..WAVE_ROWS {
            lines.push(Line::raw(""));
        }
    }

    // Bottom playhead marker.
    let mut bottom = vec![' '; width as usize];
    bottom[playhead_col] = '▲';
    lines.push(Line::styled(
        bottom.into_iter().collect::<String>(),
        Style::default().fg(Color::Cyan),
    ));

    f.render_widget(Paragraph::new(lines), inner);
    placements
}

fn draw_details(f: &mut Frame, app: &App, area: Rect) {
    let outer = block("clip");
    let inner = outer.inner(area);
    f.render_widget(outer, area);

    let mut lines = vec![Line::from(vec![
        Span::styled("playhead  ", Style::default().fg(Color::DarkGray)),
        Span::styled(app.playhead.to_string(), Style::default().fg(Color::Cyan)),
    ])];
    if let Some((index, src_time)) = app.source_time() {
        let clip = &app.project.main().clips[index];
        lines[0].push_span(Span::styled(
            format!("  →  clip {index} @ source {src_time}"),
            Style::default().fg(Color::DarkGray),
        ));
        lines.push(Line::raw(format!("src       {}", clip.src.display())));
        lines.push(Line::raw(format!(
            "range     [{} .. {}]  len {}",
            clip.in_,
            clip.out,
            clip.len()
        )));
        let start = app.project.positions()[index];
        let mut extra = format!("timeline  starts {}  ends {}", start, start + clip.len());
        if let Some(t) = clip.transition {
            extra.push_str(&format!("  crossfade {t}"));
        }
        if !clip.effects.is_empty() {
            extra.push_str(&format!("  fx {}", clip.effects.join(", ")));
        }
        lines.push(Line::raw(extra));
    } else {
        lines.push(Line::styled(
            "nothing under playhead",
            Style::default().fg(Color::DarkGray),
        ));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let widget = if app.message.is_empty() {
        Paragraph::new(
            "h/l ±0.1s  H/L ±1s  j/k clips  s split  i/o trim  d del  </> move  u undo  w save  ␣ play  P preview  r render  ? help  q quit",
        )
        .style(Style::default().fg(Color::DarkGray))
    } else {
        Paragraph::new(app.message.as_str()).style(Style::default().fg(Color::Yellow))
    };
    f.render_widget(widget, area);
}

fn draw_help(f: &mut Frame) {
    let area = centered(62, 17, f.area());
    let text = "
  playhead   h/l ±0.1s   H/L ±1s   j/k clip edges
  edit       s split   d delete   i trim start   o trim end
             </> move clip   u undo   U redo
  view       space play clip (mpv)   P preview timeline
  project    w save   r render   q quit

  The playhead selects: verbs act on the main-track clip
  under it. Trims move the clip's SOURCE in/out points.
  Overlay tracks, titles, multicam and transcripts are
  driven from the CLI/MCP: track, title, angle, take,
  transcribe, cut-text.

                  any key to close";
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(text).block(block("help")).fg(Color::White),
        area,
    );
}

fn centered(w: u16, h: u16, r: Rect) -> Rect {
    let x = r.x + r.width.saturating_sub(w) / 2;
    let y = r.y + r.height.saturating_sub(h) / 2;
    Rect::new(x, y, w.min(r.width), h.min(r.height))
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max.saturating_sub(1)).chain(['…']).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use viode_core::{Clip, Project, Time, Title, Track, PROJECT_FILE};

    #[test]
    fn renders_multitrack_without_panicking() {
        let dir = std::env::temp_dir().join(format!("viode-ui-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(PROJECT_FILE);
        let mut project = Project::new("uidemo", 30.0, [640, 360]);
        let t = |s| Time::from_secs_f64(s).unwrap();
        project
            .main_mut()
            .clips
            .push(Clip::media("media/interview.mp4".into(), t(0.0), t(2.0)));

        let mut broll = Track::new("broll", TrackKind::Video);
        let mut over = Clip::media("media/drone.mp4".into(), t(0.0), t(1.0));
        over.at = Some(t(0.5));
        broll.clips.push(over);
        project.tracks.push(broll);
        project.titles.push(Title {
            text: "Intro".into(),
            at: t(0.2),
            dur: t(1.0),
            font: None,
        });
        project.save(&file).unwrap();
        let mut app = App::open(&file).unwrap();
        app.graphics = false; // text fallback path

        let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
        let mut placements = Vec::new();
        terminal.draw(|f| placements = draw(f, &mut app)).unwrap();

        assert!(placements.is_empty(), "no images in text mode");
        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains("uidemo"), "header shows project name");
        assert!(content.contains("interview"), "main lane shows clip name");
        assert!(content.contains("broll"), "overlay lane shows track name");
        assert!(content.contains("Intro"), "title lane shows title text");
    }

    #[test]
    fn graphics_mode_places_ready_thumbs_and_waves() {
        let dir = std::env::temp_dir().join(format!("viode-ui-gfx-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(PROJECT_FILE);
        let mut project = Project::new("gfx", 30.0, [640, 360]);
        let t = |s| Time::from_secs_f64(s).unwrap();
        project
            .main_mut()
            .clips
            .push(Clip::media("media/a.mp4".into(), t(0.0), t(2.0)));
        project.save(&file).unwrap();

        let mut app = App::open(&file).unwrap();
        app.graphics = true;

        // First draw queues generation and reserves the rows.
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        let mut placements = Vec::new();
        terminal.draw(|f| placements = draw(f, &mut app)).unwrap();
        assert!(placements.is_empty(), "nothing ready on the first frame");

        // Stand in for the worker: create exactly the files the draw
        // queued (one strip + one wave), then redraw.
        let pending = app.media.pending();
        assert_eq!(pending.len(), 2, "strip + wave queued: {pending:?}");
        for dest in pending {
            std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
            std::fs::write(dest, b"png").unwrap();
        }

        terminal.draw(|f| placements = draw(f, &mut app)).unwrap();

        assert_eq!(placements.len(), 2, "one thumb + one wave: {placements:?}");
        let thumb = &placements[0];
        let wave = &placements[1];
        assert_eq!(thumb.rows, THUMB_ROWS);
        assert_eq!(wave.rows, WAVE_ROWS);
        assert!(wave.y > thumb.y, "waveform strip sits below the thumb strip");
        assert_ne!(thumb.id, wave.id);
        assert!(thumb.cols >= 2);
    }
}
