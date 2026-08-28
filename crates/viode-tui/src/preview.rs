//! Inline preview playback: mpv with --vo=kitty renders video INSIDE the
//! terminal. The TUI suspends, mpv owns the terminal (its real controls:
//! space pause, arrows seek, q returns to the editor), the TUI restores.
//! Cuts-only timelines play instantly through an mpv EDL playlist — no
//! rendering step at all.

use std::path::Path;
use std::process::Command;

use viode_core::Project;

/// mpv EDL v0 for the main track: each entry is a (file, start, length)
/// triple. The %N% length-prefix form is immune to commas in paths.
pub fn edl_for(project: &Project, project_dir: &Path) -> String {
    let mut edl = String::from("# mpv EDL v0\n");
    for clip in &project.main().clips {
        let path = viode_core::proxy_for(project_dir, &clip.src)
            .unwrap_or_else(|| project_dir.join(&clip.src));
        // mpv resolves relative EDL entries against the EDL file's own
        // directory — paths must be absolute or nothing plays.
        let path = path.canonicalize().unwrap_or_else(|_| {
            std::env::current_dir()
                .map(|d| d.join(&path))
                .unwrap_or(path)
        });
        let p = path.display().to_string();
        edl.push_str(&format!(
            "%{}%{},{},{}\n",
            p.len(),
            p,
            clip.in_.as_secs_f64(),
            clip.len().as_secs_f64()
        ));
    }
    edl
}

/// Play `target` (an .edl or a rendered file) full-terminal, blocking
/// until the user quits mpv. The caller must have restored the terminal.
pub fn play_blocking(target: &Path, start_secs: f64) -> std::io::Result<()> {
    Command::new("mpv")
        .arg("--vo=kitty")
        .arg(format!("--start={start_secs}"))
        .arg("--really-quiet")
        .arg(target)
        .status()
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use viode_core::{Clip, Time};

    #[test]
    fn edl_lists_main_track_cuts_in_source_time() {
        let dir = Path::new("/proj");
        let mut project = Project::new("p", 30.0, [640, 360]);
        let t = |s| Time::from_secs_f64(s).unwrap();
        project
            .main_mut()
            .clips
            .push(Clip::media("media/a b,c.mp4".into(), t(1.5), t(3.0)));
        project
            .main_mut()
            .clips
            .push(Clip::media("media/d.mp4".into(), t(0.0), t(2.0)));

        let edl = edl_for(&project, dir);
        let lines: Vec<&str> = edl.lines().collect();
        assert_eq!(lines[0], "# mpv EDL v0");
        // Length-prefixed path (comma-proof), then start, then length.
        let p1 = "/proj/media/a b,c.mp4";
        let p2 = "/proj/media/d.mp4";
        assert_eq!(lines[1], format!("%{}%{p1},1.5,1.5", p1.len()));
        assert_eq!(lines[2], format!("%{}%{p2},0,2", p2.len()));
    }
}
