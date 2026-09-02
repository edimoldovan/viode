//! Media management: find clips whose sources vanished (drive moved,
//! folder renamed) and relink them — a project must reconnect, not die.

use std::path::{Path, PathBuf};

use crate::model::Project;

/// Every (track, clip, src) whose source file does not exist.
#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("source has no file name")]
    NoName,
    #[error("media/{0} already exists with different content")]
    Collision(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Bring a media file into the project: paths already inside the project
/// stay referenced in place; outside files copy into media/. Re-adding
/// the same file reuses the existing copy; a different file under the
/// same name is a refused collision. Returns the project-relative path.
pub fn bring_in(project_dir: &Path, src: &Path) -> Result<PathBuf, ImportError> {
    let canon_dir = std::fs::canonicalize(project_dir)?;
    if let Ok(canon_src) = std::fs::canonicalize(src) {
        if let Ok(rel) = canon_src.strip_prefix(&canon_dir) {
            return Ok(rel.to_path_buf());
        }
        let name = canon_src.file_name().ok_or(ImportError::NoName)?;
        let dest = project_dir.join("media").join(name);
        if dest.exists() {
            let same = std::fs::metadata(&canon_src).map(|m| m.len()).ok()
                == std::fs::metadata(&dest).map(|m| m.len()).ok();
            if same {
                return Ok(PathBuf::from("media").join(name));
            }
            return Err(ImportError::Collision(name.to_string_lossy().into_owned()));
        }
        std::fs::create_dir_all(dest.parent().unwrap())?;
        std::fs::copy(&canon_src, &dest)?;
        return Ok(PathBuf::from("media").join(name));
    }
    Err(ImportError::Io(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        src.display().to_string(),
    )))
}

pub fn missing(project: &Project, project_dir: &Path) -> Vec<(usize, usize, PathBuf)> {
    let mut out = Vec::new();
    for (ti, track) in project.tracks.iter().enumerate() {
        for (ci, clip) in track.clips.iter().enumerate() {
            let p = if clip.src.is_absolute() {
                clip.src.clone()
            } else {
                project_dir.join(&clip.src)
            };
            if !p.exists() {
                out.push((ti, ci, clip.src.clone()));
            }
        }
    }
    out
}

/// Relink every missing clip by FILE NAME against `new_dir` (searched
/// recursively, 3 levels). Returns how many clips were relinked.
pub fn relink(project: &mut Project, project_dir: &Path, new_dir: &Path) -> usize {
    let lost = missing(project, project_dir);
    let mut relinked = 0;
    for (ti, ci, old_src) in lost {
        let Some(name) = old_src.file_name() else { continue };
        if let Some(found) = find_by_name(new_dir, name, 3) {
            project.tracks[ti].clips[ci].src = found;
            relinked += 1;
        }
    }
    relinked
}

fn find_by_name(dir: &Path, name: &std::ffi::OsStr, depth: u8) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut subdirs = Vec::new();
    for e in entries.flatten() {
        let p = e.path();
        if p.is_file() && p.file_name() == Some(name) {
            return Some(p);
        }
        if p.is_dir() {
            subdirs.push(p);
        }
    }
    if depth > 0 {
        for sub in subdirs {
            if let Some(found) = find_by_name(&sub, name, depth - 1) {
                return Some(found);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Clip;
    use crate::time::Time;

    #[test]
    fn missing_and_relink_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let proj_dir = dir.path();
        std::fs::create_dir_all(proj_dir.join("media")).unwrap();
        std::fs::write(proj_dir.join("media/here.mp4"), b"x").unwrap();

        let mut p = Project::new("m", 30.0, [640, 360]);
        let t = |s| Time::from_secs_f64(s).unwrap();
        p.main_mut().clips.push(Clip::media("media/here.mp4".into(), t(0.0), t(1.0)));
        p.main_mut().clips.push(Clip::media("media/gone.mp4".into(), t(0.0), t(1.0)));

        let lost = missing(&p, proj_dir);
        assert_eq!(lost.len(), 1);
        assert_eq!(lost[0].2, PathBuf::from("media/gone.mp4"));

        // The file reappears on another drive, nested one level down.
        let new_home = dir.path().join("backup/cam");
        std::fs::create_dir_all(&new_home).unwrap();
        std::fs::write(new_home.join("gone.mp4"), b"x").unwrap();

        assert_eq!(relink(&mut p, proj_dir, dir.path()), 1);
        assert!(missing(&p, proj_dir).is_empty());
        assert!(p.main().clips[1].src.ends_with("backup/cam/gone.mp4"));
    }
}
