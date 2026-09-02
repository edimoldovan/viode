//! Stabilization, baked through ffmpeg's vidstab (two passes: detect the
//! camera shake, then transform against it). Same shape as the LUT bake:
//! the WHOLE source is stabilized once into cache/steady, keyed by source
//! mtime and smoothing, and the backend plays the bake — so SOURCE time
//! stays identical and keyframes, transcripts, and trims all stay valid.

use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, thiserror::Error)]
pub enum SteadyError {
    #[error("failed to run ffmpeg (is ffmpeg installed?): {0}")]
    Spawn(#[from] std::io::Error),
    #[error(
        "stabilization needs ffmpeg built with vidstab (libvidstab); \
         this ffmpeg has no vidstabdetect filter"
    )]
    NoVidstab,
    #[error("stabilization failed for {0}: {1}")]
    Ffmpeg(String, String),
}

pub fn vidstab_available() -> bool {
    Command::new("ffmpeg")
        .args(["-hide_banner", "-filters"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains(" vidstabdetect "))
        .unwrap_or(false)
}

fn cache_key(src: &Path, smoothing: u32) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    src.hash(&mut h);
    smoothing.hash(&mut h);
    std::fs::metadata(src)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
        .hash(&mut h);
    h.finish()
}

pub fn baked_path(project_dir: &Path, src: &Path, smoothing: u32) -> PathBuf {
    let stem = src
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "clip".into());
    project_dir
        .join("cache/steady")
        .join(format!("{stem}-{:016x}.mkv", cache_key(src, smoothing)))
}

/// Stabilize `src` (absolute) unless the cached bake exists. Returns the
/// absolute path of the bake.
pub fn ensure_baked(
    project_dir: &Path,
    src: &Path,
    smoothing: u32,
) -> Result<PathBuf, SteadyError> {
    let dest = baked_path(project_dir, src, smoothing);
    if dest.exists() {
        return Ok(dest.canonicalize().unwrap_or(dest));
    }
    if !vidstab_available() {
        return Err(SteadyError::NoVidstab);
    }
    std::fs::create_dir_all(dest.parent().unwrap())?;
    let trf = dest.with_extension("trf");
    // Pass 1: analyze the shake.
    let out = Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error", "-i"])
        .arg(src)
        .arg("-vf")
        .arg(format!("vidstabdetect=result={}", trf.display()))
        .args(["-f", "null", "-"])
        .output()?;
    if !out.status.success() {
        return Err(SteadyError::Ffmpeg(
            src.display().to_string(),
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    // Pass 2: apply the counter-motion, near-lossless like the LUT bake.
    let tmp = dest.with_extension("part.mkv");
    let out = Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error", "-i"])
        .arg(src)
        .arg("-vf")
        .arg(format!(
            "vidstabtransform=input={}:smoothing={smoothing},unsharp=5:5:0.8:3:3:0.4",
            trf.display()
        ))
        .args(["-c:v", "libx264", "-crf", "10", "-preset", "veryfast"])
        .args(["-c:a", "copy"])
        .arg(&tmp)
        .output()?;
    let _ = std::fs::remove_file(&trf);
    if !out.status.success() {
        let _ = std::fs::remove_file(&tmp);
        return Err(SteadyError::Ffmpeg(
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
    fn baked_path_distinguishes_smoothing_levels() {
        let dir = std::env::temp_dir();
        let a = baked_path(&dir, Path::new("m/a.mp4"), 10);
        let b = baked_path(&dir, Path::new("m/a.mp4"), 10);
        let c = baked_path(&dir, Path::new("m/a.mp4"), 30);
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(a.starts_with(dir.join("cache/steady")));
    }
}
