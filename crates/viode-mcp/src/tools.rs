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
            "Append a clip. Files outside the project are copied into \
             media/. in/out default to the whole file. track 0 (default) is \
             the main sequence; overlay tracks also need `at`.",
            json!({
                "src": {"type": "string"},
                "in": time_schema,
                "out": time_schema,
                "track": {"type": "integer", "default": 0},
                "at": time_schema,
            }),
            &["src"],
        ),
        tool(
            "track_add",
            "Add a track: kind av (audio+video), video (overlay, keeps main \
             audio), or audio (music/VO).",
            json!({
                "name": {"type": "string"},
                "kind": {"type": "string", "enum": ["av", "video", "audio"], "default": "av"},
            }),
            &["name"],
        ),
        tool(
            "track_toggle",
            "Enable/disable a track (disabled tracks stay in the file but \
             are excluded from renders — how multicam angles wait).",
            json!({ "index": {"type": "integer"}, "enabled": {"type": "boolean"} }),
            &["index", "enabled"],
        ),
        tool(
            "fade_set",
            "Crossfade a main-track clip with the previous one (duration 0 \
             clears it).",
            json!({ "index": {"type": "integer"}, "duration": time_schema }),
            &["index", "duration"],
        ),
        tool(
            "fx_add",
            "Add a GStreamer effect to a clip, e.g. \"videobalance \
             saturation=0\" (b/w) or \"gamma gamma=1.2\".",
            json!({
                "index": {"type": "integer"},
                "effect": {"type": "string"},
                "track": {"type": "integer", "default": 0},
            }),
            &["index", "effect"],
        ),
        tool(
            "fx_clear",
            "Remove all effects from a clip.",
            json!({
                "index": {"type": "integer"},
                "track": {"type": "integer", "default": 0},
            }),
            &["index"],
        ),
        tool(
            "title_add",
            "Overlay a text title on the timeline.",
            json!({
                "text": {"type": "string"},
                "at": time_schema,
                "dur": time_schema,
                "font": {"type": "string", "description": "Pango font description, e.g. \"Sans Bold 64\""},
            }),
            &["text", "at", "dur"],
        ),
        tool(
            "title_remove",
            "Remove a title by index (see timeline_get).",
            json!({ "index": {"type": "integer"} }),
            &["index"],
        ),
        tool(
            "angle_add",
            "Multicam: add a camera angle. Syncs it to the main footage by \
             audio cross-correlation and adds it as a disabled track. Then \
             use `take` to cut to it.",
            json!({ "path": {"type": "string"} }),
            &["path"],
        ),
        tool(
            "take",
            "Multicam: replace the [start, end) timeline range of the main \
             track with the synced footage from an angle track.",
            json!({
                "track": {"type": "integer"},
                "start": time_schema,
                "end": time_schema,
            }),
            &["track", "start", "end"],
        ),
        tool(
            "transcribe",
            "Transcribe a main-track clip with whisper.cpp into timed \
             segments (source time). Enables text-based editing via text_cut.",
            json!({
                "index": {"type": "integer"},
                "model": {"type": "string", "description": "path to a ggml model; defaults to VIODE_WHISPER_MODEL"},
            }),
            &["index"],
        ),
        tool(
            "text_cut",
            "Edit video by editing text: cut transcript segments [from..=to] \
             out of a clip (run transcribe first).",
            json!({
                "index": {"type": "integer"},
                "from": {"type": "integer"},
                "to": {"type": "integer"},
                "pad": {"type": "number", "default": 0.05},
            }),
            &["index", "from", "to"],
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
            "silence_detect",
            "Find silent stretches in a clip's source audio (source-time ranges).",
            json!({
                "index": {"type": "integer"},
                "threshold_db": {"type": "number", "default": -35.0},
                "min_duration": {"type": "number", "default": 0.5, "description": "seconds"},
            }),
            &["index"],
        ),
        tool(
            "silence_cut",
            "Cut all silent stretches out of a clip — the podcast dead-air \
             remover. pad keeps a little silence at each cut for pacing.",
            json!({
                "index": {"type": "integer"},
                "threshold_db": {"type": "number", "default": -35.0},
                "min_duration": {"type": "number", "default": 0.5},
                "pad": {"type": "number", "default": 0.15, "description": "seconds kept at each edge"},
            }),
            &["index"],
        ),
        tool(
            "scene_detect",
            "Find scene changes in a clip's source video (source times).",
            json!({
                "index": {"type": "integer"},
                "threshold": {"type": "number", "default": 0.4, "description": "0.0-1.0, lower = more cuts"},
            }),
            &["index"],
        ),
        tool(
            "scene_split",
            "Split a clip at every scene change, for rough-cutting raw footage.",
            json!({
                "index": {"type": "integer"},
                "threshold": {"type": "number", "default": 0.4},
            }),
            &["index"],
        ),
        tool(
            "proxy_build",
            "Build 540p proxies for all timeline media. frame_grab and \
             render_preview automatically use proxies once built — essential \
             for long/high-res footage.",
            json!({ "force": {"type": "boolean", "default": false} }),
            &[],
        ),
        tool(
            "audio_levels",
            "RMS loudness (dBFS) per time window of a clip's source — a \
             coarse audio map (silence ≈ -100).",
            json!({
                "index": {"type": "integer"},
                "window": {"type": "number", "default": 0.5, "description": "seconds"},
            }),
            &["index"],
        ),
        tool(
            "waveform",
            "A clip's audio waveform as an image.",
            json!({ "index": {"type": "integer"} }),
            &["index"],
        ),
        tool(
            "thumbs",
            "A clip's contact sheet (one frame per interval, tiled) as an \
             image — survey footage without grabbing frames one by one.",
            json!({
                "index": {"type": "integer"},
                "interval": {"type": "number", "default": 1.0, "description": "seconds between frames"},
            }),
            &["index"],
        ),
        tool(
            "render",
            "Render the full timeline (frame-accurate GES path). Optional \
             preset finishes it for a destination: youtube (16:9, -14 LUFS), \
             shorts (1080x1920, -14 LUFS), podcast (audio-only m4a, -16 LUFS).",
            json!({
                "output": {"type": "string", "description": "defaults to renders/<name>.mp4"},
                "preset": {"type": "string", "enum": ["youtube", "shorts", "podcast"]},
            }),
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
            Ok(ops::trim(p.main_mut(), index_arg(args, "index")?, time_opt(args, "in")?, time_opt(args, "out")?)?)
        }),
        "clip_split" => edit(server, |p| {
            Ok(ops::split(p.main_mut(), index_arg(args, "index")?, time_req(args, "at")?)?)
        }),
        "clip_move" => edit(server, |p| {
            Ok(ops::move_clip(p.main_mut(), index_arg(args, "from")?, index_arg(args, "to")?)?)
        }),
        "clip_remove" => edit(server, |p| {
            ops::remove(p.main_mut(), index_arg(args, "index")?)?;
            Ok(())
        }),
        "fade_set" => edit(server, |p| {
            let d = time_req(args, "duration")?;
            let d = (d != Time::ZERO).then_some(d);
            Ok(ops::set_transition(p.main_mut(), index_arg(args, "index")?, d)?)
        }),
        "track_add" => edit(server, |p| {
            let kind = match args.get("kind").and_then(Value::as_str).unwrap_or("av") {
                "av" => viode_core::TrackKind::Av,
                "video" => viode_core::TrackKind::Video,
                "audio" => viode_core::TrackKind::Audio,
                other => bail!("unknown kind {other:?} (av, video, audio)"),
            };
            p.tracks.push(viode_core::Track::new(str_arg(args, "name")?, kind));
            Ok(())
        }),
        "track_toggle" => edit(server, |p| {
            let index = index_arg(args, "index")?;
            if index == 0 {
                bail!("the main track can't be disabled");
            }
            let enabled = args
                .get("enabled")
                .and_then(Value::as_bool)
                .context("missing boolean argument: enabled")?;
            ops::track_mut(p, index)?.enabled = enabled;
            Ok(())
        }),
        "fx_add" => edit(server, |p| {
            let track = args.get("track").and_then(Value::as_u64).unwrap_or(0) as usize;
            let index = index_arg(args, "index")?;
            let effect = str_arg(args, "effect")?.to_string();
            let t = ops::track_mut(p, track)?;
            if index >= t.clips.len() {
                bail!("clip index {index} out of range");
            }
            t.clips[index].effects.push(effect);
            Ok(())
        }),
        "fx_clear" => edit(server, |p| {
            let track = args.get("track").and_then(Value::as_u64).unwrap_or(0) as usize;
            let index = index_arg(args, "index")?;
            let t = ops::track_mut(p, track)?;
            if index >= t.clips.len() {
                bail!("clip index {index} out of range");
            }
            t.clips[index].effects.clear();
            Ok(())
        }),
        "title_add" => edit(server, |p| {
            p.titles.push(viode_core::Title {
                text: str_arg(args, "text")?.to_string(),
                at: time_req(args, "at")?,
                dur: time_req(args, "dur")?,
                font: args.get("font").and_then(Value::as_str).map(String::from),
            });
            Ok(())
        }),
        "title_remove" => edit(server, |p| {
            let index = index_arg(args, "index")?;
            if index >= p.titles.len() {
                bail!("title index {index} out of range");
            }
            p.titles.remove(index);
            Ok(())
        }),
        "angle_add" => angle_add(server, args),
        "take" => take(server, args),
        "transcribe" => transcribe_tool(server, args),
        "text_cut" => text_cut(server, args),
        "proxy_build" => proxy_build(server, args),
        "audio_levels" => audio_levels_tool(server, args),
        "waveform" => waveform_tool(server, args),
        "thumbs" => thumbs_tool(server, args),
        "silence_detect" => silence_detect(server, args),
        "silence_cut" => silence_cut(server, args),
        "scene_detect" => scene_detect(server, args),
        "scene_split" => scene_split(server, args),
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
    let tracks: Vec<Value> = project
        .tracks
        .iter()
        .enumerate()
        .map(|(ti, track)| {
            let positions: Vec<_> = if ti == 0 {
                track.positions()
            } else {
                track.clips.iter().map(|c| c.span().0).collect()
            };
            json!({
                "index": ti,
                "name": track.name,
                "kind": format!("{:?}", track.kind).to_lowercase(),
                "enabled": track.enabled,
                "clips": track.clips.iter().zip(&positions).enumerate().map(|(i, (c, start))| json!({
                    "index": i,
                    "src": c.src,
                    "in": c.in_.to_string(),
                    "out": c.out.to_string(),
                    "start": start.to_string(),
                    "len": c.len().to_string(),
                    "transition": c.transition.map(|t| t.to_string()),
                    "effects": c.effects,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({
        "name": project.project.name,
        "fps": project.project.fps,
        "resolution": project.project.resolution,
        "tracks": tracks,
        "titles": project.titles.iter().enumerate().map(|(k, t)| json!({
            "index": k, "text": t.text, "at": t.at.to_string(), "dur": t.dur.to_string(),
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
    let rel = bring_in(&dir, &src)?;

    let info = viode_core::probe::probe_cached(&dir, &dir.join(&rel))?;
    let in_ = time_opt(args, "in")?.unwrap_or(Time::ZERO);
    let out = time_opt(args, "out")?.unwrap_or(info.duration);
    if out > info.duration {
        bail!("out {out} beyond source duration {}", info.duration);
    }
    let track = args.get("track").and_then(Value::as_u64).unwrap_or(0) as usize;
    let at = time_opt(args, "at")?;
    if track > 0 && at.is_none() {
        bail!("overlay tracks need `at` (timeline position)");
    }
    let mut clip = Clip::media(rel, in_, out);
    clip.at = if track == 0 { None } else { at };
    ops::add(ops::track_mut(&mut project, track)?, clip)?;
    project.save(&file)?;
    timeline_get(server)
}

/// Clip source path + the clip itself, for analysis tools.
fn clip_and_source(server: &Server, args: &Value) -> Result<(usize, Clip, PathBuf)> {
    let (file, dir) = require_project(server)?;
    let project = Project::load(&file)?;
    let index = index_arg(args, "index")?;
    let clip = project
        .main()
        .clips
        .get(index)
        .with_context(|| format!("clip index {index} out of range"))?
        .clone();
    let src = dir.join(&clip.src);
    Ok((index, clip, src))
}

fn f64_arg(args: &Value, key: &str, default: f64) -> f64 {
    args.get(key).and_then(Value::as_f64).unwrap_or(default)
}

fn silence_detect(server: &Server, args: &Value) -> Result<Vec<Value>> {
    let (index, clip, src) = clip_and_source(server, args)?;
    let silences = viode_core::detect_silences(
        &src,
        f64_arg(args, "threshold_db", -35.0),
        f64_arg(args, "min_duration", 0.5),
    )?;
    let in_clip: Vec<Value> = silences
        .iter()
        .filter(|(s, e)| *e > clip.in_ && *s < clip.out)
        .map(|(s, e)| json!({"start": s.to_string(), "end": e.to_string(), "len": (*e - *s).to_string()}))
        .collect();
    Ok(text(serde_json::to_string_pretty(&json!({
        "clip": index,
        "silences": in_clip,
        "note": "source-time ranges; use silence_cut to remove them",
    }))?))
}

fn silence_cut(server: &mut Server, args: &Value) -> Result<Vec<Value>> {
    let (index, _, src) = clip_and_source(server, args)?;
    let silences = viode_core::detect_silences(
        &src,
        f64_arg(args, "threshold_db", -35.0),
        f64_arg(args, "min_duration", 0.5),
    )?;
    if silences.is_empty() {
        return Ok(text("no silences found — nothing cut"));
    }
    let pad = Time::from_secs_f64(f64_arg(args, "pad", 0.15))?;
    let (file, _) = require_project(server)?;
    let mut project = Project::load(&file)?;
    let stats = ops::remove_source_ranges(project.main_mut(), index, &silences, pad)?;
    project.save(&file)?;
    let mut content = text(format!(
        "cut {} of silence ({} segments kept)",
        stats.removed, stats.segments_kept
    ));
    content.extend(timeline_get(server)?);
    Ok(content)
}

fn scene_detect(server: &Server, args: &Value) -> Result<Vec<Value>> {
    let (index, clip, src) = clip_and_source(server, args)?;
    let scenes = viode_core::detect_scenes(&src, f64_arg(args, "threshold", 0.4))?;
    let in_clip: Vec<String> = scenes
        .iter()
        .filter(|t| **t > clip.in_ && **t < clip.out)
        .map(Time::to_string)
        .collect();
    Ok(text(serde_json::to_string_pretty(&json!({
        "clip": index,
        "scene_changes": in_clip,
        "note": "source times; use scene_split to cut at them",
    }))?))
}

fn scene_split(server: &mut Server, args: &Value) -> Result<Vec<Value>> {
    let (index, _, src) = clip_and_source(server, args)?;
    let scenes = viode_core::detect_scenes(&src, f64_arg(args, "threshold", 0.4))?;
    let (file, _) = require_project(server)?;
    let mut project = Project::load(&file)?;
    let n = ops::split_at_source_times(project.main_mut(), index, &scenes)?;
    project.save(&file)?;
    let mut content = text(format!("split clip {index} into {n} segments"));
    content.extend(timeline_get(server)?);
    Ok(content)
}

/// Copy a file into media/ unless already inside the project (shared by
/// clip_add and angle_add).
fn bring_in(dir: &Path, src: &Path) -> Result<PathBuf> {
    let canon_dir = std::fs::canonicalize(dir)?;
    let canon_src = std::fs::canonicalize(src)
        .with_context(|| format!("{} not found", src.display()))?;
    match canon_src.strip_prefix(&canon_dir) {
        Ok(rel) => Ok(rel.to_path_buf()),
        Err(_) => {
            let name = canon_src.file_name().context("source has no file name")?;
            let dest = dir.join("media").join(name);
            if dest.exists() {
                let same = std::fs::metadata(&canon_src).map(|m| m.len()).ok()
                    == std::fs::metadata(&dest).map(|m| m.len()).ok();
                if same {
                    return Ok(PathBuf::from("media").join(name));
                }
                bail!(
                    "media/{} already exists with different content",
                    name.to_string_lossy()
                );
            }
            std::fs::create_dir_all(dir.join("media"))?;
            std::fs::copy(&canon_src, &dest)?;
            Ok(PathBuf::from("media").join(name))
        }
    }
}

fn angle_add(server: &mut Server, args: &Value) -> Result<Vec<Value>> {
    let (file, dir) = require_project(server)?;
    let mut project = Project::load(&file)?;
    let main_clip = project
        .main()
        .clips
        .first()
        .context("add main footage before angles")?
        .clone();
    let reference = dir.join(&main_clip.src);

    let rel = bring_in(&dir, Path::new(str_arg(args, "path")?))?;
    let angle_path = dir.join(&rel);
    let info = viode_core::probe::probe_cached(&dir, &angle_path)?;
    let offset = viode_core::audio_offset(&reference, &angle_path, 60.0)?;

    let mut clip = Clip::media(rel.clone(), Time::ZERO, info.duration);
    if offset >= 0.0 {
        clip.at = Some(Time::from_secs_f64(offset)?);
    } else {
        clip.in_ = Time::from_secs_f64(-offset)?;
        clip.at = Some(Time::ZERO);
    }

    let n = project.tracks.len();
    let mut track = viode_core::Track::new(&format!("angle{n}"), viode_core::TrackKind::Av);
    track.enabled = false;
    track.clips.push(clip);
    project.tracks.push(track);
    project.save(&file)?;
    let mut content = text(format!(
        "track {n} (angle{n}): {} synced, audio offset {offset:+.3}s. \
         Use take {{track: {n}, start, end}} to cut to it.",
        rel.display()
    ));
    content.extend(timeline_get(server)?);
    Ok(content)
}

fn take(server: &mut Server, args: &Value) -> Result<Vec<Value>> {
    let (file, _) = require_project(server)?;
    let mut project = Project::load(&file)?;
    let track_idx = index_arg(args, "track")?;
    if track_idx == 0 {
        bail!("take copies FROM an angle track (1+) onto the main track");
    }
    let (start, end) = (time_req(args, "start")?, time_req(args, "end")?);
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
    project.save(&file)?;
    timeline_get(server)
}

fn transcript_path(dir: &Path, index: usize) -> PathBuf {
    dir.join("cache").join(format!("transcript_{index}.json"))
}

fn transcribe_tool(server: &mut Server, args: &Value) -> Result<Vec<Value>> {
    let (index, _, src) = clip_and_source(server, args)?;
    let (_, dir) = require_project(server)?;
    let model = args.get("model").and_then(Value::as_str).map(PathBuf::from);
    let segments = viode_core::transcribe(&src, &dir.join("cache"), model.as_deref())?;
    std::fs::write(
        transcript_path(&dir, index),
        serde_json::to_string_pretty(&segments)?,
    )?;
    let listing: Vec<Value> = segments
        .iter()
        .enumerate()
        .map(|(k, s)| {
            json!({"index": k, "start": s.start.to_string(), "end": s.end.to_string(), "text": s.text})
        })
        .collect();
    Ok(text(serde_json::to_string_pretty(&json!({
        "clip": index,
        "segments": listing,
        "note": "source time; cut ranges with text_cut {index, from, to}",
    }))?))
}

fn text_cut(server: &mut Server, args: &Value) -> Result<Vec<Value>> {
    let (file, dir) = require_project(server)?;
    let index = index_arg(args, "index")?;
    let json_text = std::fs::read_to_string(transcript_path(&dir, index))
        .with_context(|| format!("no transcript for clip {index} — run transcribe first"))?;
    let segments: Vec<viode_core::Segment> = serde_json::from_str(&json_text)?;
    let (from, to) = (index_arg(args, "from")?, index_arg(args, "to")?);
    if from > to || to >= segments.len() {
        bail!("segment range {from}..={to} out of range (0..{})", segments.len().saturating_sub(1));
    }
    let ranges: Vec<(Time, Time)> = segments[from..=to].iter().map(|s| (s.start, s.end)).collect();
    let pad = Time::from_secs_f64(f64_arg(args, "pad", 0.05))?;

    let mut project = Project::load(&file)?;
    let stats = ops::remove_source_ranges(project.main_mut(), index, &ranges, pad)?;
    project.save(&file)?;
    let mut content = text(format!(
        "cut {} across {} transcript segments ({} clip segments kept)",
        stats.removed,
        to - from + 1,
        stats.segments_kept
    ));
    content.extend(timeline_get(server)?);
    Ok(content)
}

fn png_content(bytes: &[u8], caption: String) -> Vec<Value> {
    vec![
        json!({
            "type": "image",
            "data": base64::engine::general_purpose::STANDARD.encode(bytes),
            "mimeType": "image/png",
        }),
        json!({ "type": "text", "text": caption }),
    ]
}

fn proxy_build(server: &Server, args: &Value) -> Result<Vec<Value>> {
    let (file, dir) = require_project(server)?;
    let project = Project::load(&file)?;
    let force = args.get("force").and_then(Value::as_bool).unwrap_or(false);
    let mut sources: Vec<_> = project.tracks.iter().flat_map(|t| t.clips.iter().map(|c| c.src.clone())).collect();
    sources.sort();
    sources.dedup();
    let mut lines = Vec::new();
    for src in &sources {
        let dest = viode_core::build_proxy(&dir, src, force)?;
        lines.push(format!("{} -> {}", src.display(), dest.display()));
    }
    if lines.is_empty() {
        return Ok(text("timeline references no media"));
    }
    Ok(text(lines.join("\n")))
}

fn audio_levels_tool(server: &Server, args: &Value) -> Result<Vec<Value>> {
    let (index, clip, src) = clip_and_source(server, args)?;
    let window = f64_arg(args, "window", 0.5);
    let levels: Vec<Value> = viode_core::audio_levels(&src, window)?
        .into_iter()
        .filter(|(t, _)| *t >= clip.in_ && *t < clip.out)
        .map(|(t, db)| json!({"at": t.to_string(), "rms_db": db}))
        .collect();
    Ok(text(serde_json::to_string_pretty(&json!({
        "clip": index,
        "window_seconds": window,
        "levels": levels,
        "note": "source time; rms_db near -100 is silence",
    }))?))
}

fn waveform_tool(server: &Server, args: &Value) -> Result<Vec<Value>> {
    let (index, clip, src) = clip_and_source(server, args)?;
    let (_, dir) = require_project(server)?;
    let dest = dir.join("cache").join(format!("waveform_{index}.png"));
    viode_core::waveform_png(&src, clip.in_, clip.out, &dest, 1024, 160, "white")?;
    let bytes = std::fs::read(&dest)?;
    Ok(png_content(
        &bytes,
        format!("waveform of clip {index} [{}..{}]", clip.in_, clip.out),
    ))
}

fn thumbs_tool(server: &Server, args: &Value) -> Result<Vec<Value>> {
    let (index, clip, src) = clip_and_source(server, args)?;
    let (_, dir) = require_project(server)?;
    // Prefer the proxy: contact sheets decode the whole clip range.
    let src = viode_core::proxy_for(&dir, &clip.src).unwrap_or(src);
    let interval = f64_arg(args, "interval", 1.0);
    let dest = dir.join("cache").join(format!("thumbs_{index}.png"));
    viode_core::contact_sheet_png(&src, clip.in_, clip.out, &dest, interval, 5, 256)?;
    let bytes = std::fs::read(&dest)?;
    Ok(png_content(
        &bytes,
        format!(
            "contact sheet of clip {index} [{}..{}], one frame per {interval}s, left-to-right",
            clip.in_, clip.out
        ),
    ))
}

fn frame_grab(server: &Server, args: &Value) -> Result<Vec<Value>> {
    let (file, dir) = require_project(server)?;
    let project = Project::load(&file)?;
    let at = time_req(args, "at")?;
    // The visible frame comes from the topmost enabled video-capable
    // overlay covering this time, else the main sequence.
    let mut located: Option<(std::path::PathBuf, Time, String)> = None;
    for track in project.tracks.iter().skip(1).rev() {
        if !track.enabled || track.kind == viode_core::TrackKind::Audio {
            continue;
        }
        if let Some(clip) = track.clips.iter().find(|c| {
            let (s, e) = c.span();
            at >= s && at < e
        }) {
            let src_time = clip.in_ + (at - clip.span().0);
            located = Some((clip.src.clone(), src_time, track.name.clone()));
            break;
        }
    }
    let (rel_src, src_time, from) = match located {
        Some((s, t, name)) => (s, t, name),
        None => {
            let (index, src_time) = ops::source_at(&project, at).with_context(|| {
                format!("{at} is past the end of the timeline ({})", project.total_duration())
            })?;
            (project.main().clips[index].src.clone(), src_time, format!("main clip {index}"))
        }
    };
    // Proxy when available: same picture, far cheaper seek on big footage.
    let src = viode_core::proxy_for(&dir, &rel_src).unwrap_or_else(|| dir.join(&rel_src));

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
            "text": format!("frame at {at} (from {from}, source time {src_time})"),
        }),
    ])
}

fn render_preview(server: &Server, args: &Value) -> Result<Vec<Value>> {
    let (file, dir) = require_project(server)?;
    let project = Project::load(&file)?;
    let start = time_req(args, "start")?;
    let end = time_req(args, "end")?;
    let mut sub = ops::extract_range(&project, start, end)?;

    // Previews are for looking, not delivering: use proxies where built.
    for track in &mut sub.tracks {
        for clip in &mut track.clips {
            if let Some(proxy) = viode_core::proxy_for(&dir, &clip.src) {
                clip.src = proxy;
            }
        }
    }

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
    if project.main().clips.is_empty() && project.tracks.len() == 1 && project.titles.is_empty() {
        bail!("timeline is empty, nothing to render");
    }
    let name = &project.project.name;
    let preset = match args.get("preset").and_then(Value::as_str) {
        Some(p) => Some(
            viode_core::Preset::parse(p)
                .with_context(|| format!("unknown preset {p:?} (youtube, shorts, podcast)"))?,
        ),
        None => None,
    };

    let master = match preset {
        Some(_) => dir.join("cache").join("master.mp4"),
        None => match args.get("output").and_then(Value::as_str) {
            Some(o) => dir.join(o),
            None => dir.join("renders").join(format!("{name}.mp4")),
        },
    };
    GesBackend.render(&project, &dir, &master)?;

    let final_path = if let Some(preset) = preset {
        let suffix = match preset {
            viode_core::Preset::Youtube => "youtube",
            viode_core::Preset::Shorts => "shorts",
            viode_core::Preset::Podcast => "podcast",
        };
        let out = match args.get("output").and_then(Value::as_str) {
            Some(o) => dir.join(o),
            None => dir
                .join("renders")
                .join(format!("{name}-{suffix}.{}", preset.extension())),
        };
        viode_core::apply_preset(&master, &out, preset)?;
        out
    } else {
        master
    };
    Ok(text(format!(
        "rendered {} ({})",
        final_path.display(),
        project.total_duration()
    )))
}
