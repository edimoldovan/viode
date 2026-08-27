//! Media metadata via the ffprobe sidecar.

use std::path::Path;
use std::process::Command;

use crate::time::Time;

#[derive(Debug, Clone)]
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

fn parse_rate(rate: &str) -> Option<f64> {
    let (num, den) = rate.split_once('/')?;
    let (num, den): (f64, f64) = (num.parse().ok()?, den.parse().ok()?);
    if den == 0.0 {
        return None;
    }
    Some(num / den)
}
