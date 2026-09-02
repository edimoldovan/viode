//! Transcription via whisper.cpp — the foundation of edit-video-by-editing-
//! text. We shell out to the whisper.cpp CLI (like ffmpeg, a sidecar), parse
//! its JSON, and normalize to timed segments in SOURCE time.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::time::Time;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    pub start: Time,
    pub end: Time,
    pub text: String,
}

#[derive(Debug, thiserror::Error)]
pub enum TranscribeError {
    #[error(
        "no whisper.cpp binary found (tried whisper-cli, whisper-cpp, whisper). \
         Install whisper.cpp — on Arch: pacman -S whisper-cpp"
    )]
    NoBinary,
    #[error(
        "no whisper model found — pass --model or set VIODE_WHISPER_MODEL to a \
         ggml model file (e.g. ggml-base.en.bin from huggingface.co/ggerganov/whisper.cpp)"
    )]
    NoModel,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0} failed: {1}")]
    Tool(String, String),
    #[error("could not parse whisper output: {0}")]
    Parse(String),
}

fn find_binary() -> Option<PathBuf> {
    ["whisper-cli", "whisper-cpp", "whisper"]
        .iter()
        .find_map(|name| {
            let ok = Command::new(name)
                .arg("--help")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            ok.then(|| PathBuf::from(name))
        })
}

fn find_model(explicit: Option<&Path>) -> Result<PathBuf, TranscribeError> {
    if let Some(m) = explicit {
        return Ok(m.to_path_buf());
    }
    if let Ok(m) = std::env::var("VIODE_WHISPER_MODEL") {
        return Ok(PathBuf::from(m));
    }
    // Common Arch/whisper.cpp locations.
    for dir in ["/usr/share/whisper.cpp-model-base.en", "/usr/share/whisper.cpp"] {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for e in entries.flatten() {
                if e.path().extension().is_some_and(|x| x == "bin") {
                    return Ok(e.path());
                }
            }
        }
    }
    Err(TranscribeError::NoModel)
}

/// Transcribe `media` (any av file — audio is extracted first) into timed
/// segments. `work_dir` holds intermediates (the 16k wav, whisper json).
pub fn transcribe(
    media: &Path,
    work_dir: &Path,
    model: Option<&Path>,
) -> Result<Vec<Segment>, TranscribeError> {
    let binary = find_binary().ok_or(TranscribeError::NoBinary)?;
    let model = find_model(model)?;
    std::fs::create_dir_all(work_dir)?;

    // whisper.cpp wants 16 kHz mono wav.
    let wav = work_dir.join("transcribe-input.wav");
    let out = Command::new("ffmpeg")
        .args(["-y", "-v", "error", "-i"])
        .arg(media)
        .args(["-ac", "1", "-ar", "16000", "-map", "a:0"])
        .arg(&wav)
        .output()?;
    if !out.status.success() {
        return Err(TranscribeError::Tool(
            "ffmpeg".into(),
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }

    let json_base = work_dir.join("transcribe-output");
    let out = Command::new(&binary)
        .arg("-m")
        .arg(&model)
        .args(["-oj", "-of"])
        .arg(&json_base)
        .arg("-f")
        .arg(&wav)
        .output()?;
    if !out.status.success() {
        return Err(TranscribeError::Tool(
            binary.display().to_string(),
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    let json = std::fs::read_to_string(json_base.with_extension("json"))?;
    parse_whisper_json(&json)
}

/// Parse whisper.cpp's -oj output: {"transcription":[{"offsets":{"from":ms,
/// "to":ms},"text":"..."}]}.
pub fn parse_whisper_json(json: &str) -> Result<Vec<Segment>, TranscribeError> {
    let v: serde_json::Value =
        serde_json::from_str(json).map_err(|e| TranscribeError::Parse(e.to_string()))?;
    let items = v["transcription"]
        .as_array()
        .ok_or_else(|| TranscribeError::Parse("no transcription array".into()))?;
    let mut segments = Vec::with_capacity(items.len());
    for item in items {
        let from = item["offsets"]["from"].as_u64();
        let to = item["offsets"]["to"].as_u64();
        let text = item["text"].as_str();
        if let (Some(from), Some(to), Some(text)) = (from, to, text) {
            let text = text.trim();
            if !text.is_empty() {
                segments.push(Segment {
                    start: Time(from * 1_000_000),
                    end: Time(to * 1_000_000),
                    text: text.to_string(),
                });
            }
        }
    }
    Ok(segments)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_whisper_cpp_json() {
        let json = r#"{
            "systeminfo": "ignored",
            "transcription": [
                {"timestamps": {"from": "00:00:00,000", "to": "00:00:02,500"},
                 "offsets": {"from": 0, "to": 2500},
                 "text": " Hello there."},
                {"offsets": {"from": 2500, "to": 4100}, "text": "  General Kenobi. "},
                {"offsets": {"from": 4100, "to": 4200}, "text": "   "}
            ]
        }"#;
        let segments = parse_whisper_json(json).unwrap();
        assert_eq!(segments.len(), 2, "blank segments dropped");
        assert_eq!(segments[0].text, "Hello there.");
        assert_eq!(segments[0].start, Time(0));
        assert_eq!(segments[0].end, Time(2_500_000_000));
        assert_eq!(segments[1].text, "General Kenobi.");

        assert!(parse_whisper_json("{}").is_err());
        assert!(parse_whisper_json("not json").is_err());
    }
}
