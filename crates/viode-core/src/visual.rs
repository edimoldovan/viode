//! Visual artifacts via ffmpeg: audio waveform images and clip contact
//! sheets (tiled filmstrips). Both are "senses" — a human reads them in the
//! TUI later; an AI reads them over MCP today.

use std::path::Path;
use std::process::Command;

use crate::time::Time;

#[derive(Debug, thiserror::Error)]
pub enum VisualError {
    #[error("failed to run ffmpeg (is ffmpeg installed?): {0}")]
    Spawn(#[from] std::io::Error),
    #[error("ffmpeg failed on {0}: {1}")]
    Ffmpeg(String, String),
}

fn run(path: &Path, out_path: &Path, args: Vec<String>) -> Result<(), VisualError> {
    if let Some(dir) = out_path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let out = Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error"])
        .args(args)
        .arg(out_path)
        .output()?;
    if !out.status.success() {
        return Err(VisualError::Ffmpeg(
            path.display().to_string(),
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    Ok(())
}

/// Render the waveform of `path`'s audio (within `in_..out`) to a PNG.
pub fn waveform_png(
    path: &Path,
    in_: Time,
    out: Time,
    dest: &Path,
    width: u32,
    height: u32,
    color: &str,
) -> Result<(), VisualError> {
    run(
        path,
        dest,
        vec![
            "-ss".into(), in_.as_secs_f64().to_string(),
            "-to".into(), out.as_secs_f64().to_string(),
            "-i".into(), path.display().to_string(),
            "-filter_complex".into(),
            format!("showwavespic=s={width}x{height}:colors={color}"),
            "-frames:v".into(), "1".into(),
        ],
    )
}

/// Contact sheet of `path` (within `in_..out`): one frame every `interval`
/// seconds, tiled `cols` wide, each tile `tile_width` px. Returns rows used.
pub fn contact_sheet_png(
    path: &Path,
    in_: Time,
    out: Time,
    dest: &Path,
    interval: f64,
    cols: u32,
    tile_width: u32,
) -> Result<u32, VisualError> {
    let len = (out - in_).as_secs_f64();
    let frames = (len / interval).ceil().max(1.0) as u32;
    let rows = frames.div_ceil(cols);
    run(
        path,
        dest,
        vec![
            "-ss".into(), in_.as_secs_f64().to_string(),
            "-to".into(), out.as_secs_f64().to_string(),
            "-i".into(), path.display().to_string(),
            "-vf".into(),
            format!("fps=1/{interval},scale={tile_width}:-2,tile={cols}x{rows}"),
            "-frames:v".into(), "1".into(),
        ],
    )?;
    Ok(rows)
}

/// Snapshot a video scope of the frame at `at`: "waveform" (luma levels)
/// or "vector"/"vectorscope" (chroma distribution) — the QC senses every
/// client shares (CLI `viode scope`, MCP `scope`, the GUI overlay).
pub fn scope_png(
    src: &Path,
    at: Time,
    kind: &str,
    dest: &Path,
) -> Result<(), VisualError> {
    let filter = match kind {
        "waveform" => "waveform=graticule=green:intensity=0.1",
        "vector" | "vectorscope" => "vectorscope=graticule=green",
        other => {
            return Err(VisualError::Ffmpeg(
                src.display().to_string(),
                format!("unknown scope {other:?} (waveform, vector)"),
            ))
        }
    };
    run(
        src,
        dest,
        vec![
            "-ss".into(),
            at.as_secs_f64().to_string(),
            "-i".into(),
            src.display().to_string(),
            "-frames:v".into(),
            "1".into(),
            "-vf".into(),
            filter.into(),
        ],
    )
}
