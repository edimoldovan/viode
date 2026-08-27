//! The timeline model — source of truth is the project.viode TOML file.
//!
//! Phase 1 is a cuts-only sequence: clips play back-to-back in order, no
//! gaps, no layers. Timeline positions are derived, never stored.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::time::Time;

pub const PROJECT_FILE: &str = "project.viode";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub project: Meta,
    #[serde(default, rename = "clip")]
    pub clips: Vec<Clip>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Meta {
    pub name: String,
    pub fps: f64,
    pub resolution: [u32; 2],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    /// Path relative to the project directory.
    pub src: PathBuf,
    /// Source in-point (where playback of this clip starts in the file).
    #[serde(rename = "in", default)]
    pub in_: Time,
    /// Source out-point (exclusive).
    pub out: Time,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

impl Clip {
    pub fn len(&self) -> Time {
        self.out - self.in_
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    #[error("no {PROJECT_FILE} found at {0} — run `viode new` or cd into a project")]
    NotFound(PathBuf),
    #[error("failed to read {0}: {1}")]
    Io(PathBuf, #[source] std::io::Error),
    #[error("invalid project file {0}: {1}")]
    Parse(PathBuf, #[source] toml::de::Error),
    #[error("failed to serialize project: {0}")]
    Serialize(#[from] toml::ser::Error),
}

impl Project {
    pub fn new(name: &str, fps: f64, resolution: [u32; 2]) -> Self {
        Project {
            project: Meta {
                name: name.to_string(),
                fps,
                resolution,
            },
            clips: Vec::new(),
        }
    }

    pub fn load(path: &Path) -> Result<Self, ProjectError> {
        if !path.exists() {
            return Err(ProjectError::NotFound(path.to_path_buf()));
        }
        let text =
            fs::read_to_string(path).map_err(|e| ProjectError::Io(path.to_path_buf(), e))?;
        toml::from_str(&text).map_err(|e| ProjectError::Parse(path.to_path_buf(), e))
    }

    pub fn save(&self, path: &Path) -> Result<(), ProjectError> {
        let text = toml::to_string_pretty(self)?;
        fs::write(path, text).map_err(|e| ProjectError::Io(path.to_path_buf(), e))
    }

    /// Timeline start position of every clip (derived: clips are a gapless
    /// sequence).
    pub fn positions(&self) -> Vec<Time> {
        let mut cursor = Time::ZERO;
        self.clips
            .iter()
            .map(|c| {
                let start = cursor;
                cursor = cursor + c.len();
                start
            })
            .collect()
    }

    pub fn total_duration(&self) -> Time {
        self.clips
            .iter()
            .fold(Time::ZERO, |acc, c| acc + c.len())
    }
}
