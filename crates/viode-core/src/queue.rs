//! Shared render queue: jobs stored in cache/queue.toml, executed in order
//! by whichever client (CLI, MCP) runs them.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RenderQueue {
    #[serde(default, rename = "job")]
    pub jobs: Vec<QueueJob>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueJob {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codec: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bitrate: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<PathBuf>,
}

pub fn path(project_dir: &Path) -> PathBuf {
    project_dir.join("cache").join("queue.toml")
}

pub fn load(project_dir: &Path) -> std::io::Result<RenderQueue> {
    let p = path(project_dir);
    if !p.exists() {
        return Ok(RenderQueue::default());
    }
    let text = std::fs::read_to_string(&p)?;
    toml::from_str(&text).map_err(|e| std::io::Error::other(e.to_string()))
}

pub fn save(project_dir: &Path, q: &RenderQueue) -> std::io::Result<()> {
    let p = path(project_dir);
    std::fs::create_dir_all(p.parent().unwrap())?;
    let text = toml::to_string_pretty(q).map_err(|e| std::io::Error::other(e.to_string()))?;
    std::fs::write(&p, text)
}
