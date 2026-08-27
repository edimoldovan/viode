//! Proxy media: low-res (540p) copies for fast seeking, frame grabs, and
//! previews. On 3-hour 4K footage, proxies are not a nicety — everything
//! interactive goes through them; only the final render touches originals.

use std::path::{Path, PathBuf};
use std::process::Command;

pub const PROXY_HEIGHT: u32 = 540;

#[derive(Debug, thiserror::Error)]
pub enum ProxyError {
    #[error("failed to run ffmpeg (is ffmpeg installed?): {0}")]
    Spawn(#[from] std::io::Error),
    #[error("proxy generation failed for {0}: {1}")]
    Ffmpeg(String, String),
}

/// Where the proxy for `src_rel` (a project-relative media path) lives.
pub fn proxy_rel(src_rel: &Path) -> Option<PathBuf> {
    src_rel.file_name().map(|n| Path::new("proxies").join(n))
}

/// Absolute path to an EXISTING proxy for `src_rel`, if one has been built.
pub fn proxy_for(project_dir: &Path, src_rel: &Path) -> Option<PathBuf> {
    let p = project_dir.join(proxy_rel(src_rel)?);
    p.exists().then_some(p)
}

/// Build (or rebuild with `force`) the proxy for one media file.
/// Returns the absolute proxy path.
pub fn build_proxy(
    project_dir: &Path,
    src_rel: &Path,
    force: bool,
) -> Result<PathBuf, ProxyError> {
    let src = project_dir.join(src_rel);
    let rel = proxy_rel(src_rel).ok_or_else(|| {
        ProxyError::Ffmpeg(src_rel.display().to_string(), "no file name".into())
    })?;
    let dest = project_dir.join(rel);
    if dest.exists() && !force {
        return Ok(dest);
    }
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }

    let out = Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error", "-i"])
        .arg(&src)
        .args([
            "-vf",
            &format!("scale=-2:'min({PROXY_HEIGHT},ih)'"),
            "-c:v", "libx264", "-crf", "28", "-preset", "veryfast",
            "-c:a", "aac", "-b:a", "128k",
        ])
        .arg(&dest)
        .output()?;
    if !out.status.success() {
        let _ = std::fs::remove_file(&dest); // no half-written proxies
        return Err(ProxyError::Ffmpeg(
            src.display().to_string(),
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_paths_map_by_file_name() {
        assert_eq!(
            proxy_rel(Path::new("media/interview.mp4")),
            Some(PathBuf::from("proxies/interview.mp4"))
        );
        assert_eq!(proxy_rel(Path::new("..")), None);
    }
}
