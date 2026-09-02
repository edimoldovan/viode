//! .cube LUTs, baked through ffmpeg. No stock GStreamer build ships a
//! lut3d element, and color math is ffmpeg's home turf (tetrahedral
//! interpolation, correct range handling) — so when a clip carries a LUT
//! we bake the WHOLE source file once through ffmpeg's lut3d filter and
//! hand GES the baked file instead of the original.
//!
//! Whole-file rather than per-range on purpose: the baked file is a
//! frame-identical timeline of the source, so SOURCE time stays valid —
//! keyframes, silence data, and transcripts all carry over, there is no
//! offset math, and every trim of that source reuses one bake. Bakes are
//! cached in cache/luts, keyed by source and LUT modification times, in
//! near-lossless H.264 with the audio streams copied untouched.

use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, thiserror::Error)]
pub enum LutError {
    #[error("failed to run ffmpeg (is ffmpeg installed?): {0}")]
    Spawn(#[from] std::io::Error),
    #[error("LUT bake failed for {0}: {1}")]
    Ffmpeg(String, String),
}

/// Cheap content key: paths plus modification times. Touch the source or
/// the .cube file and the next render re-bakes.
fn cache_key(src: &Path, lut: &Path) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let mtime = |p: &Path| {
        std::fs::metadata(p)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0)
    };
    src.hash(&mut h);
    lut.hash(&mut h);
    mtime(src).hash(&mut h);
    mtime(lut).hash(&mut h);
    h.finish()
}

/// Where the bake for this (source, LUT) pair lives under the project.
pub fn baked_path(project_dir: &Path, src: &Path, lut: &Path) -> PathBuf {
    let stem = src
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "clip".into());
    // .mkv holds whatever audio codec the source had (-c:a copy).
    project_dir
        .join("cache/luts")
        .join(format!("{stem}-{:016x}.mkv", cache_key(src, lut)))
}

/// Bake `src` through `lut` unless the cached bake already exists.
/// Returns the absolute path of the baked file. `src` and `lut` are
/// absolute paths; call sites resolve project-relative ones first.
pub fn ensure_baked(project_dir: &Path, src: &Path, lut: &Path) -> Result<PathBuf, LutError> {
    let dest = baked_path(project_dir, src, lut);
    if dest.exists() {
        // GES URIs need absolute paths, whatever project_dir looked like.
        return Ok(dest.canonicalize().unwrap_or(dest));
    }
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // crf 10 is visually lossless for an intermediate; veryfast keeps the
    // one-time bake cheap. Audio passes through untouched.
    let tmp = dest.with_extension("part.mkv");
    let out = Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error", "-i"])
        .arg(src)
        .arg("-vf")
        .arg(format!("lut3d={}", lut.display()))
        .args(["-c:v", "libx264", "-crf", "10", "-preset", "veryfast"])
        .args(["-c:a", "copy"])
        .arg(&tmp)
        .output()?;
    if !out.status.success() {
        let _ = std::fs::remove_file(&tmp);
        return Err(LutError::Ffmpeg(
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
    fn baked_path_is_stable_for_same_inputs_and_distinct_for_different_luts() {
        let dir = std::env::temp_dir();
        let a = baked_path(&dir, Path::new("media/a.mp4"), Path::new("looks/warm.cube"));
        let b = baked_path(&dir, Path::new("media/a.mp4"), Path::new("looks/warm.cube"));
        let c = baked_path(&dir, Path::new("media/a.mp4"), Path::new("looks/cold.cube"));
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(a.starts_with(dir.join("cache/luts")));
        assert!(a.file_name().unwrap().to_string_lossy().starts_with("a-"));
    }
}
