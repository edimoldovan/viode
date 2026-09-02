//! Theme colors from the Omarchy terminal palette — the GUI follows the
//! system theme the same way the TUI does. The TUI gets it for free by
//! naming ANSI colors; the GUI reads the same palette from the current
//! theme's alacritty.toml and derives its surfaces from background/
//! foreground mixes, so every Omarchy theme just works. Machines without
//! Omarchy get a neutral dark fallback.

use eframe::egui;
use eframe::egui::Color32;

#[derive(Clone, Copy)]
pub struct Palette {
    /// Window and panel background.
    pub bg: Color32,
    /// Primary text.
    pub fg: Color32,
    /// Secondary text (labels, ruler).
    pub dim: Color32,
    /// Track lane background.
    pub lane: Color32,
    /// Clip body and border.
    pub clip: Color32,
    pub clip_edge: Color32,
    /// The theme accent — timecode, playhead, selection.
    pub accent: Color32,
    /// Title clips in the timeline.
    pub title: Color32,
    /// Waveform tint.
    pub wave: Color32,
}

/// The subset of the alacritty color scheme the palette derives from.
struct Ansi {
    background: Color32,
    foreground: Color32,
    normal_yellow: Color32,
    normal_blue: Color32,
    bright_blue: Color32,
}

pub fn load() -> Palette {
    let path = std::env::var_os("HOME").map(|home| {
        std::path::Path::new(&home).join(".local/state/omarchy/current/theme/alacritty.toml")
    });
    let ansi = path
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| parse(&s))
        .unwrap_or_else(fallback);
    derive(ansi)
}

fn fallback() -> Ansi {
    Ansi {
        background: Color32::from_rgb(18, 20, 24),
        foreground: Color32::from_rgb(190, 196, 205),
        normal_yellow: Color32::from_rgb(196, 160, 66),
        normal_blue: Color32::from_rgb(100, 130, 180),
        bright_blue: Color32::from_rgb(120, 160, 220),
    }
}

fn derive(a: Ansi) -> Palette {
    Palette {
        bg: a.background,
        fg: a.foreground,
        dim: mix(a.foreground, a.background, 0.45),
        lane: mix(a.background, a.foreground, 0.07),
        clip: mix(a.background, a.foreground, 0.16),
        clip_edge: mix(a.background, a.foreground, 0.38),
        accent: a.bright_blue,
        title: a.normal_yellow,
        wave: a.normal_blue,
    }
}

/// egui-wide visuals derived from the palette, so every button, panel,
/// and dialog wears the Omarchy theme — not just the painter-drawn
/// timeline. Applied by both the welcome screen and the editor.
pub fn visuals(p: &Palette) -> egui::Visuals {
    let mut v = egui::Visuals::dark();
    v.override_text_color = Some(p.fg);
    v.panel_fill = p.bg;
    v.window_fill = p.bg;
    v.extreme_bg_color = mix(p.bg, p.fg, 0.04);
    v.faint_bg_color = mix(p.bg, p.fg, 0.06);
    v.window_stroke = egui::Stroke::new(1.0, p.clip_edge);
    v.selection.bg_fill = p.accent.gamma_multiply(0.35);
    v.selection.stroke = egui::Stroke::new(1.0, p.accent);
    v.hyperlink_color = p.accent;
    v.widgets.noninteractive.bg_fill = p.bg;
    v.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, p.dim);
    v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, p.lane);
    v.widgets.inactive.bg_fill = mix(p.bg, p.fg, 0.10);
    v.widgets.inactive.weak_bg_fill = mix(p.bg, p.fg, 0.10);
    v.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, p.fg);
    v.widgets.hovered.bg_fill = mix(p.bg, p.accent, 0.25);
    v.widgets.hovered.weak_bg_fill = mix(p.bg, p.accent, 0.25);
    v.widgets.hovered.fg_stroke = egui::Stroke::new(1.5, p.fg);
    v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, p.accent);
    v.widgets.active.bg_fill = mix(p.bg, p.accent, 0.40);
    v.widgets.active.weak_bg_fill = mix(p.bg, p.accent, 0.40);
    v.widgets.active.fg_stroke = egui::Stroke::new(1.5, p.fg);
    v.widgets.open.bg_fill = mix(p.bg, p.accent, 0.20);
    v.widgets.open.weak_bg_fill = mix(p.bg, p.accent, 0.20);
    v
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let ch = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(ch(a.r(), b.r()), ch(a.g(), b.g()), ch(a.b(), b.b()))
}

/// Parse the handful of colors we need out of an alacritty.toml. Kept
/// forgiving: any missing key falls back, a broken file yields None.
fn parse(toml_str: &str) -> Option<Ansi> {
    let doc: toml::Value = toml_str.parse().ok()?;
    let colors = doc.get("colors")?;
    let get = |section: &str, key: &str| -> Option<Color32> {
        colors
            .get(section)?
            .get(key)?
            .as_str()
            .and_then(hex_color)
    };
    let fb = fallback();
    Some(Ansi {
        background: get("primary", "background").unwrap_or(fb.background),
        foreground: get("primary", "foreground").unwrap_or(fb.foreground),
        normal_yellow: get("normal", "yellow").unwrap_or(fb.normal_yellow),
        normal_blue: get("normal", "blue").unwrap_or(fb.normal_blue),
        bright_blue: get("bright", "blue")
            .or_else(|| get("normal", "blue"))
            .unwrap_or(fb.bright_blue),
    })
}

fn hex_color(s: &str) -> Option<Color32> {
    let hex = s.trim().strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(hex, 16).ok()?;
    Some(Color32::from_rgb(
        (v >> 16) as u8,
        (v >> 8) as u8,
        v as u8,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r##"
[colors.primary]
background = "#121212"
foreground = "#bebebe"

[colors.normal]
yellow = "#b91c1c"
blue = "#e68e0d"

[colors.bright]
blue = "#f59e0b"
"##;

    #[test]
    fn parses_the_omarchy_alacritty_palette() {
        let a = parse(SAMPLE).expect("sample parses");
        assert_eq!(a.background, Color32::from_rgb(0x12, 0x12, 0x12));
        assert_eq!(a.foreground, Color32::from_rgb(0xbe, 0xbe, 0xbe));
        assert_eq!(a.bright_blue, Color32::from_rgb(0xf5, 0x9e, 0x0b));
        let p = derive(a);
        assert_eq!(p.bg, Color32::from_rgb(0x12, 0x12, 0x12));
        assert_eq!(p.accent, Color32::from_rgb(0xf5, 0x9e, 0x0b));
        // Lane sits between background and foreground.
        assert!(p.lane.r() > p.bg.r() && p.lane.r() < p.fg.r());
    }

    #[test]
    fn missing_keys_fall_back_instead_of_failing() {
        let a = parse("[colors.primary]\nbackground = \"#000000\"\n").expect("parses");
        assert_eq!(a.background, Color32::BLACK);
        assert_eq!(a.foreground, fallback().foreground);
        // bright.blue falls back through normal.blue to the default.
        assert_eq!(a.bright_blue, fallback().bright_blue);
    }

    #[test]
    fn garbage_input_yields_none() {
        assert!(parse("not toml at all [[[").is_none());
        assert!(parse("[colors]\n").is_some()); // empty colors -> all fallbacks
    }

    #[test]
    fn hex_colors_parse_strictly() {
        assert_eq!(hex_color("#ff0080"), Some(Color32::from_rgb(255, 0, 128)));
        assert_eq!(hex_color("ff0080"), None);
        assert_eq!(hex_color("#ff008"), None);
    }
}
