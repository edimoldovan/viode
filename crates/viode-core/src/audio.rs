//! Audio/visual analysis via the ffmpeg sidecar: silence and scene-change
//! detection. Results are in SOURCE time (positions within the media file);
//! ops like `remove_source_ranges` translate them onto the timeline.

use std::path::Path;
use std::process::Command;

use crate::probe::probe;
use crate::time::Time;

#[derive(Debug, thiserror::Error)]
pub enum AnalyzeError {
    #[error("failed to run ffmpeg (is ffmpeg installed?): {0}")]
    Spawn(#[from] std::io::Error),
    #[error("ffmpeg analysis failed on {0}: {1}")]
    Ffmpeg(String, String),
    #[error(transparent)]
    Probe(#[from] crate::probe::ProbeError),
}

/// Silent stretches in `path`'s audio, quieter than `noise_db` (e.g. -35.0)
/// for at least `min_duration` seconds. Returns (start, end) pairs.
pub fn detect_silences(
    path: &Path,
    noise_db: f64,
    min_duration: f64,
) -> Result<Vec<(Time, Time)>, AnalyzeError> {
    let stderr = run_null_render(
        path,
        &[
            "-af".into(),
            format!("silencedetect=noise={noise_db}dB:d={min_duration}"),
        ],
    )?;

    let mut silences = Vec::new();
    let mut open_start: Option<Time> = None;
    for line in stderr.lines() {
        if let Some(v) = value_after(line, "silence_start:") {
            open_start = Time::from_secs_f64(v).ok();
        } else if let Some(v) = value_after(line, "silence_end:") {
            if let (Some(start), Ok(end)) = (open_start.take(), Time::from_secs_f64(v)) {
                silences.push((start, end));
            }
        }
    }
    // Silence running into EOF has a start but no end line.
    if let Some(start) = open_start {
        silences.push((start, probe(path)?.duration));
    }
    Ok(silences)
}

/// Scene-change positions in `path`, using ffmpeg's scene score (0.0–1.0;
/// 0.4 is a sane default — lower finds more cuts).
pub fn detect_scenes(path: &Path, threshold: f64) -> Result<Vec<Time>, AnalyzeError> {
    let stderr = run_null_render(
        path,
        &[
            "-vf".into(),
            format!("select=gt(scene\\,{threshold}),showinfo"),
        ],
    )?;

    let mut scenes = Vec::new();
    for line in stderr.lines() {
        if !line.contains("Parsed_showinfo") {
            continue;
        }
        if let Some(v) = value_after(line, "pts_time:") {
            if let Ok(t) = Time::from_secs_f64(v) {
                scenes.push(t);
            }
        }
    }
    Ok(scenes)
}

/// Run ffmpeg decoding `path` to the null muxer with the given filter args,
/// returning stderr (where ffmpeg filters print their findings).
fn run_null_render(path: &Path, filter_args: &[String]) -> Result<String, AnalyzeError> {
    let out = Command::new("ffmpeg")
        .args(["-hide_banner", "-nostats", "-i"])
        .arg(path)
        .args(filter_args)
        .args(["-f", "null", "-"])
        .output()?;
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    if !out.status.success() {
        return Err(AnalyzeError::Ffmpeg(
            path.display().to_string(),
            stderr.lines().last().unwrap_or("unknown error").to_string(),
        ));
    }
    Ok(stderr)
}

/// Parse the f64 immediately following `key` in `line`.
fn value_after(line: &str, key: &str) -> Option<f64> {
    let rest = line.split(key).nth(1)?;
    rest.trim_start()
        .split(|c: char| c.is_whitespace() || c == '|')
        .next()?
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_silencedetect_output() {
        let line = "[silencedetect @ 0x5f] silence_start: 1.023";
        assert_eq!(value_after(line, "silence_start:"), Some(1.023));
        let line = "[silencedetect @ 0x5f] silence_end: 2.5 | silence_duration: 1.477";
        assert_eq!(value_after(line, "silence_end:"), Some(2.5));
    }

    #[test]
    fn parses_showinfo_output() {
        let line = "[Parsed_showinfo_1 @ 0x60] n:   0 pts:  90090 pts_time:1.001   fmt:yuv420p";
        assert_eq!(value_after(line, "pts_time:"), Some(1.001));
        assert_eq!(value_after("no match here", "pts_time:"), None);
    }
}
