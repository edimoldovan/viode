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
            "gain_set",
            "Set a clip's audio gain (linear, 1.0 = unity, 0.5 ≈ -6dB).",
            json!({
                "index": {"type": "integer"},
                "volume": {"type": "number"},
                "track": {"type": "integer", "default": 0},
            }),
            &["index", "volume"],
        ),
        tool(
            "pan_set",
            "Pan a clip's audio, -1.0 (left) .. 1.0 (right).",
            json!({
                "index": {"type": "integer"},
                "pan": {"type": "number"},
                "track": {"type": "integer", "default": 0},
            }),
            &["index", "pan"],
        ),
        tool(
            "key_add",
            "Add a keyframe animating volume (0..2) or alpha (0..1, video \
             opacity) at a SOURCE time — linear interpolation between keys. \
             This is how audio ducks and video fades in/out.",
            json!({
                "index": {"type": "integer"},
                "prop": {"type": "string", "enum": ["volume", "alpha", "x", "y", "scale"]},
                "at": time_schema,
                "value": {"type": "number"},
                "track": {"type": "integer", "default": 0},
            }),
            &["index", "prop", "at", "value"],
        ),
        tool(
            "key_remove",
            "Remove keyframe number k from a clip (see timeline_get).",
            json!({
                "index": {"type": "integer"},
                "key": {"type": "integer"},
                "track": {"type": "integer", "default": 0},
            }),
            &["index", "key"],
        ),
        tool(
            "fade_set",
            "Transition a main-track clip with the previous one (duration 0 \
             clears). kind: crossfade (default), bar-wipe-lr, bar-wipe-tb, \
             box-wipe-tl, iris-rect, clock-cw12.",
            json!({
                "index": {"type": "integer"},
                "duration": time_schema,
                "kind": {"type": "string"},
            }),
            &["index", "duration"],
        ),
        tool(
            "place_set",
            "Position/scale/rotate/opacity a clip — picture-in-picture, \
             corner cams. x/y are fractions of the frame; scale 0.25 = \
             quarter-size; clear resets to full frame.",
            json!({
                "index": {"type": "integer"},
                "x": {"type": "number"}, "y": {"type": "number"},
                "scale": {"type": "number"}, "rotate": {"type": "number"},
                "opacity": {"type": "number"},
                "track": {"type": "integer", "default": 0},
                "clear": {"type": "boolean", "default": false},
            }),
            &["index"],
        ),
        tool(
            "color_set",
            "Color grade a clip: brightness (-1..1, neutral 0), contrast/\
             saturation (0..2, neutral 1), hue (-1..1), optional .cube LUT.",
            json!({
                "index": {"type": "integer"},
                "brightness": {"type": "number"}, "contrast": {"type": "number"},
                "saturation": {"type": "number"}, "hue": {"type": "number"}, "gamma": {"type": "number"},
                "lut": {"type": "string"},
                "track": {"type": "integer", "default": 0},
                "clear": {"type": "boolean", "default": false},
            }),
            &["index"],
        ),
        tool(
            "scope",
            "Waveform or vectorscope image of a frame — judge exposure and \
             color like a colorist.",
            json!({
                "index": {"type": "integer"},
                "at": time_schema,
                "kind": {"type": "string", "enum": ["waveform", "vector"], "default": "waveform"},
            }),
            &["index"],
        ),
        tool(
            "mask_set",
            "Blur or pixelate a rectangle of a clip (hide a face, a \
             screen, a plate). region = [x,y,w,h] fractions; follow makes \
             the mask track the region's content. Baked and cached; \
             clear with on:false.",
            json!({
                "index": {"type": "integer"},
                "region": {"type": "array", "items": {"type": "number"}},
                "kind": {"type": "string", "enum": ["blur", "pixelate"], "default": "blur"},
                "follow": {"type": "boolean", "default": false},
                "on": {"type": "boolean", "default": true},
                "track": {"type": "integer", "default": 0},
            }),
            &["index"],
        ),
        tool(
            "mend",
            "Smooth the jump cut before main clip `index` with a short \
             optical-flow morph (a bridge clip is generated and inserted \
             at the cut). Good for trimmed interviews.",
            json!({
                "index": {"type": "integer"},
                "dur": {"type": "string", "default": "0.25"},
            }),
            &["index"],
        ),
        tool(
            "bundle_add",
            "Add another project as ONE clip (a nested sequence). The \
             sub-project bakes to its master at render time, cached by \
             its file mtime — edit the sub-project and the parent picks \
             it up on the next render.",
            json!({
                "path": {"type": "string", "description": "sub-project directory or project.viode"},
                "track": {"type": "integer", "default": 0},
                "at": {"type": "string", "description": "timeline position (overlay tracks)"},
            }),
            &["path"],
        ),
        tool(
            "match_grade",
            "Match a clip's exposure and saturation to a reference clip \
             (signalstats on each clip's middle frame -> brightness and \
             saturation in the clip's color grade).",
            json!({
                "index": {"type": "integer"},
                "to": {"type": "integer"},
                "track": {"type": "integer", "default": 0},
                "to_track": {"type": "integer"},
            }),
            &["index", "to"],
        ),
        tool(
            "matte_set",
            "Chroma key an overlay clip: a green or blue background \
             becomes transparent so the tracks below show through. \
             method \"off\" clears.",
            json!({
                "track": {"type": "integer"},
                "index": {"type": "integer"},
                "method": {"type": "string", "enum": ["green", "blue", "off"]},
            }),
            &["track", "index", "method"],
        ),
        tool(
            "refit",
            "Retime a music overlay clip to a target duration: one seam \
             at the quietest point, rendered as a crossfade. Shortens to \
             half or stretches to double.",
            json!({
                "track": {"type": "integer"},
                "index": {"type": "integer"},
                "to": {"type": "string", "description": "target duration"},
                "fade": {"type": "number", "default": 0.5},
            }),
            &["track", "index", "to"],
        ),
        tool(
            "clean_set",
            "Voice cleanup on a clip: rumble highpass + FFT denoise \
             (ffmpeg afftdn), baked and cached audio-only. strength = \
             noise reduction in dB (~12 light, ~30 aggressive); on:false \
             clears.",
            json!({
                "index": {"type": "integer"},
                "strength": {"type": "number", "default": 12.0},
                "on": {"type": "boolean", "default": true},
                "track": {"type": "integer", "default": 0},
            }),
            &["index"],
        ),
        tool(
            "duck",
            "Duck music under dialogue: writes volume keyframes onto an \
             overlay track's clips wherever the main track carries speech \
             (from the cached loudness analysis). Rerunning re-plans.",
            json!({
                "track": {"type": "integer"},
                "amount": {"type": "number", "default": 0.25},
                "threshold": {"type": "number", "default": -35.0},
            }),
            &["track"],
        ),
        tool(
            "mark_add",
            "Add a named marker at a timeline time (a note; never renders).",
            json!({
                "at": {"type": "string", "description": "timeline time"},
                "text": {"type": "string"},
                "color": {"type": "string", "description": "#RRGGBB"},
            }),
            &["at", "text"],
        ),
        tool(
            "mark_remove",
            "Remove a marker by index (see the timeline JSON's markers list).",
            json!({ "index": {"type": "integer"} }),
            &["index"],
        ),
        tool(
            "captions",
            "Generate captions for the whole timeline from local \
             transcription (cached per source file). srt writes a sidecar \
             file; burn adds them as lower-third titles that render in \
             preview and export. With neither, returns the caption list.",
            json!({
                "srt": {"type": "string"},
                "burn": {"type": "boolean", "default": false},
            }),
            &[],
        ),
        tool(
            "steady_set",
            "Stabilize a clip's footage (ffmpeg vidstab, baked and cached \
             like LUTs). smoothing ~10 = handheld shake; 0 or absent \
             `on:false` clears.",
            json!({
                "index": {"type": "integer"},
                "smoothing": {"type": "integer", "default": 10},
                "on": {"type": "boolean", "default": true},
                "track": {"type": "integer", "default": 0},
            }),
            &["index"],
        ),
        tool(
            "freeze",
            "Frame hold: freeze the frame at a timeline time for a \
             duration. The still becomes an ordinary clip inserted at the \
             playhead (media/freeze/), so it trims and renders like any \
             footage.",
            json!({
                "at": {"type": "string", "description": "timeline time, e.g. \"1.5\" or \"00:01:30\""},
                "dur": {"type": "string", "default": "2"},
            }),
            &["at"],
        ),
        tool(
            "ramp",
            "Speed-ramp a clip: replace it with stepped segments whose \
             rates run linearly from `from` to `to` (time remapping, \
             stepped). Total SOURCE footage is preserved; timeline length \
             rescales. Rates 0.05..20, steps 2..64.",
            json!({
                "index": {"type": "integer"},
                "from": {"type": "number"},
                "to": {"type": "number"},
                "steps": {"type": "integer", "default": 8},
                "track": {"type": "integer", "default": 0},
            }),
            &["index", "from", "to"],
        ),
        tool(
            "speed_set",
            "Playback rate: 2 = double speed, 0.5 = slow motion, 1 clears. \
             Timeline length rescales automatically.",
            json!({
                "index": {"type": "integer"},
                "rate": {"type": "number"},
                "track": {"type": "integer", "default": 0},
            }),
            &["index", "rate"],
        ),
        tool(
            "roll",
            "Move the boundary between clip index-1 and index by ±seconds — \
             total duration unchanged.",
            json!({ "index": {"type": "integer"}, "delta": {"type": "number"} }),
            &["index", "delta"],
        ),
        tool(
            "slip",
            "Shift a clip's SOURCE content by ±seconds without moving its \
             timeline slot.",
            json!({ "index": {"type": "integer"}, "delta": {"type": "number"} }),
            &["index", "delta"],
        ),
        tool(
            "slide",
            "Move a clip against its neighbours by ±seconds — its content \
             untouched, total unchanged.",
            json!({ "index": {"type": "integer"}, "delta": {"type": "number"} }),
            &["index", "delta"],
        ),
        tool(
            "play",
            "Open the LIVE composited preview in a window on the user's \
             screen (no render step). Returns immediately.",
            json!({ "from": time_schema }),
            &[],
        ),
        tool(
            "ui_open",
            "Open the video editor UI for the current project: a GUI window \
             on the user's screen with the live composited preview, the \
             timeline, and the inspector. It live-reloads as MCP edits land, \
             so open it once and keep editing — the user watches the cut \
             happen. Returns immediately. When the user says \"open the UI\", \
             this is always the tool they mean.",
            json!({}),
            &[],
        ),
        tool(
            "tui_open",
            "Open the TERMINAL UI in a new terminal window. Use only when \
             the user explicitly asks for the TUI by name; for any plain \
             \"open the UI / editor\" request use ui_open instead.",
            json!({}),
            &[],
        ),
        tool(
            "media_missing",
            "Clips whose source files no longer exist on disk.",
            json!({}),
            &[],
        ),
        tool(
            "relink",
            "Reconnect missing media by filename, searching a directory \
             recursively.",
            json!({ "dir": {"type": "string"} }),
            &["dir"],
        ),
        tool(
            "queue_add",
            "Queue a render job (preset OR codec+bitrate, optional output).",
            json!({
                "preset": {"type": "string", "enum": ["youtube", "shorts", "podcast"]},
                "codec": {"type": "string", "enum": ["h264", "hevc", "av1", "prores", "dnxhr"]},
                "bitrate": {"type": "integer"},
                "output": {"type": "string"},
            }),
            &[],
        ),
        tool(
            "bench",
            "Measure software vs hardware encoding (VA-API on Linux, \
             VideoToolbox on macOS) on a sample of the given file and \
             report which path wins on THIS machine (whether to set \
             VIODE_HWACCEL).",
            json!({
                "path": {"type": "string"},
                "secs": {"type": "integer", "default": 30},
            }),
            &["path"],
        ),
        tool(
            "doctor",
            "Check which engine pieces exist on this machine (GStreamer \
             elements, ffmpeg, mpv, whisper.cpp) and which features break \
             without them. Run this when a render fails or before relying \
             on speed changes, LUTs, wipes, or transcription.",
            json!({}),
            &[],
        ),
        tool("queue_list", "List queued render jobs.", json!({}), &[]),
        tool("queue_run", "Run every queued render job in order.", json!({}), &[]),
        tool("queue_clear", "Clear the render queue.", json!({}), &[]),
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
                "x": {"type": "number", "description": "0..1 horizontal"},
                "y": {"type": "number", "description": "0..1 vertical (0.8 = lower third)"},
                "color": {"type": "string", "description": "#RRGGBB"},
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
                "codec": {"type": "string", "enum": ["h264", "hevc", "av1", "prores", "dnxhr"]},
                "bitrate": {"type": "integer", "description": "kbps (with codec)"},
                "smooth": {"type": "integer", "description": "optical-flow interpolate to this fps"},
                "reframe": {"type": "boolean", "description": "shorts only: face-detected subject crop instead of center crop"},
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
        "gain_set" => edit(server, |p| {
            let track = args.get("track").and_then(Value::as_u64).unwrap_or(0) as usize;
            let index = index_arg(args, "index")?;
            let volume = args.get("volume").and_then(Value::as_f64).context("missing volume")?;
            if !(0.0..=10.0).contains(&volume) {
                bail!("volume {volume} out of range (0..10)");
            }
            let t = ops::track_mut(p, track)?;
            let c = t.clips.get_mut(index).context("clip index out of range")?;
            c.volume = (volume != 1.0).then_some(volume);
            Ok(())
        }),
        "pan_set" => edit(server, |p| {
            let track = args.get("track").and_then(Value::as_u64).unwrap_or(0) as usize;
            let index = index_arg(args, "index")?;
            let pan = args.get("pan").and_then(Value::as_f64).context("missing pan")?;
            if !(-1.0..=1.0).contains(&pan) {
                bail!("pan {pan} out of range (-1..1)");
            }
            let t = ops::track_mut(p, track)?;
            let c = t.clips.get_mut(index).context("clip index out of range")?;
            c.pan = (pan != 0.0).then_some(pan);
            Ok(())
        }),
        "key_add" => edit(server, |p| {
            let track = args.get("track").and_then(Value::as_u64).unwrap_or(0) as usize;
            let index = index_arg(args, "index")?;
            let prop = str_arg(args, "prop")?.to_string();
            if !["volume", "alpha", "x", "y", "scale"].contains(&prop.as_str()) {
                bail!("unknown property {prop:?} (volume, alpha, x, y, scale)");
            }
            let value = args.get("value").and_then(Value::as_f64).context("missing value")?;
            if value < 0.0 {
                bail!("keyframe values must be >= 0");
            }
            let at = time_req(args, "at")?;
            let t = ops::track_mut(p, track)?;
            let c = t.clips.get_mut(index).context("clip index out of range")?;
            c.keys.push(viode_core::Keyframe { prop, at, value });
            c.keys.sort_by(|a, b| (a.prop.clone(), a.at).cmp(&(b.prop.clone(), b.at)));
            Ok(())
        }),
        "key_remove" => edit(server, |p| {
            let track = args.get("track").and_then(Value::as_u64).unwrap_or(0) as usize;
            let index = index_arg(args, "index")?;
            let k = index_arg(args, "key")?;
            let t = ops::track_mut(p, track)?;
            let c = t.clips.get_mut(index).context("clip index out of range")?;
            if k >= c.keys.len() {
                bail!("keyframe {k} out of range");
            }
            c.keys.remove(k);
            Ok(())
        }),
        "fade_set" => edit(server, |p| {
            let index = index_arg(args, "index")?;
            let d = time_req(args, "duration")?;
            let d = (d != Time::ZERO).then_some(d);
            ops::set_transition(p.main_mut(), index, d)?;
            p.main_mut().clips[index].transition_kind = args
                .get("kind")
                .and_then(Value::as_str)
                .map(String::from)
                .filter(|_| d.is_some());
            Ok(())
        }),
        "place_set" => edit(server, |p| {
            let track = args.get("track").and_then(Value::as_u64).unwrap_or(0) as usize;
            let index = index_arg(args, "index")?;
            let t = ops::track_mut(p, track)?;
            let c = t.clips.get_mut(index).context("clip index out of range")?;
            if args.get("clear").and_then(Value::as_bool).unwrap_or(false) {
                (c.pos, c.scale, c.rotate, c.opacity) = (None, None, None, None);
                return Ok(());
            }
            let (x, y) = (args.get("x").and_then(Value::as_f64), args.get("y").and_then(Value::as_f64));
            if x.is_some() || y.is_some() {
                let old = c.pos.unwrap_or([0.0, 0.0]);
                c.pos = Some([x.unwrap_or(old[0]), y.unwrap_or(old[1])]);
            }
            if let Some(v) = args.get("scale").and_then(Value::as_f64) {
                c.scale = Some(v);
            }
            if let Some(v) = args.get("rotate").and_then(Value::as_f64) {
                c.rotate = Some(v);
            }
            if let Some(v) = args.get("opacity").and_then(Value::as_f64) {
                if !(0.0..=1.0).contains(&v) {
                    bail!("opacity {v} out of range (0..1)");
                }
                c.opacity = Some(v);
            }
            Ok(())
        }),
        "color_set" => edit(server, |p| {
            let track = args.get("track").and_then(Value::as_u64).unwrap_or(0) as usize;
            let index = index_arg(args, "index")?;
            let t = ops::track_mut(p, track)?;
            let c = t.clips.get_mut(index).context("clip index out of range")?;
            if args.get("clear").and_then(Value::as_bool).unwrap_or(false) {
                c.color = None;
                c.lut = None;
                return Ok(());
            }
            let mut g = c.color.clone().unwrap_or(viode_core::ColorGrade {
                brightness: None, contrast: None, saturation: None, hue: None, gamma: None,
            });
            for (key, slot) in [
                ("brightness", &mut g.brightness),
                ("contrast", &mut g.contrast),
                ("saturation", &mut g.saturation),
                ("hue", &mut g.hue),
                ("gamma", &mut g.gamma),
            ] {
                if let Some(v) = args.get(key).and_then(Value::as_f64) {
                    *slot = Some(v);
                }
            }
            c.color = Some(g);
            if let Some(l) = args.get("lut").and_then(Value::as_str) {
                c.lut = Some(PathBuf::from(l));
            }
            Ok(())
        }),
        "mask_set" => edit(server, |p| {
            let track = args.get("track").and_then(Value::as_u64).unwrap_or(0) as usize;
            let index = index_arg(args, "index")?;
            let t = ops::track_mut(p, track)?;
            let c = t.clips.get_mut(index).context("clip index out of range")?;
            if !args.get("on").and_then(Value::as_bool).unwrap_or(true) {
                c.mask = None;
                return Ok(());
            }
            let region: Vec<f64> = args
                .get("region")
                .and_then(Value::as_array)
                .context("missing region [x,y,w,h]")?
                .iter()
                .filter_map(Value::as_f64)
                .collect();
            if region.len() != 4 {
                bail!("region must be four numbers: [x, y, w, h]");
            }
            let mask = viode_core::Mask {
                region: [region[0], region[1], region[2], region[3]],
                kind: args.get("kind").and_then(Value::as_str).unwrap_or("blur").to_string(),
                follow: args.get("follow").and_then(Value::as_bool).unwrap_or(false),
            };
            viode_core::mask::validate(&mask)?;
            c.mask = Some(mask);
            Ok(())
        }),
        "mend" => {
            let (_, dir) = require_project(server)?;
            let index = index_arg(args, "index")?;
            let dur = time_from(args.get("dur").unwrap_or(&json!("0.25")))?;
            edit(server, |p| {
                viode_core::mend::mend_at(p, &dir, index, dur)?;
                Ok(())
            })
        }
        "bundle_add" => edit(server, |p| {
            let raw = PathBuf::from(str_arg(args, "path")?);
            let file = if raw.is_dir() { raw.join(viode_core::PROJECT_FILE) } else { raw.clone() };
            let file = file
                .canonicalize()
                .with_context(|| format!("no project at {}", raw.display()))?;
            let sub = Project::load(&file)?;
            let dur = sub.total_duration();
            if dur == Time::ZERO {
                bail!("bundled project has an empty timeline");
            }
            let track = args.get("track").and_then(Value::as_u64).unwrap_or(0) as usize;
            let mut clip = viode_core::Clip::media(file, Time::ZERO, dur);
            if track != 0 {
                clip.at = Some(time_from(args.get("at").context("overlay tracks need at")?)?);
            }
            let t = ops::track_mut(p, track)?;
            Ok(ops::add(t, clip)?)
        }),
        "match_grade" => {
            let (_, dir) = require_project(server)?;
            let track = args.get("track").and_then(Value::as_u64).unwrap_or(0) as usize;
            let index = index_arg(args, "index")?;
            let to = index_arg(args, "to")?;
            let to_track = args.get("to_track").and_then(Value::as_u64).map(|v| v as usize).unwrap_or(track);
            edit(server, |p| {
                let mid = |c: &viode_core::Clip| (c.in_.0 as f64 + c.out.0 as f64) / 2.0 / 1e9;
                let abs = |src: &Path| if src.is_absolute() { src.to_path_buf() } else { dir.join(src) };
                let tgt = ops::track(p, track)?.clips.get(index).context("clip index out of range")?.clone();
                let rf = ops::track(p, to_track)?.clips.get(to).context("reference index out of range")?.clone();
                let ts = viode_core::match_grade::frame_stats(&abs(&tgt.src), mid(&tgt))?;
                let rs = viode_core::match_grade::frame_stats(&abs(&rf.src), mid(&rf))?;
                let (brightness, saturation) = viode_core::match_grade::plan(&ts, &rs);
                let t = ops::track_mut(p, track)?;
                let c = t.clips.get_mut(index).unwrap();
                let mut g = c.color.clone().unwrap_or(viode_core::ColorGrade {
                    brightness: None, contrast: None, saturation: None, hue: None, gamma: None,
                });
                g.brightness = (brightness.abs() > 1e-3).then_some(brightness);
                g.saturation = ((saturation - 1.0).abs() > 1e-3).then_some(saturation);
                c.color = Some(g);
                Ok(())
            })
        }
        "matte_set" => edit(server, |p| {
            let track = index_arg(args, "track")?;
            let index = index_arg(args, "index")?;
            let method = str_arg(args, "method")?.to_string();
            if !["green", "blue", "off"].contains(&method.as_str()) {
                bail!("unknown matte {method:?} (green, blue, off)");
            }
            if track == 0 {
                bail!("matte applies to overlay clips");
            }
            let t = ops::track_mut(p, track)?;
            let c = t.clips.get_mut(index).context("clip index out of range")?;
            c.matte = (method != "off").then(|| method.clone());
            Ok(())
        }),
        "refit" => {
            let (_, dir) = require_project(server)?;
            let track = index_arg(args, "track")?;
            let index = index_arg(args, "index")?;
            let target = time_from(args.get("to").context("missing to")?)?;
            let fade = Time::from_secs_f64(
                args.get("fade").and_then(Value::as_f64).unwrap_or(0.5),
            )?;
            edit(server, |p| {
                viode_core::refit::refit(p, &dir, track, index, target, fade)?;
                Ok(())
            })
        }
        "clean_set" => edit(server, |p| {
            let track = args.get("track").and_then(Value::as_u64).unwrap_or(0) as usize;
            let index = index_arg(args, "index")?;
            let on = args.get("on").and_then(Value::as_bool).unwrap_or(true);
            let strength = args.get("strength").and_then(Value::as_f64).unwrap_or(12.0);
            if on && !(0.01..=97.0).contains(&strength) {
                bail!("strength {strength} out of range (0.01..=97 dB)");
            }
            let t = ops::track_mut(p, track)?;
            let c = t.clips.get_mut(index).context("clip index out of range")?;
            c.clean = on.then_some(strength);
            Ok(())
        }),
        "duck" => {
            let (_, dir) = require_project(server)?;
            let track = index_arg(args, "track")?;
            let amount = args.get("amount").and_then(Value::as_f64).unwrap_or(0.25);
            let threshold = args.get("threshold").and_then(Value::as_f64).unwrap_or(-35.0);
            if !(0.0..=1.0).contains(&amount) {
                bail!("amount {amount} out of range [0, 1]");
            }
            edit(server, |p| {
                let opts = viode_core::duck::DuckOptions {
                    amount,
                    threshold_db: threshold,
                    ..Default::default()
                };
                viode_core::duck::duck(p, &dir, track, &opts)?;
                Ok(())
            })
        }
        "mark_add" => edit(server, |p| {
            let at = time_from(args.get("at").context("missing at")?)?;
            let text = str_arg(args, "text")?.to_string();
            let color = args.get("color").and_then(Value::as_str).map(str::to_string);
            p.markers.push(viode_core::Marker { at, text, color });
            p.markers.sort_by_key(|m| m.at.0);
            Ok(())
        }),
        "mark_remove" => edit(server, |p| {
            let i = index_arg(args, "index")?;
            if i >= p.markers.len() {
                bail!("marker {i} out of range ({} markers)", p.markers.len());
            }
            p.markers.remove(i);
            Ok(())
        }),
        "captions" => captions_tool(server, args),
        "steady_set" => edit(server, |p| {
            let track = args.get("track").and_then(Value::as_u64).unwrap_or(0) as usize;
            let index = index_arg(args, "index")?;
            let on = args.get("on").and_then(Value::as_bool).unwrap_or(true);
            let smoothing = args.get("smoothing").and_then(Value::as_u64).unwrap_or(10) as u32;
            if on && !(1..=100).contains(&smoothing) {
                bail!("smoothing {smoothing} out of range (1..=100)");
            }
            let t = ops::track_mut(p, track)?;
            let c = t.clips.get_mut(index).context("clip index out of range")?;
            c.steady = on.then_some(smoothing);
            Ok(())
        }),
        "freeze" => {
            let (_, dir) = require_project(server)?;
            let at = time_from(args.get("at").context("missing at")?)?;
            let dur = time_from(args.get("dur").unwrap_or(&json!("2")))?;
            edit(server, |p| {
                viode_core::freeze::freeze_at(p, &dir, at, dur)?;
                Ok(())
            })
        }
        "ramp" => edit(server, |p| {
            let track = args.get("track").and_then(Value::as_u64).unwrap_or(0) as usize;
            let index = index_arg(args, "index")?;
            let from = args.get("from").and_then(Value::as_f64).context("missing from")?;
            let to = args.get("to").and_then(Value::as_f64).context("missing to")?;
            let steps = args.get("steps").and_then(Value::as_u64).unwrap_or(8) as usize;
            let t = ops::track_mut(p, track)?;
            Ok(ops::ramp(t, index, from, to, steps)?)
        }),
        "speed_set" => edit(server, |p| {
            let track = args.get("track").and_then(Value::as_u64).unwrap_or(0) as usize;
            let index = index_arg(args, "index")?;
            let rate = args.get("rate").and_then(Value::as_f64).context("missing rate")?;
            if rate <= 0.0 || rate > 20.0 {
                bail!("rate {rate} out of range (0..20]");
            }
            let t = ops::track_mut(p, track)?;
            let c = t.clips.get_mut(index).context("clip index out of range")?;
            c.rate = (rate != 1.0).then_some(rate);
            Ok(())
        }),
        "roll" => edit(server, |p| {
            let d = (args.get("delta").and_then(Value::as_f64).context("missing delta (seconds)")? * 1e9) as i64;
            Ok(ops::roll(p.main_mut(), index_arg(args, "index")?, d)?)
        }),
        "slip" => edit(server, |p| {
            let d = (args.get("delta").and_then(Value::as_f64).context("missing delta (seconds)")? * 1e9) as i64;
            Ok(ops::slip(p.main_mut(), index_arg(args, "index")?, d)?)
        }),
        "slide" => edit(server, |p| {
            let d = (args.get("delta").and_then(Value::as_f64).context("missing delta (seconds)")? * 1e9) as i64;
            Ok(ops::slide(p.main_mut(), index_arg(args, "index")?, d)?)
        }),
        "scope" => scope_tool(server, args),
        "play" => play_tool(server, args),
        "ui_open" => ui_open(server),
        "tui_open" => tui_open(server),
        "media_missing" => media_missing(server),
        "relink" => relink_tool(server, args),
        "queue_add" => queue_add(server, args),
        "bench" => bench_tool(args),
        "doctor" => doctor_tool(),
        "queue_list" => queue_list(server),
        "queue_run" => queue_run(server),
        "queue_clear" => queue_clear(server),
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
                xpos: args.get("x").and_then(Value::as_f64),
                ypos: args.get("y").and_then(Value::as_f64),
                color: args.get("color").and_then(Value::as_str).map(String::from),
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
                    "volume": c.volume,
                    "pan": c.pan,
                    "keys": c.keys.iter().map(|k| json!({
                        "prop": k.prop, "at": k.at.to_string(), "value": k.value,
                    })).collect::<Vec<_>>(),
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
        "markers": project.markers.iter().enumerate().map(|(k, m)| json!({
            "index": k, "text": m.text, "at": m.at.to_string(),
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

    let file = Project::init(&path, fps, [width, height])?;
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
    let (_, dir) = require_project(server)?;
    let silences = viode_core::audio_scan(
        &dir,
        &src,
        f64_arg(args, "threshold_db", -35.0),
        f64_arg(args, "min_duration", 0.5),
        viode_core::DEFAULT_LEVEL_WINDOW,
    )?
    .silences;
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
    let (_, dir) = require_project(server)?;
    let silences = viode_core::audio_scan(
        &dir,
        &src,
        f64_arg(args, "threshold_db", -35.0),
        f64_arg(args, "min_duration", 0.5),
        viode_core::DEFAULT_LEVEL_WINDOW,
    )?
    .silences;
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
    let (_, dir) = require_project(server)?;
    // O3: scene scores don't need original pixels — use the proxy.
    let src = viode_core::proxy_for(&dir, &clip.src).unwrap_or(src);
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
    let (index, clip, src) = clip_and_source(server, args)?;
    let (_, dir) = require_project(server)?;
    let src = viode_core::proxy_for(&dir, &clip.src).unwrap_or(src);
    let _ = &clip;
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

fn scope_tool(server: &Server, args: &Value) -> Result<Vec<Value>> {
    let (index, _, src) = clip_and_source(server, args)?;
    let (_, dir) = require_project(server)?;
    let at = time_opt(args, "at")?.unwrap_or(Time::ZERO);
    let kind = args.get("kind").and_then(Value::as_str).unwrap_or("waveform");
    let dest = dir.join("cache").join(format!("scope_{index}.png"));
    viode_core::scope_png(&src, at, kind, &dest)?;
    let bytes = std::fs::read(&dest)?;
    Ok(png_content(&bytes, format!("{kind} scope of clip {index} at source {at}")))
}

/// Open the LIVE composited preview in a window (detached — the tool
/// returns immediately; the user closes the window).
fn play_tool(server: &Server, args: &Value) -> Result<Vec<Value>> {
    let (file, _) = require_project(server)?;
    let from = time_opt(args, "from")?.unwrap_or(Time::ZERO);
    let exe = std::env::current_exe()?;
    Command::new(exe)
        .arg("--project")
        .arg(&file)
        .arg("play")
        .arg("--from")
        .arg(from.to_string())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    Ok(text(format!(
        "live preview window opened from {from} — no render step; the user closes it"
    )))
}

/// Open the GUI editor window (detached — returns immediately; it
/// live-reloads as further MCP edits save the project).
fn ui_open(server: &Server) -> Result<Vec<Value>> {
    let (file, _) = require_project(server)?;
    let exe = std::env::current_exe()?;
    Command::new(exe)
        .arg("--project")
        .arg(&file)
        .arg("gui")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    Ok(text(
        "editor UI opened — it live-reloads as MCP edits land; the user closes it",
    ))
}

/// Open the terminal UI in a new terminal window (explicit request only).
/// The TUI needs a terminal emulator: $TERMINAL first, then well-known
/// ones, each with its own invocation shape.
fn tui_open(server: &Server) -> Result<Vec<Value>> {
    let (file, _) = require_project(server)?;
    let exe = std::env::current_exe()?;
    let mut candidates: Vec<String> = Vec::new();
    if let Ok(t) = std::env::var("TERMINAL") {
        candidates.push(t);
    }
    for t in ["alacritty", "ghostty", "kitty", "foot"] {
        if !candidates.iter().any(|c| c == t) {
            candidates.push(t.to_string());
        }
    }
    for term in &candidates {
        let base = std::path::Path::new(term)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| term.clone());
        let mut cmd = Command::new(term);
        // kitty and foot take the command positionally; the rest use -e.
        if !matches!(base.as_str(), "kitty" | "foot") {
            cmd.arg("-e");
        }
        let spawned = cmd
            .arg(&exe)
            .arg("--project")
            .arg(&file)
            .arg("tui")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        if spawned.is_ok() {
            return Ok(text(format!(
                "terminal UI opened in {base} — the user closes it"
            )));
        }
    }
    // macOS fallback: no CLI emulator answered, so hand Terminal.app an
    // executable .command script — `open -a Terminal` runs those directly,
    // and Terminal is present on every Mac.
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::fs::PermissionsExt;
        let script = std::env::temp_dir().join("viode-tui.command");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nexec {} --project {} tui\n",
                sh_quote(&exe.to_string_lossy()),
                sh_quote(&file.to_string_lossy())
            ),
        )?;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))?;
        let opened = Command::new("open")
            .args(["-a", "Terminal"])
            .arg(&script)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if opened {
            return Ok(text("terminal UI opened in Terminal.app — the user closes it"));
        }
    }
    bail!("no terminal emulator found (set $TERMINAL, or install alacritty/ghostty/kitty/foot)")
}

