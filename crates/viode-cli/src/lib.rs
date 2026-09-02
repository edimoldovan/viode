use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};

use viode_core::{
    ops, probe, Clip, GesBackend, Project, RenderBackend, SmartCopyBackend, Time, Title, Track,
    TrackKind, PROJECT_FILE,
};

#[derive(Parser)]
#[command(name = "viode", version, about = "Terminal-native video editor")]
struct Cli {
    /// Project file (defaults to ./project.viode)
    #[arg(long, global = true, default_value = PROJECT_FILE)]
    project: PathBuf,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create a new project directory
    New {
        name: String,
        #[arg(long, default_value_t = 30.0)]
        fps: f64,
        /// Resolution as WIDTHxHEIGHT
        #[arg(long, default_value = "1920x1080")]
        res: String,
    },
    /// Copy media files into the project's media/ directory
    Import { files: Vec<PathBuf> },
    /// Show media file metadata
    Probe { file: PathBuf },
    /// Append a clip (copies the file into media/ if outside the project)
    Add {
        src: PathBuf,
        #[arg(long = "in")]
        in_: Option<String>,
        #[arg(long)]
        out: Option<String>,
        /// Target track (0 = main sequence)
        #[arg(long, default_value_t = 0)]
        track: usize,
        /// Timeline position — required for overlay tracks
        #[arg(long)]
        at: Option<String>,
    },
    /// List the timeline (all tracks and titles)
    Ls,
    /// Manage tracks: add, ls, on <i>, off <i>
    Track {
        #[command(subcommand)]
        cmd: TrackCmd,
    },
    /// Change a main-track clip's source in/out points
    Trim {
        index: usize,
        #[arg(long = "in")]
        in_: Option<String>,
        #[arg(long)]
        out: Option<String>,
    },
    /// Split a main-track clip at an offset from its start
    Split { index: usize, at: String },
    /// Move a main-track clip to a new position in the sequence
    Move { from: usize, to: usize },
    /// Remove a main-track clip
    Rm { index: usize },
    /// Crossfade/wipe with the previous clip (0 clears)
    Fade {
        index: usize,
        duration: String,
        /// crossfade (default), bar-wipe-lr, bar-wipe-tb, box-wipe-tl, iris-rect, clock-cw12
        #[arg(long)]
        kind: Option<String>,
    },
    /// Position/scale/rotate/opacity — picture-in-picture and layouts
    Place {
        index: usize,
        /// Fraction of frame width for the left edge (0..1)
        #[arg(long, allow_negative_numbers = true)]
        x: Option<f64>,
        /// Fraction of frame height for the top edge (0..1)
        #[arg(long, allow_negative_numbers = true)]
        y: Option<f64>,
        /// Uniform scale (0.25 = corner PiP)
        #[arg(long)]
        scale: Option<f64>,
        /// Rotation in degrees
        #[arg(long, allow_negative_numbers = true)]
        rotate: Option<f64>,
        /// Opacity 0..1
        #[arg(long)]
        opacity: Option<f64>,
        #[arg(long, default_value_t = 0)]
        track: usize,
        /// Reset all placement to full-frame defaults
        #[arg(long)]
        clear: bool,
    },
    /// Color grade a clip (neutral: brightness 0, contrast/saturation 1, hue 0)
    Color {
        index: usize,
        #[arg(long, allow_negative_numbers = true)]
        brightness: Option<f64>,
        #[arg(long)]
        contrast: Option<f64>,
        #[arg(long)]
        saturation: Option<f64>,
        #[arg(long, allow_negative_numbers = true)]
        hue: Option<f64>,
        /// Gamma (0.01..10, neutral 1)
        #[arg(long)]
        gamma: Option<f64>,
        /// 3D LUT (.cube) to apply
        #[arg(long)]
        lut: Option<PathBuf>,
        #[arg(long, default_value_t = 0)]
        track: usize,
        #[arg(long)]
        clear: bool,
    },
    /// Waveform/vectorscope of a frame — the colorist's instruments
    Scope {
        index: usize,
        /// Source time of the frame to analyze
        #[arg(long, default_value = "0")]
        at: String,
        #[arg(long, default_value = "waveform")]
        kind: String,
    },
    /// Playback rate: 2 = double speed, 0.5 = slow motion (1 clears)
    Speed {
        index: usize,
        rate: f64,
        #[arg(long, default_value_t = 0)]
        track: usize,
    },
    /// Connect Viode to the AI apps on this machine (Claude, Cursor,
    /// opencode, ...) so you can edit by talking to them
    Connect {
        /// A specific client id (default: connect every one found)
        client: Option<String>,
        /// Print the manual config snippet for unlisted clients
        #[arg(long)]
        print: bool,
    },
    /// Blur or pixelate a region of a clip (optionally tracking it)
    Mask {
        index: usize,
        /// Region as x,y,w,h fractions (e.g. 0.6,0.1,0.25,0.3)
        #[arg(long)]
        region: Option<String>,
        /// blur or pixelate
        #[arg(long, default_value = "blur")]
        kind: String,
        /// Track the region's content and move the mask with it
        #[arg(long)]
        follow: bool,
        #[arg(long)]
        off: bool,
        #[arg(long, default_value_t = 0)]
        track: usize,
    },
    /// Smooth the jump cut before clip i with a short optical-flow morph
    Mend {
        /// The right-hand clip of the cut (bridges i-1 | i)
        index: usize,
        #[arg(long, default_value = "0.25")]
        dur: String,
    },
    /// Add another project as one clip (a nested sequence)
    Bundle {
        /// Path to the sub-project's directory or project.viode
        path: PathBuf,
        #[arg(long, default_value_t = 0)]
        track: usize,
        /// Timeline position — required for overlay tracks
        #[arg(long)]
        at: Option<String>,
    },
    /// Match a clip's exposure and saturation to a reference clip
    Match {
        /// Clip to correct
        index: usize,
        /// Reference clip index
        #[arg(long)]
        to: usize,
        #[arg(long, default_value_t = 0)]
        track: usize,
        /// Reference clip's track (defaults to the same track)
        #[arg(long)]
        to_track: Option<usize>,
    },
    /// Chroma key an overlay clip: green/blue becomes transparent
    Matte {
        /// Overlay track holding the keyed clip
        track: usize,
        index: usize,
        /// green, blue, or off
        method: String,
    },
    /// Retime a music overlay clip to a target duration with an
    /// invisible crossfaded seam at the quietest point
    Refit {
        /// Overlay track holding the music
        track: usize,
        /// Clip index on that track
        index: usize,
        /// Target duration, e.g. 90 or 01:30
        #[arg(long)]
        to: String,
        /// Crossfade at the seam, seconds
        #[arg(long, default_value_t = 0.5)]
        fade: f64,
    },
    /// Clean up a clip's voice audio (denoise + rumble cut; --off clears)
    Clean {
        index: usize,
        /// Noise reduction in dB (afftdn nr)
        #[arg(long, default_value_t = 12.0)]
        strength: f64,
        #[arg(long)]
        off: bool,
        #[arg(long, default_value_t = 0)]
        track: usize,
    },
    /// Duck music under dialogue: volume keyframes on an overlay track,
    /// planned from the main track's loudness analysis
    Duck {
        /// The music/overlay track to duck (see `viode track ls`)
        track: usize,
        /// Ducked volume as a fraction of the clip's own volume
        #[arg(long, default_value_t = 0.25)]
        amount: f64,
        /// Speech threshold in RMS dBFS
        #[arg(long, default_value_t = -35.0, allow_negative_numbers = true)]
        threshold: f64,
    },
    /// Add a marker at a timeline time (a named note; never renders)
    Mark {
        /// Timeline time, e.g. 1.5 or 00:01:30 (omit with --rm)
        at: Option<String>,
        /// Marker text
        text: Vec<String>,
        /// Remove marker by index (see `viode marks`)
        #[arg(long)]
        rm: Option<usize>,
    },
    /// List markers
    Marks,
    /// Stabilize a clip's footage (vidstab bake; --off clears)
    Steady {
        index: usize,
        #[arg(long, default_value_t = 10)]
        smoothing: u32,
        #[arg(long)]
        off: bool,
        #[arg(long, default_value_t = 0)]
        track: usize,
    },
    /// Freeze the frame at a timeline time for a duration (frame hold)
    Freeze {
        /// Timeline time of the frame to hold (e.g. 1.5 or 00:01:30)
        at: String,
        #[arg(long, default_value = "2")]
        dur: String,
    },
    /// Speed-ramp a clip: split into stepped segments from one rate to
    /// another (Premiere's time remapping, stepped form)
    Ramp {
        index: usize,
        #[arg(long)]
        from: f64,
        #[arg(long)]
        to: f64,
        #[arg(long, default_value_t = 8)]
        steps: usize,
        #[arg(long, default_value_t = 0)]
        track: usize,
    },
    /// Roll the boundary between clip i-1 and i by ±seconds (total unchanged)
    Roll {
        index: usize,
        #[arg(allow_negative_numbers = true)]
        delta: f64,
    },
    /// Slip a clip's content by ±seconds (slot unchanged)
    Slip {
        index: usize,
        #[arg(allow_negative_numbers = true)]
        delta: f64,
    },
    /// Slide a clip against its neighbours by ±seconds (total unchanged)
    Slide {
        index: usize,
        #[arg(allow_negative_numbers = true)]
        delta: f64,
    },
    /// Measure software vs VA-API proxy encoding on YOUR footage and
    /// print which path wins on this machine
    Bench {
        file: PathBuf,
        #[arg(long, default_value_t = 30)]
        secs: u32,
    },
    /// Check which engine pieces exist on this machine and what breaks
    /// without them
    Doctor,
    /// LIVE composited preview: play the timeline in a window, no render
    Play {
        #[arg(long, default_value = "0")]
        from: String,
    },
    /// Render queue: add jobs, run them in order
    Queue {
        #[command(subcommand)]
        cmd: QueueCmd,
    },
    /// List media / find missing sources
    Media {
        #[command(subcommand)]
        cmd: MediaCmd,
    },
    /// Relink missing media by filename from a directory (recursive)
    Relink { dir: PathBuf },
    /// Set a clip's audio gain (linear, 1.0 = unity; e.g. 0.5 = -6dB)
    Gain {
        index: usize,
        volume: f64,
        #[arg(long, default_value_t = 0)]
        track: usize,
    },
    /// Pan a clip's audio: -1.0 (left) .. 1.0 (right)
    Pan {
        index: usize,
        #[arg(allow_negative_numbers = true)]
        pan: f64,
        #[arg(long, default_value_t = 0)]
        track: usize,
    },
    /// Add a keyframe: volume (0..2), alpha (0..1), or x/y/scale (frame
    /// fractions, like `place`); at is SOURCE time
    Key {
        index: usize,
        prop: String,
        at: String,
        value: f64,
        #[arg(long, default_value_t = 0)]
        track: usize,
    },
    /// List / remove keyframes on a clip
    Keys {
        index: usize,
        /// Remove the keyframe with this number
        #[arg(long)]
        rm: Option<usize>,
        #[arg(long, default_value_t = 0)]
        track: usize,
    },
    /// Add/clear GStreamer effects on a clip, e.g. "videobalance saturation=0"
    Fx {
        index: usize,
        /// Effect description; omit with --clear to remove all
        effect: Option<String>,
        #[arg(long, default_value_t = 0)]
        track: usize,
        #[arg(long)]
        clear: bool,
    },
    /// Add a title overlay
    Title {
        text: String,
        #[arg(long)]
        at: String,
        #[arg(long)]
        dur: String,
        /// Pango font description, e.g. "Sans Bold 64"
        #[arg(long)]
        font: Option<String>,
        /// Horizontal position 0..1
        #[arg(long)]
        x: Option<f64>,
        /// Vertical position 0..1 (0.8 = lower third)
        #[arg(long)]
        y: Option<f64>,
        /// Text color "#RRGGBB"
        #[arg(long)]
        color: Option<String>,
    },
    /// List / remove titles
    Titles {
        /// Remove the title with this index
        #[arg(long)]
        rm: Option<usize>,
    },
    /// Audio-sync offset between two media files (multicam)
    Sync {
        a: PathBuf,
        b: PathBuf,
        #[arg(long, default_value_t = 60.0)]
        max_lag: f64,
    },
    /// Add a synced multicam angle as a disabled track
    Angle { file: PathBuf },
    /// Cut to an angle track for a timeline range (multicam take)
    Take {
        track: usize,
        start: String,
        end: String,
    },
    /// Transcribe a main-track clip (whisper.cpp)
    Transcribe {
        index: usize,
        /// Path to a ggml model (or set VIODE_WHISPER_MODEL)
        #[arg(long)]
        model: Option<PathBuf>,
    },
    /// Generate captions for the whole timeline: SRT sidecar and/or
    /// burned-in lower-third titles (uses whisper.cpp, cached per source)
    Captions {
        /// Write an SRT file (e.g. captions.srt)
        #[arg(long)]
        srt: Option<PathBuf>,
        /// Add the captions as lower-third titles (visible in preview and render)
        #[arg(long)]
        burn: bool,
        /// Path to a ggml model (or set VIODE_WHISPER_MODEL)
        #[arg(long)]
        model: Option<PathBuf>,
    },
    /// Cut transcript segments [from..=to] out of a clip (see `transcribe`)
    CutText {
        index: usize,
        from: usize,
        to: usize,
        #[arg(long, default_value_t = 0.05)]
        pad: f64,
    },
    /// Build 540p proxies for all media
    Proxy {
        #[arg(long)]
        force: bool,
    },
    /// Render a clip's audio waveform to a PNG in cache/
    Waveform {
        index: usize,
        #[arg(long, default_value_t = 1024)]
        width: u32,
        #[arg(long, default_value_t = 160)]
        height: u32,
    },
    /// Render a clip's contact sheet (tiled filmstrip) to a PNG in cache/
    Thumbs {
        index: usize,
        #[arg(long, default_value_t = 1.0)]
        interval: f64,
        #[arg(long, default_value_t = 5)]
        cols: u32,
    },
    /// Print a clip's RMS loudness (dBFS) per time window
    Levels {
        index: usize,
        #[arg(long, default_value_t = 0.5)]
        window: f64,
    },
    /// List silent stretches in a clip's source audio
    Silences {
        index: usize,
        #[arg(long, default_value_t = -35.0)]
        threshold: f64,
        #[arg(long, default_value_t = 0.5)]
        min: f64,
    },
    /// Cut silent stretches out of a clip (the podcast dead-air remover)
    CutSilences {
        index: usize,
        #[arg(long, default_value_t = -35.0)]
        threshold: f64,
        #[arg(long, default_value_t = 0.5)]
        min: f64,
        #[arg(long, default_value_t = 0.15)]
        pad: f64,
    },
    /// List scene changes in a clip's source video
    Scenes {
        index: usize,
        #[arg(long, default_value_t = 0.4)]
        threshold: f64,
    },
    /// Split a clip at every scene change
    SplitScenes {
        index: usize,
        #[arg(long, default_value_t = 0.4)]
        threshold: f64,
    },
    /// Open the timeline in the terminal UI
    Tui,
    /// Open the GUI: a project file or directory, the current directory's
    /// project, or the welcome screen when there is neither
    #[command(alias = "ui")]
    Gui {
        /// Project file or project directory (as passed by file managers)
        path: Option<PathBuf>,
    },
    /// Run the MCP server (stdio) — lets AI clients edit the project
    Serve {
        #[arg(long)]
        mcp: bool,
    },
    /// Render the timeline
    Render {
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Lossless stream-copy (cut-only projects; cuts snap to keyframes)
        #[arg(long)]
        smart: bool,
        /// Finish for a destination: youtube, shorts, or podcast
        #[arg(long)]
        preset: Option<String>,
        /// Delivery codec: h264, hevc, av1, prores, dnxhr
        #[arg(long)]
        codec: Option<String>,
        /// Video bitrate in kbps (default: quality-targeted CRF)
        #[arg(long)]
        bitrate: Option<u32>,
        /// Shorts only: follow the subject instead of center-cropping
        /// (face detection, per scene)
        #[arg(long)]
        reframe: bool,
        /// Optical-flow smooth slow motion to this fps (ffmpeg minterpolate)
        #[arg(long)]
        smooth: Option<u32>,
    },
}

