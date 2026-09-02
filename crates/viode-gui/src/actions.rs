//! The single table of everything the editor can do, with the names,
//! search keywords, and shortcut labels the discoverable surfaces show.
//!
//! House rule (PLAN.md, "the GUI discoverability rule"): every capability
//! must be mouse-reachable through the command palette, context menus,
//! the inspector, and a small toolbar. This table is what makes that true
//! by construction — the keyboard handler, the palette, the menus, and
//! the toolbar all dispatch the same `Action`, so a verb registered here
//! appears everywhere at once. Countdown waves add their verbs HERE.

/// Everything dispatchable. Parametric edits (gain, place, grade, fades,
/// keyframes, titles' text) live in the inspector by design; actions are
/// the argumentless verbs plus the doors to panels and dialogs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    // Transport.
    PlayPause,
    ShuttleReverse,
    ShuttlePause,
    ShuttleForward,
    FrameBack,
    FrameForward,
    NudgeBack,
    NudgeForward,
    JumpBack,
    JumpForward,
    GoToStart,
    GoToEnd,
    PreviousEdge,
    NextEdge,
    MarkIn,
    MarkOut,
    ClearMarks,
    // Edits.
    Split,
    TrimInToPlayhead,
    TrimOutToPlayhead,
    Delete,
    MoveEarlier,
    MoveLater,
    AddTitle,
    Freeze,
    Captions,
    Undo,
    Redo,
    Save,
    // Doors.
    RenderDialog,
    ToggleScopes,
    EngineCheckup,
    CommandPalette,
    Help,
    Quit,
}

