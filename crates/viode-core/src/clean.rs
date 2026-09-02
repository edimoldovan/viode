//! Voice cleanup, baked through ffmpeg's afftdn (FFT denoise — in every
//! ffmpeg build, no model file). Audio-only bake: the video stream is
//! stream-copied, the audio comes back as lossless FLAC, so the bake is
//! fast and the frame timeline is untouched. Chained after `steady` and
//! before the LUT bake; SOURCE time survives the whole chain.

use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, thiserror::Error)]
pub enum CleanError {
    #[error("failed to run ffmpeg (is ffmpeg installed?): {0}")]
    Spawn(#[from] std::io::Error),
    #[error("audio cleanup failed for {0}: {1}")]
    Ffmpeg(String, String),
}

fn cache_key(src: &Path, strength_centi: u64) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    src.hash(&mut h);
    strength_centi.hash(&mut h);
    std::fs::metadata(src)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
        .hash(&mut h);
    h.finish()
}

pub fn baked_path(project_dir: &Path, src: &Path, strength: f64) -> PathBuf {
    let stem = src
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "clip".into());
    let key = cache_key(src, (strength * 100.0) as u64);
    project_dir
        .join("cache/clean")
        .join(format!("{stem}-{key:016x}.mkv"))
}

/// Denoise `src` (absolute) unless the cached bake exists. Returns the
/// absolute bake path.
pub fn ensure_baked(project_dir: &Path, src: &Path, strength: f64) -> Result<PathBuf, CleanError> {
    let dest = baked_path(project_dir, src, strength);
    if dest.exists() {
        return Ok(dest.canonicalize().unwrap_or(dest));
    }
    std::fs::create_dir_all(dest.parent().unwrap())?;
    let tmp = dest.with_extension("part.mkv");
    let out = Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error", "-i"])
        .arg(src)
        .args(["-c:v", "copy"])
        .args(["-af", &format!("highpass=f=60,afftdn=nr={strength}:nf=-30")])
        .args(["-c:a", "flac"])
        .arg(&tmp)
        .output()?;
    if !out.status.success() {
        let _ = std::fs::remove_file(&tmp);
        return Err(CleanError::Ffmpeg(
            src.display().to_string(),
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    std::fs::rename(&tmp, &dest)?;
    Ok(dest.canonicalize().unwrap_or(dest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baked_path_distinguishes_strengths() {
        let dir = std::env::temp_dir();
        let a = baked_path(&dir, Path::new("m/a.mp4"), 12.0);
        let b = baked_path(&dir, Path::new("m/a.mp4"), 12.0);
        let c = baked_path(&dir, Path::new("m/a.mp4"), 30.0);
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(a.starts_with(dir.join("cache/clean")));
    }
}
