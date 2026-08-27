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

fn track_type(kind: crate::model::TrackKind) -> ges::TrackType {
    match kind {
        crate::model::TrackKind::Av => ges::TrackType::UNKNOWN,
        crate::model::TrackKind::Video => ges::TrackType::VIDEO,
        crate::model::TrackKind::Audio => ges::TrackType::AUDIO,
    }
}

fn add_media_clip(
    layer: &ges::Layer,
    project_dir: &Path,
    clip: &crate::model::Clip,
    start: gst::ClockTime,
    types: ges::TrackType,
) -> Result<(), RenderError> {
    let path = resolve(project_dir, &clip.src);
    let uri = gst::glib::filename_to_uri(&path, None)?;
    let asset = ges::UriClipAsset::request_sync(&uri)?;
    let ges_clip = layer.add_asset(
        &asset,
        start,
        clip.in_.to_clocktime(),
        clip.len().to_clocktime(),
        types,
    )?;
    for desc in &clip.effects {
        let effect = ges::Effect::new(desc)
            .map_err(|e| RenderError::Gst(format!("bad effect {desc:?}: {e}")))?;
        ges_clip
            .add(&effect)
            .map_err(|e| RenderError::Gst(format!("could not add effect {desc:?}: {e}")))?;
    }
    Ok(())
}

impl RenderBackend for GesBackend {
    fn render(
        &self,
        project: &Project,
        project_dir: &Path,
        output: &Path,
    ) -> Result<(), RenderError> {
        ges::init()?;

        let timeline = ges::Timeline::new_audio_video();
        timeline.set_auto_transition(true); // overlaps crossfade automatically

        // GES layer priority: first appended = topmost. Titles, then
        // overlay tracks (later file order on top), main sequence at the
        // bottom.
        if !project.titles.is_empty() {
            let title_layer = timeline.append_layer();
            for title in &project.titles {
                let t = ges::TitleClip::new()
                    .ok_or_else(|| RenderError::Gst("could not create title clip".into()))?;
                t.set_start(title.at.to_clocktime());
                t.set_duration(title.dur.to_clocktime());
                title_layer
                    .add_clip(&t)
                    .map_err(|e| RenderError::Gst(format!("title add: {e}")))?;
                t.set_child_property("text", &title.text)
                    .map_err(|e| RenderError::Gst(format!("title text: {e}")))?;
                if let Some(font) = &title.font {
                    t.set_child_property("font-desc", font)
                        .map_err(|e| RenderError::Gst(format!("title font: {e}")))?;
                }
            }
        }

        for track in project.tracks.iter().skip(1).rev() {
            if !track.enabled || track.clips.is_empty() {
                continue;
            }
            let layer = timeline.append_layer();
            for clip in &track.clips {
                let start = clip.at.unwrap_or(crate::Time::ZERO).to_clocktime();
                add_media_clip(&layer, project_dir, clip, start, track_type(track.kind))?;
            }
        }

        let main = project.main();
        let layer = timeline.append_layer();
        let positions = main.positions();
        for (clip, start) in main.clips.iter().zip(&positions) {
            add_media_clip(
                &layer,
                project_dir,
                clip,
                start.to_clocktime(),
                track_type(main.kind),
            )?;
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
        // Stream-copy can only concatenate — no compositing.
        let simple = project.tracks.len() == 1
            && project.titles.is_empty()
            && project
                .main()
                .clips
                .iter()
                .all(|c| c.transition.is_none() && c.effects.is_empty());
        if !simple {
            return Err(RenderError::Gst(
                "smart-copy only supports single-track cut-only projects \
                 (no overlays, titles, transitions, or effects) — use a plain render"
                    .into(),
            ));
        }
        let tmp = absolutize(&project_dir.join("cache").join("smartcopy"));
        std::fs::create_dir_all(&tmp)?;
        if let Some(dir) = output.parent() {
            std::fs::create_dir_all(dir)?;
        }

        let mut list = String::new();
        for (i, clip) in project.main().clips.iter().enumerate() {
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
