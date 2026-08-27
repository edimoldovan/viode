//! Export presets: take the frame-accurate GES master render and finish it
//! for a destination — correct loudness (two-pass EBU R128 loudnorm),
//! correct shape (9:16 for Shorts), correct container.

use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    /// 16:9 H.264 + AAC, -14 LUFS (YouTube's normalization target).
    Youtube,
    /// 1080x1920 center-crop, -14 LUFS.
    Shorts,
    /// Audio-only m4a, -16 LUFS (podcast standard).
    Podcast,
}

impl Preset {
    pub fn parse(s: &str) -> Option<Preset> {
        match s.to_ascii_lowercase().as_str() {
            "youtube" => Some(Preset::Youtube),
            "shorts" => Some(Preset::Shorts),
            "podcast" => Some(Preset::Podcast),
            _ => None,
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Preset::Podcast => "m4a",
            _ => "mp4",
        }
    }

    fn lufs(self) -> f64 {
        match self {
            Preset::Podcast => -16.0,
            _ => -14.0,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("failed to run ffmpeg (is ffmpeg installed?): {0}")]
    Spawn(#[from] std::io::Error),
    #[error("export failed: {0}")]
    Ffmpeg(String),
    #[error("could not parse loudnorm measurement: {0}")]
    Loudnorm(String),
}

/// Finish `master` into `output` per `preset`. Audio is always two-pass
/// loudness-normalized; video is stream-copied when the shape is unchanged
/// (YouTube) and re-encoded only when it must be (Shorts).
pub fn apply_preset(master: &Path, output: &Path, preset: Preset) -> Result<(), ExportError> {
    if let Some(dir) = output.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let measured = measure_loudness(master, preset.lufs())?;
    let loudnorm = format!(
        "loudnorm=I={}:TP=-1.5:LRA=11:measured_I={}:measured_TP={}:measured_LRA={}:measured_thresh={}:offset={}:linear=true",
        preset.lufs(),
        measured.input_i,
        measured.input_tp,
        measured.input_lra,
        measured.input_thresh,
        measured.target_offset,
    );

    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-y", "-loglevel", "error", "-i"]).arg(master);
    match preset {
        Preset::Youtube => {
            cmd.args(["-c:v", "copy", "-af", &loudnorm, "-c:a", "aac", "-b:a", "256k"]);
        }
        Preset::Shorts => {
            cmd.args([
                "-vf",
                "scale=1080:1920:force_original_aspect_ratio=increase,crop=1080:1920",
                "-c:v", "libx264", "-crf", "20", "-preset", "medium",
                "-af", &loudnorm, "-c:a", "aac", "-b:a", "256k",
            ]);
        }
        Preset::Podcast => {
            cmd.args(["-vn", "-af", &loudnorm, "-c:a", "aac", "-b:a", "160k"]);
        }
    }
    let out = cmd.arg(output).output()?;
    if !out.status.success() {
        return Err(ExportError::Ffmpeg(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    Ok(())
}

struct Measured {
    input_i: f64,
    input_tp: f64,
    input_lra: f64,
    input_thresh: f64,
    target_offset: f64,
}

/// Loudnorm pass 1: measure. ffmpeg prints a JSON block on stderr.
fn measure_loudness(master: &Path, target_i: f64) -> Result<Measured, ExportError> {
    let out = Command::new("ffmpeg")
        .args(["-hide_banner", "-nostats", "-i"])
        .arg(master)
        .args([
            "-af",
            &format!("loudnorm=I={target_i}:TP=-1.5:LRA=11:print_format=json"),
            "-f", "null", "-",
        ])
        .output()?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !out.status.success() {
        return Err(ExportError::Ffmpeg(stderr.trim().to_string()));
    }
    let json_start = stderr
        .rfind('{')
        .ok_or_else(|| ExportError::Loudnorm("no JSON in output".into()))?;
    let block = &stderr[json_start..];
    let v: serde_json::Value = serde_json::from_str(
        &block[..=block.rfind('}').ok_or_else(|| ExportError::Loudnorm("unterminated JSON".into()))?],
    )
    .map_err(|e| ExportError::Loudnorm(e.to_string()))?;

    let f = |key: &str| -> Result<f64, ExportError> {
        v[key]
            .as_str()
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| ExportError::Loudnorm(format!("missing {key}")))
    };
    Ok(Measured {
        input_i: f("input_i")?,
        input_tp: f("input_tp")?,
        input_lra: f("input_lra")?,
        input_thresh: f("input_thresh")?,
        target_offset: f("target_offset")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_parsing() {
        assert_eq!(Preset::parse("YouTube"), Some(Preset::Youtube));
        assert_eq!(Preset::parse("shorts"), Some(Preset::Shorts));
        assert_eq!(Preset::parse("podcast"), Some(Preset::Podcast));
        assert_eq!(Preset::parse("tiktok"), None);
        assert_eq!(Preset::Podcast.extension(), "m4a");
    }
}
