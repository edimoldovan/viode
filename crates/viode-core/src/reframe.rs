//! Subject-aware reframing for vertical exports (our answer to Premiere's
//! Auto Reframe, under our own name). The Shorts preset center-crops;
//! `reframe` moves that crop to the subject, scene by scene:
//!
//! 1. Scene-detect the rendered master (existing detector).
//! 2. Sample the middle frame of each scene, grayscale, and find faces
//!    with rustface (SeetaFace — pure Rust, no native libraries, so it
//!    ships identically on Linux, macOS, and Windows later).
//! 3. Emit one crop x per scene and apply them in a single ffmpeg pass
//!    via sendcmd + crop, finishing with the preset's loudness pass.
//!
//! Scenes with no detectable face inherit the previous scene's framing
//! (a talking head that briefly turns away should not yank the crop),
//! and fall back to center. The model downloads once, like whisper's.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::time::Time;

pub const MODEL_URL: &str =
    "https://github.com/atomashpolskiy/rustface/raw/master/model/seeta_fd_frontal_v1.0.bin";

#[derive(Debug, thiserror::Error)]
pub enum ReframeError {
    #[error(
        "reframe needs the face model — download it with:\n  curl -L -o \
         ~/.local/share/viode/models/seeta_fd_frontal_v1.0.bin {MODEL_URL}\n\
         (or set VIODE_FACE_MODEL to a SeetaFace detection model)"
    )]
    NoModel,
    #[error("face detector failed to load: {0}")]
    BadModel(String),
    #[error("failed to run ffmpeg (is ffmpeg installed?): {0}")]
    Spawn(#[from] std::io::Error),
    #[error("reframe failed: {0}")]
    Ffmpeg(String),
    #[error(transparent)]
    Analyze(#[from] crate::audio::AnalyzeError),
    #[error(transparent)]
    Probe(#[from] crate::probe::ProbeError),
    #[error(transparent)]
    Export(#[from] crate::export::ExportError),
}

/// Where the face model lives: `VIODE_FACE_MODEL`, else the shared
/// user-writable model directory (same one the whisper model uses).
pub fn model_path() -> PathBuf {
    if let Ok(p) = std::env::var("VIODE_FACE_MODEL") {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME").unwrap_or_default();
    Path::new(&home).join(".local/share/viode/models/seeta_fd_frontal_v1.0.bin")
}

pub fn model_present() -> bool {
    model_path().exists()
}

/// Detection resolution: small frames keep detection fast (a 3-minute
/// Short analyzes in seconds) without hurting face-scale accuracy.
const DETECT_W: u32 = 480;
const DETECT_H: u32 = 270;

/// The horizontal center (0..1) of the dominant face in one frame, if any.
fn face_center_x(detector: &mut dyn rustface::Detector, gray: &[u8]) -> Option<f64> {
    let mut image = rustface::ImageData::new(gray, DETECT_W, DETECT_H);
    let faces = detector.detect(&mut image);
    faces
        .iter()
        .max_by_key(|f| f.bbox().width() * f.bbox().height())
        .map(|f| (f.bbox().x() as f64 + f.bbox().width() as f64 / 2.0) / DETECT_W as f64)
}

/// Grab one grayscale frame at `at` seconds.
fn grab_gray(master: &Path, at: f64) -> Result<Vec<u8>, ReframeError> {
    let out = Command::new("ffmpeg")
        .args(["-loglevel", "error", "-ss", &format!("{at}")])
        .arg("-i")
        .arg(master)
        .args(["-frames:v", "1"])
        .args(["-vf", &format!("scale={DETECT_W}:{DETECT_H},format=gray")])
        .args(["-f", "rawvideo", "-"])
        .output()?;
    if !out.status.success() || out.stdout.len() < (DETECT_W * DETECT_H) as usize {
        return Err(ReframeError::Ffmpeg(format!(
            "could not sample a frame at {at}s: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(out.stdout)
}

/// One scene's framing: start time (seconds) and subject center (0..1).
#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    pub start: f64,
    pub center_x: f64,
}

/// Scene-detect the master and find the subject per scene.
pub fn analyze(master: &Path) -> Result<Vec<Span>, ReframeError> {
    if !model_present() {
        return Err(ReframeError::NoModel);
    }
    let mut detector = rustface::create_detector(model_path().to_string_lossy().as_ref())
        .map_err(|e| ReframeError::BadModel(e.to_string()))?;
    detector.set_min_face_size(20);
    detector.set_score_thresh(2.0);
    detector.set_pyramid_scale_factor(0.8);
    detector.set_slide_window_step(4, 4);

    let duration = crate::probe::probe(master)?.duration.0 as f64 / 1e9;
    let mut cuts: Vec<f64> = vec![0.0];
    cuts.extend(
        crate::audio::detect_scenes(master, 0.4)?
            .iter()
            .map(|t: &Time| t.0 as f64 / 1e9),
    );
    cuts.push(duration);

    let mut spans = Vec::new();
    let mut last_x = 0.5;
    for w in cuts.windows(2) {
        let (start, end) = (w[0], w[1]);
        if end - start < 0.05 {
            continue;
        }
        let mid = (start + end) / 2.0;
        let x = grab_gray(master, mid)
            .ok()
            .and_then(|gray| face_center_x(detector.as_mut(), &gray))
            .unwrap_or(last_x); // no face: hold the previous framing
        last_x = x;
        spans.push(Span { start, center_x: x });
    }
    if spans.is_empty() {
        spans.push(Span { start: 0.0, center_x: 0.5 });
    }
    Ok(spans)
}

/// Left edge of a 9:16 crop centered on `center_x`, clamped into frame.
/// Pure — this is the math the ffmpeg pass executes.
pub fn crop_x(width: u32, height: u32, center_x: f64) -> u32 {
    let crop_w = (height as f64 * 9.0 / 16.0).round();
    let x = center_x * width as f64 - crop_w / 2.0;
    x.clamp(0.0, (width as f64 - crop_w).max(0.0)).round() as u32
}

/// The Shorts export with the crop following the subject: one ffmpeg
/// pass driven by a sendcmd schedule, plus the preset's loudness pass.
pub fn shorts_reframed(master: &Path, output: &Path) -> Result<Vec<Span>, ReframeError> {
    let spans = analyze(master)?;
    let info = crate::probe::probe(master)?;
    let (w, h) = (info.width.unwrap_or(1920), info.height.unwrap_or(1080));

    // sendcmd schedule: absolute second -> crop x. The first span seeds
    // the crop's initial x directly, so there is no pre-command flash.
    let mut cmds = String::new();
    for s in &spans {
        cmds.push_str(&format!("{:.3} crop@c x {};\n", s.start, crop_x(w, h, s.center_x)));
    }
    let cmd_file = std::env::temp_dir().join(format!("viode-reframe-{}.cmd", std::process::id()));
    let mut f = std::fs::File::create(&cmd_file)?;
    f.write_all(cmds.as_bytes())?;

    let first_x = crop_x(w, h, spans[0].center_x);
    let loudnorm = crate::export::loudnorm_filter(master, -14.0)?;
    if let Some(dir) = output.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let filter = format!(
        "sendcmd=f={},crop@c=w=ih*9/16:h=ih:x={first_x}:y=0,scale=1080:1920",
        cmd_file.display()
    );
    let out = Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error", "-i"])
        .arg(master)
        .args(["-vf", &filter])
        .args(["-c:v", "libx264", "-crf", "20", "-preset", "medium"])
        .args(["-af", &loudnorm, "-c:a", "aac", "-b:a", "256k"])
        .arg(output)
        .output()?;
    let _ = std::fs::remove_file(&cmd_file);
    if !out.status.success() {
        return Err(ReframeError::Ffmpeg(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    Ok(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crop_math_centers_and_clamps() {
        // 1920x1080 master: crop is 608 wide (1080 * 9/16 = 607.5 -> 608).
        assert_eq!(crop_x(1920, 1080, 0.5), 656); // centered
        assert_eq!(crop_x(1920, 1080, 0.0), 0); // clamped left
        assert_eq!(crop_x(1920, 1080, 1.0), 1312); // clamped right (1920-608)
        // A master narrower than the crop degenerates to 0 safely.
        assert_eq!(crop_x(200, 1080, 0.5), 0);
    }

    #[test]
    fn missing_model_error_carries_the_download_command() {
        if model_present() {
            eprintln!("SKIP: face model installed on this machine");
            return;
        }
        let err = analyze(Path::new("/nonexistent.mp4")).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("curl -L -o") && msg.contains("seeta_fd_frontal_v1.0.bin"));
    }
}
