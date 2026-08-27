//! Tool definitions and dispatch. Verbs mirror the CLI; the extras
//! (frame_grab, render_preview) are the tools that give the model senses.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use base64::Engine;
use serde_json::{json, Value};

use viode_core::{ops, probe, Clip, GesBackend, Project, RenderBackend, Time, PROJECT_FILE};

use crate::Server;

pub fn definitions() -> Vec<Value> {
    let time_schema = json!({
        "type": ["number", "string"],
        "description": "seconds (number) or \"[HH:]MM:SS.mmm\""
    });
    let tool = |name: &str, desc: &str, props: Value, required: &[&str]| {
        json!({
            "name": name,
            "description": desc,
            "inputSchema": {
                "type": "object",
                "properties": props,
                "required": required,
            }
        })
    };
    vec![
        tool(
            "project_new",
            "Create a new Viode project directory and make it the active project.",
            json!({
                "path": {"type": "string", "description": "directory to create"},
                "fps": {"type": "number", "default": 30},
                "width": {"type": "integer", "default": 1920},
                "height": {"type": "integer", "default": 1080},
            }),
            &["path"],
        ),
        tool(
            "project_open",
            "Open an existing Viode project directory (containing project.viode).",
            json!({ "path": {"type": "string"} }),
            &["path"],
        ),
        tool(
            "timeline_get",
            "The active project's timeline: clips with source in/out points, \
             derived start positions, and total duration.",
            json!({}),
            &[],
        ),
        tool(
            "media_probe",
            "Media file metadata: duration, resolution, fps, codecs.",
            json!({ "path": {"type": "string"} }),
            &["path"],
        ),
        tool(
            "clip_add",
            "Append a clip to the timeline. Files outside the project are \
             copied into media/. in/out default to the whole file.",
            json!({
                "src": {"type": "string"},
                "in": time_schema,
                "out": time_schema,
            }),
            &["src"],
        ),
        tool(
            "clip_trim",
            "Change a clip's source in/out points.",
            json!({
                "index": {"type": "integer"},
                "in": time_schema,
                "out": time_schema,
            }),
            &["index"],
        ),
        tool(
            "clip_split",
            "Split a clip at an offset from its own start into two clips.",
            json!({ "index": {"type": "integer"}, "at": time_schema }),
            &["index", "at"],
        ),
        tool(
            "clip_move",
            "Move a clip to a new position in the sequence.",
            json!({ "from": {"type": "integer"}, "to": {"type": "integer"} }),
            &["from", "to"],
        ),
        tool(
            "clip_remove",
            "Remove a clip from the timeline.",
            json!({ "index": {"type": "integer"} }),
            &["index"],
        ),
        tool(
            "frame_grab",
            "Grab the frame at a timeline position as an image — look at the \
             edit before judging a cut.",
            json!({ "at": time_schema }),
            &["at"],
        ),
        tool(
            "render_preview",
            "Fast render of a timeline range to cache/preview.mp4 for \
             checking a section without a full export.",
            json!({ "start": time_schema, "end": time_schema }),
            &["start", "end"],
        ),
        tool(
            "render",
            "Render the full timeline (frame-accurate GES path).",
            json!({ "output": {"type": "string", "description": "defaults to renders/<name>.mp4"} }),
            &[],
        ),
    ]
}

pub fn dispatch(server: &mut Server, name: &str, args: &Value) -> Result<Vec<Value>> {
    match name {
        "project_new" => project_new(server, args),
        "project_open" => project_open(server, args),
        "timeline_get" => timeline_get(server),
        "media_probe" => media_probe(args),
        "clip_add" => clip_add(server, args),
        "clip_trim" => edit(server, |p| {
            Ok(ops::trim(p, index_arg(args, "index")?, time_opt(args, "in")?, time_opt(args, "out")?)?)
        }),
        "clip_split" => edit(server, |p| {
            Ok(ops::split(p, index_arg(args, "index")?, time_req(args, "at")?)?)
        }),
        "clip_move" => edit(server, |p| {
            Ok(ops::move_clip(p, index_arg(args, "from")?, index_arg(args, "to")?)?)
        }),
        "clip_remove" => edit(server, |p| {
            ops::remove(p, index_arg(args, "index")?)?;
            Ok(())
        }),
        "frame_grab" => frame_grab(server, args),
        "render_preview" => render_preview(server, args),
        "render" => render(server, args),
        other => bail!("unknown tool: {other}"),
    }
}