/// Single-quote `s` for /bin/sh so paths with spaces or quotes survive
/// the .command script round-trip.
#[cfg(any(target_os = "macos", test))]
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

fn media_missing(server: &Server) -> Result<Vec<Value>> {
    let (file, dir) = require_project(server)?;
    let project = Project::load(&file)?;
    let lost = viode_core::media::missing(&project, &dir);
    let items: Vec<Value> = lost
        .iter()
        .map(|(ti, ci, src)| json!({"track": ti, "clip": ci, "src": src}))
        .collect();
    Ok(text(serde_json::to_string_pretty(&json!({
        "missing": items,
        "note": "relink {dir} reconnects by filename",
    }))?))
}

fn relink_tool(server: &mut Server, args: &Value) -> Result<Vec<Value>> {
    let (file, dir) = require_project(server)?;
    let new_dir = PathBuf::from(str_arg(args, "dir")?);
    let mut project = Project::load(&file)?;
    let n = viode_core::media::relink(&mut project, &dir, &new_dir);
    project.save(&file)?;
    let still = viode_core::media::missing(&project, &dir).len();
    Ok(text(format!("relinked {n} clip(s); {still} still missing")))
}

fn queue_add(server: &Server, args: &Value) -> Result<Vec<Value>> {
    let (_, dir) = require_project(server)?;
    let mut q = viode_core::queue::load(&dir)?;
    q.jobs.push(viode_core::queue::QueueJob {
        preset: args.get("preset").and_then(Value::as_str).map(String::from),
        codec: args.get("codec").and_then(Value::as_str).map(String::from),
        bitrate: args.get("bitrate").and_then(Value::as_u64).map(|b| b as u32),
        output: args.get("output").and_then(Value::as_str).map(PathBuf::from),
    });
    viode_core::queue::save(&dir, &q)?;
    Ok(text(format!("queued job {} — queue_run executes all", q.jobs.len())))
}