#[derive(Subcommand)]
enum QueueCmd {
    /// Queue a render job (same options as `render`)
    Add {
        #[arg(long)]
        preset: Option<String>,
        #[arg(long)]
        codec: Option<String>,
        #[arg(long)]
        bitrate: Option<u32>,
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    Ls,
    /// Run every queued job in order
    Run,
    Clear,
}

#[derive(Subcommand)]
enum MediaCmd {
    /// Every source referenced by the timeline
    Ls,
    /// Sources that no longer exist on disk
    Missing,
}

#[derive(Subcommand)]
enum TrackCmd {
    /// Add a track: kind av (default), video, or audio
    Add {
        name: String,
        #[arg(long, default_value = "av")]
        kind: String,
    },
    /// List tracks
    Ls,
    /// Enable a track
    On { index: usize },
    /// Disable a track (kept in the file, excluded from renders)
    Off { index: usize },
}

/// Parse the command line and execute the chosen verb. This is the whole
/// CLI as a library call so that other binaries (the official licensed
/// build lives in a separate private crate) can embed it unchanged.
pub fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::New { name, fps, res } => cmd_new(&name, fps, &res),
        Cmd::Probe { file } => cmd_probe(&file),
        Cmd::Import { files } => cmd_import(&cli.project, &files),
        Cmd::Add { src, in_, out, track, at } => {
            cmd_add(&cli.project, &src, in_.as_deref(), out.as_deref(), track, at.as_deref())
        }
        Cmd::Ls => cmd_ls(&cli.project),
        Cmd::Track { cmd } => cmd_track(&cli.project, cmd),
        Cmd::Trim { index, in_, out } => with_project(&cli.project, |p| {
            Ok(ops::trim(p.main_mut(), index, parse_opt(in_.as_deref())?, parse_opt(out.as_deref())?)?)
        }),
        Cmd::Split { index, at } => with_project(&cli.project, |p| {
            Ok(ops::split(p.main_mut(), index, Time::parse(&at)?)?)
        }),
        Cmd::Move { from, to } => {
            with_project(&cli.project, |p| Ok(ops::move_clip(p.main_mut(), from, to)?))
        }
        Cmd::Rm { index } => with_project(&cli.project, |p| {
            let clip = ops::remove(p.main_mut(), index)?;
            println!("removed [{}] {}", index, clip.src.display());
            Ok(())
        }),
        Cmd::Fade { index, duration, kind } => with_project(&cli.project, |p| {
            let d = Time::parse(&duration)?;
            let d = (d != Time::ZERO).then_some(d);
            ops::set_transition(p.main_mut(), index, d)?;
            p.main_mut().clips[index].transition_kind = kind.filter(|_| d.is_some());
            println!(
                "clip {index} transition: {}",
                d.map(|d| d.to_string()).unwrap_or_else(|| "none".into())
            );
            Ok(())
        }),
        Cmd::Place { index, x, y, scale, rotate, opacity, track, clear } => {
            with_project(&cli.project, |p| {
                let t = ops::track_mut(p, track)?;
                let c = t.clips.get_mut(index).context("clip index out of range")?;
                if clear {
                    (c.pos, c.scale, c.rotate, c.opacity) = (None, None, None, None);
                    println!("clip {index}: full-frame");
                    return Ok(());
                }
                if x.is_some() || y.is_some() {
                    let old = c.pos.unwrap_or([0.0, 0.0]);
                    c.pos = Some([x.unwrap_or(old[0]), y.unwrap_or(old[1])]);
                }
                if scale.is_some() {
                    c.scale = scale;
                }
                if rotate.is_some() {
                    c.rotate = rotate;
                }
                if let Some(o) = opacity {
                    if !(0.0..=1.0).contains(&o) {
                        bail!("opacity {o} out of range (0..1)");
                    }
                    c.opacity = Some(o);
                }
                println!("clip {index} placed");
                Ok(())
            })
        }
        Cmd::Color { index, brightness, contrast, saturation, hue, gamma, lut, track, clear } => {
            with_project(&cli.project, |p| {
                let t = ops::track_mut(p, track)?;
                let c = t.clips.get_mut(index).context("clip index out of range")?;
                if clear {
                    c.color = None;
                    c.lut = None;
                    println!("clip {index}: neutral color");
                    return Ok(());
                }
                if brightness.is_some() || contrast.is_some() || saturation.is_some() || hue.is_some() || gamma.is_some() {
                    let mut g = c.color.clone().unwrap_or(viode_core::ColorGrade {
                        brightness: None, contrast: None, saturation: None, hue: None, gamma: None,
                    });
                    if brightness.is_some() { g.brightness = brightness; }
                    if contrast.is_some() { g.contrast = contrast; }
                    if saturation.is_some() { g.saturation = saturation; }
                    if hue.is_some() { g.hue = hue; }
                    if gamma.is_some() { g.gamma = gamma; }
                    c.color = Some(g);
                }
                if lut.is_some() {
                    c.lut = lut;
                }
                println!("clip {index} graded");
                Ok(())
            })
        }
        Cmd::Scope { index, at, kind } => {
            let project = Project::load(&cli.project)?;
            let src = clip_source(&cli.project, &project, index)?;
            let dir = project_dir(&cli.project);
            let dest = dir.join("cache").join(format!("scope_{index}.png"));
            viode_core::scope_png(&src, Time::parse(&at)?, &kind, &dest)?;
            println!("{}", dest.display());
            Ok(())
        }
        Cmd::Speed { index, rate, track } => with_project(&cli.project, |p| {
            if rate <= 0.0 || rate > 20.0 {
                bail!("rate {rate} out of range (0..20]");
            }
            let t = ops::track_mut(p, track)?;
            let c = t.clips.get_mut(index).context("clip index out of range")?;
            c.rate = (rate != 1.0).then_some(rate);
            println!("clip {index} rate {rate} (timeline length {})", c.len());
            Ok(())
        }),
        Cmd::Connect { client, print } => {
            if print {
                println!("Add this to your AI app's tool-server config:\n{}", viode_core::connect::snippet());
                return Ok(());
            }
            if let Some(id) = client {
                println!("{}", viode_core::connect::connect(&id)?);
                return Ok(());
            }
            let statuses = viode_core::connect::detect();
            let mut connected_any = false;
            for s in &statuses {
                if !s.found {
                    continue;
                }
                if s.connected {
                    println!("{} — already connected", s.name);
                    connected_any = true;
                    continue;
                }
                match viode_core::connect::connect(&s.id) {
                    Ok(msg) => {
                        println!("{msg}");
                        connected_any = true;
                    }
                    Err(e) => println!("{}: {e}", s.name),
                }
            }
            if !connected_any {
                println!(
                    "No compatible AI app found. Viode works with Claude Desktop, \
                     Claude Code, Cursor, Windsurf, Gemini CLI, and opencode.\n\
                     Install one, or use `viode connect --print` for the manual snippet."
                );
            }
            Ok(())
        }
        Cmd::Mask { index, region, kind, follow, off, track } => with_project(&cli.project, |p| {
            let t = ops::track_mut(p, track)?;
            let c = t.clips.get_mut(index).context("clip index out of range")?;
            if off {
                c.mask = None;
                println!("clip {index} mask off");
                return Ok(());
            }
            let region_str = region.context("give --region x,y,w,h (fractions)")?;
            let parts: Vec<f64> = region_str
                .split(',')
                .map(|v| v.trim().parse::<f64>())
                .collect::<Result<_, _>>()
                .context("region must be four numbers: x,y,w,h")?;
            if parts.len() != 4 {
                bail!("region must be four numbers: x,y,w,h");
            }
            let mask = viode_core::Mask {
                region: [parts[0], parts[1], parts[2], parts[3]],
                kind: kind.clone(),
                follow,
            };
            viode_core::mask::validate(&mask)?;
            c.mask = Some(mask);
            println!(
                "clip {index} mask {kind} at {region_str}{}",
                if follow { " (following)" } else { "" }
            );
            Ok(())
        }),
        Cmd::Mend { index, dur } => {
            let project_dir = cli
                .project
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."))
                .to_path_buf();
            with_project(&cli.project, |p| {
                let dur = Time::parse(&dur)?;
                let i = viode_core::mend::mend_at(p, &project_dir, index, dur)?;
                println!("mended the cut with a {dur} morph (clip {i})");
                Ok(())
            })
        }
        Cmd::Bundle { path, track, at } => with_project(&cli.project, |p| {
            let file = if path.is_dir() { path.join(PROJECT_FILE) } else { path.clone() };
            let file = file.canonicalize().with_context(|| format!("no project at {}", path.display()))?;
            let sub = Project::load(&file)?;
            let dur = sub.total_duration();
            if dur == Time::ZERO {
                bail!("bundled project {} has an empty timeline", file.display());
            }
            let mut clip = Clip::media(file.clone(), Time::ZERO, dur);
            if track != 0 {
                clip.at = Some(Time::parse(
                    at.as_deref().context("overlay tracks need --at")?,
                )?);
            }
            let t = ops::track_mut(p, track)?;
            ops::add(t, clip)?;
            println!(
                "bundled {} ({dur}) as a clip on track {track}",
                sub.project.name
            );
            Ok(())
        }),
        Cmd::Match { index, to, track, to_track } => {
            let project_dir = cli
                .project
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."))
                .to_path_buf();
            with_project(&cli.project, |p| {
                let mid = |c: &Clip| (c.in_.0 as f64 + c.out.0 as f64) / 2.0 / 1e9;
                let abs = |src: &Path| if src.is_absolute() { src.to_path_buf() } else { project_dir.join(src) };
                let tgt = ops::track(p, track)?.clips.get(index).context("clip index out of range")?.clone();
                let rt = to_track.unwrap_or(track);
                let rf = ops::track(p, rt)?.clips.get(to).context("reference index out of range")?.clone();
                let tgt_stats = viode_core::match_grade::frame_stats(&abs(&tgt.src), mid(&tgt))?;
                let ref_stats = viode_core::match_grade::frame_stats(&abs(&rf.src), mid(&rf))?;
                let (brightness, saturation) = viode_core::match_grade::plan(&tgt_stats, &ref_stats);
                let t = ops::track_mut(p, track)?;
                let c = t.clips.get_mut(index).unwrap();
                let mut g = c.color.clone().unwrap_or(viode_core::ColorGrade {
                    brightness: None, contrast: None, saturation: None, hue: None, gamma: None,
                });
                g.brightness = (brightness.abs() > 1e-3).then_some(brightness);
                g.saturation = ((saturation - 1.0).abs() > 1e-3).then_some(saturation);
                c.color = Some(g);
                println!(
                    "matched clip {index} to clip {to}: brightness {brightness:+.3}, saturation x{saturation:.3}"
                );
                Ok(())
            })
        }
        Cmd::Matte { track, index, method } => with_project(&cli.project, |p| {
            if !["green", "blue", "off"].contains(&method.as_str()) {
                bail!("unknown matte {method:?} (green, blue, off)");
            }
            if track == 0 {
                bail!("matte applies to overlay clips (the tracks below show through)");
            }
            let t = ops::track_mut(p, track)?;
            let c = t.clips.get_mut(index).context("clip index out of range")?;
            c.matte = (method != "off").then(|| method.clone());
            println!("track {track} clip {index} matte {method}");
            Ok(())
        }),
        Cmd::Refit { track, index, to, fade } => {
            let project_dir = cli
                .project
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."))
                .to_path_buf();
            with_project(&cli.project, |p| {
                let target = Time::parse(&to)?;
                let fade = Time::from_secs_f64(fade)?;
                let plan =
                    viode_core::refit::refit(p, &project_dir, track, index, target, fade)?;
                match plan {
                    viode_core::refit::RefitPlan::Cut { from, to } => {
                        println!("refit: cut source {from} - {to}, crossfaded seam")
                    }
                    viode_core::refit::RefitPlan::Repeat { from, to } => {
                        println!("refit: repeated source {from} - {to}, crossfaded seam")
                    }
                }
                Ok(())
            })
        }
        Cmd::Clean { index, strength, off, track } => with_project(&cli.project, |p| {
            if !off && !(0.01..=97.0).contains(&strength) {
                bail!("strength {strength} out of range (0.01..=97 dB)");
            }
            let t = ops::track_mut(p, track)?;
            let c = t.clips.get_mut(index).context("clip index out of range")?;
            c.clean = (!off).then_some(strength);
            println!(
                "clip {index} audio cleanup {}",
                if off { "off".to_string() } else { format!("on ({strength} dB)") }
            );
            Ok(())
        }),
        Cmd::Duck { track, amount, threshold } => {
            let project_dir = cli
                .project
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."))
                .to_path_buf();
            with_project(&cli.project, |p| {
                if !(0.0..=1.0).contains(&amount) {
                    bail!("amount {amount} out of range [0, 1]");
                }
                let opts = viode_core::duck::DuckOptions {
                    amount,
                    threshold_db: threshold,
                    ..Default::default()
                };
                let ducks = viode_core::duck::duck(p, &project_dir, track, &opts)?;
                println!(
                    "ducked track {track} under {ducks} speech window(s) \
                     (volume keyframes — inspect with `viode keys`)"
                );
                Ok(())
            })
        }
        Cmd::Mark { at, text, rm } => with_project(&cli.project, |p| {
            if let Some(i) = rm {
                if i >= p.markers.len() {
                    bail!("marker {i} out of range ({} markers)", p.markers.len());
                }
                let m = p.markers.remove(i);
                println!("removed marker {i} ({} {:?})", m.at, m.text);
                return Ok(());
            }
            let at = Time::parse(at.as_deref().context("give a time, or --rm <i>")?)?;
            let text = if text.is_empty() {
                format!("marker {}", p.markers.len())
            } else {
                text.join(" ")
            };
            p.markers.push(viode_core::Marker { at, text: text.clone(), color: None });
            p.markers.sort_by_key(|m| m.at.0);
            println!("marker at {at}: {text}");
            Ok(())
        }),
        Cmd::Marks => {
            let p = Project::load(&cli.project)?;
            if p.markers.is_empty() {
                println!("no markers (add with `viode mark <time> <text>`)");
            }
            for (i, m) in p.markers.iter().enumerate() {
                println!("[{i:>3}] {}  {}", m.at, m.text);
            }
            Ok(())
        }
        Cmd::Steady { index, smoothing, off, track } => with_project(&cli.project, |p| {
            if !off && !viode_core::steady::vidstab_available() {
                bail!(
                    "stabilization needs ffmpeg built with vidstab (libvidstab); \
                     run `viode doctor`"
                );
            }
            if !(1..=100).contains(&smoothing) {
                bail!("smoothing {smoothing} out of range (1..=100)");
            }
            let t = ops::track_mut(p, track)?;
            let c = t.clips.get_mut(index).context("clip index out of range")?;
            c.steady = (!off).then_some(smoothing);
            println!(
                "clip {index} stabilization {}",
                if off { "off".to_string() } else { format!("on (smoothing {smoothing})") }
            );
            Ok(())
        }),
        Cmd::Freeze { at, dur } => {
            let project_dir = cli
                .project
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."))
                .to_path_buf();
            with_project(&cli.project, |p| {
                let at = Time::parse(&at)?;
                let dur = Time::parse(&dur)?;
                let i = viode_core::freeze::freeze_at(p, &project_dir, at, dur)?;
                println!("froze frame at {at} for {dur} (clip {i})");
                Ok(())
            })
        }
        Cmd::Ramp { index, from, to, steps, track } => with_project(&cli.project, |p| {
            let t = ops::track_mut(p, track)?;
            ops::ramp(t, index, from, to, steps)?;
            println!("clip {index} ramped {from}x -> {to}x over {steps} segments");
            Ok(())
        }),
        Cmd::Roll { index, delta } => with_project(&cli.project, |p| {
            ops::roll(p.main_mut(), index, (delta * 1e9) as i64)?;
            println!("rolled boundary {index} by {delta}s (total {})", p.total_duration());
            Ok(())
        }),
        Cmd::Slip { index, delta } => with_project(&cli.project, |p| {
            ops::slip(p.main_mut(), index, (delta * 1e9) as i64)?;
            println!("slipped clip {index} by {delta}s");
            Ok(())
        }),
        Cmd::Slide { index, delta } => with_project(&cli.project, |p| {
            ops::slide(p.main_mut(), index, (delta * 1e9) as i64)?;
            println!("slid clip {index} by {delta}s (total {})", p.total_duration());
            Ok(())
        }),
        Cmd::Bench { file, secs } => cmd_bench(&file, secs),
        Cmd::Doctor => cmd_doctor(),
        Cmd::Play { from } => {
            let project = Project::load(&cli.project)?;
            let dir = project_dir(&cli.project);
            println!("live preview — close the window or wait for the end");
            let start = Time::parse(&from)?;
            viode_core::run_gui(move || viode_core::preview_play(&project, &dir, start))?;
            Ok(())
        }
        Cmd::Queue { cmd } => cmd_queue(&cli.project, cmd),
        Cmd::Media { cmd } => cmd_media(&cli.project, cmd),
        Cmd::Relink { dir } => with_project(&cli.project, |p| {
            let pdir = project_dir(&cli.project);
            let n = viode_core::media::relink(p, &pdir, &dir);
            let still = viode_core::media::missing(p, &pdir).len();
            println!("relinked {n} clip(s); {still} still missing");
            Ok(())
        }),
        Cmd::Gain { index, volume, track } => with_project(&cli.project, |p| {
            if !(0.0..=10.0).contains(&volume) {
                bail!("volume {volume} out of range (0..10, 1.0 = unity)");
            }
            let t = ops::track_mut(p, track)?;
            let c = t.clips.get_mut(index).with_context(|| format!("clip {index} out of range"))?;
            c.volume = (volume != 1.0).then_some(volume);
            println!("track {track} clip {index} gain {volume}");
            Ok(())
        }),
        Cmd::Pan { index, pan, track } => with_project(&cli.project, |p| {
            if !(-1.0..=1.0).contains(&pan) {
                bail!("pan {pan} out of range (-1..1)");
            }
            let t = ops::track_mut(p, track)?;
            let c = t.clips.get_mut(index).with_context(|| format!("clip {index} out of range"))?;
            c.pan = (pan != 0.0).then_some(pan);
            println!("track {track} clip {index} pan {pan}");
            Ok(())
        }),
        Cmd::Key { index, prop, at, value, track } => with_project(&cli.project, |p| {
            if !["volume", "alpha", "x", "y", "scale"].contains(&prop.as_str()) {
                bail!("unknown property {prop:?} (volume, alpha, x, y, scale)");
            }
            if value < 0.0 {
                bail!("keyframe values must be >= 0");
            }
            let at = Time::parse(&at)?;
            let t = ops::track_mut(p, track)?;
            let c = t.clips.get_mut(index).with_context(|| format!("clip {index} out of range"))?;
            c.keys.push(viode_core::Keyframe { prop: prop.clone(), at, value });
            c.keys.sort_by(|a, b| (a.prop.clone(), a.at).cmp(&(b.prop.clone(), b.at)));
            println!("clip {index}: {prop} -> {value} at source {at}");
            Ok(())
        }),
        Cmd::Keys { index, rm, track } => with_project(&cli.project, |p| {
            let t = ops::track_mut(p, track)?;
            let c = t.clips.get_mut(index).with_context(|| format!("clip {index} out of range"))?;
            if let Some(k) = rm {
                if k >= c.keys.len() {
                    bail!("keyframe {k} out of range");
                }
                let key = c.keys.remove(k);
                println!("removed {} @ {}", key.prop, key.at);
            } else {
                for (k, key) in c.keys.iter().enumerate() {
                    println!("[{k}] {} @ {} = {}", key.prop, key.at, key.value);
                }
                if c.keys.is_empty() {
                    println!("no keyframes");
                }
            }
            Ok(())
        }),
        Cmd::Fx { index, effect, track, clear } => with_project(&cli.project, |p| {
            let t = ops::track_mut(p, track)?;
            if index >= t.clips.len() {
                bail!("clip index {index} out of range");
            }
            if clear {
                t.clips[index].effects.clear();
                println!("cleared effects on track {track} clip {index}");
            } else {
                let fx = effect.context("give an effect description or --clear")?;
                t.clips[index].effects.push(fx.clone());
                println!("added {fx:?} to track {track} clip {index}");
            }
            Ok(())
        }),
        Cmd::Title { text, at, dur, font, x, y, color } => with_project(&cli.project, |p| {
            p.titles.push(Title {
                text: text.clone(),
                at: Time::parse(&at)?,
                dur: Time::parse(&dur)?,
                font,
                xpos: x,
                ypos: y,
                color,
            });
            println!("title {:?} at {at} for {dur}", text);
            Ok(())
        }),
        Cmd::Titles { rm } => with_project(&cli.project, |p| {
            if let Some(k) = rm {
                if k >= p.titles.len() {
                    bail!("title index {k} out of range");
                }
                let t = p.titles.remove(k);
                println!("removed title {:?}", t.text);
            } else {
                for (k, t) in p.titles.iter().enumerate() {
                    println!("[{k}] {} +{} {:?}", t.at, t.dur, t.text);
                }
                if p.titles.is_empty() {
                    println!("no titles");
                }
            }
            Ok(())
        }),
        Cmd::Sync { a, b, max_lag } => {
            let offset = viode_core::audio_offset(&a, &b, max_lag)?;
            println!("{offset:+.3}s ({} starts that much after {})", b.display(), a.display());
            Ok(())
        }
        Cmd::Angle { file } => cmd_angle(&cli.project, &file),
        Cmd::Take { track, start, end } => cmd_take(&cli.project, track, &start, &end),
        Cmd::Transcribe { index, model } => cmd_transcribe(&cli.project, index, model.as_deref()),
        Cmd::Captions { srt, burn, model } => {
            cmd_captions(&cli.project, srt.as_deref(), burn, model.as_deref())
        }
        Cmd::CutText { index, from, to, pad } => cmd_cut_text(&cli.project, index, from, to, pad),
        Cmd::Proxy { force } => cmd_proxy(&cli.project, force),
        Cmd::Waveform { index, width, height } => {
            let project = Project::load(&cli.project)?;
            let src = clip_source(&cli.project, &project, index)?;
            let clip = &project.main().clips[index];
            let dest = project_dir(&cli.project).join("cache").join(format!("waveform_{index}.png"));
            viode_core::waveform_png(&src, clip.in_, clip.out, &dest, width, height, "white")?;
            println!("{}", dest.display());
            Ok(())
        }
        Cmd::Thumbs { index, interval, cols } => {
            let project = Project::load(&cli.project)?;
            let src = clip_source(&cli.project, &project, index)?;
            let clip = &project.main().clips[index];
            let dest = project_dir(&cli.project).join("cache").join(format!("thumbs_{index}.png"));
            viode_core::contact_sheet_png(&src, clip.in_, clip.out, &dest, interval, cols, 256)?;
            println!("{}", dest.display());
            Ok(())
        }
        Cmd::Levels { index, window } => {
            let project = Project::load(&cli.project)?;
            let src = clip_source(&cli.project, &project, index)?;
            let dir = project_dir(&cli.project);
            let scan = viode_core::audio_scan(
                &dir,
                &src,
                viode_core::DEFAULT_NOISE_DB,
                viode_core::DEFAULT_MIN_SILENCE,
                window,
            )?;
            for (at, db) in scan.levels {
                println!("{at}  {db:>7.1} dB");
            }
            Ok(())
        }
        Cmd::Silences { index, threshold, min } => cmd_silences(&cli.project, index, threshold, min),
        Cmd::CutSilences { index, threshold, min, pad } => {
            cmd_cut_silences(&cli.project, index, threshold, min, pad)
        }
        Cmd::Scenes { index, threshold } => cmd_scenes(&cli.project, index, threshold),
        Cmd::SplitScenes { index, threshold } => with_project(&cli.project, |p| {
            let src = analysis_source(&cli.project, p, index)?;
            let scenes = viode_core::detect_scenes(&src, threshold)?;
            let n = ops::split_at_source_times(p.main_mut(), index, &scenes)?;
            println!("split clip {index} into {n} segments at {} scene changes", scenes.len());
            Ok(())
        }),
        Cmd::Tui => viode_tui::run(&cli.project),
        // The GUI must NOT go through run_gui: eframe/winit owns the Cocoa
        // main loop itself (macos_main would run it off the main thread,
        // and winit aborts when its EventLoop is created anywhere else).
        Cmd::Gui { path } => {
            // App-launcher and file-manager starts land here with no useful
            // working directory: no path and no project.viode in cwd means
            // the welcome screen, not an error.
            let target = match path {
                Some(p) => Some(if p.is_dir() { p.join(PROJECT_FILE) } else { p }),
                None => cli.project.exists().then(|| cli.project.clone()),
            };
            match target {
                Some(file) => viode_gui::run(&file),
                None => viode_gui::run_welcome(),
            }
        }
        Cmd::Serve { mcp } => {
            if !mcp {
                bail!("only --mcp is supported for now (viode serve --mcp)");
            }
            let initial = cli.project.exists().then(|| cli.project.clone());
            viode_mcp::serve(initial)
        }
        Cmd::Render { output, smart, preset, codec, bitrate, reframe, smooth } => cmd_render(
            &cli.project,
            output,
            smart,
            preset.as_deref(),
            codec.as_deref(),
            bitrate,
            reframe,
            smooth,
        ),
    }
}