// --- helpers ---------------------------------------------------------------

fn text(s: impl Into<String>) -> Vec<Value> {
    vec![json!({"type": "text", "text": s.into()})]
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .with_context(|| format!("missing required argument: {key}"))
}

fn index_arg(args: &Value, key: &str) -> Result<usize> {
    args.get(key)
        .and_then(Value::as_u64)
        .map(|v| v as usize)
        .with_context(|| format!("missing/invalid integer argument: {key}"))
}

fn time_from(v: &Value) -> Result<Time> {
    match v {
        Value::Number(n) => {
            let secs = n.as_f64().context("time out of range")?;
            Ok(Time::from_secs_f64(secs)?)
        }
        Value::String(s) => Ok(Time::parse(s)?),
        other => bail!("expected time as number or string, got {other}"),
    }
}

fn time_opt(args: &Value, key: &str) -> Result<Option<Time>> {
    args.get(key)
        .filter(|v| !v.is_null())
        .map(time_from)
        .transpose()
}

fn time_req(args: &Value, key: &str) -> Result<Time> {
    time_opt(args, key)?.with_context(|| format!("missing required argument: {key}"))
}

fn require_project(server: &Server) -> Result<(PathBuf, PathBuf)> {
    let file = server
        .project_file
        .clone()
        .context("no active project — call project_open or project_new first")?;
    let dir = file
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    Ok((file, dir))
}

/// Load → edit → save, then return the fresh timeline so the model always
/// sees the state it just produced.
fn edit(server: &mut Server, f: impl FnOnce(&mut Project) -> Result<()>) -> Result<Vec<Value>> {
    let (file, _) = require_project(server)?;
    let mut project = Project::load(&file)?;
    f(&mut project)?;
    project.save(&file)?;
    timeline_get(server)
}

fn timeline_json(project: &Project) -> Value {
    let positions = project.positions();
    json!({
        "name": project.project.name,
        "fps": project.project.fps,
        "resolution": project.project.resolution,
        "clips": project.clips.iter().zip(&positions).enumerate().map(|(i, (c, start))| json!({
            "index": i,
            "src": c.src,
            "in": c.in_.to_string(),
            "out": c.out.to_string(),
            "start": start.to_string(),
            "len": c.len().to_string(),
        })).collect::<Vec<_>>(),
        "total": project.total_duration().to_string(),
    })
}

// --- tools -----------------------------------------------------------------

fn project_new(server: &mut Server, args: &Value) -> Result<Vec<Value>> {
    let path = PathBuf::from(str_arg(args, "path")?);
    let fps = args.get("fps").and_then(Value::as_f64).unwrap_or(30.0);
    let width = args.get("width").and_then(Value::as_u64).unwrap_or(1920) as u32;
    let height = args.get("height").and_then(Value::as_u64).unwrap_or(1080) as u32;

    if path.exists() {
        bail!("{} already exists", path.display());
    }
    for sub in ["media", "renders", "cache", "proxies"] {
        std::fs::create_dir_all(path.join(sub))?;
    }
    std::fs::write(path.join(".gitignore"), "/renders/\n/cache/\n/proxies/\n")?;
    let name = path
        .file_name()
        .context("project path has no name")?
        .to_string_lossy()
        .to_string();
    let file = path.join(PROJECT_FILE);
    Project::new(&name, fps, [width, height]).save(&file)?;
    server.project_file = Some(file);
    Ok(text(format!(
        "created and opened {} ({width}x{height} @ {fps} fps)",
        path.display()
    )))
}

fn project_open(server: &mut Server, args: &Value) -> Result<Vec<Value>> {
    let path = PathBuf::from(str_arg(args, "path")?);
    let file = if path.ends_with(PROJECT_FILE) {
        path
    } else {
        path.join(PROJECT_FILE)
    };
    Project::load(&file)?; // validate before switching
    server.project_file = Some(file.clone());
    timeline_get(server)
}

fn timeline_get(server: &Server) -> Result<Vec<Value>> {
    let (file, _) = require_project(server)?;
    let project = Project::load(&file)?;
    Ok(text(serde_json::to_string_pretty(&timeline_json(&project))?))
}

