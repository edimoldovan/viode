//! Connect Viode to the user's AI assistant — without the user ever
//! seeing the letters M, C, P. Every mainstream AI client reads a config
//! file registering local tool servers; this module knows where those
//! files live per client and OS, writes Viode's entry into them, and
//! reports in plain words. Idempotent by construction: connecting twice
//! rewrites the same entry, and configs we cannot parse are refused, not
//! clobbered.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    #[error("{0} has a config at {1} that could not be parsed — not touching it")]
    Unreadable(String, PathBuf),
    #[error("unknown client {0:?} — `viode connect` lists the known ones")]
    Unknown(String),
    #[error("{0} was not found on this machine")]
    NotFound(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// How a client's config file expects the server entry.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Shape {
    /// {"mcpServers": {"viode": {"command": "...", "args": [...]}}}
    McpServers,
    /// opencode: {"mcp": {"viode": {"type": "local", "command": [...]}}}
    Opencode,
}

struct ClientDef {
    /// Stable id for the CLI ("claude-desktop") and display name.
    id: &'static str,
    name: &'static str,
    shape: Shape,
    /// Config path relative to HOME, per OS.
    #[cfg(target_os = "macos")]
    config: &'static str,
    #[cfg(not(target_os = "macos"))]
    config: &'static str,
    /// A directory whose existence means "this client is installed",
    /// relative to HOME (the config file itself may not exist yet).
    marker: &'static str,
}

#[cfg(target_os = "macos")]
const CLIENTS: &[ClientDef] = &[
    ClientDef { id: "claude-desktop", name: "Claude Desktop", shape: Shape::McpServers,
        config: "Library/Application Support/Claude/claude_desktop_config.json",
        marker: "Library/Application Support/Claude" },
    ClientDef { id: "claude-code", name: "Claude Code", shape: Shape::McpServers,
        config: ".claude.json", marker: ".claude" },
    ClientDef { id: "cursor", name: "Cursor", shape: Shape::McpServers,
        config: ".cursor/mcp.json", marker: ".cursor" },
    ClientDef { id: "windsurf", name: "Windsurf", shape: Shape::McpServers,
        config: ".codeium/windsurf/mcp_config.json", marker: ".codeium/windsurf" },
    ClientDef { id: "gemini-cli", name: "Gemini CLI", shape: Shape::McpServers,
        config: ".gemini/settings.json", marker: ".gemini" },
    ClientDef { id: "opencode", name: "opencode", shape: Shape::Opencode,
        config: ".config/opencode/opencode.json", marker: ".config/opencode" },
];

#[cfg(not(target_os = "macos"))]
const CLIENTS: &[ClientDef] = &[
    ClientDef { id: "claude-desktop", name: "Claude Desktop", shape: Shape::McpServers,
        config: ".config/Claude/claude_desktop_config.json", marker: ".config/Claude" },
    ClientDef { id: "claude-code", name: "Claude Code", shape: Shape::McpServers,
        config: ".claude.json", marker: ".claude" },
    ClientDef { id: "cursor", name: "Cursor", shape: Shape::McpServers,
        config: ".cursor/mcp.json", marker: ".cursor" },
    ClientDef { id: "windsurf", name: "Windsurf", shape: Shape::McpServers,
        config: ".codeium/windsurf/mcp_config.json", marker: ".codeium/windsurf" },
    ClientDef { id: "gemini-cli", name: "Gemini CLI", shape: Shape::McpServers,
        config: ".gemini/settings.json", marker: ".gemini" },
    ClientDef { id: "opencode", name: "opencode", shape: Shape::Opencode,
        config: ".config/opencode/opencode.json", marker: ".config/opencode" },
];

/// One client's situation on this machine.
#[derive(Debug, Clone, PartialEq)]
pub struct ClientStatus {
    pub id: String,
    pub name: String,
    pub found: bool,
    pub connected: bool,
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default())
}

/// The absolute path official binaries should register. Falls back to
/// plain "viode" (PATH lookup) when the exe path is unknowable.
fn viode_command() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.canonicalize().ok())
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "viode".into())
}

fn entry_for(shape: Shape, command: &str) -> (&'static str, &'static str, Value) {
    match shape {
        Shape::McpServers => (
            "mcpServers",
            "viode",
            json!({ "command": command, "args": ["serve", "--mcp"] }),
        ),
        Shape::Opencode => (
            "mcp",
            "viode",
            json!({ "type": "local", "command": [command, "serve", "--mcp"], "enabled": true }),
        ),
    }
}

fn read_config(path: &Path) -> Result<Value, ()> {
    match std::fs::read_to_string(path) {
        Ok(text) if text.trim().is_empty() => Ok(json!({})),
        Ok(text) => serde_json::from_str(&text).map_err(|_| ()),
        Err(_) => Ok(json!({})),
    }
}

fn is_connected(def: &ClientDef, home: &Path) -> bool {
    let (section, key, _) = entry_for(def.shape, "x");
    read_config(&home.join(def.config))
        .map(|c| c.get(section).and_then(|s| s.get(key)).is_some())
        .unwrap_or(false)
}