fn captions_tool(server: &mut Server, args: &Value) -> Result<Vec<Value>> {
    let (file, dir) = require_project(server)?;
    let mut project = Project::load(&file)?;
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
        let stem = src
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let cache = dir.join("cache").join(format!("captions-{stem}.json"));
        let segments: Vec<viode_core::Segment> = if cache.exists() {
            serde_json::from_str(&std::fs::read_to_string(&cache)?)?
        } else {
            let segs = viode_core::transcribe(&abs, &dir.join("cache"), None)?;
            std::fs::write(&cache, serde_json::to_string_pretty(&segs)?)?;
            segs
        };
        captions.extend(viode_core::captions::map_segments(&project, src, &segments));
    }
    captions.sort_by_key(|c| c.start.0);
    if captions.is_empty() {
        bail!("no speech found — nothing to caption");
    }
    let mut notes = vec![format!("{} captions", captions.len())];
    if let Some(srt) = args.get("srt").and_then(Value::as_str) {
        let path = if Path::new(srt).is_absolute() {
            PathBuf::from(srt)
        } else {
            dir.join(srt)
        };
        std::fs::write(&path, viode_core::captions::to_srt(&captions))?;
        notes.push(format!("SRT written to {}", path.display()));
    }
    if args.get("burn").and_then(Value::as_bool).unwrap_or(false) {
        let n = viode_core::captions::burn(&mut project, &captions);
        project.save(&file)?;
        notes.push(format!("{n} lower-third titles added"));
    }
    let list: Vec<Value> = captions
        .iter()
        .map(|c| json!({"start": c.start.to_string(), "end": c.end.to_string(), "text": c.text}))
        .collect();
    Ok(vec![json!({
        "type": "text",
        "text": json!({"summary": notes.join("; "), "captions": list}).to_string(),
    })])
}

