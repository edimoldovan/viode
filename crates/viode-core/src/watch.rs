//! `watch` — open a finished render in the best keyboard-driven player
//! available. When mpv is present (official bundles ship it) the render
//! opens with a generated chapter list: every timeline marker keeps its
//! text and every cut on the main track becomes a chapter, so the
//! viewer jumps edit-to-edit with the chapter keys on top of mpv's
//! frame-stepping and speed grammar. Without mpv the render still
//! opens, through the platform's default player.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::model::Project;
use crate::time::Time;

#[derive(Debug, thiserror::Error)]
pub enum WatchError {
    #[error("{0} does not exist")]
    Missing(PathBuf),
    #[error("no renders yet — render first, or pass the file to watch")]
    NoRenders,
    #[error("{player} failed to start: {source}")]
    Spawn {
        player: &'static str,
        source: std::io::Error,
    },
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// A planned player chapter: where it starts and what to call it.
#[derive(Debug, Clone, PartialEq)]
pub struct Chapter {
    pub at: Time,
    pub title: String,
}

/// What `watch` opened, for the caller's message to the user.
#[derive(Debug)]
pub struct Watched {
    pub file: PathBuf,
    pub player: String,
    pub chapters: usize,
}

/// Plan the chapter list for a render of this project: chapter one is
/// the start, every further main-track cut is a chapter named after the
/// clip it starts (its source file's stem), and every marker is a
/// chapter with the marker's own text. A marker sitting exactly on a
/// cut wins over the cut's generic name.
pub fn chapters(p: &Project) -> Vec<Chapter> {
    let main = p.main();
    let mut out: Vec<Chapter> = Vec::new();
    for (clip, at) in main.clips.iter().zip(main.positions()) {
        let stem = clip
            .src
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| clip.src.display().to_string());
        out.push(Chapter { at, title: stem });
    }
    for m in &p.markers {
        let title = if m.text.is_empty() { "marker".into() } else { m.text.clone() };
        if let Some(hit) = out.iter_mut().find(|c| c.at == m.at) {
            hit.title = title;
        } else {
            out.push(Chapter { at: m.at, title });
        }
    }
    out.sort_by(|a, b| a.at.partial_cmp(&b.at).unwrap_or(std::cmp::Ordering::Equal));
    out.dedup_by(|a, b| a.at == b.at);
    out
}

/// The chapter list as an ffmetadata file, the format both mpv and
/// ffmpeg understand. `end` bounds the final chapter.
pub fn ffmetadata(chapters: &[Chapter], end: Time) -> String {
    fn escape(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        for ch in s.chars() {
            match ch {
                '=' | ';' | '#' | '\\' => {
                    out.push('\\');
                    out.push(ch);
                }
                '\n' => out.push(' '),
                _ => out.push(ch),
            }
        }
        out
    }
    let ms = |t: Time| (t.as_secs_f64() * 1000.0).round() as i64;
    let mut out = String::from(";FFMETADATA1\n");
    for (i, c) in chapters.iter().enumerate() {
        let stop = chapters
            .get(i + 1)
            .map(|n| ms(n.at))
            .unwrap_or_else(|| ms(end).max(ms(c.at) + 1));
        out.push_str(&format!(
            "[CHAPTER]\nTIMEBASE=1/1000\nSTART={}\nEND={}\ntitle={}\n",
            ms(c.at),
            stop,
            escape(&c.title)
        ));
    }
    out
}

/// The newest regular file in the project's renders directory.
pub fn newest_render(renders: &Path) -> Option<PathBuf> {
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in std::fs::read_dir(renders).ok()?.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let modified = entry.metadata().ok()?.modified().ok()?;
        if best.as_ref().map(|(t, _)| modified > *t).unwrap_or(true) {
            best = Some((modified, path));
        }
    }
    best.map(|(_, p)| p)
}

fn on_path(bin: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| {
            std::env::split_paths(&paths).any(|dir| dir.join(bin).is_file())
        })
        .unwrap_or(false)
}

