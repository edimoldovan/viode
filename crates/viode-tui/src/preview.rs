//! Inline preview playback: mpv with --vo=kitty renders video INSIDE the
//! terminal, positioned over the TUI's preview area. Cuts-only timelines
//! play instantly through an mpv EDL playlist (no rendering); we drive the
//! player over its IPC socket.

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use ratatui::layout::Rect;

use viode_core::Project;

/// mpv EDL v0 for the main track: each entry is a (file, start, length)
/// triple. The %N% length-prefix form is immune to commas in paths.
pub fn edl_for(project: &Project, project_dir: &Path) -> String {
    let mut edl = String::from("# mpv EDL v0\n");
    for clip in &project.main().clips {
        let path = viode_core::proxy_for(project_dir, &clip.src)
            .unwrap_or_else(|| project_dir.join(&clip.src));
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

pub struct Preview {
    child: Child,
    sock: PathBuf,
}

impl Preview {
    /// Play `target` (an .edl or a rendered file) inside `area`, starting
    /// at `start_secs`.
    pub fn spawn(target: &Path, area: Rect, start_secs: f64, sock: PathBuf) -> std::io::Result<Preview> {
        let _ = std::fs::remove_file(&sock);
        let child = Command::new("mpv")
            .arg("--vo=kitty")
            // kitty vo coordinates are 1-based terminal cells.
            .arg(format!("--vo-kitty-left={}", area.x + 1))
            .arg(format!("--vo-kitty-top={}", area.y + 1))
            .arg(format!("--vo-kitty-cols={}", area.width))
            .arg(format!("--vo-kitty-rows={}", area.height))
            .arg("--really-quiet")
            .arg("--no-input-terminal") // our event loop owns the keyboard
            .arg(format!("--input-ipc-server={}", sock.display()))
            .arg(format!("--start={start_secs}"))
            .arg(target)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        Ok(Preview { child, sock })
    }

    /// Fire-and-forget IPC command, e.g. ["cycle", "pause"].
    pub fn command(&self, args: &[&str]) {
        if let Ok(mut stream) = UnixStream::connect(&self.sock) {
            let quoted: Vec<String> = args.iter().map(|a| format!("{a:?}")).collect();
            let _ = writeln!(stream, "{{\"command\":[{}]}}", quoted.join(","));
        }
    }

    pub fn toggle_pause(&self) {
        self.command(&["cycle", "pause"]);
    }

    /// True if mpv already exited on its own (end of playlist, error).
    pub fn finished(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(Some(_)))
    }

    pub fn stop(&mut self) {
        self.command(&["quit"]);
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.sock);
    }
}

impl Drop for Preview {
    fn drop(&mut self) {
        self.stop();
    }
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
