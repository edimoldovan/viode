//! viode-core — timeline model, edit operations, probing, render backends.
//! Every client (CLI, MCP, future TUI/GUI) goes through this crate.

pub mod audio;
pub mod backend;
pub mod model;
pub mod ops;
pub mod probe;
pub mod time;

pub use audio::{detect_scenes, detect_silences, AnalyzeError};
pub use backend::{GesBackend, RenderBackend, RenderError, SmartCopyBackend};
pub use model::{Clip, Meta, Project, ProjectError, PROJECT_FILE};
pub use ops::OpError;
pub use probe::{probe, MediaInfo, ProbeError};
pub use time::Time;