fn parse_opt(s: Option<&str>) -> Result<Option<Time>> {
    Ok(match s {
        Some(s) => Some(Time::parse(s)?),
        None => None,
    })
}

fn project_dir(project_file: &Path) -> PathBuf {
    project_file
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn with_project(path: &Path, f: impl FnOnce(&mut Project) -> Result<()>) -> Result<()> {
    let mut project = Project::load(path)?;
    f(&mut project)?;
    project.save(path)?;
    Ok(())
}

fn cmd_new(name: &str, fps: f64, res: &str) -> Result<()> {
    let (w, h) = res
        .split_once('x')
        .and_then(|(w, h)| Some((w.parse::<u32>().ok()?, h.parse::<u32>().ok()?)))
        .with_context(|| format!("invalid --res {res:?}, expected WIDTHxHEIGHT"))?;

    Project::init(&PathBuf::from(name), fps, [w, h])?;
    println!("created {name}/ ({w}x{h} @ {fps} fps)");
    Ok(())
}

fn cmd_probe(file: &Path) -> Result<()> {
    let info = probe(file)?;
    println!("{}", file.display());
    println!("  duration  {}", info.duration);
    if let (Some(w), Some(h)) = (info.width, info.height) {
        let fps = info.fps.map(|f| format!(" @ {f:.3} fps")).unwrap_or_default();
        println!("  video     {w}x{h}{fps} ({})", info.video_codec.as_deref().unwrap_or("?"));
    }
    if let Some(a) = &info.audio_codec {
        println!("  audio     {a}");
    }
    Ok(())
}

/// Copy a file into media/ unless it is already inside the project dir.
fn bring_in(dir: &Path, src: &Path) -> Result<PathBuf> {
    let outside = fs::canonicalize(src)
        .ok()
        .and_then(|c| fs::canonicalize(dir).ok().map(|d| !c.starts_with(d)))
        .unwrap_or(false);
    let rel = viode_core::media::bring_in(dir, src)?;
    if outside {
        println!("imported {} -> {}", src.display(), rel.display());
    }
    Ok(rel)
}

fn cmd_import(project_file: &Path, files: &[PathBuf]) -> Result<()> {
    Project::load(project_file)?;
    let dir = project_dir(project_file);
    for file in files {
        let rel = bring_in(&dir, file)?;
        let info = probe(&dir.join(&rel))?;
        println!("  {} ({})", rel.display(), info.duration);
    }
    Ok(())
}

fn cmd_add(
    project_file: &Path,
    src: &Path,
    in_: Option<&str>,
    out: Option<&str>,
    track: usize,
    at: Option<&str>,
) -> Result<()> {
    let dir = project_dir(project_file);
    let mut project = Project::load(project_file)?;

    let rel = bring_in(&dir, src)?;
    let info = viode_core::probe::probe_cached(&dir, &dir.join(&rel))?;

    let in_ = parse_opt(in_)?.unwrap_or(Time::ZERO);
    let out = parse_opt(out)?.unwrap_or(info.duration);
    if out > info.duration {
        bail!("out {} beyond source duration {}", out, info.duration);
    }
    let at = parse_opt(at)?;
    if track > 0 && at.is_none() {
        bail!("overlay tracks need --at <time> (timeline position)");
    }

    let mut clip = Clip::media(rel.clone(), in_, out);
    clip.at = if track == 0 { None } else { at };
    ops::add(ops::track_mut(&mut project, track)?, clip)?;
    project.save(project_file)?;
    println!(
        "track {track} [{}] {} [{}..{}] (timeline: {})",
        ops::track(&project, track)?.clips.len() - 1,
        rel.display(),
        in_,
        out,
        project.total_duration()
    );
    Ok(())
}

fn cmd_track(project_file: &Path, cmd: TrackCmd) -> Result<()> {
    with_project(project_file, |p| match cmd {
        TrackCmd::Add { name, kind } => {
            let kind = match kind.as_str() {
                "av" => TrackKind::Av,
                "video" => TrackKind::Video,
                "audio" => TrackKind::Audio,
                other => bail!("unknown kind {other:?} (av, video, audio)"),
            };
            p.tracks.push(Track::new(&name, kind));
            println!("track {} added: {name} ({kind:?})", p.tracks.len() - 1);
            Ok(())
        }
        TrackCmd::Ls => {
            for (i, t) in p.tracks.iter().enumerate() {
                println!(
                    "{i}  {:<12} {:<6} {:<9} {} clips",
                    t.name,
                    format!("{:?}", t.kind).to_lowercase(),
                    if t.enabled { "enabled" } else { "disabled" },
                    t.clips.len(),
                );
            }
            Ok(())
        }
        TrackCmd::On { index } | TrackCmd::Off { index } => {
            let on = matches!(cmd, TrackCmd::On { .. });
            if index == 0 {
                bail!("the main track can't be disabled");
            }
            ops::track_mut(p, index)?.enabled = on;
            println!("track {index} {}", if on { "enabled" } else { "disabled" });
            Ok(())
        }
    })
}

fn cmd_angle(project_file: &Path, file: &Path) -> Result<()> {
    let dir = project_dir(project_file);
    let mut project = Project::load(project_file)?;
    let main_clip = project
        .main()
        .clips
        .first()
        .context("add main footage before angles")?
        .clone();
    let reference = dir.join(&main_clip.src);

    let rel = bring_in(&dir, file)?;
    let angle_path = dir.join(&rel);
    let info = viode_core::probe::probe_cached(&dir, &angle_path)?;
    let offset = viode_core::audio_offset(&reference, &angle_path, 60.0)?;

    // offset > 0: the angle's audio begins AFTER the reference's — its
    // recording started late, so place it later on the timeline.
    // offset < 0: the angle started early — skip its head instead.
    let mut clip = Clip::media(rel.clone(), Time::ZERO, info.duration);
    if offset >= 0.0 {
        clip.at = Some(Time::from_secs_f64(offset)?);
    } else {
        clip.in_ = Time::from_secs_f64(-offset)?;
        clip.at = Some(Time::ZERO);
    }

    let n = project.tracks.len();
    let mut track = Track::new(&format!("angle{n}"), TrackKind::Av);
    track.enabled = false; // waits for `viode take`
    track.clips.push(clip);
    project.tracks.push(track);
    project.save(project_file)?;
    println!(
        "track {n} (angle{n}): {} synced, offset {offset:+.3}s — use `viode take {n} <start> <end>`",
        rel.display()
    );
    Ok(())
}

fn cmd_take(project_file: &Path, track_idx: usize, start: &str, end: &str) -> Result<()> {
    let mut project = Project::load(project_file)?;
    if track_idx == 0 {
        bail!("take copies FROM an angle track (1+) onto the main track");
    }
    let (start, end) = (Time::parse(start)?, Time::parse(end)?);
    let angle = ops::track(&project, track_idx)?;
    let clip = angle.clips.first().context("angle track has no clip")?;
    let (a_start, a_end) = clip.span();
    if start < a_start || end > a_end {
        bail!("angle {track_idx} only covers {a_start}..{a_end}");
    }
    let mut take = clip.clone();
    take.in_ = clip.in_ + (start - a_start);
    take.out = take.in_ + (end - start);
    ops::replace_range(project.main_mut(), start, end, take)?;
    project.save(project_file)?;
    println!(
        "took {start}..{end} from track {track_idx} (timeline still {})",
        project.total_duration()
    );
    Ok(())
}

fn transcript_path(dir: &Path, index: usize) -> PathBuf {
    dir.join("cache").join(format!("transcript_{index}.json"))
}

fn cmd_captions(
    project_file: &Path,
    srt: Option<&Path>,
    burn: bool,
    model: Option<&Path>,
) -> Result<()> {
    let mut project = Project::load(project_file)?;
    let dir = project_dir(project_file);
    // Every distinct source on the main track gets one cached transcript.
    // Freeze stills are silent by construction — skip them.
    let mut sources: Vec<PathBuf> = Vec::new();
    for clip in &project.main().clips {
        if !clip.src.starts_with("media/freeze") && !sources.contains(&clip.src) {
            sources.push(clip.src.clone());
        }
    }
    if sources.is_empty() {
        bail!("the timeline has no clips to caption");
    }
    let mut captions = Vec::new();
    for src in &sources {
        let abs = if src.is_absolute() { src.clone() } else { dir.join(src) };
        let stem = src.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let cache = dir.join("cache").join(format!("captions-{stem}.json"));
        let segments: Vec<viode_core::Segment> = if cache.exists() {
            serde_json::from_str(&fs::read_to_string(&cache)?)?
        } else {
            let segs = viode_core::transcribe(&abs, &dir.join("cache"), model)?;
            fs::write(&cache, serde_json::to_string_pretty(&segs)?)?;
            segs
        };
        captions.extend(viode_core::captions::map_segments(&project, src, &segments));
    }
    captions.sort_by_key(|c| c.start.0);
    if captions.is_empty() {
        bail!("no speech found — nothing to caption");
    }
    if let Some(srt_path) = srt {
        fs::write(srt_path, viode_core::captions::to_srt(&captions))?;
        println!("wrote {} captions to {}", captions.len(), srt_path.display());
    }
    if burn {
        let n = viode_core::captions::burn(&mut project, &captions);
        project.save(project_file)?;
        println!("burned {n} captions in as lower-third titles");
    }
    if srt.is_none() && !burn {
        for c in &captions {
            println!("{} — {}  {}", c.start, c.end, c.text);
        }
        println!(
            "{} captions (use --srt file.srt and/or --burn to deliver them)",
            captions.len()
        );
    }
    Ok(())
}

fn cmd_transcribe(project_file: &Path, index: usize, model: Option<&Path>) -> Result<()> {
    let project = Project::load(project_file)?;
    let dir = project_dir(project_file);
    let src = clip_source(project_file, &project, index)?;
    let segments = viode_core::transcribe(&src, &dir.join("cache"), model)?;
    fs::write(
        transcript_path(&dir, index),
        serde_json::to_string_pretty(&segments)?,
    )?;
    for (k, s) in segments.iter().enumerate() {
        println!("[{k:>3}] {} — {}  {}", s.start, s.end, s.text);
    }
    println!(
        "{} segments (source time) — cut with `viode cut-text {index} <from> <to>`",
        segments.len()
    );
    Ok(())
}

fn cmd_cut_text(project_file: &Path, index: usize, from: usize, to: usize, pad: f64) -> Result<()> {
    let dir = project_dir(project_file);
    let path = transcript_path(&dir, index);
    let json = fs::read_to_string(&path)
        .with_context(|| format!("no transcript — run `viode transcribe {index}` first"))?;
    let segments: Vec<viode_core::Segment> = serde_json::from_str(&json)?;
    if from > to || to >= segments.len() {
        bail!("segment range {from}..={to} out of range (0..{})", segments.len() - 1);
    }
    let ranges: Vec<(Time, Time)> = segments[from..=to]
        .iter()
        .map(|s| (s.start, s.end))
        .collect();
    let removed_text: Vec<&str> = segments[from..=to].iter().map(|s| s.text.as_str()).collect();

    let mut project = Project::load(project_file)?;
    let stats = ops::remove_source_ranges(
        project.main_mut(),
        index,
        &ranges,
        Time::from_secs_f64(pad)?,
    )?;
    project.save(project_file)?;
    println!(
        "cut {} ({} segments kept): {:?}",
        stats.removed,
        stats.segments_kept,
        removed_text.join(" ")
    );
    Ok(())
}

/// Absolute path to a main-track clip's source file, with a range check.
fn clip_source(project_file: &Path, project: &Project, index: usize) -> Result<PathBuf> {
    let clip = project
        .main()
        .clips
        .get(index)
        .with_context(|| format!("clip index {index} out of range"))?;
    Ok(project_dir(project_file).join(&clip.src))
}

/// O3: video ANALYSIS runs on the 540p proxy when one exists — scene
/// scores don't need 4K pixels. Audio analysis stays on originals.
fn analysis_source(project_file: &Path, project: &Project, index: usize) -> Result<PathBuf> {
    let clip = project
        .main()
        .clips
        .get(index)
        .with_context(|| format!("clip index {index} out of range"))?;
    let dir = project_dir(project_file);
    Ok(viode_core::proxy_for(&dir, &clip.src).unwrap_or_else(|| dir.join(&clip.src)))
}

fn cmd_silences(project_file: &Path, index: usize, threshold: f64, min: f64) -> Result<()> {
    let project = Project::load(project_file)?;
    let src = clip_source(project_file, &project, index)?;
    let clip = &project.main().clips[index];
    let dir = project_dir(project_file);
    let silences = viode_core::audio_scan(&dir, &src, threshold, min, viode_core::DEFAULT_LEVEL_WINDOW)?.silences;
    let in_clip: Vec<_> = silences
        .iter()
        .filter(|(s, e)| *e > clip.in_ && *s < clip.out)
        .collect();
    if in_clip.is_empty() {
        println!("no silences ≥ {min}s below {threshold} dB in clip {index}");
        return Ok(());
    }
    println!("{:<13} {:<13} len", "start", "end");
    for (s, e) in &in_clip {
        println!("{:<13} {:<13} {}", s.to_string(), e.to_string(), *e - *s);
    }
    println!("{} silences (source time within clip [{}..{}])", in_clip.len(), clip.in_, clip.out);
    Ok(())
}

fn cmd_cut_silences(
    project_file: &Path,
    index: usize,
    threshold: f64,
    min: f64,
    pad: f64,
) -> Result<()> {
    let mut project = Project::load(project_file)?;
    let src = clip_source(project_file, &project, index)?;
    let dir = project_dir(project_file);
    let silences =
        viode_core::audio_scan(&dir, &src, threshold, min, viode_core::DEFAULT_LEVEL_WINDOW)?.silences;
    if silences.is_empty() {
        println!("no silences to cut");
        return Ok(());
    }
    let stats = ops::remove_source_ranges(
        project.main_mut(),
        index,
        &silences,
        Time::from_secs_f64(pad)?,
    )?;
    project.save(project_file)?;
    println!(
        "cut {} of silence from clip {index} ({} segments kept, timeline now {})",
        stats.removed,
        stats.segments_kept,
        project.total_duration()
    );
    Ok(())
}

fn cmd_scenes(project_file: &Path, index: usize, threshold: f64) -> Result<()> {
    let project = Project::load(project_file)?;
    let src = analysis_source(project_file, &project, index)?;
    let scenes = viode_core::detect_scenes(&src, threshold)?;
    if scenes.is_empty() {
        println!("no scene changes above {threshold} in clip {index}");
        return Ok(());
    }
    for t in &scenes {
        println!("{t}");
    }
    println!("{} scene changes (source time)", scenes.len());
    Ok(())
}

fn cmd_ls(project_file: &Path) -> Result<()> {
    let project = Project::load(project_file)?;
    for (ti, track) in project.tracks.iter().enumerate() {
        let flag = if track.enabled { "" } else { " (disabled)" };
        println!("track {ti}: {} [{:?}]{}", track.name, track.kind, flag);
        if track.clips.is_empty() {
            println!("  (empty)");
            continue;
        }
        let positions = if ti == 0 {
            track.positions()
        } else {
            track.clips.iter().map(|c| c.span().0).collect()
        };
        for (i, (clip, start)) in track.clips.iter().zip(&positions).enumerate() {
            let fx = if clip.effects.is_empty() {
                String::new()
            } else {
                format!("  fx:{}", clip.effects.len())
            };
            let fade = clip
                .transition
                .map(|t| format!("  ⤬{t}"))
                .unwrap_or_default();
            println!(
                "  {:<4} {:<13} {:<13} {} [{}..{}]{}{}",
                i,
                start.to_string(),
                clip.len().to_string(),
                clip.src.display(),
                clip.in_,
                clip.out,
                fade,
                fx,
            );
        }
    }
    for (k, t) in project.titles.iter().enumerate() {
        println!("title [{k}] {} +{} {:?}", t.at, t.dur, t.text);
    }
    println!("total {}", project.total_duration());
    Ok(())
}

/// ffmpeg waveform/vectorscope of one frame — the colorist's instruments.
use viode_core::queue::{self as rqueue, QueueJob, RenderQueue};

fn load_queue(project_file: &Path) -> Result<RenderQueue> {
    Ok(rqueue::load(&project_dir(project_file))?)
}

fn save_queue(project_file: &Path, q: &RenderQueue) -> Result<()> {
    Ok(rqueue::save(&project_dir(project_file), q)?)
}

fn cmd_queue(project_file: &Path, cmd: QueueCmd) -> Result<()> {
    match cmd {
        QueueCmd::Add { preset, codec, bitrate, output } => {
            let mut q = load_queue(project_file)?;
            q.jobs.push(QueueJob { preset, codec, bitrate, output });
            save_queue(project_file, &q)?;
            println!("queued job {} — run with `viode queue run`", q.jobs.len());
            Ok(())
        }
        QueueCmd::Ls => {
            let q = load_queue(project_file)?;
            for (i, j) in q.jobs.iter().enumerate() {
                println!(
                    "[{i}] preset={} codec={} bitrate={} output={}",
                    j.preset.as_deref().unwrap_or("-"),
                    j.codec.as_deref().unwrap_or("-"),
                    j.bitrate.map(|b| b.to_string()).unwrap_or_else(|| "-".into()),
                    j.output.as_ref().map(|o| o.display().to_string()).unwrap_or_else(|| "-".into()),
                );
            }
            if q.jobs.is_empty() {
                println!("queue empty");
            }
            Ok(())
        }
        QueueCmd::Run => {
            let q = load_queue(project_file)?;
            if q.jobs.is_empty() {
                bail!("queue empty");
            }
            let n = q.jobs.len();
            for (i, j) in q.jobs.iter().enumerate() {
                println!("== job {}/{n} ==", i + 1);
                cmd_render(
                    project_file,
                    j.output.clone(),
                    false,
                    j.preset.as_deref(),
                    j.codec.as_deref(),
                    j.bitrate,
                    false,
                    None,
                )?;
            }
            save_queue(project_file, &RenderQueue::default())?;
            println!("queue complete ({n} render(s))");
            Ok(())
        }
        QueueCmd::Clear => {
            save_queue(project_file, &RenderQueue::default())?;
            println!("queue cleared");
            Ok(())
        }
    }
}

fn cmd_media(project_file: &Path, cmd: MediaCmd) -> Result<()> {
    let project = Project::load(project_file)?;
    let dir = project_dir(project_file);
    match cmd {
        MediaCmd::Ls => {
            let mut seen = std::collections::BTreeSet::new();
            for track in &project.tracks {
                for clip in &track.clips {
                    if seen.insert(clip.src.clone()) {
                        let p = dir.join(&clip.src);
                        let status = if p.exists() {
                            viode_core::probe::probe_cached(&dir, &p)
                                .map(|i| i.duration.to_string())
                                .unwrap_or_else(|_| "unreadable".into())
                        } else {
                            "MISSING".into()
                        };
                        println!("{}  {status}", clip.src.display());
                    }
                }
            }
            Ok(())
        }
        MediaCmd::Missing => {
            let lost = viode_core::media::missing(&project, &dir);
            for (ti, ci, src) in &lost {
                println!("track {ti} clip {ci}: {}", src.display());
            }
            if lost.is_empty() {
                println!("all media present");
            } else {
                println!("{} missing — `viode relink <dir>` to reconnect", lost.len());
            }
            Ok(())
        }
    }
}

/// Opt-B honesty tool: measure both encode paths on the user's OWN
/// footage; the winner (and the env to set) is printed, never assumed.
fn cmd_bench(file: &Path, secs: u32) -> Result<()> {
    let run = |label: &str, args: &[String]| -> Result<Option<f64>> {
        let tmp = std::env::temp_dir().join(format!("viode-bench-{label}.mp4"));
        let start = std::time::Instant::now();
        let status = std::process::Command::new("ffmpeg")
            .args(["-y", "-loglevel", "error"])
            .args(args.iter().take_while(|a| **a != "-i"))
            .args(["-t", &secs.to_string(), "-i"])
            .arg(file)
            .args(args.iter().skip_while(|a| **a != "-i").skip(1))
            .arg(&tmp)
            .status()?;
        let elapsed = start.elapsed().as_secs_f64();
        let _ = fs::remove_file(&tmp);
        if status.success() {
            println!("  {label:<28} {elapsed:>7.1}s");
            Ok(Some(elapsed))
        } else {
            println!("  {label:<28}  failed (path unavailable)");
            Ok(None)
        }
    };
    println!("benchmarking {} ({secs}s sample):", file.display());
    let owned = |v: &[&str]| v.iter().map(|a| a.to_string()).collect::<Vec<_>>();
    let sw = run(
        "software (libx264)",
        &owned(&["-i", "-vf", "scale=-2:540", "-c:v", "libx264", "-crf", "28", "-preset", "veryfast", "-an"]),
    )?;
    // The candidate hardware path is per-platform (viode_core::hwaccel):
    // VA-API on Linux, VideoToolbox on macOS.
    let Some(hwdef) = viode_core::hwaccel::platform() else {
        println!("verdict: no hardware path is defined for this platform — software is the path");
        return Ok(());
    };
    let mut hw_args = owned(hwdef.decode_args);
    hw_args.push("-i".into());
    hw_args.extend(hwdef.encode_args(540));
    hw_args.push("-an".into());
    let hw = run(&format!("{} (hw decode+encode)", hwdef.label), &hw_args)?;
    match (sw, hw) {
        (Some(s), Some(h)) if h < s => println!(
            "verdict: {} wins {:.1}x on this machine — export VIODE_HWACCEL={}",
            hwdef.label, s / h, hwdef.env_value
        ),
        (Some(s), Some(h)) => println!(
            "verdict: software wins {:.1}x on this machine — leave VIODE_HWACCEL unset",
            h / s
        ),
        (Some(_), None) => println!("verdict: {} unavailable — software stays the path", hwdef.label),
        _ => bail!("benchmark could not run either path"),
    }
    Ok(())
}

fn cmd_render(
    project_file: &Path,
    output: Option<PathBuf>,
    smart: bool,
    preset: Option<&str>,
    codec: Option<&str>,
    bitrate: Option<u32>,
    reframe: bool,
    smooth: Option<u32>,
) -> Result<()> {
    let project = Project::load(project_file)?;
    if project.main().clips.is_empty() && project.tracks.len() == 1 && project.titles.is_empty() {
        bail!("timeline is empty, nothing to render");
    }
    let dir = project_dir(project_file);
    let preset = preset
        .map(|p| {
            viode_core::Preset::parse(p)
                .with_context(|| format!("unknown preset {p:?} (youtube, shorts, podcast)"))
        })
        .transpose()?;
    let codec = codec
        .map(|c| {
            viode_core::Codec::parse(c)
                .with_context(|| format!("unknown codec {c:?} (h264, hevc, av1, prores, dnxhr)"))
        })
        .transpose()?;
    if smart && (preset.is_some() || codec.is_some() || smooth.is_some()) {
        bail!("--smart can't combine with preset/codec/smooth: they re-process the master");
    }
    if preset.is_some() && codec.is_some() {
        bail!("--preset and --codec are alternatives — pick one");
    }

    let started = std::time::Instant::now();
    let name = &project.project.name;

    let needs_post = preset.is_some() || codec.is_some() || smooth.is_some();
    let master = if needs_post {
        dir.join("cache").join("master.mp4")
    } else {
        output
            .clone()
            .unwrap_or_else(|| dir.join("renders").join(format!("{name}.mp4")))
    };

    let backend: Box<dyn RenderBackend> = if smart {
        eprintln!(
            "note: smart-copy is lossless but cuts snap to source keyframes — \
             output length can differ from the timeline. Use a plain render \
             for frame accuracy."
        );
        Box::new(SmartCopyBackend)
    } else {
        Box::new(GesBackend)
    };
    backend.render(&project, &dir, &master)?;

    if reframe && preset != Some(viode_core::Preset::Shorts) {
        bail!("--reframe only applies to --preset shorts");
    }
    let final_path = if let Some(preset) = preset {
        let out = output.unwrap_or_else(|| {
            dir.join("renders")
                .join(format!("{name}-{}.{}", preset_name(preset), preset.extension()))
        });
        if reframe {
            let spans = viode_core::reframe::shorts_reframed(&master, &out)?;
            println!("reframed across {} scene(s)", spans.len());
        } else {
            viode_core::apply_preset(&master, &out, preset)?;
        }
        out
    } else if let Some(codec) = codec {
        let out = output.unwrap_or_else(|| {
            dir.join("renders")
                .join(format!("{name}-{codec:?}.{}", codec.extension()).to_lowercase())
        });
        viode_core::transcode(&master, &out, codec, bitrate)?;
        out
    } else if let Some(fps) = smooth {
        let out = output
            .unwrap_or_else(|| dir.join("renders").join(format!("{name}-smooth.mp4")));
        viode_core::smooth(&master, &out, fps)?;
        out
    } else {
        master
    };
    println!(
        "rendered {} ({}, {:.1}s{}{})",
        final_path.display(),
        project.total_duration(),
        started.elapsed().as_secs_f64(),
        if smart { ", smart-copy" } else { "" },
        preset.map(|p| format!(", {} preset", preset_name(p))).unwrap_or_default(),
    );
    Ok(())
}

fn preset_name(p: viode_core::Preset) -> &'static str {
    match p {
        viode_core::Preset::Youtube => "youtube",
        viode_core::Preset::Shorts => "shorts",
        viode_core::Preset::Podcast => "podcast",
    }
}

