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
    /// Crossfade a main-track clip with the previous one (0 clears)
    Fade { index: usize, duration: String },
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
    },
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

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
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
        Cmd::Fade { index, duration } => with_project(&cli.project, |p| {
            let d = Time::parse(&duration)?;
            let d = (d != Time::ZERO).then_some(d);
            ops::set_transition(p.main_mut(), index, d)?;
            println!(
                "clip {index} crossfade: {}",
                d.map(|d| d.to_string()).unwrap_or_else(|| "none".into())
            );
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
        Cmd::Title { text, at, dur, font } => with_project(&cli.project, |p| {
            p.titles.push(Title {
                text: text.clone(),
                at: Time::parse(&at)?,
                dur: Time::parse(&dur)?,
                font,
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
        Cmd::CutText { index, from, to, pad } => cmd_cut_text(&cli.project, index, from, to, pad),
        Cmd::Proxy { force } => cmd_proxy(&cli.project, force),
        Cmd::Waveform { index, width, height } => {
            let project = Project::load(&cli.project)?;
            let src = clip_source(&cli.project, &project, index)?;
            let clip = &project.main().clips[index];
            let dest = project_dir(&cli.project).join("cache").join(format!("waveform_{index}.png"));
            viode_core::waveform_png(&src, clip.in_, clip.out, &dest, width, height)?;
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
            for (at, db) in viode_core::audio_levels(&src, window)? {
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
            let src = clip_source_owned(&cli.project, p, index)?;
            let scenes = viode_core::detect_scenes(&src, threshold)?;
            let n = ops::split_at_source_times(p.main_mut(), index, &scenes)?;
            println!("split clip {index} into {n} segments at {} scene changes", scenes.len());
            Ok(())
        }),
        Cmd::Tui => viode_tui::run(&cli.project),
        Cmd::Serve { mcp } => {
            if !mcp {
                bail!("only --mcp is supported for now (viode serve --mcp)");
            }
            let initial = cli.project.exists().then(|| cli.project.clone());
            viode_mcp::serve(initial)
        }
        Cmd::Render { output, smart, preset } => {
            cmd_render(&cli.project, output, smart, preset.as_deref())
        }
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

    let dir = PathBuf::from(name);
    if dir.exists() {
        bail!("{name} already exists");
    }
    for sub in ["media", "renders", "cache", "proxies"] {
        fs::create_dir_all(dir.join(sub))?;
    }
    fs::write(dir.join(".gitignore"), "/renders/\n/cache/\n/proxies/\n")?;
    Project::new(name, fps, [w, h]).save(&dir.join(PROJECT_FILE))?;
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
    let canon_dir = fs::canonicalize(dir)?;
    if let Ok(canon_src) = fs::canonicalize(src) {
        if let Ok(rel) = canon_src.strip_prefix(&canon_dir) {
            return Ok(rel.to_path_buf());
        }
        let name = canon_src.file_name().context("source has no file name")?;
        let dest = dir.join("media").join(name);
        if dest.exists() {
            // Same file re-added (very normal — clips get reused): point at
            // the existing copy. A different file under the same name is a
            // real collision.
            let same = fs::metadata(&canon_src).map(|m| m.len()).ok()
                == fs::metadata(&dest).map(|m| m.len()).ok();
            if same {
                return Ok(PathBuf::from("media").join(name));
            }
            bail!(
                "media/{} already exists with different content",
                name.to_string_lossy()
            );
        }
        fs::create_dir_all(dir.join("media"))?;
        fs::copy(&canon_src, &dest)?;
        println!("imported {} -> media/{}", src.display(), name.to_string_lossy());
        return Ok(PathBuf::from("media").join(name));
    }
    bail!("{} not found", src.display())
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

fn clip_source_owned(project_file: &Path, project: &Project, index: usize) -> Result<PathBuf> {
    clip_source(project_file, project, index)
}

fn cmd_silences(project_file: &Path, index: usize, threshold: f64, min: f64) -> Result<()> {
    let project = Project::load(project_file)?;
    let src = clip_source(project_file, &project, index)?;
    let clip = &project.main().clips[index];
    let silences = viode_core::detect_silences(&src, threshold, min)?;
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
    let silences = viode_core::detect_silences(&src, threshold, min)?;
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
    let src = clip_source(project_file, &project, index)?;
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

fn cmd_render(
    project_file: &Path,
    output: Option<PathBuf>,
    smart: bool,
    preset: Option<&str>,
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
    if smart && preset.is_some() {
        bail!("--smart and --preset don't combine: presets re-process the master render");
    }

    let started = std::time::Instant::now();
    let name = &project.project.name;

    let master = match preset {
        Some(_) => dir.join("cache").join("master.mp4"),
        None => output
            .clone()
            .unwrap_or_else(|| dir.join("renders").join(format!("{name}.mp4"))),
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

    let final_path = if let Some(preset) = preset {
        let out = output.unwrap_or_else(|| {
            dir.join("renders")
                .join(format!("{name}-{}.{}", preset_name(preset), preset.extension()))
        });
        viode_core::apply_preset(&master, &out, preset)?;
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
    for src in &sources {
        let started = std::time::Instant::now();
        let dest = viode_core::build_proxy(&dir, src, force)?;
        println!(
            "{} -> {} ({:.1}s)",
            src.display(),
            dest.strip_prefix(&dir).unwrap_or(&dest).display(),
            started.elapsed().as_secs_f64()
        );
    }
    Ok(())
}
