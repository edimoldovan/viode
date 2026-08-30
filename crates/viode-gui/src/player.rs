//! The live composited preview: the same `build_timeline` the renderer
//! uses, played through a GES pipeline whose video sink is an appsink —
//! every frame lands in memory as RGBA for the UI to upload as a texture.
//! Audio goes to the system sink on the same pipeline clock, so A/V sync
//! is the pipeline's job, not ours.
//!
//! Threading: GES is single-threaded by contract (the bindings make its
//! types !Send), and building/prerolling a pipeline can take seconds — so
//! a dedicated ACTOR THREAD owns the pipeline for its entire life, and
//! the UI holds a `Player` handle that sends commands. The UI thread
//! never blocks on GStreamer, which is what keeps the window responsive
//! through every rebuild (the compositor kills windows that stall).
//!
//! Headless by construction: nothing here opens a window, which is also
//! what makes the integration tests possible. VIODE_PREVIEW_SINK=fake
//! swaps the audio sink for a fakesink (machines without audio, tests).

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use gstreamer_editing_services as ges;
use gstreamer_editing_services::prelude::*;
use gstreamer_video as gst_video;
use gstreamer_video::VideoFrameExt;

use viode_core::{build_timeline, Project, RenderError, Time};

/// Preview frames are capped at 720p — PLAN.md's mitigation for 4K
/// sources: scale in the pipeline, not in the UI.
const MAX_W: f64 = 1280.0;
const MAX_H: f64 = 720.0;

/// One decoded video frame, tightly packed RGBA.
pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

/// Something the pipeline reported that the UI must react to.
#[derive(Debug, PartialEq)]
pub enum PlayerEvent {
    Eos,
    Error(String),
}

enum Req {
    Load { project: Box<Project>, dir: PathBuf },
    Play,
    Pause,
    Seek(Time),
    SetRate(f64),
    Shutdown,
}

/// The UI-side handle: cheap, Send, never blocks.
pub struct Player {
    tx: Sender<Req>,
    frame: Arc<Mutex<Option<Frame>>>,
    seq: Arc<AtomicU64>,
    position: Arc<Mutex<Option<Time>>>,
    events: Arc<Mutex<Vec<PlayerEvent>>>,
}

impl Player {
    /// Start the actor thread. `repaint` is called from GStreamer/actor
    /// threads whenever something visible changed (hand it
    /// `egui::Context::request_repaint`).
    pub fn spawn(repaint: impl Fn() + Send + Sync + 'static) -> Player {
        let (tx, rx) = channel();
        let frame: Arc<Mutex<Option<Frame>>> = Arc::new(Mutex::new(None));
        let seq = Arc::new(AtomicU64::new(0));
        let position: Arc<Mutex<Option<Time>>> = Arc::new(Mutex::new(None));
        let events: Arc<Mutex<Vec<PlayerEvent>>> = Arc::new(Mutex::new(Vec::new()));
        {
            let (frame, seq, position, events) =
                (frame.clone(), seq.clone(), position.clone(), events.clone());
            std::thread::spawn(move || {
                actor(rx, frame, seq, position, events, Arc::new(repaint));
            });
        }
        Player {
            tx,
            frame,
            seq,
            position,
            events,
        }
    }

    /// Build (or rebuild) the pipeline for this project, paused at frame
    /// zero. Returns immediately; failures surface as PlayerEvent::Error.
    pub fn load(&self, project: &Project, dir: &std::path::Path) {
        let _ = self.tx.send(Req::Load {
            project: Box::new(project.clone()),
            dir: dir.to_path_buf(),
        });
    }

    /// Monotonic counter bumped on every new frame — cheap "is the
    /// texture stale" check for the UI.
    pub fn frame_seq(&self) -> u64 {
        self.seq.load(Ordering::Acquire)
    }

    /// Hand the latest frame to `f` without copying it out.
    pub fn with_frame<R>(&self, f: impl FnOnce(&Frame) -> R) -> Option<R> {
        self.frame.lock().unwrap().as_ref().map(f)
    }

    pub fn play(&self) {
        let _ = self.tx.send(Req::Play);
    }

    pub fn pause(&self) {
        let _ = self.tx.send(Req::Pause);
    }

    pub fn seek(&self, t: Time) {
        let _ = self.tx.send(Req::Seek(t));
    }

    /// Change playback rate in place (JKL shuttle). Reverse rates ask GES
    /// to play backwards; sources that cannot are reported on the bus and
    /// surface through poll_events, never as a hang.
    pub fn set_rate(&self, rate: f64) {
        let _ = self.tx.send(Req::SetRate(rate));
    }

    /// Last position the actor observed (refreshed ~30x/s).
    pub fn position(&self) -> Option<Time> {
        *self.position.lock().unwrap()
    }

