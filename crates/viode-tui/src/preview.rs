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

/// A player positioned INSIDE the TUI: mpv --vo=kitty pinned to a cell
/// rectangle, driven over its IPC socket. The TUI freezes its own drawing
/// while this is alive so the two never interleave terminal writes.
pub struct Preview {
    child: std::process::Child,
    sock: std::path::PathBuf,
}

impl Preview {
    pub fn spawn(
        target: &Path,
        area: ratatui::layout::Rect,
        start_secs: f64,
        paused: bool,
        sock: std::path::PathBuf,
    ) -> std::io::Result<Preview> {
        let _ = std::fs::remove_file(&sock);
        let child = Command::new("mpv")
            .arg("--vo=kitty")
            .arg(format!("--pause={}", if paused { "yes" } else { "no" }))
            // kitty vo geometry is 1-based terminal cells.
            .arg(format!("--vo-kitty-left={}", area.x + 1))
            .arg(format!("--vo-kitty-top={}", area.y + 1))
            .arg(format!("--vo-kitty-cols={}", area.width))
            .arg(format!("--vo-kitty-rows={}", area.height))
            .arg(format!("--start={start_secs}"))
            .arg(format!("--input-ipc-server={}", sock.display()))
            .arg("--really-quiet")
            // Draw into OUR screen — no alternate screen, no clearing the
            // TUI away.
            .arg("--vo-kitty-alt-screen=no")
            .arg("--no-input-terminal") // our event loop owns the keyboard
            .arg(target)
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?;
        Ok(Preview { child, sock })
    }

    /// Ask mpv where it is: (seconds into the playlist, paused?). Best
    /// effort with a short timeout — None if mpv isn't answering.
    pub fn position(&self) -> Option<(f64, bool)> {
        use std::io::{BufRead, BufReader, Write};
        let mut stream = std::os::unix::net::UnixStream::connect(&self.sock).ok()?;
        stream
            .set_read_timeout(Some(std::time::Duration::from_millis(300)))
            .ok()?;
        writeln!(stream, "{{\"command\":[\"get_property\",\"time-pos\"],\"request_id\":1}}").ok()?;
        writeln!(stream, "{{\"command\":[\"get_property\",\"pause\"],\"request_id\":2}}").ok()?;
        let reader = BufReader::new(stream);
        let (mut pos, mut paused) = (None, None);
        for line in reader.lines() {
            let Ok(line) = line else { break };
            if line.contains("\"request_id\":1") {
                pos = line
                    .split("\"data\":")
                    .nth(1)
                    .and_then(|r| r.split([',', '}']).next())
                    .and_then(|v| v.trim().parse::<f64>().ok());
            } else if line.contains("\"request_id\":2") {
                paused = Some(line.contains("\"data\":true"));
            }
            if pos.is_some() && paused.is_some() {
                break;
            }
        }
        Some((pos?, paused.unwrap_or(false)))
    }

    pub fn toggle_pause(&self) {
        if let Ok(mut s) = std::os::unix::net::UnixStream::connect(&self.sock) {
            use std::io::Write;
            let _ = writeln!(s, "{{\"command\":[\"cycle\",\"pause\"]}}");
        }
    }

    pub fn finished(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(Some(_)))
    }

    pub fn stop(&mut self) {
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
