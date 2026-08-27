//! Media metadata via the ffprobe sidecar.

use std::path::Path;
use std::process::Command;

use crate::time::Time;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MediaInfo {
    pub duration: Time,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps: Option<f64>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    #[error("failed to run ffprobe (is ffmpeg installed?): {0}")]
    Spawn(#[from] std::io::Error),
    #[error("ffprobe failed on {0}: {1}")]
    Ffprobe(String, String),
    #[error("could not parse ffprobe output for {0}: {1}")]
    Parse(String, String),
}

pub fn probe(path: &Path) -> Result<MediaInfo, ProbeError> {
    let display = path.display().to_string();
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-print_format", "json", "-show_format", "-show_streams"])
        .arg(path)
        .output()?;
    if !out.status.success() {
        return Err(ProbeError::Ffprobe(
            display,
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    let json: serde_json::Value = serde_json::from_slice(&out.stdout)
        .map_err(|e| ProbeError::Parse(display.clone(), e.to_string()))?;

    let duration = json["format"]["duration"]
        .as_str()
        .and_then(|s| s.parse::<f64>().ok())
        .and_then(|s| Time::from_secs_f64(s).ok())
        .ok_or_else(|| ProbeError::Parse(display.clone(), "no duration".into()))?;

    let mut info = MediaInfo {
        duration,
        width: None,
        height: None,
        fps: None,
        video_codec: None,
        audio_codec: None,
    };

    for stream in json["streams"].as_array().into_iter().flatten() {
        match stream["codec_type"].as_str() {
            Some("video") if info.video_codec.is_none() => {
                info.video_codec = stream["codec_name"].as_str().map(String::from);
                info.width = stream["width"].as_u64().map(|w| w as u32);
                info.height = stream["height"].as_u64().map(|h| h as u32);
                info.fps = stream["avg_frame_rate"].as_str().and_then(parse_rate);
            }
            Some("audio") if info.audio_codec.is_none() => {
                info.audio_codec = stream["codec_name"].as_str().map(String::from);
            }
            _ => {}
        }
    }
    Ok(info)
}

/// Cache key: path + size + mtime — any change to the file invalidates it.
fn cache_key(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    Some(format!("{}|{}|{}", path.display(), meta.len(), mtime))
}

/// Like `probe`, but memoized in `<project_dir>/cache/probe.json`. On
/// 3-hour files ffprobe isn't free, and add/take/sync hit the same sources
/// repeatedly.
pub fn probe_cached(project_dir: &Path, path: &Path) -> Result<MediaInfo, ProbeError> {
    let cache_path = project_dir.join("cache").join("probe.json");
    let mut cache: std::collections::HashMap<String, MediaInfo> = std::fs::read_to_string(&cache_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    let key = cache_key(path);
    if let Some(k) = &key {
        if let Some(hit) = cache.get(k) {
            return Ok(hit.clone());
        }
    }
    let info = probe(path)?;
    if let Some(k) = key {
        cache.insert(k, info.clone());
        if let Ok(json) = serde_json::to_string(&cache) {
            let _ = std::fs::create_dir_all(project_dir.join("cache"));
            let _ = std::fs::write(&cache_path, json);
        }
    }
    Ok(info)
}

fn parse_rate(rate: &str) -> Option<f64> {
    let (num, den) = rate.split_once('/')?;
    let (num, den): (f64, f64) = (num.parse().ok()?, den.parse().ok()?);
    if den == 0.0 {
        return None;
    }
    Some(num / den)
}
