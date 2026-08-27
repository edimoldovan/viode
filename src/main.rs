// Phase 0 spike: prove the Rust + GES bet.
// Loads clips, places them back-to-back on a GES timeline, renders H.264/AAC MP4.
//
// Usage: viode-spike [input1 input2 ... inputN] [-o output.mp4]
// Defaults: assets/clip1.mp4 assets/clip2.mp4 -> renders/spike.mp4

use gstreamer as gst;
use gstreamer_editing_services as ges;
use gstreamer_pbutils as gst_pbutils;

use ges::prelude::*;

use std::path::{Path, PathBuf};

fn abs(path: &str) -> PathBuf {
    let p = Path::new(path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir().unwrap().join(p)
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    ges::init()?;

    let mut inputs: Vec<String> = Vec::new();
    let mut output = String::from("renders/spike.mp4");
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "-o" {
            output = args.next().ok_or("-o needs a path")?;
        } else {
            inputs.push(arg);
        }
    }
    if inputs.is_empty() {
        inputs = vec!["assets/clip1.mp4".into(), "assets/clip2.mp4".into()];
    }

    let timeline = ges::Timeline::new_audio_video();
    let layer = timeline.append_layer();

    let mut cursor = gst::ClockTime::ZERO;
    for input in &inputs {
        let uri = gst::glib::filename_to_uri(abs(input), None)?;
        let asset = ges::UriClipAsset::request_sync(&uri)?;
        let duration = asset
            .duration()
            .ok_or_else(|| format!("no duration discovered for {input}"))?;
        layer.add_asset(
            &asset,
            cursor,
            gst::ClockTime::ZERO,
            duration,
            ges::TrackType::UNKNOWN,
        )?;
        println!("placed {input} at {cursor} (duration {duration})");
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

    let out_path = abs(&output);
    if let Some(dir) = out_path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let out_uri = gst::glib::filename_to_uri(&out_path, None)?;

    let pipeline = ges::Pipeline::new();
    pipeline.set_timeline(&timeline)?;
    pipeline.set_render_settings(&out_uri, &profile)?;
    pipeline.set_mode(ges::PipelineFlags::RENDER)?;
    pipeline.set_state(gst::State::Playing)?;

    let bus = pipeline.bus().ok_or("pipeline has no bus")?;
    for msg in bus.iter_timed(gst::ClockTime::NONE) {
        match msg.view() {
            gst::MessageView::Eos(..) => break,
            gst::MessageView::Error(err) => {
                pipeline.set_state(gst::State::Null)?;
                return Err(format!(
                    "render error from {:?}: {} ({:?})",
                    err.src().map(|s| s.path_string()),
                    err.error(),
                    err.debug()
                )
                .into());
            }
            _ => {}
        }
    }
    pipeline.set_state(gst::State::Null)?;

    println!("rendered {} ({} total)", out_path.display(), cursor);
    Ok(())
}
