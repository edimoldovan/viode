//! Region masks: blur or pixelate a rectangle of the picture — a face,
//! a screen, a license plate — optionally FOLLOWING the content as it
//! moves. Baked whole-source like the other heavy passes (steady, clean,
//! LUT), so source time survives and the result is cached.
//!
//! The follow tracker is deliberately boring: normalized sum-of-absolute
//! differences template matching on small grayscale frames, streamed out
//! of ffmpeg so a long file never sits in memory. The measured positions
//! drive the crop and overlay filters through one sendcmd schedule — the
//! same mechanism the subject reframe uses.

use std::io::Read;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::model::Mask;

/// Tracking resolution and rate: small and sparse is plenty for a mask
/// that follows a face or a screen.
const TRACK_W: usize = 320;
const TRACK_H: usize = 180;
const TRACK_FPS: f64 = 4.0;
/// Search radius around the last known position, in tracking pixels.
const SEARCH: isize = 24;

#[derive(Debug, thiserror::Error)]
pub enum MaskError {
    #[error("failed to run ffmpeg (is ffmpeg installed?): {0}")]
    Spawn(#[from] std::io::Error),
    #[error("mask bake failed for {0}: {1}")]
    Ffmpeg(String, String),
    #[error("bad mask region: {0}")]
    BadRegion(String),
}

pub fn validate(mask: &Mask) -> Result<(), MaskError> {
    let [x, y, w, h] = mask.region;
    if !(0.0..1.0).contains(&x)
        || !(0.0..1.0).contains(&y)
        || w <= 0.01
        || h <= 0.01
        || x + w > 1.0
        || y + h > 1.0
    {
        return Err(MaskError::BadRegion(format!(
            "[{x}, {y}, {w}, {h}] — fractions, region must sit inside the frame"
        )));
    }
    if !["blur", "pixelate"].contains(&mask.kind.as_str()) {
        return Err(MaskError::BadRegion(format!(
            "kind {:?} (blur, pixelate)",
            mask.kind
        )));
    }
    Ok(())
}

/// One tracking step, pure: find the template's best position in `frame`
/// near `guess` (top-left, tracking pixels). Returns the new top-left.
pub fn track_step(
    frame: &[u8],
    template: &[u8],
    tw: usize,
    th: usize,
    guess: (isize, isize),
) -> (isize, isize) {
    let mut best = guess;
    let mut best_score = u64::MAX;
    let (gx, gy) = guess;
    for dy in -SEARCH..=SEARCH {
        for dx in -SEARCH..=SEARCH {
            let x = gx + dx;
            let y = gy + dy;
            if x < 0 || y < 0 || x as usize + tw > TRACK_W || y as usize + th > TRACK_H {
                continue;
            }
            let mut score: u64 = 0;
            // Coarse SAD on every other row is 2x faster and just as stable.
            for ty in (0..th).step_by(2) {
                let frow = (y as usize + ty) * TRACK_W + x as usize;
                let trow = ty * tw;
                for tx in 0..tw {
                    let f = frame[frow + tx] as i64;
                    let t = template[trow + tx] as i64;
                    score += f.abs_diff(t);
                }
                if score >= best_score {
                    break;
                }
            }
            if score < best_score {
                best_score = score;
                best = (x, y);
            }
        }
    }
    best
}

fn cache_key(src: &Path, mask: &Mask) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    src.hash(&mut h);
    format!("{mask:?}").hash(&mut h);
    std::fs::metadata(src)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
        .hash(&mut h);
    h.finish()
}

pub fn baked_path(project_dir: &Path, src: &Path, mask: &Mask) -> PathBuf {
    let stem = src
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "clip".into());
    project_dir
        .join("cache/mask")
        .join(format!("{stem}-{:016x}.mkv", cache_key(src, mask)))
}

