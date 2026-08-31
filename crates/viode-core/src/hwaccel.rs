//! The one hardware encode path Viode knows per platform, opt-in via
//! VIODE_HWACCEL (Opt-B rule: a measured win on THIS machine or it stays
//! off — `viode bench` prints the verdict). Linux uses VA-API, macOS uses
//! VideoToolbox; everywhere else there is no hardware path yet. Keeping
//! the definition here means proxy building, the GES render, and both
//! bench verbs cannot drift apart.

/// A platform hardware encode path: what to export, how to invoke ffmpeg,
/// and which GStreamer encoder GES should prefer.
pub struct Hw {
    /// The value the user exports as VIODE_HWACCEL to opt in.
    pub env_value: &'static str,
    /// Human name for bench output ("VA-API", "VideoToolbox").
    pub label: &'static str,
    /// ffmpeg arguments that go BEFORE `-i` (hardware decode setup).
    pub decode_args: &'static [&'static str],
    /// GStreamer element factory name for the GES encoding profile.
    pub ges_encoder: &'static str,
}

impl Hw {
    /// ffmpeg arguments that go AFTER `-i`: hardware scale to `height`
    /// plus the hardware encoder, tuned to match the software proxy
    /// quality roughly (VA-API qp 28, VideoToolbox q 50).
    pub fn encode_args(&self, height: u32) -> Vec<String> {
        let s = |v: &[&str]| v.iter().map(|a| a.to_string()).collect::<Vec<_>>();
        match self.env_value {
            "vaapi" => {
                let mut a = vec!["-vf".into(), format!("scale_vaapi=w=-2:h={height}")];
                a.extend(s(&["-c:v", "h264_vaapi", "-qp", "28"]));
                a
            }
            "videotoolbox" => {
                let mut a = vec!["-vf".into(), format!("scale_vt=w=-2:h={height}")];
                a.extend(s(&["-c:v", "h264_videotoolbox", "-q:v", "50"]));
                a
            }
            other => unreachable!("no encode args defined for {other}"),
        }
    }
}

#[cfg(target_os = "linux")]
static PLATFORM_HW: Option<Hw> = Some(Hw {
    env_value: "vaapi",
    label: "VA-API",
    decode_args: &[
        "-hwaccel", "vaapi",
        "-hwaccel_device", "/dev/dri/renderD128",
        "-hwaccel_output_format", "vaapi",
    ],
    ges_encoder: "vah264enc",
});

#[cfg(target_os = "macos")]
static PLATFORM_HW: Option<Hw> = Some(Hw {
    env_value: "videotoolbox",
    label: "VideoToolbox",
    decode_args: &["-hwaccel", "videotoolbox", "-hwaccel_output_format", "videotoolbox_vld"],
    ges_encoder: "vtenc_h264",
});

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
static PLATFORM_HW: Option<Hw> = None;

/// This platform's hardware path, whether or not the user opted in.
/// Bench uses this to measure the path before recommending it.
pub fn platform() -> Option<&'static Hw> {
    PLATFORM_HW.as_ref()
}

/// The hardware path to actually USE: the platform path, but only when
/// `want` (the VIODE_HWACCEL value) names it. A value from the wrong
/// platform (vaapi on macOS) is ignored rather than an error — the
/// software path always works.
pub fn matching(want: &str) -> Option<&'static Hw> {
    platform().filter(|h| h.env_value == want)
}

/// `matching` driven by the real environment variable.
pub fn from_env() -> Option<&'static Hw> {
    std::env::var("VIODE_HWACCEL").ok().and_then(|v| matching(&v))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_os = "macos")]
    fn macos_path_is_videotoolbox() {
        let hw = platform().expect("macOS has a hardware path");
        assert_eq!(hw.env_value, "videotoolbox");
        assert_eq!(hw.ges_encoder, "vtenc_h264");
        let args = hw.encode_args(540);
        assert!(args.contains(&"scale_vt=w=-2:h=540".to_string()));
        assert!(args.contains(&"h264_videotoolbox".to_string()));
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn linux_path_is_vaapi() {
        let hw = platform().expect("Linux has a hardware path");
        assert_eq!(hw.env_value, "vaapi");
        assert_eq!(hw.ges_encoder, "vah264enc");
        let args = hw.encode_args(540);
        assert!(args.contains(&"scale_vaapi=w=-2:h=540".to_string()));
        assert!(args.contains(&"h264_vaapi".to_string()));
    }

    #[test]
    fn wrong_platform_value_is_ignored() {
        #[cfg(target_os = "macos")]
        assert!(matching("vaapi").is_none());
        #[cfg(target_os = "linux")]
        assert!(matching("videotoolbox").is_none());
        assert!(matching("nonsense").is_none());
    }
}