fn doctor_tool() -> Result<Vec<Value>> {
    let checks = viode_core::doctor::run();
    let report = json!({
        "checks": checks.iter().map(|c| json!({
            "feature": c.feature,
            "probe": c.probe,
            "ok": c.ok,
            "required": c.required,
            "fix": if c.ok { Value::Null } else { Value::String(c.fix.to_string()) },
        })).collect::<Vec<_>>(),
        "summary": viode_core::doctor::summary(
            &checks.into_iter().filter(|c| !c.ok).collect::<Vec<_>>(),
        ).unwrap_or_else(|| "every engine piece is present on this machine".into()),
    });
    Ok(vec![json!({ "type": "text", "text": report.to_string() })])
}

fn bench_tool(args: &Value) -> Result<Vec<Value>> {
    let file = PathBuf::from(str_arg(args, "path")?);
    let secs = args.get("secs").and_then(Value::as_u64).unwrap_or(30);
    let run = |label: &str, pre: &[String], post: &[String]| -> Option<f64> {
        let tmp = std::env::temp_dir().join(format!("viode-bench-{label}.mp4"));
        let start = std::time::Instant::now();
        let ok = Command::new("ffmpeg")
            .args(["-y", "-loglevel", "error"])
            .args(pre)
            .args(["-t", &secs.to_string(), "-i"])
            .arg(&file)
            .args(post)
            .arg(&tmp)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        let elapsed = start.elapsed().as_secs_f64();
        let _ = std::fs::remove_file(&tmp);
        ok.then_some(elapsed)
    };
    let owned = |v: &[&str]| v.iter().map(|a| a.to_string()).collect::<Vec<_>>();
    let sw = run("sw", &[], &owned(&["-vf", "scale=-2:540", "-c:v", "libx264", "-crf", "28", "-preset", "veryfast", "-an"]));
    // Per-platform hardware candidate, shared with the CLI and the proxy
    // builder (viode_core::hwaccel).
    let Some(hwdef) = viode_core::hwaccel::platform() else {
        return Ok(text(serde_json::to_string_pretty(&json!({
            "software_secs": sw,
            "verdict": "no hardware path is defined for this platform — software is the path",
        }))?));
    };
    let mut enc = hwdef.encode_args(540);
    enc.push("-an".into());
    let hw = run("hw", &owned(hwdef.decode_args), &enc);
    let verdict = match (sw, hw) {
        (Some(s), Some(h)) if h < s => format!(
            "{} wins {:.1}x — set VIODE_HWACCEL={} on this machine",
            hwdef.label, s / h, hwdef.env_value),
        (Some(s), Some(h)) => format!(
            "software wins {:.1}x — leave VIODE_HWACCEL unset", h / s),
        (Some(_), None) => format!("{} unavailable — software stays the path", hwdef.label),
        _ => bail!("benchmark could not run either path"),
    };
    Ok(text(serde_json::to_string_pretty(&json!({
        "software_secs": sw, "hardware_secs": hw, "hardware": hwdef.env_value,
        "verdict": verdict,
    }))?))
}

