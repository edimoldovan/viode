//! viode-core — timeline model, edit operations, probing, render backends.
//! Every client (CLI, MCP, future TUI/GUI) goes through this crate.

pub mod audio;
pub mod backend;
pub mod export;
pub mod model;
pub mod ops;
pub mod probe;
pub mod proxy;
pub mod sync;
pub mod time;
pub mod transcript;
pub mod visual;

pub use audio::{audio_levels, detect_scenes, detect_silences, AnalyzeError};
pub use backend::{GesBackend, RenderBackend, RenderError, SmartCopyBackend};
pub use export::{apply_preset, ExportError, Preset};
pub use model::{Keyframe, Title, Track, TrackKind};
pub use proxy::{build_proxy, proxy_for, ProxyError, PROXY_HEIGHT};
pub use sync::{audio_offset, SyncError};
pub use transcript::{parse_whisper_json, transcribe, Segment, TranscribeError};
pub use visual::{contact_sheet_png, waveform_png, VisualError};
pub use model::{Clip, Meta, Project, ProjectError, PROJECT_FILE};
pub use ops::OpError;
pub use probe::{probe, MediaInfo, ProbeError};
pub use time::Time;
