//! The timeline model — source of truth is the project.viode TOML file.
//!
//! Track 0 ("main") is a gapless sequence: clips play back-to-back, and an
//! optional per-clip `transition` overlaps a clip with its predecessor for a
//! crossfade. Overlay tracks (B-roll, angles, music) position clips
//! explicitly with `at`. Titles are project-level overlays on top.
//! Positions on the main track are derived, never stored.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::time::Time;

pub const PROJECT_FILE: &str = "project.viode";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub project: Meta,
    #[serde(default, rename = "track")]
    pub tracks: Vec<Track>,
    #[serde(default, rename = "title", skip_serializing_if = "Vec::is_empty")]
    pub titles: Vec<Title>,
    #[serde(default, rename = "marker", skip_serializing_if = "Vec::is_empty")]
    pub markers: Vec<Marker>,
    /// Pre-multitrack files had [[clip]] at the root; migrated into
    /// tracks[0] on load, never written back.
    #[serde(default, rename = "clip", skip_serializing)]
    legacy_clips: Vec<Clip>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Meta {
    pub name: String,
    pub fps: f64,
    pub resolution: [u32; 2],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrackKind {
    /// Audio + video from the sources (normal footage).
    #[default]
    Av,
    /// Video only — B-roll overlays that keep the main track's audio.
    Video,
    /// Audio only — music beds, voiceover.
    Audio,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub name: String,
    #[serde(default)]
    pub kind: TrackKind,
    /// Disabled tracks are kept in the file but excluded from renders —
    /// how multicam angles wait their turn.
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default, rename = "clip")]
    pub clips: Vec<Clip>,
}

impl Track {
    pub fn new(name: &str, kind: TrackKind) -> Track {
        Track {
            name: name.to_string(),
            kind,
            enabled: true,
            clips: Vec::new(),
        }
    }

    /// Sequence-track starts: each clip begins where the previous ended,
    /// minus its crossfade overlap.
    pub fn positions(&self) -> Vec<Time> {
        let mut cursor = Time::ZERO;
        self.clips
            .iter()
            .map(|c| {
                let start = cursor - c.transition.unwrap_or(Time::ZERO);
                cursor = start + c.len();
                start
            })
            .collect()
    }

    /// End of the sequence (total duration of a main track).
    pub fn end(&self) -> Time {
        self.positions()
            .last()
            .map(|s| *s + self.clips.last().unwrap().len())
            .unwrap_or(Time::ZERO)
    }
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
    /// Timeline position — overlay tracks only; ignored on the main track.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Time>,
    /// Crossfade duration with the PREVIOUS clip — main track only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transition: Option<Time>,
    /// GStreamer effect descriptions, e.g. "videobalance saturation=0.0".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<String>,
    /// Audio gain, linear (1.0 = unity). None = untouched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume: Option<f64>,
    /// Stereo pan, -1.0 (left) .. 1.0 (right).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pan: Option<f64>,
    /// Keyframes animating a property over SOURCE time (linear
    /// interpolation): "volume" (0..2+) or "alpha" (0..1, video opacity).
    #[serde(default, rename = "key", skip_serializing_if = "Vec::is_empty")]
    pub keys: Vec<Keyframe>,
    /// Playback rate: 2.0 = double speed, 0.5 = slow motion. None = 1.0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate: Option<f64>,
    /// Stabilization smoothing (vidstab frames, ~10 = default camera
    /// shake). None = no stabilization. Applied as a cached ffmpeg bake.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steady: Option<u32>,
    /// Voice denoise strength in dB (ffmpeg afftdn nr, ~12 = light hum
    /// removal). None = untouched. Applied as a cached audio-only bake.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clean: Option<f64>,
    /// Chroma key: "green" or "blue" backgrounds become transparent
    /// (overlay clips — the tracks below show through).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matte: Option<String>,
    /// Region mask: blur or pixelate a rectangle (optionally tracking
    /// its content). Applied as a cached whole-source bake.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask: Option<Mask>,
    /// Top-left position as fractions of the frame (0,0 = top-left).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pos: Option<[f64; 2]>,
    /// Uniform scale of the video (1.0 = full frame). 0.25 = corner PiP.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<f64>,
    /// Rotation in degrees (clockwise).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotate: Option<f64>,
    /// Static opacity 0..1 (for animated opacity use an "alpha" keyframe).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f64>,
    /// Color grade (all fields neutral by default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<ColorGrade>,
    /// 3D LUT file (.cube) applied to the clip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lut: Option<PathBuf>,
    /// Transition type with the previous clip: "crossfade" (default) or a
    /// GES transition nick like "bar-wipe-lr", "box-wipe-tl".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transition_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Keyframe {
    pub prop: String,
    pub at: Time,
    pub value: f64,
}

/// videobalance-style grade; every field defaults to neutral.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColorGrade {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brightness: Option<f64>, // -1..1, neutral 0
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contrast: Option<f64>, // 0..2, neutral 1
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saturation: Option<f64>, // 0..2, neutral 1
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hue: Option<f64>, // -1..1, neutral 0
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gamma: Option<f64>, // 0.01..10, neutral 1 (gamma element)
}

impl Clip {
    pub fn media(src: PathBuf, in_: Time, out: Time) -> Clip {
        Clip {
            src,
            in_,
            out,
            at: None,
            transition: None,
            effects: Vec::new(),
            volume: None,
            pan: None,
            keys: Vec::new(),
            rate: None,
            steady: None,
            clean: None,
            matte: None,
            mask: None,
            pos: None,
            scale: None,
            rotate: None,
            opacity: None,
            color: None,
            lut: None,
            transition_kind: None,
            label: None,
        }
    }