fn queue_list(server: &Server) -> Result<Vec<Value>> {
    let (_, dir) = require_project(server)?;
    let q = viode_core::queue::load(&dir)?;
    Ok(text(serde_json::to_string_pretty(&json!({
        "jobs": q.jobs.iter().map(|j| json!({
            "preset": j.preset, "codec": j.codec,
            "bitrate": j.bitrate, "output": j.output,
        })).collect::<Vec<_>>(),
    }))?))
}

fn queue_run(server: &mut Server) -> Result<Vec<Value>> {
    let (_, dir) = require_project(server)?;
    let q = viode_core::queue::load(&dir)?;
    if q.jobs.is_empty() {
        bail!("queue empty");
    }
    let mut outputs = Vec::new();
    for job in &q.jobs {
        let mut args = serde_json::Map::new();
        if let Some(p) = &job.preset {
            args.insert("preset".into(), json!(p));
        }
        if let Some(c) = &job.codec {
            args.insert("codec".into(), json!(c));
        }
        if let Some(b) = job.bitrate {
            args.insert("bitrate".into(), json!(b));
        }
        if let Some(o) = &job.output {
            args.insert("output".into(), json!(o.display().to_string()));
        }
        let result = render(server, &Value::Object(args))?;
        outputs.extend(result);
    }
    viode_core::queue::save(&dir, &viode_core::queue::RenderQueue::default())?;
    outputs.push(json!({"type": "text", "text": format!("queue complete ({} jobs)", q.jobs.len())}));
    Ok(outputs)
}

