//! Kitty graphics protocol: real images inside the terminal. Supported by
//! kitty and ghostty (both first-class on Omarchy); everywhere else the TUI
//! falls back to its text rendering. Reference:
//! https://sw.kovidgoyal.net/kitty/graphics-protocol/

use std::io::Write;

use base64::Engine;

/// Where an image goes, in terminal cells. `ui::draw` computes these; the
/// event loop emits them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    pub png: std::path::PathBuf,
    pub id: u32,
    pub x: u16,
    pub y: u16,
    pub cols: u16,
    pub rows: u16,
}

/// Can this terminal draw images?
pub fn detect() -> bool {
    detect_from(
        &std::env::var("TERM").unwrap_or_default(),
        &std::env::var("TERM_PROGRAM").unwrap_or_default(),
        std::env::var_os("KITTY_WINDOW_ID").is_some(),
        std::env::var_os("VIODE_NO_GRAPHICS").is_some(),
    )
}

fn detect_from(term: &str, term_program: &str, kitty_id: bool, disabled: bool) -> bool {
    if disabled {
        return false;
    }
    kitty_id
        || term.contains("kitty")
        || term.contains("ghostty")
        || term_program.eq_ignore_ascii_case("ghostty")
}

/// Escape sequence deleting every image placement (start of a redraw).
pub fn delete_all() -> &'static [u8] {
    b"\x1b_Ga=d,d=A,q=2\x1b\\"
}

/// Escape sequences that transmit a PNG and display it at the CURRENT
/// cursor position, scaled to `cols` x `rows` cells. Payload is chunked at
/// 4096 base64 bytes as the protocol requires; `q=2` suppresses terminal
/// replies (they would land on our stdin).
pub fn encode_png_at(data: &[u8], id: u32, cols: u16, rows: u16) -> Vec<u8> {
    let b64 = base64::engine::general_purpose::STANDARD.encode(data);
    let chunks: Vec<&[u8]> = b64.as_bytes().chunks(4096).collect();
    let mut out = Vec::with_capacity(b64.len() + chunks.len() * 32);
    let last = chunks.len() - 1;
    for (k, chunk) in chunks.iter().enumerate() {
        let more = u8::from(k < last);
        if k == 0 {
            write!(out, "\x1b_Ga=T,f=100,q=2,i={id},c={cols},r={rows},m={more};").unwrap();
        } else {
            write!(out, "\x1b_Gq=2,m={more};").unwrap();
        }
        out.extend_from_slice(chunk);
        out.extend_from_slice(b"\x1b\\");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_capable_terminals_only() {
        assert!(detect_from("xterm-kitty", "", false, false));
        assert!(detect_from("xterm-ghostty", "", false, false));
        assert!(detect_from("xterm-256color", "ghostty", false, false));
        assert!(detect_from("xterm-256color", "", true, false));
        assert!(!detect_from("alacritty", "", false, false));
        // The escape hatch wins over everything.
        assert!(!detect_from("xterm-kitty", "", true, true));
    }

    #[test]
    fn single_chunk_payload() {
        let seq = encode_png_at(b"tiny", 7, 12, 4);
        let s = String::from_utf8_lossy(&seq);
        assert!(s.starts_with("\x1b_Ga=T,f=100,q=2,i=7,c=12,r=4,m=0;"));
        assert!(s.ends_with("\x1b\\"));
        assert_eq!(s.matches("\x1b_G").count(), 1);
    }

    #[test]
    fn large_payload_is_chunked_with_continuations() {
        // 9000 raw bytes -> 12000 base64 chars -> 3 chunks of <= 4096.
        let seq = encode_png_at(&vec![0u8; 9000], 1, 10, 3);
        let s = String::from_utf8_lossy(&seq);
        assert_eq!(s.matches("\x1b_G").count(), 3);
        assert_eq!(s.matches("m=1;").count(), 2, "all but last continue");
        assert_eq!(s.matches("m=0;").count(), 1, "exactly one final chunk");
        assert_eq!(s.matches("a=T").count(), 1, "transmit header only once");
    }
}