    /// Drain pending pipeline events — call once per UI frame.
    pub fn poll_events(&self) -> Vec<PlayerEvent> {
        std::mem::take(&mut *self.events.lock().unwrap())
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        let _ = self.tx.send(Req::Shutdown);
    }
}

/// The actor: sole owner of every GES object. One iteration ~every 33ms
/// (or immediately on a command): handle a request, drain the bus, note
/// the position.
fn actor(
    rx: Receiver<Req>,
    frame: Arc<Mutex<Option<Frame>>>,
    seq: Arc<AtomicU64>,
    position: Arc<Mutex<Option<Time>>>,
    events: Arc<Mutex<Vec<PlayerEvent>>>,
    repaint: Arc<dyn Fn() + Send + Sync>,
) {
    let mut pipeline: Option<ges::Pipeline> = None;
    let mut rate = 1.0;
    let shutdown = |p: &mut Option<ges::Pipeline>| {
        if let Some(p) = p.take() {
            let _ = p.set_state(gst::State::Null);
        }
    };
    loop {
        let req = rx.recv_timeout(Duration::from_millis(33));
        match req {
            Ok(Req::Load { project, dir }) => {
                shutdown(&mut pipeline);
                rate = 1.0;
                *position.lock().unwrap() = Some(Time::ZERO);
                // The stored frame belongs to the previous pipeline; the
                // UI keeps its uploaded texture (no flash), but a fresh
                // upload must wait for a fresh frame.
                *frame.lock().unwrap() = None;
                match build_pipeline(&project, &dir, &frame, &seq, &repaint) {
                    Ok(p) => pipeline = Some(p),
                    Err(e) => {
                        events.lock().unwrap().push(PlayerEvent::Error(e.to_string()));
                    }
                }
                repaint();
            }
            Ok(Req::Play) => {
                if let Some(p) = &pipeline {
                    let _ = p.set_state(gst::State::Playing);
                }
            }
            Ok(Req::Pause) => {
                if let Some(p) = &pipeline {
                    let _ = p.set_state(gst::State::Paused);
                }
            }
            Ok(Req::Seek(t)) => {
                if let Some(p) = &pipeline {
                    let _ = p.seek_simple(
                        gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
                        t.to_clocktime(),
                    );
                    *position.lock().unwrap() = Some(t);
                }
            }
            Ok(Req::SetRate(new_rate)) => {
                if let Some(p) = &pipeline {
                    if new_rate != rate && new_rate != 0.0 {
                        let pos = p
                            .query_position::<gst::ClockTime>()
                            .unwrap_or(gst::ClockTime::ZERO);
                        let flags = gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE;
                        let ok = if new_rate > 0.0 {
                            p.seek(
                                new_rate,
                                flags,
                                gst::SeekType::Set,
                                pos,
                                gst::SeekType::Set,
                                gst::ClockTime::NONE,
                            )
                        } else {
                            p.seek(
                                new_rate,
                                flags,
                                gst::SeekType::Set,
                                gst::ClockTime::ZERO,
                                gst::SeekType::Set,
                                pos,
                            )
                        };
                        if ok.is_ok() {
                            rate = new_rate;
                        }
                    }
                }
            }
            Ok(Req::Shutdown) | Err(RecvTimeoutError::Disconnected) => {
                shutdown(&mut pipeline);
                return;
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
        if let Some(p) = &pipeline {
            if let Some(bus) = p.bus() {
                let mut fresh = Vec::new();
                while let Some(msg) =
                    bus.pop_filtered(&[gst::MessageType::Eos, gst::MessageType::Error])
                {
                    match msg.view() {
                        gst::MessageView::Eos(..) => fresh.push(PlayerEvent::Eos),
                        gst::MessageView::Error(e) => {
                            fresh.push(PlayerEvent::Error(e.error().to_string()))
                        }
                        _ => {}
                    }
                }
                if !fresh.is_empty() {
                    events.lock().unwrap().extend(fresh);
                    repaint();
                }
            }
            if let Some(pos) = p.query_position::<gst::ClockTime>() {
                *position.lock().unwrap() = Some(Time::from_clocktime(pos));
            }
        }
    }
}

/// Build the GES preview pipeline: timeline -> appsink bin, paused and
/// prerolled so the first frame exists before anyone presses play.
fn build_pipeline(
    project: &Project,
    project_dir: &std::path::Path,
    frame: &Arc<Mutex<Option<Frame>>>,
    seq: &Arc<AtomicU64>,
    repaint: &Arc<dyn Fn() + Send + Sync>,
) -> Result<ges::Pipeline, RenderError> {
    let timeline = build_timeline(project, project_dir)?;
    let pipeline = ges::Pipeline::new();
    pipeline.set_timeline(&timeline)?;

    let [pw, ph] = project.project.resolution;
    let (w, h) = fit_preview(pw, ph);

    let gerr = |e: &dyn std::fmt::Display| RenderError::Gst(e.to_string());
    let convert = gst::ElementFactory::make("videoconvert")
        .build()
        .map_err(|e| gerr(&e))?;
    let scale = gst::ElementFactory::make("videoscale")
        .build()
        .map_err(|e| gerr(&e))?;
    let caps = gst::Caps::builder("video/x-raw")
        .field("format", "RGBA")
        .field("width", w)
        .field("height", h)
        .build();
    let appsink = gst_app::AppSink::builder()
        .caps(&caps)
        .max_buffers(2)
        .drop(true)
        .build();

    {
        let frame = frame.clone();
        let seq = seq.clone();
        let repaint = repaint.clone();
        let on_sample = move |sample: gst::Sample| {
            if let Some(f) = copy_frame(&sample) {
                *frame.lock().unwrap() = Some(f);
                seq.fetch_add(1, Ordering::Release);
                repaint();
            }
        };
        let preroll = on_sample.clone();
        appsink.set_callbacks(
            gst_app::AppSinkCallbacks::builder()
                .new_sample(move |sink| {
                    let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                    on_sample(sample);
                    Ok(gst::FlowSuccess::Ok)
                })
                // Preroll fires on every paused seek — this is what
                // makes scrubbing show frames without playing.
                .new_preroll(move |sink| {
                    let sample = sink.pull_preroll().map_err(|_| gst::FlowError::Eos)?;
                    preroll(sample);
                    Ok(gst::FlowSuccess::Ok)
                })
                .build(),
        );
    }

    let bin = gst::Bin::builder().name("viode-gui-sink").build();
    bin.add_many([&convert, &scale, appsink.upcast_ref()])
        .map_err(|e| gerr(&e))?;
    gst::Element::link_many([&convert, &scale, appsink.upcast_ref()])
        .map_err(|e| gerr(&e))?;
    let sink_pad = convert
        .static_pad("sink")
        .ok_or_else(|| RenderError::Gst("videoconvert has no sink pad".into()))?;
    let ghost = gst::GhostPad::with_target(&sink_pad).map_err(|e| gerr(&e))?;
    bin.add_pad(&ghost).map_err(|e| gerr(&e))?;
    pipeline.set_property("video-sink", &bin);

    if std::env::var_os("VIODE_PREVIEW_SINK").is_some_and(|v| v == "fake") {
        let fake = gst::ElementFactory::make("fakesink")
            .property("sync", false)
            .build()
            .map_err(|e| gerr(&e))?;
        pipeline.set_property("audio-sink", &fake);
    }

    pipeline.set_state(gst::State::Paused)?;
    // Wait for preroll so the first frame exists before play is pressed.
    // This blocks the ACTOR thread only — the UI keeps painting.
    let bus = pipeline
        .bus()
        .ok_or_else(|| RenderError::Gst("pipeline has no bus".into()))?;
    let _ = bus.timed_pop_filtered(
        gst::ClockTime::from_seconds(10),
        &[gst::MessageType::AsyncDone, gst::MessageType::Error],
    );
    Ok(pipeline)
}

/// Fit the project resolution inside the preview cap, even dimensions.
fn fit_preview(w: u32, h: u32) -> (i32, i32) {
    let (w, h) = (w.max(2) as f64, h.max(2) as f64);
    let s = (MAX_W / w).min(MAX_H / h).min(1.0);
    let even = |v: f64| ((v / 2.0).round() as i32 * 2).max(2);
    (even(w * s), even(h * s))
}

/// Copy a sample out into tightly packed RGBA, honoring row strides.
fn copy_frame(sample: &gst::Sample) -> Option<Frame> {
    let caps = sample.caps()?;
    let info = gst_video::VideoInfo::from_caps(caps).ok()?;
    let buffer = sample.buffer()?;
    let vframe = gst_video::VideoFrameRef::from_buffer_ref_readable(buffer, &info).ok()?;
    let (w, h) = (info.width() as usize, info.height() as usize);
    let stride = vframe.info().stride()[0] as usize;
    let data = vframe.plane_data(0).ok()?;
    let mut rgba = vec![0u8; w * h * 4];
    for row in 0..h {
        rgba[row * w * 4..][..w * 4].copy_from_slice(&data[row * stride..][..w * 4]);
    }
    Some(Frame {
        width: w,
        height: h,
        rgba,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_fits_within_720p() {
        assert_eq!(fit_preview(3840, 2160), (1280, 720));
        assert_eq!(fit_preview(1920, 1080), (1280, 720));
        // Smaller sources stay native.
        assert_eq!(fit_preview(640, 360), (640, 360));
        // Odd portrait source: scaled by height, kept even.
        assert_eq!(fit_preview(1080, 1920), (406, 720));
    }
}
