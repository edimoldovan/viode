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

    // Opt-B: hardware path is OPT-IN because it is machine-dependent —
    // `viode bench` measures which path wins on this box.
    let hw = crate::hwaccel::from_env();
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-y", "-loglevel", "error"]);
    if let Some(hw) = hw {
        cmd.args(hw.decode_args);
    }
    cmd.arg("-i").arg(&src);
    if let Some(hw) = hw {
        cmd.args(hw.encode_args(PROXY_HEIGHT));
    } else {
        cmd.args([
            "-vf", &format!("scale=-2:'min({PROXY_HEIGHT},ih)'"),
            "-c:v", "libx264", "-crf", "28", "-preset", "veryfast",
        ]);
    }
    cmd.args(["-c:a", "aac", "-b:a", "128k"]);
    let out = cmd.arg(&dest).output()?;
    if !out.status.success() {
        let _ = std::fs::remove_file(&dest); // no half-written proxies
        return Err(ProxyError::Ffmpeg(
            src.display().to_string(),
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    Ok(dest)
}

/// Build proxies for many sources CONCURRENTLY (bounded pool). Each
/// ffmpeg already multithreads internally, so a small pool saturates the
/// machine without starving it. Returns per-source results in input order.
pub fn build_all(
    project_dir: &Path,
    sources: &[PathBuf],
    force: bool,
    jobs: usize,
) -> Vec<(PathBuf, Result<PathBuf, ProxyError>)> {
    let jobs = jobs.max(1);
    let mut results: Vec<Option<(PathBuf, Result<PathBuf, ProxyError>)>> =
        (0..sources.len()).map(|_| None).collect();
    let mut i = 0;
    while i < sources.len() {
        let batch = &sources[i..(i + jobs).min(sources.len())];
        let handles: Vec<_> = batch
            .iter()
            .map(|src| {
                let (dir, src) = (project_dir.to_path_buf(), src.clone());
                std::thread::spawn(move || {
                    let r = build_proxy(&dir, &src, force);
                    (src, r)
                })
            })
            .collect();
        for (k, h) in handles.into_iter().enumerate() {
            results[i + k] = Some(h.join().unwrap_or_else(|_| {
                (
                    batch[k].clone(),
                    Err(ProxyError::Ffmpeg(batch[k].display().to_string(), "worker panicked".into())),
                )
            }));
        }
        i += batch.len();
    }
    results.into_iter().map(|r| r.unwrap()).collect()
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
