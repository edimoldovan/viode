//! Render backends. The timeline model is authoritative; backends only
//! consume it. GES is the accurate path; ffmpeg smart-copy is the fast,
//! lossless path for cut-only projects (cuts snap to keyframes).

use std::path::{Path, PathBuf};
use std::process::Command;

use ges::prelude::*;
use gst_controller::prelude::*;
use gstreamer as gst;
use gstreamer_controller as gst_controller;
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

fn add_effect(ges_clip: &ges::Clip, desc: &str) -> Result<(), RenderError> {
    let effect = ges::Effect::new(desc)
        .map_err(|e| RenderError::Gst(format!("bad effect {desc:?}: {e}")))?;
    ges_clip
        .add(&effect)
        .map_err(|e| RenderError::Gst(format!("could not add effect {desc:?}: {e}")))?;
    Ok(())
}

fn add_media_clip(
    layer: &ges::Layer,
    project_dir: &Path,
    clip: &crate::model::Clip,
    start: gst::ClockTime,
    types: ges::TrackType,
    project_res: [u32; 2],
) -> Result<(), RenderError> {
    let path = resolve(project_dir, &clip.src);
    // A LUT'd clip plays from its ffmpeg bake (frame-identical timeline,
    // colors applied) instead of the original — see lut.rs for why.
    let path = if let Some(lut) = &clip.lut {
        let lut_path = resolve(project_dir, lut);
        crate::lut::ensure_baked(project_dir, &path, &lut_path)
            .map_err(|e| RenderError::Gst(e.to_string()))?
    } else {
        path
    };
    let uri = gst::glib::filename_to_uri(&path, None)?;
    let asset = ges::UriClipAsset::request_sync(&uri)?;
    let ges_clip = layer.add_asset(
        &asset,
        start,
        clip.in_.to_clocktime(),
        clip.len().to_clocktime(),
        types,
    )?;
    // Speed: videorate/pitch are GES time effects — they change how much
    // source the (already rate-scaled) timeline duration consumes.
    if let Some(r) = clip.rate.filter(|r| *r > 0.0 && *r != 1.0) {
        // Homebrew's GStreamer ships without soundtouch, so name the fix
        // instead of failing with a generic "bad effect".
        if gst::ElementFactory::find("pitch").is_none() {
            return Err(RenderError::Gst(
                "speed changes need the GStreamer 'pitch' element (soundtouch plugin, \
                 part of gst-plugins-bad), which is not installed on this machine"
                    .into(),
            ));
        }
        add_effect(&ges_clip, &format!("videorate rate={r}"))?;
        add_effect(&ges_clip, &format!("pitch tempo={r}"))?;
    }
    // Transform: position/scale via the frame positioner, opacity via alpha.
    if clip.pos.is_some() || clip.scale.is_some() {
        let scale = clip.scale.unwrap_or(1.0).clamp(0.01, 4.0);
        let [px, py] = clip.pos.unwrap_or([0.0, 0.0]);
        let (w, h) = (
            (project_res[0] as f64 * scale) as i32,
            (project_res[1] as f64 * scale) as i32,
        );
        for (prop, v) in [
            ("width", w),
            ("height", h),
            ("posx", (project_res[0] as f64 * px) as i32),
            ("posy", (project_res[1] as f64 * py) as i32),
        ] {
            ges_clip
                .set_child_property(prop, &v)
                .map_err(|e| RenderError::Gst(format!("transform {prop}: {e}")))?;
        }
    }
    if let Some(a) = clip.opacity {
        ges_clip
            .set_child_property("alpha", &a.clamp(0.0, 1.0))
            .map_err(|e| RenderError::Gst(format!("opacity: {e}")))?;
    }
    if let Some(deg) = clip.rotate {
        add_effect(&ges_clip, &format!("rotate angle={}", deg.to_radians()))?;
    }
    if let Some(grade) = &clip.color {
        add_effect(
            &ges_clip,
            &format!(
                "videobalance brightness={} contrast={} saturation={} hue={}",
                grade.brightness.unwrap_or(0.0),
                grade.contrast.unwrap_or(1.0),
                grade.saturation.unwrap_or(1.0),
                grade.hue.unwrap_or(0.0),
            ),
        )?;
    }
    for desc in &clip.effects {
        let effect = ges::Effect::new(desc)
            .map_err(|e| RenderError::Gst(format!("bad effect {desc:?}: {e}")))?;
        ges_clip
            .add(&effect)
            .map_err(|e| RenderError::Gst(format!("could not add effect {desc:?}: {e}")))?;
    }
    if let Some(v) = clip.volume {
        ges_clip
            .set_child_property("volume", &v)
            .map_err(|e| RenderError::Gst(format!("volume: {e}")))?;
    }
    if let Some(pan) = clip.pan {
        let effect = ges::Effect::new(&format!("audiopanorama panorama={pan}"))
            .map_err(|e| RenderError::Gst(format!("pan: {e}")))?;
        ges_clip
            .add(&effect)
            .map_err(|e| RenderError::Gst(format!("pan: {e}")))?;
    }
    if !clip.keys.is_empty() {
        let mut props: Vec<&str> = clip.keys.iter().map(|k| k.prop.as_str()).collect();
        props.sort();
        props.dedup();
        for prop in props {
            let cs = gst_controller::InterpolationControlSource::new();
            cs.set_mode(gst_controller::InterpolationMode::Linear);
            for k in clip.keys.iter().filter(|k| k.prop == prop) {
                // Keyframe timestamps are SOURCE time — the coordinates
                // control bindings evaluate in.
                cs.set(k.at.to_clocktime(), k.value);
            }
            // Bindings attach to the track elements INSIDE the clip (the
            // audio source owns "volume", the video source owns "alpha").
            let bound = ges_clip
                .children(true)
                .iter()
                .filter_map(|c| c.downcast_ref::<ges::TrackElement>())
                .any(|te| te.set_control_source(&cs, prop, "direct"));
            if !bound {
                return Err(RenderError::Gst(format!(
                    "could not bind keyframes to {prop:?} (valid: volume, alpha)"
                )));
            }
        }
    }
    Ok(())
}