    /// Length of the SOURCE range consumed.
    pub fn src_len(&self) -> Time {
        self.out - self.in_
    }

    /// Length on the TIMELINE: source range divided by playback rate.
    pub fn len(&self) -> Time {
        match self.rate {
            Some(r) if r > 0.0 => Time((self.src_len().0 as f64 / r).round() as u64),
            _ => self.src_len(),
        }
    }

    /// Convert a timeline offset within this clip to a source offset.
    pub fn src_offset(&self, timeline_offset: Time) -> Time {
        match self.rate {
            Some(r) if r > 0.0 => Time((timeline_offset.0 as f64 * r).round() as u64),
            _ => timeline_offset,
        }
    }

    /// Timeline span for an overlay clip.
    pub fn span(&self) -> (Time, Time) {
        let start = self.at.unwrap_or(Time::ZERO);
        (start, start + self.len())
    }
}

/// A region mask: hide a face, a screen, a license plate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Mask {
    /// [x, y, w, h] as fractions of the frame.
    pub region: [f64; 4],
    /// "blur" or "pixelate".
    #[serde(default = "default_mask_kind")]
    pub kind: String,
    /// Track the region's content and move the mask with it.
    #[serde(default)]
    pub follow: bool,
}

fn default_mask_kind() -> String {
    "blur".into()
}

/// A named note on the timeline. Markers never render; they are the
/// editor's margin notes — chapter starts, retakes, todo points.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Marker {
    pub at: Time,
    pub text: String,
    /// "#RRGGBB"; pickers/UIs may color-code markers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Title {
    pub text: String,
    pub at: Time,
    pub dur: Time,
    /// Pango font description, e.g. "Sans Bold 64".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font: Option<String>,
    /// Horizontal position 0..1 (0 = left). Default centered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xpos: Option<f64>,
    /// Vertical position 0..1 (0 = top). 0.8 = lower third.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ypos: Option<f64>,
    /// Text color as "#RRGGBB" or "#AARRGGBB". Default white.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
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
    #[error("{0} already exists")]
    Exists(PathBuf),
    #[error("{0} is not a usable project directory name")]
    InvalidName(PathBuf),
}

impl Project {
    pub fn new(name: &str, fps: f64, resolution: [u32; 2]) -> Self {
        Project {
            project: Meta {
                name: name.to_string(),
                fps,
                resolution,
            },
            tracks: vec![Track::new("main", TrackKind::Av)],
            titles: Vec::new(),
            markers: Vec::new(),
            legacy_clips: Vec::new(),
        }
    }

    pub fn load(path: &Path) -> Result<Self, ProjectError> {
        if !path.exists() {
            return Err(ProjectError::NotFound(path.to_path_buf()));
        }
        let text =
            fs::read_to_string(path).map_err(|e| ProjectError::Io(path.to_path_buf(), e))?;
        let mut project: Project =
            toml::from_str(&text).map_err(|e| ProjectError::Parse(path.to_path_buf(), e))?;
        // Migrate pre-multitrack files; always have a main track.
        if project.tracks.is_empty() {
            let mut main = Track::new("main", TrackKind::Av);
            main.clips = std::mem::take(&mut project.legacy_clips);
            project.tracks.push(main);
        }
        project.legacy_clips.clear();
        Ok(project)
    }

    pub fn save(&self, path: &Path) -> Result<(), ProjectError> {
        let text = toml::to_string_pretty(self)?;
        fs::write(path, text).map_err(|e| ProjectError::Io(path.to_path_buf(), e))
    }

    /// Scaffold a new project directory — subdirectories, .gitignore, and a
    /// fresh project file named after the directory — and return the path of
    /// the project file. Every interface (CLI `new`, MCP `project_new`, the
    /// GUI welcome screen) creates projects through this one function.
    pub fn init(dir: &Path, fps: f64, resolution: [u32; 2]) -> Result<PathBuf, ProjectError> {
        if dir.exists() {
            return Err(ProjectError::Exists(dir.to_path_buf()));
        }
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .filter(|n| !n.is_empty())
            .ok_or_else(|| ProjectError::InvalidName(dir.to_path_buf()))?;
        for sub in ["media", "renders", "cache", "proxies"] {
            let sub = dir.join(sub);
            fs::create_dir_all(&sub).map_err(|e| ProjectError::Io(sub.clone(), e))?;
        }
        let gitignore = dir.join(".gitignore");
        fs::write(&gitignore, "/renders/\n/cache/\n/proxies/\n")
            .map_err(|e| ProjectError::Io(gitignore.clone(), e))?;
        let file = dir.join(PROJECT_FILE);
        Project::new(&name, fps, resolution).save(&file)?;
        Ok(file)
    }

    pub fn main(&self) -> &Track {
        &self.tracks[0]
    }

    pub fn main_mut(&mut self) -> &mut Track {
        &mut self.tracks[0]
    }

    /// Main-track clip start positions (the sequence).
    pub fn positions(&self) -> Vec<Time> {
        self.main().positions()
    }

    /// Timeline length: the furthest end over enabled tracks and titles.
    pub fn total_duration(&self) -> Time {
        let mut total = self.main().end();
        for track in self.tracks.iter().skip(1).filter(|t| t.enabled) {
            for clip in &track.clips {
                total = total.max(clip.span().1);
            }
        }
        for title in &self.titles {
            total = total.max(title.at + title.dur);
        }
        total
    }
}
