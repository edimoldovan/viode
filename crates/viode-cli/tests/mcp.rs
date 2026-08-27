//! End-to-end MCP test: spawn `viode serve --mcp` and speak newline-delimited
//! JSON-RPC to it, exactly as Claude (or any MCP client) would. This is both
//! the protocol regression net and a worked example of an MCP session.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command as Proc, Stdio};

use serde_json::{json, Value};

struct Mcp {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
}

impl Mcp {
    fn spawn(dir: &Path) -> Self {
        let mut child = Proc::new(assert_cmd::cargo::cargo_bin("viode"))
            .args(["serve", "--mcp"])
            .current_dir(dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("failed to spawn viode serve --mcp");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Mcp { child, stdin, stdout, next_id: 0 }
    }

    /// Send a request and read its response line.
    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let msg = json!({
            "jsonrpc": "2.0", "id": self.next_id,
            "method": method, "params": params,
        });
        writeln!(self.stdin, "{msg}").unwrap();
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        let response: Value = serde_json::from_str(&line)
            .unwrap_or_else(|e| panic!("bad JSON response {line:?}: {e}"));
        assert_eq!(response["id"], json!(self.next_id), "response id mismatch");
        response
    }

    /// Call a tool, asserting the call itself succeeded (isError: false).
    fn tool(&mut self, name: &str, args: Value) -> Value {
        let response = self.request("tools/call", json!({"name": name, "arguments": args}));
        let result = &response["result"];
        assert_eq!(
            result["isError"], json!(false),
            "tool {name} failed: {}",
            result["content"]
        );
        result.clone()
    }

    /// Call a tool expecting failure; returns the error text.
    fn tool_err(&mut self, name: &str, args: Value) -> String {
        let response = self.request("tools/call", json!({"name": name, "arguments": args}));
        let result = &response["result"];
        assert_eq!(result["isError"], json!(true), "tool {name} unexpectedly succeeded");
        result["content"][0]["text"].as_str().unwrap().to_string()
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn ffmpeg_available() -> bool {
    Proc::new("ffmpeg").arg("-version").output().is_ok()
}

fn make_clip(path: &Path, dur: f64) {
    let status = Proc::new("ffmpeg")
        .args([
            "-y", "-loglevel", "error",
            "-f", "lavfi", "-i", &format!("testsrc2=duration={dur}:size=320x180:rate=30"),
            "-f", "lavfi", "-i", &format!("sine=frequency=440:duration={dur}"),
            "-c:v", "libx264", "-pix_fmt", "yuv420p", "-preset", "ultrafast",
            "-c:a", "aac", "-shortest",
        ])
        .arg(path)
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn handshake_and_tool_listing() {
    let tmp = tempfile::tempdir().unwrap();
    let mut mcp = Mcp::spawn(tmp.path());

    let init = mcp.request(
        "initialize",
        json!({
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "test", "version": "0"}
        }),
    );
    assert_eq!(init["result"]["serverInfo"]["name"], json!("viode"));
    assert_eq!(init["result"]["protocolVersion"], json!("2025-06-18"));

    let tools = mcp.request("tools/list", json!({}));
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    for expected in [
        "project_new", "project_open", "timeline_get", "media_probe",
        "clip_add", "clip_trim", "clip_split", "clip_move", "clip_remove",
        "frame_grab", "render_preview", "render",
    ] {
        assert!(names.contains(&expected), "missing tool {expected}, have {names:?}");
    }

    let unknown = mcp.request("no/such/method", json!({}));
    assert_eq!(unknown["error"]["code"], json!(-32601));
}

#[test]
fn full_mcp_editing_session() {
    if !ffmpeg_available() {
        eprintln!("SKIP full_mcp_editing_session: ffmpeg not installed");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let clip_path = tmp.path().join("interview.mp4");
    make_clip(&clip_path, 2.0);

    let mut mcp = Mcp::spawn(tmp.path());
    mcp.request("initialize", json!({"protocolVersion": "2025-06-18"}));

    // No project yet: tools must say so, not crash.
    let err = mcp.tool_err("timeline_get", json!({}));
    assert!(err.contains("no active project"), "unhelpful: {err}");

    // Create a project, add the clip (external file -> copied to media/).
    mcp.tool("project_new", json!({"path": "podcast", "width": 320, "height": 180}));
    let result = mcp.tool("clip_add", json!({"src": clip_path.to_str().unwrap()}));
    let timeline: Value =
        serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(timeline["clips"][0]["src"], json!("media/interview.mp4"));
    assert_eq!(timeline["total"], json!("00:00:02.000"));

    // Edit: split at 0.5, drop the head, keep 1.5s.
    mcp.tool("clip_split", json!({"index": 0, "at": 0.5}));
    let result = mcp.tool("clip_remove", json!({"index": 0}));
    let timeline: Value =
        serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(timeline["total"], json!("00:00:01.500"));
    assert_eq!(timeline["clips"][0]["in"], json!("00:00:00.500"));

    // The model can look at the edit: frame_grab returns a PNG image block.
    let result = mcp.tool("frame_grab", json!({"at": "0.2"}));
    assert_eq!(result["content"][0]["type"], json!("image"));
    assert_eq!(result["content"][0]["mimeType"], json!("image/png"));
    let data = result["content"][0]["data"].as_str().unwrap();
    assert!(data.starts_with("iVBOR"), "not a PNG: {}", &data[..20.min(data.len())]);

    // Past-the-end grabs fail with a useful message.
    let err = mcp.tool_err("frame_grab", json!({"at": 99}));
    assert!(err.contains("past the end"), "unhelpful: {err}");

    // Bad edits report, they don't wedge the server.
    let err = mcp.tool_err("clip_split", json!({"index": 0, "at": 0}));
    assert!(err.contains("split point"), "unhelpful: {err}");
    mcp.tool("timeline_get", json!({}));

    // State survives across connections: the TOML on disk is the session.
    drop(mcp);
    let mut mcp = Mcp::spawn(&tmp.path().join("podcast"));
    mcp.request("initialize", json!({"protocolVersion": "2025-06-18"}));
    let result = mcp.tool("timeline_get", json!({}));
    let timeline: Value =
        serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(timeline["total"], json!("00:00:01.500"));
}

#[test]
fn render_over_mcp() {
    let ges = Proc::new("pkg-config")
        .args(["--exists", "gst-editing-services-1.0"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ffmpeg_available() || !ges {
        eprintln!("SKIP render_over_mcp: ffmpeg/GES not installed");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let clip_path = tmp.path().join("a.mp4");
    make_clip(&clip_path, 1.0);

    let mut mcp = Mcp::spawn(tmp.path());
    mcp.request("initialize", json!({"protocolVersion": "2025-06-18"}));
    mcp.tool("project_new", json!({"path": "r", "width": 320, "height": 180}));
    mcp.tool("clip_add", json!({"src": clip_path.to_str().unwrap()}));

    // Preview of a sub-range, then the full render.
    mcp.tool("render_preview", json!({"start": 0, "end": 0.5}));
    assert!(tmp.path().join("r/cache/preview.mp4").exists());

    mcp.tool("render", json!({}));
    assert!(tmp.path().join("r/renders/r.mp4").exists());
}