/// Transition kinds Viode advertises across every interface (CLI docs,
/// MCP schema, GUI dropdown, error hints). Any GES
/// VideoStandardTransitionType nick works in the project file; these are
/// the curated ones — and a unit test proves each really exists in
/// GStreamer, so the list can never drift from the engine again.
pub const TRANSITION_KINDS: &[&str] = &[
    "crossfade",
    "bar-wipe-lr",
    "bar-wipe-tb",
    "box-wipe-tl",
    "iris-rect",
    "clock-cw12",
];

/// Build the full GES timeline for a project — shared by the renderer and
/// the live preview.
pub fn build_timeline(project: &Project, project_dir: &Path) -> Result<ges::Timeline, RenderError> {
    ges::init()?;

    {
        let timeline = ges::Timeline::new_audio_video();
        timeline.set_auto_transition(true); // overlaps crossfade automatically

        // GES video tracks default to 720p30 restriction caps — without
        // this, renders silently ignore the project resolution.
        let [w, h] = project.project.resolution;
        let fps = gst::Fraction::approximate_f64(project.project.fps)
            .unwrap_or_else(|| gst::Fraction::new(30, 1));
        for track in timeline.tracks() {
            if track.track_type() == ges::TrackType::VIDEO {
                track.set_restriction_caps(
                    &gst::Caps::builder("video/x-raw")
                        .field("width", w as i32)
                        .field("height", h as i32)
                        .field("framerate", fps)
                        .build(),
                );
            }
        }

        // GES layer priority: first appended = topmost. Titles, then
        // overlay tracks (later file order on top), main sequence at the
        // bottom.
        if !project.titles.is_empty() {
            // One layer per title: titles legitimately overlap (stacked
            // captions, end cards), and GES auto-transition refuses
            // overlapping clips that share a layer.
            for title in &project.titles {
                let title_layer = timeline.append_layer();
                let t = ges::TitleClip::new()
                    .ok_or_else(|| RenderError::Gst("could not create title clip".into()))?;
                t.set_start(title.at.to_clocktime());
                t.set_duration(title.dur.to_clocktime());
                title_layer
                    .add_clip(&t)
                    .map_err(|e| RenderError::Gst(format!("title add: {e}")))?;
                t.set_child_property("text", &title.text)
                    .map_err(|e| RenderError::Gst(format!("title text: {e}")))?;
                // GES defaults the title background to opaque white
                // (0xFFFFFFFF), which blanks every layer below now that
                // each title owns a full-frame layer.
                t.set_child_property("background", &0u32)
                    .map_err(|e| RenderError::Gst(format!("title background: {e}")))?;
                if let Some(font) = &title.font {
                    t.set_child_property("font-desc", font)
                        .map_err(|e| RenderError::Gst(format!("title font: {e}")))?;
                }
                if let Some(x) = title.xpos {
                    t.set_child_property("xpos", &x)
                        .map_err(|e| RenderError::Gst(format!("title xpos: {e}")))?;
                }
                if let Some(y) = title.ypos {
                    t.set_child_property("ypos", &y)
                        .map_err(|e| RenderError::Gst(format!("title ypos: {e}")))?;
                }
                if let Some(c) = &title.color {
                    let argb = parse_color(c)
                        .ok_or_else(|| RenderError::Gst(format!("bad color {c:?}")))?;
                    t.set_child_property("color", &argb)
                        .map_err(|e| RenderError::Gst(format!("title color: {e}")))?;
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
                add_media_clip(
                    &layer,
                    project_dir,
                    clip,
                    start,
                    track_type(track.kind),
                    project.project.resolution,
                )?;
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
                project.project.resolution,
            )?;
        }
        timeline.commit();

        // Typed transitions: auto-transition created crossfades wherever
        // clips overlap; re-type the ones whose right-hand clip asks for a
        // wipe (nick like "bar-wipe-lr").
        for (i, clip) in main.clips.iter().enumerate() {
            let Some(kind) = clip.transition_kind.as_deref() else { continue };
            if kind == "crossfade" || clip.transition.is_none() {
                continue;
            }
            let boundary = positions[i].to_clocktime();
            let enum_class =
                gst::glib::EnumClass::with_type(ges::VideoStandardTransitionType::static_type())
                    .ok_or_else(|| RenderError::Gst("no transition enum".into()))?;
            let value = enum_class.value_by_nick(kind).ok_or_else(|| {
                RenderError::Gst(format!(
                    "unknown transition {kind:?} (try {})",
                    TRANSITION_KINDS.join(", ")
                ))
            })?;
            for tclip in layer.clips() {
                if tclip.is::<ges::TransitionClip>() && tclip.start() == boundary {
                    // vtype is a direct property of GESTransitionClip, not
                    // a child property.
                    tclip.set_property_from_value("vtype", &value.to_value(&enum_class));
                }
            }
        }
        Ok(timeline)
    }
}

impl RenderBackend for GesBackend {
    fn render(
        &self,
        project: &Project,
        project_dir: &Path,
        output: &Path,
    ) -> Result<(), RenderError> {
        let timeline = build_timeline(project, project_dir)?;

        let h264_caps = gst::Caps::builder("video/x-h264").build();
        let mut video_builder = gst_pbutils::EncodingVideoProfile::builder(&h264_caps);
        // Opt-B: force a hardware encoder by factory name when asked.
        if let Some(hw) = crate::hwaccel::from_env() {
            video_builder = video_builder.preset_name(hw.ges_encoder);
        }
        let video_profile = video_builder.build();
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

/// Run `f` under a Cocoa main loop on macOS — GUI video sinks need
/// NSApplication running on the main thread, so call this from the main
/// thread before any preview window exists. Everywhere else it is a
/// plain call.
pub fn run_gui<T, F: FnOnce() -> T + Send>(f: F) -> T {
    #[cfg(target_os = "macos")]
    {
        gst::macos_main(f)
    }
    #[cfg(not(target_os = "macos"))]
    {
        f()
    }
}

/// Live composited preview: play the timeline through a GES preview
/// pipeline — tracks, transforms, fades, titles, keyframes, NO render
/// step. Video appears in a window (Hyprland tiles it); blocks until EOS.
/// VIODE_PREVIEW_SINK=fake swaps in sync-less fakesinks (tests).
pub fn preview_play(
    project: &Project,
    project_dir: &Path,
    start: crate::Time,
) -> Result<(), RenderError> {
    let timeline = build_timeline(project, project_dir)?;
    let pipeline = ges::Pipeline::new();
    pipeline.set_timeline(&timeline)?;
    if std::env::var_os("VIODE_PREVIEW_SINK").is_some_and(|v| v == "fake") {
        let mk = |name: &str| {
            gst::ElementFactory::make(name)
                .property("sync", false)
                .build()
                .map_err(|e| RenderError::Gst(e.to_string()))
        };
        pipeline.set_property("video-sink", mk("fakesink")?);
        pipeline.set_property("audio-sink", mk("fakesink")?);
    }
    pipeline.set_state(gst::State::Paused)?;
    let bus = pipeline
        .bus()
        .ok_or_else(|| RenderError::Gst("pipeline has no bus".into()))?;
    // Wait for preroll, then seek and roll.
    let _ = bus.timed_pop_filtered(
        gst::ClockTime::from_seconds(10),
        &[gst::MessageType::AsyncDone, gst::MessageType::Error],
    );
    if start != crate::Time::ZERO {
        let _ = pipeline.seek_simple(
            gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
            start.to_clocktime(),
        );
    }
    pipeline.set_state(gst::State::Playing)?;
    let mut result = Ok(());
    for msg in bus.iter_timed(gst::ClockTime::NONE) {
        match msg.view() {
            gst::MessageView::Eos(..) => break,
            gst::MessageView::Error(err) => {
                result = Err(RenderError::Gst(format!("{}", err.error())));
                break;
            }
            _ => {}
        }
    }
    pipeline.set_state(gst::State::Null)?;
    result
}

/// "#RRGGBB" or "#AARRGGBB" -> ARGB u32 (alpha defaults to FF).
fn parse_color(s: &str) -> Option<u32> {
    let hex = s.strip_prefix('#')?;
    match hex.len() {
        6 => u32::from_str_radix(hex, 16).ok().map(|v| 0xFF00_0000 | v),
        8 => u32::from_str_radix(hex, 16).ok(),
        _ => None,
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
            && project.main().clips.iter().all(|c| {
                c.transition.is_none()
                    && c.effects.is_empty()
                    && c.rate.is_none()
                    && c.pos.is_none()
                    && c.scale.is_none()
                    && c.rotate.is_none()
                    && c.opacity.is_none()
                    && c.color.is_none()
                    && c.lut.is_none()
                    && c.volume.is_none()
                    && c.pan.is_none()
                    && c.keys.is_empty()
            });
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Every transition kind Viode advertises must be a real GES nick —
    /// this is the regression net for the "clock-cw" bug, where a wrong
    /// name in the suggestion list became a clickable way to break the
    /// preview once the GUI turned suggestions into a dropdown.
    #[test]
    fn advertised_transition_kinds_exist_in_ges() {
        if ges::init().is_err() {
            eprintln!("SKIP advertised_transition_kinds_exist_in_ges: GES not available");
            return;
        }
        let enum_class =
            gst::glib::EnumClass::with_type(ges::VideoStandardTransitionType::static_type())
                .expect("transition enum");
        for kind in TRANSITION_KINDS {
            assert!(
                enum_class.value_by_nick(kind).is_some(),
                "{kind:?} is not a GES transition nick"
            );
        }
    }
}
