use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};

use viode_core::{
    ops, probe, Clip, GesBackend, Project, RenderBackend, SmartCopyBackend, Time, PROJECT_FILE,
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
    /// Append a clip to the timeline (copies the file into media/ if outside
    /// the project)
    Add {
        src: PathBuf,
        /// Source in-point (default: start)
        #[arg(long = "in")]
        in_: Option<String>,
        /// Source out-point (default: end)
        #[arg(long)]
        out: Option<String>,
    },
    /// List the timeline
    Ls,
    /// Change a clip's source in/out points
    Trim {
        index: usize,
        #[arg(long = "in")]
        in_: Option<String>,
        #[arg(long)]
        out: Option<String>,
    },
    /// Split a clip at an offset from its start
    Split { index: usize, at: String },
    /// Move a clip to a new position in the sequence
    Move { from: usize, to: usize },
    /// Remove a clip from the timeline
    Rm { index: usize },
    /// Run the MCP server (stdio) — lets AI clients edit the project
    Serve {
        /// Speak the Model Context Protocol on stdin/stdout
        #[arg(long)]
        mcp: bool,
    },
    /// Render the timeline
    Render {
        /// Output path (default: renders/<name>.mp4)
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Lossless stream-copy render — near-instant, but cuts snap to
        /// keyframes
        #[arg(long)]
        smart: bool,
    },
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
        Cmd::Add { src, in_, out } => cmd_add(&cli.project, &src, in_.as_deref(), out.as_deref()),
        Cmd::Ls => cmd_ls(&cli.project),
        Cmd::Trim { index, in_, out } => {
            with_project(&cli.project, |p| {
                Ok(ops::trim(p, index, parse_opt(in_.as_deref())?, parse_opt(out.as_deref())?)?)
            })
        }
        Cmd::Split { index, at } => with_project(&cli.project, |p| {
            Ok(ops::split(p, index, Time::parse(&at)?)?)
        }),
        Cmd::Move { from, to } => {
            with_project(&cli.project, |p| Ok(ops::move_clip(p, from, to)?))
        }
        Cmd::Rm { index } => with_project(&cli.project, |p| {
            let clip = ops::remove(p, index)?;
            println!("removed [{}] {}", index, clip.src.display());
            Ok(())
        }),
        Cmd::Render { output, smart } => cmd_render(&cli.project, output, smart),
        Cmd::Serve { mcp } => {
            if !mcp {
                bail!("only --mcp is supported for now (viode serve --mcp)");
            }
            // Start with the project if one exists here; tools can also
            // project_open/project_new later.
            let initial = cli.project.exists().then(|| cli.project.clone());
            viode_mcp::serve(initial)
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
        let fps = info
            .fps
            .map(|f| format!(" @ {f:.3} fps"))
            .unwrap_or_default();
        println!("  video     {w}x{h}{fps} ({})", info.video_codec.as_deref().unwrap_or("?"));
    }
    if let Some(a) = &info.audio_codec {
        println!("  audio     {a}");
    }
    Ok(())
}

/// Copy a file into media/ unless it is already inside the project dir.
/// Returns the clip src path relative to the project dir.
fn bring_in(dir: &Path, src: &Path) -> Result<PathBuf> {
    let canon_dir = fs::canonicalize(dir)?;
    if let Ok(canon_src) = fs::canonicalize(src) {
        if let Ok(rel) = canon_src.strip_prefix(&canon_dir) {
            return Ok(rel.to_path_buf());
        }
        let name = canon_src
            .file_name()
            .context("source has no file name")?;
        let dest = dir.join("media").join(name);
        if dest.exists() {
            bail!("media/{} already exists", name.to_string_lossy());
        }
        fs::create_dir_all(dir.join("media"))?;
        fs::copy(&canon_src, &dest)?;
        println!("imported {} -> media/{}", src.display(), name.to_string_lossy());
        return Ok(PathBuf::from("media").join(name));
    }
    bail!("{} not found", src.display())
}

fn cmd_import(project_file: &Path, files: &[PathBuf]) -> Result<()> {
    Project::load(project_file)?; // just validate we're in a project
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
) -> Result<()> {
    let dir = project_dir(project_file);
    let mut project = Project::load(project_file)?;

    let rel = bring_in(&dir, src)?;
    let info = probe(&dir.join(&rel))?;

    let in_ = parse_opt(in_)?.unwrap_or(Time::ZERO);
    let out = parse_opt(out)?.unwrap_or(info.duration);
    if out > info.duration {
        bail!("out {} beyond source duration {}", out, info.duration);
    }

    let clip = Clip {
        src: rel.clone(),
        in_,
        out,
        label: None,
    };
    ops::add(&mut project, clip)?;
    project.save(project_file)?;
    println!(
        "[{}] {} [{}..{}] appended (timeline: {})",
        project.clips.len() - 1,
        rel.display(),
        in_,
        out,
        project.total_duration()
    );
    Ok(())
}

fn cmd_ls(project_file: &Path) -> Result<()> {
    let project = Project::load(project_file)?;
    if project.clips.is_empty() {
        println!("timeline empty — `viode add <file>` to get started");
        return Ok(());
    }
    let positions = project.positions();
    println!("{:<4} {:<13} {:<13} {}", "#", "start", "len", "src [in..out]");
    for (i, (clip, start)) in project.clips.iter().zip(&positions).enumerate() {
        println!(
            "{:<4} {:<13} {:<13} {} [{}..{}]",
            i,
            start.to_string(),
            clip.len().to_string(),
            clip.src.display(),
            clip.in_,
            clip.out
        );
    }
    println!("total {}", project.total_duration());
    Ok(())
}

fn cmd_render(project_file: &Path, output: Option<PathBuf>, smart: bool) -> Result<()> {
    let project = Project::load(project_file)?;
    if project.clips.is_empty() {
        bail!("timeline is empty, nothing to render");
    }
    let dir = project_dir(project_file);
    let output = output.unwrap_or_else(|| {
        dir.join("renders").join(format!("{}.mp4", project.project.name))
    });

    let started = std::time::Instant::now();
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
    backend.render(&project, &dir, &output)?;
    println!(
        "rendered {} ({}, {:.1}s{})",
        output.display(),
        project.total_duration(),
        started.elapsed().as_secs_f64(),
        if smart { ", smart-copy" } else { "" }
    );
    Ok(())
}