fn cmd_proxy(project_file: &Path, force: bool) -> Result<()> {
    let project = Project::load(project_file)?;
    let dir = project_dir(project_file);
    let mut sources: Vec<_> = project
        .tracks
        .iter()
        .flat_map(|t| t.clips.iter().map(|c| c.src.clone()))
        .collect();
    sources.sort();
    sources.dedup();
    if sources.is_empty() {
        println!("timeline references no media");
        return Ok(());
    }
    let started = std::time::Instant::now();
    // O1: per-file parallelism — each ffmpeg multithreads, a pool of 3
    // keeps the machine busy across many sources.
    let results = viode_core::build_all(&dir, &sources, force, 3);
    let mut failed = 0;
    for (src, result) in results {
        match result {
            Ok(dest) => println!(
                "{} -> {}",
                src.display(),
                dest.strip_prefix(&dir).unwrap_or(&dest).display()
            ),
            Err(e) => {
                failed += 1;
                eprintln!("{}: {e}", src.display());
            }
        }
    }
    println!(
        "{} proxie(s) in {:.1}s{}",
        sources.len() - failed,
        started.elapsed().as_secs_f64(),
        if failed > 0 { " (with failures)" } else { "" }
    );
    if failed > 0 {
        bail!("{failed} proxy build(s) failed");
    }
    Ok(())
}

fn cmd_doctor() -> Result<()> {
    let checks = viode_core::doctor::run();
    println!("engine checkup for this machine");
    for c in &checks {
        if c.ok {
            println!("  ok    {} ({})", c.feature, c.probe);
        } else {
            println!("  MISS  {} ({}) — {}", c.feature, c.probe, c.fix);
        }
    }
    let missing = checks.iter().filter(|c| !c.ok).count();
    if missing == 0 {
        println!("\nAll {} checks passed — every feature works here.", checks.len());
    } else {
        println!(
            "\n{missing} of {} checks failed — the features marked MISS will \
             error until their piece is installed.",
            checks.len()
        );
    }
    if checks.iter().any(|c| !c.ok && c.required) {
        bail!("core dependencies are missing — Viode cannot edit on this machine yet");
    }
    Ok(())
}
