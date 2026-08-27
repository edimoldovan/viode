//! Multicam sync: find the time offset between two recordings of the same
//! event by cross-correlating their audio envelopes. Coarse pass at 10 Hz
//! over the full lag window, refined at 100 Hz around the peak — fast even
//! on hours-long files, no FFT needed.

use std::path::Path;
use std::process::Command;

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("failed to run ffmpeg (is ffmpeg installed?): {0}")]
    Spawn(#[from] std::io::Error),
    #[error("audio extraction failed for {0}: {1}")]
    Ffmpeg(String, String),
    #[error("not enough audio to correlate (need a few seconds of overlap)")]
    TooShort,
}

/// RMS envelope of a file's audio at `hz` samples per second.
fn envelope(path: &Path, hz: u32) -> Result<Vec<f32>, SyncError> {
    let rate = 8000u32;
    let out = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(path)
        .args([
            "-map", "a:0", "-ac", "1",
            "-ar", &rate.to_string(),
            "-f", "f32le", "-",
        ])
        .output()?;
    if !out.status.success() {
        return Err(SyncError::Ffmpeg(
            path.display().to_string(),
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    let samples: Vec<f32> = out
        .stdout
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect();
    let win = (rate / hz).max(1) as usize;
    let env: Vec<f32> = samples
        .chunks(win)
        .map(|c| (c.iter().map(|s| s * s).sum::<f32>() / c.len() as f32).sqrt())
        .collect();
    Ok(env)
}

fn normalize(env: &mut [f32]) {
    let n = env.len() as f32;
    let mean = env.iter().sum::<f32>() / n;
    for v in env.iter_mut() {
        *v -= mean;
    }
    let norm = (env.iter().map(|v| v * v).sum::<f32>()).sqrt();
    if norm > 0.0 {
        for v in env.iter_mut() {
            *v /= norm;
        }
    }
}

/// Best lag (in envelope samples) of `b` relative to `a` within ±max_lag.
fn best_lag(a: &[f32], b: &[f32], max_lag: i64) -> i64 {
    let mut best = (f32::MIN, 0i64);
    for lag in -max_lag..=max_lag {
        let mut score = 0.0f32;
        for (i, bv) in b.iter().enumerate() {
            let j = i as i64 + lag;
            if j >= 0 && (j as usize) < a.len() {
                score += a[j as usize] * bv;
            }
        }
        if score > best.0 {
            best = (score, lag);
        }
    }
    best.1
}

/// Seconds that `b`'s audio starts AFTER `a`'s (negative: b started first).
/// `max_lag_secs` bounds the search window.
pub fn audio_offset(a: &Path, b: &Path, max_lag_secs: f64) -> Result<f64, SyncError> {
    const COARSE_HZ: u32 = 10;
    const FINE_HZ: u32 = 100;

    let (mut ea, mut eb) = (envelope(a, COARSE_HZ)?, envelope(b, COARSE_HZ)?);
    if ea.len() < 2 || eb.len() < 2 {
        return Err(SyncError::TooShort);
    }
    normalize(&mut ea);
    normalize(&mut eb);
    let coarse = best_lag(&ea, &eb, (max_lag_secs * COARSE_HZ as f64) as i64);

    let (mut fa, mut fb) = (envelope(a, FINE_HZ)?, envelope(b, FINE_HZ)?);
    normalize(&mut fa);
    normalize(&mut fb);
    let center = coarse * (FINE_HZ / COARSE_HZ) as i64;
    let window = 2 * FINE_HZ as i64; // ±2s around the coarse peak
    let mut best = (f32::MIN, center);
    for lag in (center - window)..=(center + window) {
        let mut score = 0.0f32;
        for (i, bv) in fb.iter().enumerate() {
            let j = i as i64 + lag;
            if j >= 0 && (j as usize) < fa.len() {
                score += fa[j as usize] * bv;
            }
        }
        if score > best.0 {
            best = (score, lag);
        }
    }
    Ok(best.1 as f64 / FINE_HZ as f64)
}