/// Track the region through the whole file, streaming frames out of
/// ffmpeg. Returns (time in seconds, top-left in tracking pixels).
fn track_positions(src: &Path, region_px: (isize, isize, usize, usize)) -> Result<Vec<(f64, (isize, isize))>, MaskError> {
    let (rx, ry, rw, rh) = region_px;
    let mut child = Command::new("ffmpeg")
        .args(["-loglevel", "error", "-i"])
        .arg(src)
        .args(["-vf", &format!("fps={TRACK_FPS},scale={TRACK_W}:{TRACK_H},format=gray")])
        .args(["-f", "rawvideo", "-"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut stdout = child.stdout.take().unwrap();
    let frame_len = TRACK_W * TRACK_H;
    let mut frame = vec![0u8; frame_len];
    let mut template: Option<Vec<u8>> = None;
    let mut pos = (rx, ry);
    let mut positions = Vec::new();
    let mut i = 0usize;
    loop {
        let mut filled = 0;
        while filled < frame_len {
            match stdout.read(&mut frame[filled..]) {
                Ok(0) => break,
                Ok(n) => filled += n,
                Err(e) => return Err(MaskError::Spawn(e)),
            }
        }
        if filled < frame_len {
            break;
        }
        match &template {
            None => {
                // First frame: the region's content IS the template.
                let mut t = vec![0u8; rw * rh];
                for ty in 0..rh {
                    let frow = (ry as usize + ty) * TRACK_W + rx as usize;
                    t[ty * rw..(ty + 1) * rw].copy_from_slice(&frame[frow..frow + rw]);
                }
                template = Some(t);
            }
            Some(t) => {
                pos = track_step(&frame, t, rw, rh, pos);
            }
        }
        positions.push((i as f64 / TRACK_FPS, pos));
        i += 1;
    }
    let _ = child.wait();
    Ok(positions)
}

/// Bake the mask onto the whole source unless cached. Absolute path out.
pub fn ensure_baked(project_dir: &Path, src: &Path, mask: &Mask) -> Result<PathBuf, MaskError> {
    validate(mask)?;
    let dest = baked_path(project_dir, src, mask);
    if dest.exists() {
        return Ok(dest.canonicalize().unwrap_or(dest));
    }
    std::fs::create_dir_all(dest.parent().unwrap())?;

    let info = crate::probe::probe(src)
        .map_err(|e| MaskError::Ffmpeg(src.display().to_string(), e.to_string()))?;
    let (w, h) = (info.width.unwrap_or(1920) as f64, info.height.unwrap_or(1080) as f64);
    let [fx, fy, fw, fh] = mask.region;
    // Even-aligned pixel rectangle (yuv420 wants even everything).
    let even = |v: f64| ((v as usize) / 2) * 2;
    let (px, py) = (even(fx * w), even(fy * h));
    let (pw, ph) = (even(fw * w).max(16), even(fh * h).max(16));

    let effect = match mask.kind.as_str() {
        // Down with area averaging (real block colors), up with neighbor
        // (hard block edges) — the classic pixelation.
        "pixelate" => format!(
            "scale={}:{}:flags=area,scale={pw}:{ph}:flags=neighbor",
            (pw / 12).max(2),
            (ph / 12).max(2)
        ),
        _ => "gblur=sigma=24".to_string(),
    };

    // The follow schedule moves crop and overlay together via sendcmd.
    let mut sendcmd = String::new();
    let cmd_file = std::env::temp_dir().join(format!("viode-mask-{}.cmd", std::process::id()));
    if mask.follow {
        let region_px = (
            (fx * TRACK_W as f64) as isize,
            (fy * TRACK_H as f64) as isize,
            ((fw * TRACK_W as f64) as usize).clamp(4, TRACK_W - 2),
            ((fh * TRACK_H as f64) as usize).clamp(4, TRACK_H - 2),
        );
        let sx = w / TRACK_W as f64;
        let sy = h / TRACK_H as f64;
        let mut cmds = String::new();
        for (t, (tx, ty)) in track_positions(src, region_px)? {
            let ox = even((tx.max(0) as f64 * sx).min(w - pw as f64));
            let oy = even((ty.max(0) as f64 * sy).min(h - ph as f64));
            cmds.push_str(&format!(
                "{t:.3} crop@c x {ox};\n{t:.3} crop@c y {oy};\n{t:.3} overlay@o x {ox};\n{t:.3} overlay@o y {oy};\n"
            ));
        }
        let mut f = std::fs::File::create(&cmd_file)?;
        f.write_all(cmds.as_bytes())?;
        sendcmd = format!("sendcmd=f={},", cmd_file.display());
    }

    let filter = format!(
        "[0:v]{sendcmd}split[base][top];\
         [top]crop@c=w={pw}:h={ph}:x={px}:y={py},{effect}[fg];\
         [base][fg]overlay@o=x={px}:y={py}[v]"
    );
    let tmp = dest.with_extension("part.mkv");
    let out = Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error", "-i"])
        .arg(src)
        .args(["-filter_complex", &filter])
        .args(["-map", "[v]", "-map", "0:a?"])
        .args(["-c:v", "libx264", "-crf", "10", "-preset", "veryfast"])
        .args(["-c:a", "copy"])
        .arg(&tmp)
        .output()?;
    if mask.follow {
        let _ = std::fs::remove_file(&cmd_file);
    }
    if !out.status.success() {
        let _ = std::fs::remove_file(&tmp);
        return Err(MaskError::Ffmpeg(
            src.display().to_string(),
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    std::fs::rename(&tmp, &dest)?;
    Ok(dest.canonicalize().unwrap_or(dest))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame_with_square(x: usize, y: usize) -> Vec<u8> {
        let mut f = vec![0u8; TRACK_W * TRACK_H];
        for dy in 0..12 {
            for dx in 0..12 {
                f[(y + dy) * TRACK_W + x + dx] = 255;
            }
        }
        f
    }

    #[test]
    fn the_tracker_follows_a_moving_square() {
        let first = frame_with_square(40, 30);
        // Template = the square's region in the first frame.
        let mut template = vec![0u8; 12 * 12];
        for ty in 0..12 {
            let row = (30 + ty) * TRACK_W + 40;
            template[ty * 12..(ty + 1) * 12].copy_from_slice(&first[row..row + 12]);
        }
        let mut pos = (40isize, 30isize);
        for step in 1..=6 {
            let moved = frame_with_square(40 + step * 3, 30 + step * 2);
            pos = track_step(&moved, &template, 12, 12, pos);
            assert_eq!(
                pos,
                (40 + step as isize * 3, 30 + step as isize * 2),
                "lost the square at step {step}"
            );
        }
    }

    #[test]
    fn bad_regions_and_kinds_refuse() {
        let mut m = Mask { region: [0.8, 0.8, 0.4, 0.1], kind: "blur".into(), follow: false };
        assert!(validate(&m).is_err(), "region past the right edge");
        m.region = [0.1, 0.1, 0.2, 0.2];
        assert!(validate(&m).is_ok());
        m.kind = "vaseline".into();
        assert!(validate(&m).is_err());
    }
}