fn detect_in(home: &Path) -> Vec<ClientStatus> {
    CLIENTS
        .iter()
        .map(|def| ClientStatus {
            id: def.id.into(),
            name: def.name.into(),
            found: home.join(def.marker).exists(),
            connected: is_connected(def, home),
        })
        .collect()
}

/// What AI clients exist on this machine, and which already know Viode.
pub fn detect() -> Vec<ClientStatus> {
    detect_in(&home())
}

fn connect_in(id: &str, home: &Path, command: &str) -> Result<String, ConnectError> {
    let def = CLIENTS
        .iter()
        .find(|c| c.id == id)
        .ok_or_else(|| ConnectError::Unknown(id.into()))?;
    if !home.join(def.marker).exists() {
        return Err(ConnectError::NotFound(def.name.into()));
    }
    let path = home.join(def.config);
    let mut config = read_config(&path)
        .map_err(|_| ConnectError::Unreadable(def.name.into(), path.clone()))?;
    let (section, key, entry) = entry_for(def.shape, command);
    if !config.is_object() {
        return Err(ConnectError::Unreadable(def.name.into(), path));
    }
    let obj = config.as_object_mut().unwrap();
    let sect = obj.entry(section).or_insert_with(|| json!({}));
    if !sect.is_object() {
        return Err(ConnectError::Unreadable(def.name.into(), path));
    }
    sect.as_object_mut().unwrap().insert(key.into(), entry);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(&config)?.as_bytes())?;
    Ok(format!(
        "{} connected — restart it, then just talk to it about your video.",
        def.name
    ))
}

/// Register Viode with one client by id.
pub fn connect(id: &str) -> Result<String, ConnectError> {
    connect_in(id, &home(), &viode_command())
}

/// The manual snippet for clients we do not know.
pub fn snippet() -> String {
    serde_json::to_string_pretty(&json!({
        "mcpServers": { "viode": { "command": viode_command(), "args": ["serve", "--mcp"] } }
    }))
    .unwrap_or_default()
}

impl From<serde_json::Error> for ConnectError {
    fn from(e: serde_json::Error) -> Self {
        ConnectError::Io(std::io::Error::other(e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_home() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn detects_found_and_connected_states() {
        let home = fake_home();
        std::fs::create_dir_all(home.path().join(".cursor")).unwrap();
        let statuses = detect_in(home.path());
        let cursor = statuses.iter().find(|s| s.id == "cursor").unwrap();
        assert!(cursor.found && !cursor.connected);
        let missing = statuses.iter().find(|s| s.id == "claude-desktop").unwrap();
        assert!(!missing.found);

        connect_in("cursor", home.path(), "/usr/bin/viode").unwrap();
        let cursor = detect_in(home.path()).into_iter().find(|s| s.id == "cursor").unwrap();
        assert!(cursor.connected);
    }

    #[test]
    fn connecting_preserves_existing_config_and_is_idempotent() {
        let home = fake_home();
        std::fs::create_dir_all(home.path().join(".cursor")).unwrap();
        std::fs::write(
            home.path().join(".cursor/mcp.json"),
            r#"{"mcpServers": {"other": {"command": "keepme"}}, "unrelated": 7}"#,
        )
        .unwrap();
        connect_in("cursor", home.path(), "/usr/bin/viode").unwrap();
        connect_in("cursor", home.path(), "/usr/bin/viode").unwrap();
        let config: Value = serde_json::from_str(
            &std::fs::read_to_string(home.path().join(".cursor/mcp.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(config["unrelated"], 7);
        assert_eq!(config["mcpServers"]["other"]["command"], "keepme");
        assert_eq!(config["mcpServers"]["viode"]["command"], "/usr/bin/viode");
        assert_eq!(config["mcpServers"]["viode"]["args"][0], "serve");
    }

    #[test]
    fn opencode_gets_its_own_shape() {
        let home = fake_home();
        std::fs::create_dir_all(home.path().join(".config/opencode")).unwrap();
        connect_in("opencode", home.path(), "/usr/bin/viode").unwrap();
        let config: Value = serde_json::from_str(
            &std::fs::read_to_string(home.path().join(".config/opencode/opencode.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(config["mcp"]["viode"]["type"], "local");
        assert_eq!(config["mcp"]["viode"]["command"][1], "serve");
    }

    #[test]
    fn broken_configs_are_refused_not_clobbered() {
        let home = fake_home();
        std::fs::create_dir_all(home.path().join(".cursor")).unwrap();
        std::fs::write(home.path().join(".cursor/mcp.json"), "{not json").unwrap();
        let err = connect_in("cursor", home.path(), "/usr/bin/viode").unwrap_err();
        assert!(err.to_string().contains("not touching it"));
        assert_eq!(
            std::fs::read_to_string(home.path().join(".cursor/mcp.json")).unwrap(),
            "{not json"
        );
    }

    #[test]
    fn absent_clients_and_unknown_ids_refuse_clearly() {
        let home = fake_home();
        assert!(matches!(
            connect_in("cursor", home.path(), "v"),
            Err(ConnectError::NotFound(_))
        ));
        assert!(matches!(
            connect_in("clippy", home.path(), "v"),
            Err(ConnectError::Unknown(_))
        ));
    }
}
