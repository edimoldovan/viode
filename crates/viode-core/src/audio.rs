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

pub const DEFAULT_NOISE_DB: f64 = -35.0;
pub const DEFAULT_MIN_SILENCE: f64 = 0.5;
pub const DEFAULT_LEVEL_WINDOW: f64 = 0.5;

/// Both audio answers from ONE decode of the file.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AudioScan {
    pub silences: Vec<(Time, Time)>,
    pub levels: Vec<(Time, f64)>,
}

/// O2: silences AND RMS levels in a single ffmpeg pass, cached under the
/// project's cache/ keyed by file identity + parameters. A 92-minute file
/// costs one decode ever, not one per question.
pub fn audio_scan(
    project_dir: &Path,
    path: &Path,
    noise_db: f64,
    min_duration: f64,
    window: f64,
) -> Result<AudioScan, AnalyzeError> {
    // The two halves cache INDEPENDENTLY: a silences query and a levels
    // query with unrelated parameters still share the one decode that
    // produced them both.
    let file_key = file_identity(path);
    let sil_cache = project_dir.join("cache").join(format!(
        "audiosil_{file_key:016x}_{:x}_{:x}.json",
        noise_db.to_bits(),
        min_duration.to_bits()
    ));
    let lvl_cache = project_dir
        .join("cache")
        .join(format!("audiolvl_{file_key:016x}_{:x}.json", window.to_bits()));
    let cached_sil: Option<Vec<(Time, Time)>> = std::fs::read_to_string(&sil_cache)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok());
    let cached_lvl: Option<Vec<(Time, f64)>> = std::fs::read_to_string(&lvl_cache)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok());
    if let (Some(silences), Some(levels)) = (&cached_sil, &cached_lvl) {
        return Ok(AudioScan {
            silences: silences.clone(),
            levels: levels.clone(),
        });
    }

    let samples_per_window = ((window * 8000.0).round() as u64).max(1);
    let out = Command::new("ffmpeg")
        .args(["-hide_banner", "-nostats", "-i"])
        .arg(path)
        .args([
            "-af",
            &format!(
                "silencedetect=noise={noise_db}dB:d={min_duration},\
                 aresample=8000,asetnsamples=n={samples_per_window},\
                 astats=metadata=1:reset=1,\
                 ametadata=print:key=lavfi.astats.Overall.RMS_level:file=-"
            ),
            "-f", "null", "-",
        ])
        .output()?;
    if !out.status.success() {
        return Err(AnalyzeError::Ffmpeg(
            path.display().to_string(),
            String::from_utf8_lossy(&out.stderr)
                .lines()
                .last()
                .unwrap_or("unknown error")
                .to_string(),
        ));
    }

    // Silences arrive on stderr, level metadata on stdout.
    let stderr = String::from_utf8_lossy(&out.stderr);
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
    if let Some(start) = open_start {
        silences.push((start, probe(path)?.duration));
    }

    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut levels = Vec::new();
    let mut at: Option<Time> = None;
    for line in stdout.lines() {
        if let Some(v) = value_after(line, "pts_time:") {
            at = Time::from_secs_f64(v).ok();
        } else if let Some(rest) = line.strip_prefix("lavfi.astats.Overall.RMS_level=") {
            if let Some(t) = at.take() {
                let db = rest.trim().parse::<f64>().unwrap_or(-100.0).max(-100.0);
                levels.push((t, db));
            }
        }
    }

    let scan = AudioScan { silences, levels };
    let _ = std::fs::create_dir_all(project_dir.join("cache"));
    if let Ok(json) = serde_json::to_string(&scan.silences) {
        let _ = std::fs::write(&sil_cache, json);
    }
    if let Ok(json) = serde_json::to_string(&scan.levels) {
        let _ = std::fs::write(&lvl_cache, json);
    }
    Ok(scan)
}

fn file_identity(path: &Path) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    if let Ok(m) = std::fs::metadata(path) {
        m.len().hash(&mut h);
        if let Ok(t) = m.modified() {
            if let Ok(d) = t.duration_since(std::time::UNIX_EPOCH) {
                d.as_secs().hash(&mut h);
            }
        }
    }
    h.finish()
}

/// RMS loudness (dBFS) per `window` seconds — a coarse audio map the AI (or
/// a waveform column) can reason about. Silence shows up near -100.
pub fn audio_levels(path: &Path, window: f64) -> Result<Vec<(Time, f64)>, AnalyzeError> {
    let samples_per_window = ((window * 8000.0).round() as u64).max(1);
    let out = Command::new("ffmpeg")
        .args(["-hide_banner", "-nostats", "-i"])
        .arg(path)
        .args([
            "-af",
            &format!(
                "aresample=8000,asetnsamples=n={samples_per_window},\
                 astats=metadata=1:reset=1,\
                 ametadata=print:key=lavfi.astats.Overall.RMS_level:file=-"
            ),
            "-f", "null", "-",
        ])
        .output()?;
    if !out.status.success() {
        return Err(AnalyzeError::Ffmpeg(
            path.display().to_string(),
            String::from_utf8_lossy(&out.stderr)
                .lines()
                .last()
                .unwrap_or("unknown error")
                .to_string(),
        ));
    }

    // ametadata prints pairs of lines:
    //   frame:0    pts:0      pts_time:0
    //   lavfi.astats.Overall.RMS_level=-23.47
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut levels = Vec::new();
    let mut at: Option<Time> = None;
    for line in stdout.lines() {
        if let Some(v) = value_after(line, "pts_time:") {
            at = Time::from_secs_f64(v).ok();
        } else if let Some(rest) = line.strip_prefix("lavfi.astats.Overall.RMS_level=") {
            if let Some(t) = at.take() {
                // "-inf" (digital silence) clamps to the practical floor.
                let db = rest.trim().parse::<f64>().unwrap_or(-100.0).max(-100.0);
                levels.push((t, db));
            }
        }
    }
    Ok(levels)
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
