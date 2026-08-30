//! The background thumbnail/waveform worker moved to viode-core so the GUI
//! shares the same artifacts (Phase 8). This re-export keeps the TUI's
//! internal paths (`crate::media::*`) unchanged.

pub use viode_core::artifacts::{Kind, MediaCache, Spec};
