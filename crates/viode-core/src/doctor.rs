//! The engine checkup: which pieces of this machine's GStreamer build and
//! sidecar toolchain exist, and which features break without them.
//!
//! GStreamer's capabilities vary with how each platform built it (Homebrew
//! ships without soundtouch, patent-averse distros strip encoders), so the
//! honest move is to tell the user upfront instead of failing mid-render.
//! Every interface surfaces this: `viode doctor` on the CLI, a banner in
//! the GUI, the TUI status line, and the MCP server's initialize response.

use std::process::{Command, Stdio};

use gstreamer as gst;

/// One capability probe. `fix` is what a missing piece costs and how to
/// get it — user-facing text, part of the interface.
pub struct Check {
    /// The feature the user cares about ("Speed changes").
    pub feature: &'static str,
    /// What is actually probed (an element or binary name).
    pub probe: &'static str,
    pub ok: bool,
    /// Required pieces make Viode unusable; optional ones cost a feature.
    pub required: bool,
    pub fix: &'static str,
}

fn binary(bin: &str, arg: &str) -> bool {
    Command::new(bin)
        .arg(arg)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

/// Run every check. Cheap enough for startup: a handful of process spawns
/// plus in-process GStreamer registry lookups.
pub fn run() -> Vec<Check> {
    let gst_ok = gst::init().is_ok() && gstreamer_editing_services::init().is_ok();
    let el = |name: &str| gst_ok && gst::ElementFactory::find(name).is_some();
    vec![
        Check {
            feature: "Probing, proxies, analysis, exports",
            probe: "ffmpeg",
            ok: binary("ffmpeg", "-version"),
            required: true,
            fix: "install ffmpeg",
        },
        Check {
            feature: "Media metadata",
            probe: "ffprobe",
            ok: binary("ffprobe", "-version"),
            required: true,
            fix: "install ffmpeg (ffprobe ships with it)",
        },
        Check {
            feature: "The render and preview engine",
            probe: "gstreamer + ges",
            ok: gst_ok,
            required: true,
            fix: "install gstreamer and gst-editing-services",
        },
        Check {
            feature: "H.264 renders",
            probe: "x264enc",
            ok: el("x264enc"),
            required: false,
            fix: "install gst-plugins-ugly (the x264 plugin)",
        },
        Check {
            feature: "Speed changes",
            probe: "pitch",
            ok: el("pitch"),
            required: false,
            fix: "install gst-plugins-bad with soundtouch \
                  (Homebrew's GStreamer does not ship it)",
        },
        Check {
            feature: "Clip rotation",
            probe: "rotate",
            ok: el("rotate"),
            required: false,
            fix: "install gst-plugins-bad (the geometrictransform plugin)",
        },
        Check {
            feature: "Color grading",
            probe: "videobalance",
            ok: el("videobalance"),
            required: false,
            fix: "install gst-plugins-good (the videofilter plugin)",
        },
        Check {
            feature: "Audio pan",
            probe: "audiopanorama",
            ok: el("audiopanorama"),
            required: false,
            fix: "install gst-plugins-good (the audiofx plugin)",
        },
        Check {
            feature: "Chroma key (matte)",
            probe: "alpha",
            ok: el("alpha"),
            required: false,
            fix: "install gst-plugins-good (the alpha plugin)",
        },
        Check {
            feature: "Wipe transitions",
            probe: "smpte",
            ok: el("smpte"),
            required: false,
            fix: "install gst-plugins-good (the smpte plugin)",
        },
        Check {
            feature: "Region masks",
            probe: "ffmpeg gblur",
            ok: ffmpeg_has_filter("gblur"),
            required: false,
            fix: "install a standard ffmpeg (gblur ships in every stock build)",
        },
        Check {
            feature: "Voice cleanup",
            probe: "ffmpeg afftdn",
            ok: ffmpeg_has_filter("afftdn"),
            required: false,
            fix: "install a standard ffmpeg (afftdn ships in every stock build)",
        },
        Check {
            feature: "Stabilization",
            probe: "ffmpeg vidstab",
            ok: ffmpeg_has_filter("vidstabdetect"),
            required: false,
            fix: "install ffmpeg built with libvidstab (Arch ffmpeg has it; \
                  Homebrew: brew install ffmpeg)",
        },
        Check {
            feature: ".cube LUTs",
            probe: "ffmpeg lut3d filter",
            ok: ffmpeg_has_filter("lut3d"),
            required: false,
            fix: "this ffmpeg build lacks the lut3d filter — install a \
                  standard ffmpeg",
        },
        Check {
            feature: "Inline terminal playback",
            probe: "mpv",
            ok: binary("mpv", "--version"),
            required: false,
            fix: "install mpv",
        },
        Check {
            feature: "AI assistant connection",
            probe: "viode connect",
            ok: crate::connect::detect().iter().any(|c| c.connected),
            required: false,
            fix: "run `viode connect` to register Viode with the AI apps \
                  on this machine (Claude, Cursor, opencode, ...)",
        },
        Check {
            feature: "Transcription and text-based editing",
            probe: "whisper.cpp",
            ok: binary("whisper-cli", "--help")
                || binary("whisper-cpp", "--help")
                || binary("whisper", "--help"),
            required: false,
            fix: "install whisper.cpp (Arch: pacman -S whisper-cpp)",
        },
        Check {
            feature: "Subject reframe model",
            probe: "seeta face model",
            ok: crate::reframe::model_present(),
            required: false,
            fix: "download the face model: curl -L -o \
                  ~/.local/share/viode/models/seeta_fd_frontal_v1.0.bin \
                  https://github.com/atomashpolskiy/rustface/raw/master/model/seeta_fd_frontal_v1.0.bin",
        },
        Check {
            feature: "Transcription model",
            probe: "ggml model",
            ok: whisper_model_present(),
            required: false,
            fix: "download a ggml model (e.g. ggml-base.en.bin from \
                  huggingface.co/ggerganov/whisper.cpp) into \
                  ~/.local/share/viode/models, or set VIODE_WHISPER_MODEL",
        },
    ]
}

fn ffmpeg_has_filter(name: &str) -> bool {
    Command::new("ffmpeg")
        .args(["-hide_banner", "-filters"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains(&format!(" {name} ")))
        .unwrap_or(false)
}

fn whisper_model_present() -> bool {
    if std::env::var("VIODE_WHISPER_MODEL").is_ok_and(|m| std::path::Path::new(&m).exists()) {
        return true;
    }
    let user_models = std::env::var("HOME")
        .map(|h| format!("{h}/.local/share/viode/models"))
        .unwrap_or_default();
    for dir in [
        user_models.as_str(),
        "/usr/share/whisper.cpp-model-base.en",
        "/usr/share/whisper.cpp",
    ] {
        if let Ok(entries) = std::fs::read_dir(dir) {
            if entries
                .flatten()
                .any(|e| e.path().extension().is_some_and(|x| x == "bin"))
            {
                return true;
            }
        }
    }
    false
}

/// Only the failing checks.
pub fn problems() -> Vec<Check> {
    run().into_iter().filter(|c| !c.ok).collect()
}

/// One sentence for banners and the MCP initialize response, or None when
/// the machine is complete.
pub fn summary(problems: &[Check]) -> Option<String> {
    if problems.is_empty() {
        return None;
    }
    let list = problems
        .iter()
        .map(|c| format!("{} ({} missing)", c.feature, c.probe))
        .collect::<Vec<_>>()
        .join("; ");
    Some(format!(
        "This machine is missing {} engine piece(s): {}. Run `viode doctor` for fixes.",
        problems.len(),
        list
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_check_names_its_feature_probe_and_fix() {
        let checks = run();
        assert!(checks.len() >= 10);
        for c in &checks {
            assert!(!c.feature.is_empty() && !c.probe.is_empty() && !c.fix.is_empty());
        }
        // The three load-bearing dependencies are marked required.
        let required: Vec<_> = checks.iter().filter(|c| c.required).map(|c| c.probe).collect();
        assert_eq!(required, ["ffmpeg", "ffprobe", "gstreamer + ges"]);
    }

    #[test]
    fn summary_is_silent_on_a_complete_machine_and_specific_otherwise() {
        assert!(summary(&[]).is_none());
        let missing = vec![Check {
            feature: "Speed changes",
            probe: "pitch",
            ok: false,
            required: false,
            fix: "install soundtouch",
        }];
        let s = summary(&missing).unwrap();
        assert!(s.contains("Speed changes") && s.contains("pitch") && s.contains("viode doctor"));
    }
}