fn media_probe(args: &Value) -> Result<Vec<Value>> {
    let path = PathBuf::from(str_arg(args, "path")?);
    let info = probe(&path)?;
    Ok(text(serde_json::to_string_pretty(&json!({
        "path": path,
        "duration": info.duration.to_string(),
        "width": info.width,
        "height": info.height,
        "fps": info.fps,
        "video_codec": info.video_codec,
        "audio_codec": info.audio_codec,
    }))?))
}

fn clip_add(server: &mut Server, args: &Value) -> Result<Vec<Value>> {
    let (file, dir) = require_project(server)?;
    let src = PathBuf::from(str_arg(args, "src")?);
    let mut project = Project::load(&file)?;

    // Same convention as the CLI: outside files get copied into media/.
    let canon_dir = std::fs::canonicalize(&dir)?;
    let canon_src = std::fs::canonicalize(&src)
        .with_context(|| format!("{} not found", src.display()))?;
    let rel = match canon_src.strip_prefix(&canon_dir) {
        Ok(rel) => rel.to_path_buf(),
        Err(_) => {
            let name = canon_src.file_name().context("source has no file name")?;
            let dest = dir.join("media").join(name);
            if dest.exists() {
                bail!("media/{} already exists", name.to_string_lossy());
            }
            std::fs::create_dir_all(dir.join("media"))?;
            std::fs::copy(&canon_src, &dest)?;
            PathBuf::from("media").join(name)
        }
    };

    let info = probe(&dir.join(&rel))?;
    let in_ = time_opt(args, "in")?.unwrap_or(Time::ZERO);
    let out = time_opt(args, "out")?.unwrap_or(info.duration);
    if out > info.duration {
        bail!("out {out} beyond source duration {}", info.duration);
    }
    ops::add(&mut project, Clip { src: rel, in_, out, label: None })?;
    project.save(&file)?;
    timeline_get(server)
}

fn frame_grab(server: &Server, args: &Value) -> Result<Vec<Value>> {
    let (file, dir) = require_project(server)?;
    let project = Project::load(&file)?;
    let at = time_req(args, "at")?;
    let (index, src_time) = ops::source_at(&project, at).with_context(|| {
        format!("{at} is past the end of the timeline ({})", project.total_duration())
    })?;
    let src = dir.join(&project.clips[index].src);

    let out = Command::new("ffmpeg")
        .args(["-v", "error", "-ss", &src_time.as_secs_f64().to_string(), "-i"])
        .arg(&src)
        .args([
            "-frames:v", "1",
            "-vf", "scale='min(640,iw)':-2",
            "-f", "image2pipe", "-vcodec", "png", "pipe:1",
        ])
        .output()?;
    if !out.status.success() || out.stdout.is_empty() {
        bail!(
            "frame grab failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(vec![
        json!({
            "type": "image",
            "data": base64::engine::general_purpose::STANDARD.encode(&out.stdout),
            "mimeType": "image/png",
        }),
        json!({
            "type": "text",
            "text": format!("frame at {at} (clip {index}, source time {src_time})"),
        }),
    ])
}

fn render_preview(server: &Server, args: &Value) -> Result<Vec<Value>> {
    let (file, dir) = require_project(server)?;
    let project = Project::load(&file)?;
    let start = time_req(args, "start")?;
    let end = time_req(args, "end")?;
    let sub = ops::extract_range(&project, start, end)?;

    let output = dir.join("cache").join("preview.mp4");
    GesBackend.render(&sub, &dir, &output)?;
    Ok(text(format!(
        "rendered preview of {start}..{end} ({}) to {}",
        sub.total_duration(),
        output.display()
    )))
}

fn render(server: &Server, args: &Value) -> Result<Vec<Value>> {
    let (file, dir) = require_project(server)?;
    let project = Project::load(&file)?;
    if project.clips.is_empty() {
        bail!("timeline is empty, nothing to render");
    }
    let output = match args.get("output").and_then(Value::as_str) {
        Some(o) => dir.join(o),
        None => dir.join("renders").join(format!("{}.mp4", project.project.name)),
    };
    GesBackend.render(&project, &dir, &output)?;
    Ok(text(format!(
        "rendered {} ({})",
        output.display(),
        project.total_duration()
    )))
}
