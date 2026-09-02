//! viode-core — timeline model, edit operations, probing, render backends.
//! Every client (CLI, MCP, future TUI/GUI) goes through this crate.

pub mod artifacts;
pub mod audio;
pub mod backend;
pub mod doctor;
pub mod export;
pub mod hwaccel;
pub mod media;
pub mod model;
pub mod ops;
pub mod probe;
pub mod proxy;
pub mod queue;
pub mod sync;
pub mod time;
pub mod transcript;
pub mod visual;

pub use audio::{audio_levels, audio_scan, detect_scenes, detect_silences, AnalyzeError, AudioScan,
    DEFAULT_LEVEL_WINDOW, DEFAULT_MIN_SILENCE, DEFAULT_NOISE_DB};
pub use backend::{build_timeline, preview_play, run_gui, GesBackend, RenderBackend, RenderError, SmartCopyBackend, TRANSITION_KINDS};
pub use export::{apply_preset, smooth, transcode, Codec, ExportError, Preset};
pub use model::{ColorGrade, Keyframe, Title, Track, TrackKind};
pub use proxy::{build_all, build_proxy, proxy_for, ProxyError, PROXY_HEIGHT};
pub use sync::{audio_offset, SyncError};
pub use transcript::{parse_whisper_json, transcribe, Segment, TranscribeError};
pub use visual::{contact_sheet_png, scope_png, waveform_png, VisualError};
pub use model::{Clip, Meta, Project, ProjectError, PROJECT_FILE};
pub use ops::OpError;
pub use probe::{probe, MediaInfo, ProbeError};
pub use time::Time;