fn queue_clear(server: &Server) -> Result<Vec<Value>> {
    let (_, dir) = require_project(server)?;
    viode_core::queue::save(&dir, &viode_core::queue::RenderQueue::default())?;
    Ok(text("queue cleared"))
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
    if sources.is_empty() {
        return Ok(text("timeline references no media"));
    }
    let mut lines = Vec::new();
    for (src, result) in viode_core::build_all(&dir, &sources, force, 3) {
        match result {
            Ok(dest) => lines.push(format!("{} -> {}", src.display(), dest.display())),
            Err(e) => lines.push(format!("{} FAILED: {e}", src.display())),
        }
    }
    Ok(text(lines.join("\n")))
}

fn audio_levels_tool(server: &Server, args: &Value) -> Result<Vec<Value>> {
    let (index, clip, src) = clip_and_source(server, args)?;
    let (_, dir) = require_project(server)?;
    let window = f64_arg(args, "window", 0.5);
    let levels: Vec<Value> = viode_core::audio_scan(
        &dir,
        &src,
        viode_core::DEFAULT_NOISE_DB,
        viode_core::DEFAULT_MIN_SILENCE,
        window,
    )?
    .levels
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
    let reframe = args.get("reframe").and_then(Value::as_bool).unwrap_or(false);
    let preset = match args.get("preset").and_then(Value::as_str) {
        Some(p) => Some(
            viode_core::Preset::parse(p)
                .with_context(|| format!("unknown preset {p:?} (youtube, shorts, podcast)"))?,
        ),
        None => None,
    };

    let codec = match args.get("codec").and_then(Value::as_str) {
        Some(c) => Some(
            viode_core::Codec::parse(c)
                .with_context(|| format!("unknown codec {c:?} (h264, hevc, av1, prores, dnxhr)"))?,
        ),
        None => None,
    };
    let smooth = args.get("smooth").and_then(Value::as_u64).map(|f| f as u32);
    let needs_post = preset.is_some() || codec.is_some() || smooth.is_some();
    let master = if needs_post {
        dir.join("cache").join("master.mp4")
    } else {
        match args.get("output").and_then(Value::as_str) {
            Some(o) => dir.join(o),
            None => dir.join("renders").join(format!("{name}.mp4")),
        }
    };
    GesBackend.render(&project, &dir, &master)?;

    let requested_out = args.get("output").and_then(Value::as_str).map(|o| dir.join(o));
    let final_path = if let Some(preset) = preset {
        let suffix = match preset {
            viode_core::Preset::Youtube => "youtube",
            viode_core::Preset::Shorts => "shorts",
            viode_core::Preset::Podcast => "podcast",
        };
        let out = requested_out.unwrap_or_else(|| {
            dir.join("renders")
                .join(format!("{name}-{suffix}.{}", preset.extension()))
        });
        if reframe && preset == viode_core::Preset::Shorts {
            viode_core::reframe::shorts_reframed(&master, &out)?;
        } else {
            viode_core::apply_preset(&master, &out, preset)?;
        }
        out
    } else if let Some(codec) = codec {
        let out = requested_out.unwrap_or_else(|| {
            dir.join("renders")
                .join(format!("{name}-{codec:?}.{}", codec.extension()).to_lowercase())
        });
        let bitrate = args.get("bitrate").and_then(Value::as_u64).map(|b| b as u32);
        viode_core::transcode(&master, &out, codec, bitrate)?;
        out
    } else if let Some(fps) = smooth {
        let out = requested_out
            .unwrap_or_else(|| dir.join("renders").join(format!("{name}-smooth.mp4")));
        viode_core::smooth(&master, &out, fps)?;
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

#[cfg(test)]
mod tests {
    use super::sh_quote;

    #[test]
    fn sh_quote_survives_spaces_and_quotes() {
        assert_eq!(sh_quote("/plain/path"), "'/plain/path'");
        assert_eq!(sh_quote("/with space/x"), "'/with space/x'");
        // A single quote closes, escapes, and reopens: ' -> '\''
        assert_eq!(sh_quote("it's"), r"'it'\''s'");
    }
}
