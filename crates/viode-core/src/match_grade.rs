//! Shot match: make one clip's exposure and saturation sit where a
//! reference clip's does. Honest v1 — it matches the two statistics
//! videobalance can actually move (average luma via brightness, average
//! saturation via the saturation gain), measured with ffmpeg's
//! signalstats on each clip's middle frame. The result lands in the
//! clip's ordinary color grade: visible in the inspector, revertable,
//! and hand-tunable afterwards.

use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameStats {
    /// Average luma, 0..255.
    pub yavg: f64,
    /// Average chroma saturation, 0..~181.
    pub satavg: f64,
}

#[derive(Debug, thiserror::Error)]
pub enum MatchError {
    #[error("failed to run ffmpeg (is ffmpeg installed?): {0}")]
    Spawn(#[from] std::io::Error),
    #[error("could not measure {0}: {1}")]
    Stats(String, String),
    #[error("clip index out of range")]
    BadIndex,
}

/// Measure the frame at `at` seconds.
pub fn frame_stats(path: &Path, at: f64) -> Result<FrameStats, MatchError> {
    let out = Command::new("ffmpeg")
        .args(["-loglevel", "info", "-ss", &format!("{at}")])
        .arg("-i")
        .arg(path)
        .args(["-frames:v", "1", "-vf", "signalstats,metadata=print", "-f", "null", "-"])
        .output()?;
    let log = String::from_utf8_lossy(&out.stderr);
    let grab = |key: &str| -> Option<f64> {
        log.lines()
            .find_map(|l| l.split(key).nth(1))
            .and_then(|v| v.trim().parse().ok())
    };
    match (grab("signalstats.YAVG="), grab("signalstats.SATAVG=")) {
        (Some(yavg), Some(satavg)) => Ok(FrameStats { yavg, satavg }),
        _ => Err(MatchError::Stats(
            path.display().to_string(),
            "no signalstats in ffmpeg output".into(),
        )),
    }
}

/// The correction that moves `target` onto `reference`: a videobalance
/// brightness delta and a saturation gain. Pure.
pub fn plan(target: &FrameStats, reference: &FrameStats) -> (f64, f64) {
    let brightness = ((reference.yavg - target.yavg) / 255.0).clamp(-1.0, 1.0);
    // Near-grayscale frames have no meaningful saturation to scale.
    let saturation = if target.satavg < 1.0 {
        1.0
    } else {
        (reference.satavg / target.satavg).clamp(0.0, 2.0)
    };
    (brightness, saturation)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_moves_dark_toward_bright_and_scales_saturation() {
        let dark = FrameStats { yavg: 60.0, satavg: 40.0 };
        let bright = FrameStats { yavg: 162.0, satavg: 80.0 };
        let (b, s) = plan(&dark, &bright);
        assert!((b - 0.4).abs() < 0.01, "brightness {b}");
        assert!((s - 2.0).abs() < 0.01, "saturation {s}");
        // The reverse direction darkens and desaturates.
        let (b2, s2) = plan(&bright, &dark);
        assert!(b2 < -0.39 && s2 < 0.51, "{b2} {s2}");
    }

    #[test]
    fn grayscale_targets_keep_their_saturation_alone() {
        let gray = FrameStats { yavg: 100.0, satavg: 0.2 };
        let colorful = FrameStats { yavg: 100.0, satavg: 90.0 };
        let (_, s) = plan(&gray, &colorful);
        assert_eq!(s, 1.0);
    }
}