impl Action {
    /// Registry order is palette order for an empty query: edits first,
    /// because they are what people open the palette for.
    pub const ALL: &'static [Action] = &[
        Action::Split,
        Action::TrimInToPlayhead,
        Action::TrimOutToPlayhead,
        Action::Delete,
        Action::MoveEarlier,
        Action::MoveLater,
        Action::AddTitle,
        Action::Freeze,
        Action::Captions,
        Action::Undo,
        Action::Redo,
        Action::Save,
        Action::RenderDialog,
        Action::PlayPause,
        Action::ShuttleReverse,
        Action::ShuttlePause,
        Action::ShuttleForward,
        Action::FrameBack,
        Action::FrameForward,
        Action::NudgeBack,
        Action::NudgeForward,
        Action::JumpBack,
        Action::JumpForward,
        Action::GoToStart,
        Action::GoToEnd,
        Action::PreviousEdge,
        Action::NextEdge,
        Action::MarkIn,
        Action::MarkOut,
        Action::ClearMarks,
        Action::ToggleScopes,
        Action::EngineCheckup,
        Action::Help,
        Action::CommandPalette,
        Action::Quit,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Action::PlayPause => "Play / pause",
            Action::ShuttleReverse => "Shuttle backward",
            Action::ShuttlePause => "Pause shuttle",
            Action::ShuttleForward => "Shuttle forward",
            Action::FrameBack => "Step one frame back",
            Action::FrameForward => "Step one frame forward",
            Action::NudgeBack => "Seek back 0.1s",
            Action::NudgeForward => "Seek forward 0.1s",
            Action::JumpBack => "Jump back 1s",
            Action::JumpForward => "Jump forward 1s",
            Action::GoToStart => "Go to start",
            Action::GoToEnd => "Go to end",
            Action::PreviousEdge => "Go to previous cut",
            Action::NextEdge => "Go to next cut",
            Action::MarkIn => "Mark range in-point",
            Action::MarkOut => "Mark range out-point",
            Action::ClearMarks => "Clear marks / deselect",
            Action::Split => "Split at playhead",
            Action::TrimInToPlayhead => "Trim in-point to playhead",
            Action::TrimOutToPlayhead => "Trim out-point to playhead",
            Action::Delete => "Delete clip or title",
            Action::MoveEarlier => "Move clip earlier",
            Action::MoveLater => "Move clip later",
            Action::AddTitle => "Add title at playhead",
            Action::Freeze => "Freeze frame at playhead",
            Action::Captions => "Generate captions",
            Action::Undo => "Undo",
            Action::Redo => "Redo",
            Action::Save => "Save project",
            Action::RenderDialog => "Render…",
            Action::ToggleScopes => "Toggle scopes",
            Action::EngineCheckup => "Engine checkup",
            Action::CommandPalette => "Command palette",
            Action::Help => "Keyboard help",
            Action::Quit => "Quit",
        }
    }

    /// Displayed next to the entry — the palette teaches the fast path.
    pub fn shortcut(self) -> &'static str {
        match self {
            Action::PlayPause => "space",
            Action::ShuttleReverse => "J",
            Action::ShuttlePause => "K",
            Action::ShuttleForward => "L",
            Action::FrameBack => ",",
            Action::FrameForward => ".",
            Action::NudgeBack => "←",
            Action::NudgeForward => "→",
            Action::JumpBack => "shift+←",
            Action::JumpForward => "shift+→",
            Action::GoToStart => "home",
            Action::GoToEnd => "end",
            Action::PreviousEdge => "↑",
            Action::NextEdge => "↓",
            Action::MarkIn => "[",
            Action::MarkOut => "]",
            Action::ClearMarks => "esc",
            Action::Split => "S",
            Action::TrimInToPlayhead => "I",
            Action::TrimOutToPlayhead => "O",
            Action::Delete => "D",
            Action::MoveEarlier => "shift+,",
            Action::MoveLater => "shift+.",
            Action::AddTitle => "T",
            Action::Freeze => "F",
            Action::Captions => "",
            Action::Undo => "U",
            Action::Redo => "shift+U",
            Action::Save => "W",
            Action::RenderDialog => "R",
            Action::ToggleScopes => "",
            Action::EngineCheckup => "",
            Action::CommandPalette => "ctrl+K",
            Action::Help => "?",
            Action::Quit => "Q",
        }
    }

    /// Extra search terms beyond the label (what a Premiere hand or a
    /// first-timer might type).
    pub fn keywords(self) -> &'static str {
        match self {
            Action::PlayPause => "space transport preview",
            Action::ShuttleReverse => "jkl rewind",
            Action::ShuttlePause => "jkl stop",
            Action::ShuttleForward => "jkl fast",
            Action::FrameBack => "frame step nudge",
            Action::FrameForward => "frame step nudge",
            Action::NudgeBack => "scrub left arrow",
            Action::NudgeForward => "scrub right arrow",
            Action::JumpBack => "seek second",
            Action::JumpForward => "seek second",
            Action::GoToStart => "beginning home first",
            Action::GoToEnd => "finish last",
            Action::PreviousEdge => "edit point jump clip edge",
            Action::NextEdge => "edit point jump clip edge",
            Action::MarkIn => "range bracket take multicam",
            Action::MarkOut => "range bracket take multicam",
            Action::ClearMarks => "escape reset selection",
            Action::Split => "cut razor blade",
            Action::TrimInToPlayhead => "head start ripple",
            Action::TrimOutToPlayhead => "tail end ripple",
            Action::Delete => "remove cut ripple",
            Action::MoveEarlier => "reorder swap left",
            Action::MoveLater => "reorder swap right",
            Action::AddTitle => "text lower third",
            Action::Freeze => "frame hold still pause",
            Action::Captions => "subtitles srt transcript burn lower third",
            Action::Undo => "revert back history",
            Action::Redo => "again history",
            Action::Save => "write project file",
            Action::RenderDialog => "export encode preset youtube shorts podcast master",
            Action::ToggleScopes => "waveform vectorscope color measure",
            Action::EngineCheckup => "doctor missing plugins gstreamer diagnose",
            Action::CommandPalette => "search everything commands",
            Action::Help => "shortcuts keys cheatsheet",
            Action::Quit => "exit close",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_registry_is_complete_and_labeled() {
        assert!(Action::ALL.len() >= 30);
        let mut labels = std::collections::HashSet::new();
        for a in Action::ALL {
            assert!(!a.label().is_empty(), "{a:?} has no label");
            assert!(!a.keywords().is_empty(), "{a:?} has no keywords");
            assert!(labels.insert(a.label()), "duplicate label {:?}", a.label());
        }
    }

    #[test]
    fn edits_lead_the_registry() {
        assert_eq!(Action::ALL[0], Action::Split);
        let pos = |a: Action| Action::ALL.iter().position(|x| *x == a).unwrap();
        assert!(pos(Action::Save) < pos(Action::PlayPause));
    }
}
