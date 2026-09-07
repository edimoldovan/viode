//! MCP server for Viode — stdio transport.
//!
//! MCP's stdio transport is newline-delimited JSON-RPC 2.0: one request per
//! line in, one response per line out. That's simple enough that we speak it
//! directly — the whole protocol layer is this file, no framework. If you're
//! new to MCP, read `serve()` then `tools_call()`; that's the entire flow.

mod tools;

use std::io::{BufRead, Write};
use std::path::PathBuf;

use serde_json::{json, Value};

pub struct Server {
    /// Path to the active project.viode, if any. Tools that edit the
    /// timeline load-modify-save this file; the TOML on disk stays the
    /// single source of truth even mid-session.
    pub project_file: Option<PathBuf>,
}

pub fn serve(project_file: Option<PathBuf>) -> anyhow::Result<()> {
    let mut server = Server { project_file };
    let stdin = std::io::stdin().lock();
    let mut stdout = std::io::stdout().lock();

    for line in stdin.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue, // not JSON — nothing sane to answer
        };
        // Messages without an id are notifications; nothing to answer.
        let Some(id) = msg.get("id").filter(|id| !id.is_null()).cloned() else {
            continue;
        };
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
        let params = msg.get("params").cloned().unwrap_or_else(|| json!({}));

        let reply = match method {
            "initialize" => Ok(initialize(&params)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tools::definitions() })),
            "tools/call" => tools_call(&mut server, &params),
            other => Err((-32601, format!("method not found: {other}"))),
        };
        let response = match reply {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err((code, message)) => json!({
                "jsonrpc": "2.0", "id": id,
                "error": {"code": code, "message": message}
            }),
        };
        writeln!(stdout, "{response}")?;
        stdout.flush()?;
    }
    Ok(())
}

fn initialize(params: &Value) -> Value {
    // Negotiate honestly: we answer with the newest protocol we actually
    // implement. Claiming the client's (possibly newer) version breaks
    // clients that then expect newer behavior.
    let _ = params;
    let mut result = json!({
        "protocolVersion": "2025-06-18",
        "capabilities": { "tools": {} },
        "serverInfo": {
            "name": "viode",
            "version": env!("CARGO_PKG_VERSION"),
        }
    });
    // The editing briefing: Viode's own working knowledge, delivered to
    // every connected model at handshake. It ships inside the binary,
    // so every install carries it — nothing to copy or configure.
    const BRIEFING: &str = "You are operating Viode, a professional \
        video editor, with its complete tool surface. Work like an \
        editor, not a script: \
        (1) Ground the session in a project first (project_new or \
        project_open); clip_add copies footage in. \
        (2) You have senses — use them. frame_grab shows you any \
        timeline moment, waveform and audio_levels show the sound, \
        scope measures color, timeline_get shows structure, and \
        render_preview checks a section cheaply. Look before you cut, \
        and look again before declaring anything done. \
        (3) On long or 4K footage, run proxy_build early — everything \
        interactive gets fast. \
        (4) silence_detect and scene_detect answer in SOURCE time \
        (positions inside the file), not timeline time. \
        (5) A proven long-form flow: silence_cut for dead air, \
        angle_add + take for multicam, duck to seat music under \
        speech, clean for voice noise, captions, then render with a \
        preset (youtube, shorts, podcast; shorts can reframe onto the \
        subject's face). \
        (6) Keep the user watching when they want to: ui_open shows a \
        live editor window that follows every edit you make. After a \
        render, watch opens the finished file in a player with \
        chapters at your cuts and markers. \
        (7) Prefer many small verified steps over one blind batch — \
        every edit is undoable and the timeline is a readable file.";

    // The engine checkup runs once at initialize so the model knows this
    // machine's gaps BEFORE it plans an edit (the `instructions` field is
    // shown to the client's model). A complete machine adds nothing.
    let problems = viode_core::doctor::problems();
    let mut instructions = vec![BRIEFING.to_string()];
    if let Some(summary) = viode_core::doctor::summary(&problems) {
        instructions.push(format!(
            "{summary} The doctor tool returns the full report; features \
             listed as missing will fail with actionable errors until \
             their piece is installed."
        ));
    }
    // Official binaries surface developer announcements here too, so the
    // model can relay them to the user.
    if let Ok(notice) = std::env::var("VIODE_NOTICE") {
        if !notice.is_empty() {
            instructions.push(format!("Announcement from the Viode developer: {notice}"));
        }
    }
    result["instructions"] = json!(instructions.join("\n\n"));
    result
}

fn tools_call(server: &mut Server, params: &Value) -> Result<Value, (i64, String)> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or((-32602, "missing tool name".to_string()))?;
    let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));

    // Per MCP: tool failures are results with isError, not protocol errors —
    // the model is supposed to see them and react.
    match tools::dispatch(server, name, &args) {
        Ok(content) => Ok(json!({ "content": content, "isError": false })),
        Err(e) => Ok(json!({
            "content": [{ "type": "text", "text": format!("{e:#}") }],
            "isError": true
        })),
    }
}
