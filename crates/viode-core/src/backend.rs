//! Render backends. The timeline model is authoritative; backends only
//! consume it. GES is the accurate path; ffmpeg smart-copy is the fast,
//! lossless path for cut-only projects (cuts snap to keyframes).

use std::path::{Path, PathBuf};
use std::process::Command;

use ges::prelude::*;
use gstreamer as gst;
use gstreamer_editing_services as ges;
use gstreamer_pbutils as gst_pbutils;

use crate::model::Project;

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("gstreamer error: {0}")]
    Gst(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("ffmpeg failed: {0}")]
    Ffmpeg(String),
}

impl From<gst::glib::Error> for RenderError {
    fn from(e: gst::glib::Error) -> Self {
        RenderError::Gst(e.to_string())
    }
}

impl From<gst::glib::BoolError> for RenderError {
    fn from(e: gst::glib::BoolError) -> Self {
        RenderError::Gst(e.to_string())
    }
}

impl From<gst::StateChangeError> for RenderError {
    fn from(e: gst::StateChangeError) -> Self {
        RenderError::Gst(e.to_string())
    }
}

pub trait RenderBackend {
    /// Render `project` (paths resolved relative to `project_dir`) to `output`.
    fn render(
        &self,
        project: &Project,
        project_dir: &Path,
        output: &Path,
    ) -> Result<(), RenderError>;
}

fn resolve(project_dir: &Path, src: &Path) -> PathBuf {
    let path = if src.is_absolute() {
        src.to_path_buf()
    } else {
        project_dir.join(src)
    };
    // GES URIs and ffmpeg concat lists both need absolute paths.
    path.canonicalize().unwrap_or(path)
}

fn absolutize(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|d| d.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    }
}

// ---------------------------------------------------------------------------
// GES backend: frame-accurate re-encode.

pub struct GesBackend;

impl RenderBackend for GesBackend {
    fn render(
        &self,
        project: &Project,
        project_dir: &Path,
        output: &Path,
    ) -> Result<(), RenderError> {
        ges::init()?;

        let timeline = ges::Timeline::new_audio_video();
        let layer = timeline.append_layer();

        let mut cursor = gst::ClockTime::ZERO;
        for clip in &project.clips {
            let path = resolve(project_dir, &clip.src);
            let uri = gst::glib::filename_to_uri(&path, None)?;
            let asset = ges::UriClipAsset::request_sync(&uri)?;
            let duration = clip.len().to_clocktime();
            layer.add_asset(
                &asset,
                cursor,
                clip.in_.to_clocktime(),
                duration,
                ges::TrackType::UNKNOWN,
            )?;
            cursor += duration;
        }
        timeline.commit();

        let video_profile = gst_pbutils::EncodingVideoProfile::builder(
            &gst::Caps::builder("video/x-h264").build(),
        )
        .build();
        let audio_profile = gst_pbutils::EncodingAudioProfile::builder(
            &gst::Caps::builder("audio/mpeg")
                .field("mpegversion", 4i32)
                .build(),
        )
        .build();
        let profile = gst_pbutils::EncodingContainerProfile::builder(
            &gst::Caps::builder("video/quicktime")
                .field("variant", "iso")
                .build(),
        )
        .add_profile(video_profile)
        .add_profile(audio_profile)
        .build();

        let output = absolutize(output);
        if let Some(dir) = output.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let out_uri = gst::glib::filename_to_uri(&output, None)?;

        let pipeline = ges::Pipeline::new();
        pipeline.set_timeline(&timeline)?;
        pipeline.set_render_settings(&out_uri, &profile)?;
        pipeline.set_mode(ges::PipelineFlags::RENDER)?;
        pipeline.set_state(gst::State::Playing)?;

        let bus = pipeline
            .bus()
            .ok_or_else(|| RenderError::Gst("pipeline has no bus".into()))?;
        let mut result = Ok(());
        for msg in bus.iter_timed(gst::ClockTime::NONE) {
            match msg.view() {
                gst::MessageView::Eos(..) => break,
                gst::MessageView::Error(err) => {
                    result = Err(RenderError::Gst(format!(
                        "{} ({:?})",
                        err.error(),
                        err.debug()
                    )));
                    break;
                }
                _ => {}
            }
        }
        pipeline.set_state(gst::State::Null)?;
        result
    }
}

// ---------------------------------------------------------------------------
// Smart-copy backend: lossless, near-instant, keyframe-snapped cuts.

pub struct SmartCopyBackend;

impl RenderBackend for SmartCopyBackend {
    fn render(
        &self,
        project: &Project,
        project_dir: &Path,
        output: &Path,
    ) -> Result<(), RenderError> {
        let tmp = absolutize(&project_dir.join("cache").join("smartcopy"));
        std::fs::create_dir_all(&tmp)?;
        if let Some(dir) = output.parent() {
            std::fs::create_dir_all(dir)?;
        }

        let mut list = String::new();
        for (i, clip) in project.clips.iter().enumerate() {
            let src = resolve(project_dir, &clip.src);
            let part = tmp.join(format!("part_{i:04}.mp4"));
            // -ss before -i: fast keyframe seek. With -c copy the cut lands on
            // the nearest preceding keyframe — lossless but not frame-accurate.
            run_ffmpeg(&[
                "-y".into(),
                "-loglevel".into(),
                "error".into(),
                "-ss".into(),
                format!("{}", clip.in_.as_secs_f64()),
                "-i".into(),
                src.display().to_string(),
                "-t".into(),
                format!("{}", clip.len().as_secs_f64()),
                "-c".into(),
                "copy".into(),
                "-avoid_negative_ts".into(),
                "make_zero".into(),
                part.display().to_string(),
            ])?;
            list.push_str(&format!("file '{}'\n", part.display()));
        }

        let list_path = tmp.join("concat.txt");
        std::fs::write(&list_path, list)?;
        run_ffmpeg(&[
            "-y".into(),
            "-loglevel".into(),
            "error".into(),
            "-f".into(),
            "concat".into(),
            "-safe".into(),
            "0".into(),
            "-i".into(),
            list_path.display().to_string(),
            "-c".into(),
            "copy".into(),
            output.display().to_string(),
        ])?;
        Ok(())
    }
}

fn run_ffmpeg(args: &[String]) -> Result<(), RenderError> {
    let out = Command::new("ffmpeg").args(args).output()?;
    if !out.status.success() {
        return Err(RenderError::Ffmpeg(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    Ok(())
}