/// Open `file` (or the newest render) in the player, detached. The
/// project supplies the chapter list; the total timeline duration
/// bounds the last chapter.
pub fn watch(project_dir: &Path, p: &Project, file: Option<&Path>) -> Result<Watched, WatchError> {
    let file = match file {
        Some(f) => {
            let f = if f.is_absolute() { f.to_path_buf() } else { project_dir.join(f) };
            if !f.is_file() {
                return Err(WatchError::Missing(f));
            }
            f
        }
        None => newest_render(&project_dir.join("renders")).ok_or(WatchError::NoRenders)?,
    };

    let chapter_list = chapters(p);
    let main = p.main();
    let end = main
        .positions()
        .last()
        .copied()
        .zip(main.clips.last())
        .map(|(at, c)| at + c.len())
        .unwrap_or(Time::ZERO);

    if on_path("mpv") {
        let cache = project_dir.join("cache");
        std::fs::create_dir_all(&cache)?;
        let chapters_file = cache.join("watch-chapters.ffmetadata");
        std::fs::write(&chapters_file, ffmetadata(&chapter_list, end))?;
        let title = format!(
            "Viode — {}",
            file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
        );
        Command::new("mpv")
            .arg("--keep-open=yes")
            .arg("--force-window=yes")
            .arg(format!("--title={title}"))
            .arg(format!("--chapters-file={}", chapters_file.display()))
            .arg(&file)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|source| WatchError::Spawn { player: "mpv", source })?;
        return Ok(Watched { file, player: "mpv".into(), chapters: chapter_list.len() });
    }

    // No mpv: the platform opener still shows the render, chapterless.
    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    Command::new(opener)
        .arg(&file)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|source| WatchError::Spawn { player: opener, source })?;
    Ok(Watched { file, player: "the default player (install mpv for chapters and frame-stepping)".into(), chapters: 0 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Clip, Marker};

    fn project_with_two_clips_and_a_marker() -> Project {
        let mut p = Project::new("t", 30.0, [1920, 1080]);
        p.main_mut().clips.push(Clip::media(
            "media/intro take.mp4".into(),
            Time::ZERO,
            Time::from_secs_f64(4.0).unwrap(),
        ));
        p.main_mut().clips.push(Clip::media(
            "media/main.mp4".into(),
            Time::ZERO,
            Time::from_secs_f64(6.0).unwrap(),
        ));
        p.markers.push(Marker {
            at: Time::from_secs_f64(2.0).unwrap(),
            text: "fix; color=here".into(),
            color: None,
        });
        p
    }

    #[test]
    fn chapters_cover_cuts_and_markers_sorted() {
        let p = project_with_two_clips_and_a_marker();
        let ch = chapters(&p);
        let titles: Vec<_> = ch.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(titles, ["intro take", "fix; color=here", "main"]);
        assert!(ch.windows(2).all(|w| w[0].at < w[1].at));
    }

    #[test]
    fn a_marker_on_a_cut_replaces_the_cut_name() {
        let mut p = project_with_two_clips_and_a_marker();
        p.markers[0].at = Time::from_secs_f64(4.0).unwrap();
        let ch = chapters(&p);
        assert_eq!(ch.len(), 2, "no duplicate chapter at the cut");
        assert_eq!(ch[1].title, "fix; color=here");
    }

    #[test]
    fn ffmetadata_escapes_and_bounds_chapters() {
        let p = project_with_two_clips_and_a_marker();
        let meta = ffmetadata(&chapters(&p), Time::from_secs_f64(10.0).unwrap());
        assert!(meta.starts_with(";FFMETADATA1\n"));
        assert!(meta.contains("title=fix\\; color\\=here"));
        assert!(meta.contains("TIMEBASE=1/1000"));
        // The marker chapter ends where the second clip starts, and the
        // last chapter ends at the timeline's end.
        assert!(meta.contains("START=2000\nEND=4000"));
        assert!(meta.contains("START=4000\nEND=10000"));
    }

    #[test]
    fn newest_render_picks_by_mtime() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("old.mp4");
        let new = dir.path().join("new.mp4");
        std::fs::write(&old, b"a").unwrap();
        std::fs::write(&new, b"b").unwrap();
        let earlier = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
        let f = std::fs::File::options().write(true).open(&old).unwrap();
        f.set_modified(earlier).unwrap();
        assert_eq!(newest_render(dir.path()).unwrap(), new);
    }

    #[test]
    fn watching_without_renders_errors_helpfully() {
        let dir = tempfile::tempdir().unwrap();
        let p = Project::new("t", 30.0, [1920, 1080]);
        let err = watch(dir.path(), &p, None).unwrap_err();
        assert!(err.to_string().contains("render first"));
    }
}
