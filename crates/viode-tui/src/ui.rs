//! Rendering: App state -> one ratatui frame. Pure function of the state,
//! smoke-tested against a TestBackend.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::app::App;

const CLIP_COLORS: [Color; 4] = [Color::Blue, Color::Cyan, Color::Magenta, Color::Green];

pub fn draw(f: &mut Frame, app: &App) {
    let [header, timeline, details, status] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(6),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .areas(f.area());

    draw_header(f, app, header);
    draw_timeline(f, app, timeline);
    draw_details(f, app, details);
    draw_status(f, app, status);
    if app.show_help {
        draw_help(f);
    }
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let meta = &app.project.project;
    let line = Line::from(vec![
        Span::styled(
            format!(" {}{} ", meta.name, if app.dirty { " *" } else { "" }),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(
            "{}x{} @ {} fps   {} clips   total {}",
            meta.resolution[0],
            meta.resolution[1],
            meta.fps,
            app.project.clips.len(),
            app.project.total_duration(),
        )),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

fn draw_timeline(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default().borders(Borders::ALL).title(" timeline ");
    let inner = block.inner(area);
    f.render_widget(block, area);
    if app.project.clips.is_empty() {
        f.render_widget(
            Paragraph::new("empty — `viode add <file>` some footage first"),
            inner,
        );
        return;
    }

    let width = inner.width as u64;
    let total = app.project.total_duration().0.max(1);
    let selected = app.selected();

    // Each clip gets a proportional span of the width (at least 1 column).
    let mut widths: Vec<u64> = app
        .project
        .clips
        .iter()
        .map(|c| ((c.len().0 as u128 * width as u128) / total as u128).max(1) as u64)
        .collect();
    // Trim overflow from the widest clips so the lane fits.
    while widths.iter().sum::<u64>() > width && widths.iter().any(|w| *w > 1) {
        if let Some(max) = widths.iter_mut().max() {
            *max -= 1;
        }
    }

    // Ruler with the playhead marker.
    let playhead_col = ((app.playhead.0 as u128 * width.max(1) as u128) / total as u128)
        .min(width.saturating_sub(1) as u128) as usize;
    let ruler: String = (0..width as usize)
        .map(|i| if i == playhead_col { '▼' } else { ' ' })
        .collect();

    // The clip lane: colored blocks, selected clip bold-reversed.
    let mut lane = Vec::new();
    let mut labels = Vec::new();
    for (i, (clip, w)) in app.project.clips.iter().zip(&widths).enumerate() {
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
        let w = *w as usize;
        let text = truncate(&format!("{i}:{name}"), w);
        lane.push(Span::styled(format!("{text:^w$}"), style));
        labels.push(Span::raw(truncate(
            &format!("{:^w$}", clip.len().to_string()),
            w,
        )));
    }

    let lines = vec![
        Line::raw(ruler.clone()),
        Line::from(lane),
        Line::from(labels),
        Line::raw(ruler.replace('▼', "▲")),
    ];
    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_details(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default().borders(Borders::ALL).title(" clip ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines = vec![Line::raw(format!(
        "playhead  {}  ",
        app.playhead
    ))];
    if let Some((index, src_time)) = app.source_time() {
        let clip = &app.project.clips[index];
        lines[0].push_span(Span::raw(format!("→ clip {index} @ source {src_time}")));
        lines.push(Line::raw(format!("src       {}", clip.src.display())));
        lines.push(Line::raw(format!(
            "range     [{} .. {}]  len {}",
            clip.in_,
            clip.out,
            clip.len()
        )));
        let start = app.project.positions()[index];
        lines.push(Line::raw(format!(
            "timeline  starts {}  ends {}",
            start,
            start + clip.len()
        )));
    } else {
        lines.push(Line::raw("nothing under playhead"));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let text = if app.message.is_empty() {
        "h/l ±0.1s  H/L ±1s  j/k clips  s split  i/o trim  d del  </> move  u undo  w save  ␣ play  P preview  r render  ? help  q quit"
    } else {
        &app.message
    };
    f.render_widget(
        Paragraph::new(text).style(Style::default().add_modifier(Modifier::DIM)),
        area,
    );
}

fn draw_help(f: &mut Frame) {
    let area = centered(60, 16, f.area());
    let text = "\
  playhead   h/l ±0.1s   H/L ±1s   j/k clip edges
  edit       s split   d delete   i trim start   o trim end
             </> move clip   u undo   U redo
  view       space play clip (mpv)   P preview timeline
  project    w save   r render   q quit

  The playhead selects: verbs act on the clip under it.
  Trims move the clip's SOURCE in/out to the playhead.

                any key to close";
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(text).block(Block::default().borders(Borders::ALL).title(" help ")),
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
    use viode_core::{Clip, Project, Time, PROJECT_FILE};

    #[test]
    fn renders_without_panicking_and_shows_clips() {
        let dir = std::env::temp_dir().join(format!("viode-ui-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(PROJECT_FILE);
        let mut project = Project::new("uidemo", 30.0, [640, 360]);
        let t = |s| Time::from_secs_f64(s).unwrap();
        project.clips.push(Clip {
            src: "media/interview.mp4".into(),
            in_: t(0.0),
            out: t(2.0),
            label: None,
        });
        project.save(&file).unwrap();
        let app = App::open(&file).unwrap();

        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal.draw(|f| draw(f, &app)).unwrap();

        let content = format!("{:?}", terminal.backend().buffer());
        assert!(content.contains("uidemo"), "header shows project name");
        assert!(content.contains("interview"), "timeline shows clip name");
    }
}
