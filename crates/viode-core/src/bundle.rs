//! Bundles: a whole project used as one clip in another project — the
//! nested sequence, Viode style. The clip's `src` simply points at the
//! sub-project's .viode file; at render/preview time the sub-project's
//! master is baked into cache/bundles (keyed by the sub-project file's
//! mtime) and plays like ordinary footage. Nesting recurses naturally;
//! a depth guard stops accidental cycles.

use std::path::{Path, PathBuf};

use crate::backend::{GesBackend, RenderBackend, RenderError};
use crate::model::Project;

const MAX_DEPTH: usize = 4;
const DEPTH_VAR: &str = "VIODE_BUNDLE_DEPTH";

#[derive(Debug, thiserror::Error)]
pub enum BundleError {
    #[error("bundles nest at most {MAX_DEPTH} deep — is there a cycle?")]
    TooDeep,
    #[error("could not load bundled project {0}: {1}")]
    Load(PathBuf, String),
    #[error("could not render bundled project {0}: {1}")]
    Render(PathBuf, String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Is this clip source a bundled project?
pub fn is_bundle(src: &Path) -> bool {
    src.extension().is_some_and(|e| e == "viode")
}

fn cache_key(sub_file: &Path) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    sub_file.hash(&mut h);
    std::fs::metadata(sub_file)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
        .hash(&mut h);
    h.finish()
}

pub fn baked_path(project_dir: &Path, sub_file: &Path) -> PathBuf {
    let stem = sub_file
        .parent()
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "bundle".into());
    project_dir
        .join("cache/bundles")
        .join(format!("{stem}-{:016x}.mp4", cache_key(sub_file)))
}

/// Render (or reuse) the bundled project's master. `sub_file` is the
/// absolute path of the sub-project's .viode file.
pub fn ensure_baked(project_dir: &Path, sub_file: &Path) -> Result<PathBuf, BundleError> {
    let depth: usize = std::env::var(DEPTH_VAR)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if depth >= MAX_DEPTH {
        return Err(BundleError::TooDeep);
    }
    let dest = baked_path(project_dir, sub_file);
    if dest.exists() {
        return Ok(dest.canonicalize().unwrap_or(dest));
    }
    std::fs::create_dir_all(dest.parent().unwrap())?;
    let sub = Project::load(sub_file)
        .map_err(|e| BundleError::Load(sub_file.to_path_buf(), e.to_string()))?;
    let sub_dir = sub_file
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    std::env::set_var(DEPTH_VAR, (depth + 1).to_string());
    let result = GesBackend
        .render(&sub, &sub_dir, &dest)
        .map_err(|e: RenderError| BundleError::Render(sub_file.to_path_buf(), e.to_string()));
    std::env::set_var(DEPTH_VAR, depth.to_string());
    result?;
    Ok(dest.canonicalize().unwrap_or(dest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_sources_are_recognized_by_extension() {
        assert!(is_bundle(Path::new("../intro/project.viode")));
        assert!(!is_bundle(Path::new("media/a.mp4")));
    }

    #[test]
    fn baked_path_is_named_after_the_sub_project_directory() {
        let p = baked_path(Path::new("/proj"), Path::new("/elsewhere/intro/project.viode"));
        assert!(p.starts_with("/proj/cache/bundles"));
        assert!(p.file_name().unwrap().to_string_lossy().starts_with("intro-"));
    }
}
